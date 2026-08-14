#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use wasmtime::component::{
    Component, ComponentExportIndex, InstancePre, Linker, Val, types::ComponentItem,
};
use wasmtime::{Config, Engine, ResourceLimiter, Store};
use wasmtime::{Error as WasmtimeError, Result as WasmtimeResult};

mod errors;

pub(crate) use errors::{ExecError, LoadError};
use errors::{MemoryLimitExceeded, map_call_error, map_dispatch_error, map_instantiate_error};
#[allow(
    unused_imports,
    reason = "later host adapters consume the marker helper and trap detail"
)]
pub(crate) use errors::{TrapDetail, host_import_error};

pub(crate) struct ExecEngine {
    engine: Engine,
}

impl ExecEngine {
    pub(crate) fn new() -> WasmtimeResult<Self> {
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

    /// Compiles a component and prepares its host imports for later invocations.
    ///
    /// `S` is the application's per-invocation data type. Each fresh Store
    /// receives its own value before instantiation so constructor imports and
    /// later guest calls observe the correct invocation context.
    ///
    /// `imports` is called exactly once to register host-import implementations
    /// in the `Linker` before the component is baked into an `InstancePre`.
    /// Tests are currently its only authors, and registration failures surface
    /// as [`LoadError::Link`].
    pub(crate) fn load<S: Send + 'static>(
        &self,
        bytes: &[u8],
        imports: impl FnOnce(&mut Linker<StoreCtx<S>>) -> WasmtimeResult<()>,
    ) -> Result<LoadedComponent<S>, LoadError> {
        let component = Component::new(&self.engine, bytes).map_err(LoadError::compile)?;
        let mut linker = Linker::new(&self.engine);
        imports(&mut linker).map_err(LoadError::link)?;
        let instance_pre = linker
            .instantiate_pre(&component)
            .map_err(LoadError::link)?;

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
        data: S,
        limits: ExecLimits,
        invocation_fuel: u64,
    ) -> Result<Vec<Val>, ExecError> {
        let mut store = self.configured_store(data, limits.max_memory_bytes);
        store
            .set_fuel(limits.instantiation_fuel)
            .map_err(map_instantiate_error)?;

        let instance = self
            .instance_pre
            .instantiate_async(&mut store)
            .await
            .map_err(map_instantiate_error)?;

        store
            .set_fuel(invocation_fuel)
            .map_err(map_dispatch_error)?;
        let Some(func) = instance.get_func(&mut store, &export.func) else {
            return Err(ExecError::Dispatch(anyhow::anyhow!(
                "resolved component export was not a function"
            )));
        };
        let mut results = vec![Val::Bool(false); export.result_count];

        let call_result = store
            .run_concurrent(async |accessor| {
                func.call_concurrent(accessor, args, &mut results).await
            })
            .await
            .map_err(map_call_error)?;
        call_result.map_err(map_call_error)?;

        Ok(results)
    }

    pub(crate) async fn smoke(
        &self,
        data: S,
        limits: ExecLimits,
        startup_fuel: u64,
    ) -> Result<(), ExecError> {
        let store = self.configured_store(data, limits.max_memory_bytes);
        self.smoke_store(store, limits.instantiation_fuel.min(startup_fuel))
            .await
    }

    fn configured_store(&self, data: S, max_memory_bytes: usize) -> Store<StoreCtx<S>> {
        let mut store = Store::new(
            self.instance_pre.engine(),
            StoreCtx::new(data, max_memory_bytes),
        );
        store.limiter(|ctx| &mut ctx.limiter);
        store.set_epoch_deadline(u64::MAX);
        store
    }

    async fn smoke_store(&self, mut store: Store<StoreCtx<S>>, fuel: u64) -> Result<(), ExecError> {
        store.set_fuel(fuel).map_err(map_instantiate_error)?;
        self.instance_pre
            .instantiate_async(&mut store)
            .await
            .map_err(map_instantiate_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn smoke_observing_drop(
        &self,
        data: S,
        limits: ExecLimits,
        startup_fuel: u64,
        dropped: Arc<AtomicBool>,
    ) -> Result<(), ExecError> {
        let mut store = self.configured_store(data, limits.max_memory_bytes);
        store.data_mut().observe_drop(dropped);
        self.smoke_store(store, limits.instantiation_fuel.min(startup_fuel))
            .await
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

/// Data owned by every `Store` this module creates.
///
/// The invocation data is installed before instantiation so constructor host
/// imports see the same context as later guest calls.
pub(crate) struct StoreCtx<S> {
    limiter: MemoryLimiter,
    data: S,
    #[cfg(test)]
    drop_probe: Option<StoreDropProbe>,
}

impl<S> StoreCtx<S> {
    fn new(data: S, max_memory_bytes: usize) -> Self {
        Self {
            limiter: MemoryLimiter { max_memory_bytes },
            data,
            #[cfg(test)]
            drop_probe: None,
        }
    }

    #[allow(
        dead_code,
        reason = "generated host-import adapters consume invocation data in the next step"
    )]
    pub(crate) fn data(&self) -> &S {
        &self.data
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
        current: usize,
        desired: usize,
        _maximum: Option<usize>,
    ) -> WasmtimeResult<bool> {
        if desired > self.max_memory_bytes {
            return Err(WasmtimeError::new(MemoryLimitExceeded {
                current,
                desired,
                limit: self.max_memory_bytes,
            }));
        }
        Ok(true)
    }

    fn table_growing(
        &mut self,
        _current: usize,
        _desired: usize,
        _maximum: Option<usize>,
    ) -> WasmtimeResult<bool> {
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
