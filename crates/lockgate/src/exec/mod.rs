use std::marker::PhantomData;

#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use wasmtime::Result;
use wasmtime::component::{
    Component, ComponentExportIndex, InstancePre, Linker, Val, types::ComponentItem,
};
use wasmtime::error::Context as _;
use wasmtime::{Config, Engine, ResourceLimiter, Store};

pub(crate) struct ExecEngine {
    engine: Engine,
}

impl ExecEngine {
    pub(crate) fn new() -> Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .concurrency_support(true)
            .consume_fuel(true)
            .epoch_interruption(true);

        Ok(Self {
            engine: Engine::new(&config)?,
        })
    }

    pub(crate) fn load<S: Send + 'static>(
        &self,
        bytes: &[u8],
        imports: impl FnOnce(&mut Linker<StoreCtx<S>>) -> Result<()>,
    ) -> Result<LoadedComponent<S>> {
        let component = Component::new(&self.engine, bytes)
            .context("failed to compile trusted component bytes")?;
        let mut linker = Linker::new(&self.engine);
        imports(&mut linker).context("failed to configure component imports")?;
        let instance_pre = linker
            .instantiate_pre(&component)
            .context("failed to prelink component imports")?;

        Ok(LoadedComponent { instance_pre })
    }
}

pub(crate) struct LoadedComponent<S: 'static> {
    instance_pre: InstancePre<StoreCtx<S>>,
}

impl<S: Send + 'static> LoadedComponent<S> {
    pub(crate) fn export(&self, interface: &str, func: &str) -> Option<ExportRef> {
        let component = self.instance_pre.component();
        let interface = component.get_export_index(None, interface)?;
        let (item, func) = component.get_export(Some(&interface), func)?;
        let ComponentItem::ComponentFunc(ty) = item else {
            return None;
        };

        Some(ExportRef {
            func,
            result_count: ty.results().len(),
        })
    }

    pub(crate) async fn invoke(
        &self,
        export: ExportRef,
        args: &[Val],
        limits: ExecLimits,
        invocation_fuel: u64,
    ) -> Result<Vec<Val>> {
        let mut store = Store::new(
            self.instance_pre.engine(),
            StoreCtx::new(limits.max_memory_bytes),
        );
        store.limiter(|ctx| &mut ctx.limiter);
        store.set_epoch_deadline(u64::MAX);
        store
            .set_fuel(limits.instantiation_fuel)
            .context("failed to set instantiation fuel")?;

        let instance = self
            .instance_pre
            .instantiate_async(&mut store)
            .await
            .context("failed to instantiate component")?;

        store
            .set_fuel(invocation_fuel)
            .context("failed to set invocation fuel")?;
        let func = instance
            .get_func(&mut store, &export.func)
            .context("resolved component export was not a function")?;
        let mut results = vec![Val::Bool(false); export.result_count];

        store
            .run_concurrent(async |accessor| {
                func.call_concurrent(accessor, args, &mut results).await
            })
            .await
            .context("concurrent dispatch failed")??;

        Ok(results)
    }
}

#[derive(Clone, Copy)]
pub(crate) struct ExportRef {
    func: ComponentExportIndex,
    result_count: usize,
}

#[derive(Clone, Copy)]
pub(crate) struct ExecLimits {
    pub(crate) instantiation_fuel: u64,
    pub(crate) max_memory_bytes: usize,
}

pub(crate) struct StoreCtx<S> {
    limiter: MemoryLimiter,
    marker: PhantomData<fn() -> S>,
    #[cfg(test)]
    drop_probe: Option<StoreDropProbe>,
}

impl<S> StoreCtx<S> {
    fn new(max_memory_bytes: usize) -> Self {
        Self {
            limiter: MemoryLimiter { max_memory_bytes },
            marker: PhantomData,
            #[cfg(test)]
            drop_probe: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn observe_drop(&mut self, dropped: Arc<AtomicBool>) {
        self.drop_probe = Some(StoreDropProbe(dropped));
    }
}

struct MemoryLimiter {
    max_memory_bytes: usize,
}

impl ResourceLimiter for MemoryLimiter {
    fn memory_growing(
        &mut self,
        _current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(desired <= self.max_memory_bytes)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> wasmtime::Result<bool> {
        Ok(true)
    }
}

#[cfg(test)]
struct StoreDropProbe(Arc<AtomicBool>);

#[cfg(test)]
impl Drop for StoreDropProbe {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}
