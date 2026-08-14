#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

#[cfg(not(test))]
use std::sync::Arc;
use std::{any::Any, marker::PhantomData};

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
    #[allow(
        dead_code,
        reason = "the direct linker hook remains an execution-test seam"
    )]
    pub(crate) fn load<S: Send + Sync + 'static>(
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

        Ok(LoadedComponent {
            instance_pre,
            imports: Arc::new(UnitImports),
            plugin: None,
        })
    }

    pub(crate) fn load_hosted<S: Send + Sync + 'static>(
        &self,
        bytes: &[u8],
        imports: Arc<dyn ImportsFactory<S>>,
    ) -> Result<LoadedComponent<S>, LoadError> {
        let component = Component::new(&self.engine, bytes).map_err(LoadError::compile)?;
        let mut linker = Linker::new(&self.engine);
        imports.register(&mut linker).map_err(LoadError::link)?;
        let instance_pre = linker
            .instantiate_pre(&component)
            .map_err(LoadError::link)?;

        Ok(LoadedComponent {
            instance_pre,
            imports,
            plugin: None,
        })
    }
}

pub(crate) struct LoadedComponent<S: 'static> {
    instance_pre: InstancePre<StoreCtx<S>>,
    imports: Arc<dyn ImportsFactory<S>>,
    plugin: Option<Arc<dyn Any + Send + Sync>>,
}

impl<S: Send + Sync + 'static> LoadedComponent<S> {
    pub(crate) fn set_plugin<P: Clone + Send + Sync + 'static>(&mut self, plugin: P) {
        self.plugin = Some(Arc::new(plugin));
    }

    pub(crate) fn exports_interface(&self, interface: &str) -> bool {
        self.instance_pre
            .component()
            .get_export_index(None, interface)
            .is_some()
    }

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
        let Some(func) = instance.get_func(&mut store, export.func) else {
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

    /// Instantiates the component under its runtime limits and startup budget.
    ///
    /// Instantiation executes the component plan: inner core modules instantiate,
    /// `start` sections run, memories and tables allocate, and guest runtime and
    /// allocator initialization runs—effectively the plugin's constructors. It can
    /// therefore trap, exhaust fuel, or exceed the memory cap at admission instead
    /// of at first call. Once host imports exist, start code may call them and
    /// observes `data`, which is why admission takes a full startup invocation
    /// context rather than a bare fuel value.
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
            StoreCtx::new(
                data,
                self.imports.create(),
                self.plugin.clone(),
                max_memory_bytes,
            ),
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
#[doc(hidden)]
pub struct StoreCtx<S> {
    limiter: MemoryLimiter,
    // Invocation futures may move between executor threads. The Store owns an
    // Arc so host calls can cheaply retain data across await points; Arc<S> is
    // Send only when S is Sync, which is why that bound reaches the public API.
    data: Arc<S>,
    imports: Box<dyn Any + Send>,
    plugin: Option<Arc<dyn Any + Send + Sync>>,
    #[cfg(test)]
    drop_probe: Option<StoreDropProbe>,
}

impl<S> StoreCtx<S> {
    fn new(
        data: S,
        imports: Box<dyn Any + Send>,
        plugin: Option<Arc<dyn Any + Send + Sync>>,
        max_memory_bytes: usize,
    ) -> Self {
        Self {
            limiter: MemoryLimiter { max_memory_bytes },
            data: Arc::new(data),
            imports,
            plugin,
            #[cfg(test)]
            drop_probe: None,
        }
    }

    #[allow(
        dead_code,
        reason = "generated host-import adapters consume invocation data in the next step"
    )]
    pub(crate) fn data(&self) -> &S {
        self.data.as_ref()
    }

    #[doc(hidden)]
    pub fn host_parts<I, P>(&self) -> (I, Arc<S>, Arc<P>)
    where
        I: Clone + 'static,
        P: Send + Sync + 'static,
    {
        let imports = self
            .imports
            .downcast_ref::<I>()
            .expect("Store imports must match their generated host bindings")
            .clone();
        let plugin = Arc::clone(
            self.plugin
                .as_ref()
                .expect("public plugin Stores must carry a PluginHandle"),
        )
        .downcast::<P>()
        .expect("Store plugin identity must be a PluginHandle");
        (imports, Arc::clone(&self.data), plugin)
    }

    #[cfg(test)]
    pub(crate) fn observe_drop(&mut self, dropped: Arc<AtomicBool>) {
        self.drop_probe = Some(StoreDropProbe(dropped));
    }
}

pub(crate) trait ImportsFactory<S>: Send + Sync {
    fn register(&self, linker: &mut Linker<StoreCtx<S>>) -> WasmtimeResult<()>;
    fn create(&self) -> Box<dyn Any + Send>;
}

pub(crate) struct TypedImports<S: 'static, I> {
    imports: I,
    register: fn(&I, &mut Linker<StoreCtx<S>>) -> WasmtimeResult<()>,
    marker: PhantomData<fn() -> S>,
}

impl<S: 'static, I> TypedImports<S, I> {
    pub(crate) fn new(
        imports: I,
        register: fn(&I, &mut Linker<StoreCtx<S>>) -> WasmtimeResult<()>,
    ) -> Self {
        Self {
            imports,
            register,
            marker: PhantomData,
        }
    }
}

impl<S, I> ImportsFactory<S> for TypedImports<S, I>
where
    S: Send + Sync + 'static,
    I: Clone + Send + Sync + 'static,
{
    fn register(&self, linker: &mut Linker<StoreCtx<S>>) -> WasmtimeResult<()> {
        (self.register)(&self.imports, linker)
    }

    fn create(&self) -> Box<dyn Any + Send> {
        Box::new(self.imports.clone())
    }
}

#[allow(dead_code, reason = "used by the direct execution-test linker seam")]
struct UnitImports;

impl<S> ImportsFactory<S> for UnitImports {
    fn register(&self, _linker: &mut Linker<StoreCtx<S>>) -> WasmtimeResult<()> {
        Ok(())
    }

    fn create(&self) -> Box<dyn Any + Send> {
        Box::new(())
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
