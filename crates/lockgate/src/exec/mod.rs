#[cfg(test)]
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicU64, Ordering},
};

#[cfg(not(test))]
use std::sync::Arc;
use std::{any::Any, marker::PhantomData, time::Duration};

use lockgate_policy::PluginId;
use wasmtime::component::{
    Component, ComponentExportIndex, InstancePre, Linker, ResourceTable, Val, types::ComponentItem,
};
use wasmtime::{
    Config, Engine, InstanceAllocationStrategy as WasmtimeInstanceAllocationStrategy,
    PoolingAllocationConfig as WasmtimePoolingAllocationConfig, ResourceLimiter, Store,
};
use wasmtime::{Error as WasmtimeError, Result as WasmtimeResult};
use wasmtime_wasi::{WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wasmtime_wasi_http::{WasiHttpCtx, WasiHttpCtxView, WasiHttpView};

use super::config::{SettingsState, ValidatedSettings, add_settings_to_linker};
use super::jobs::DetachedJobContext;
#[cfg(not(test))]
use super::policy::ResourceStore;
#[cfg(test)]
use lockgate::__private::ResourceStore;

pub(crate) mod cache;
mod errors;
pub(crate) mod wasi_http;
mod wasi_http_sender;

use cache::CompiledComponentCache;
#[doc(hidden)]
#[allow(
    unused_imports,
    reason = "generated host bindings call this re-export from downstream crates"
)]
pub use errors::catch_host_panic;
pub(crate) use errors::{EnvironmentError, ExecError, HostPanicState, LoadError};
use errors::{
    MemoryLimitExceeded, host_import_call_limit_error, map_call_error, map_dispatch_error,
    map_instantiate_error,
};
#[allow(
    unused_imports,
    reason = "later host adapters consume the marker helper and trap detail"
)]
pub(crate) use errors::{TrapDetail, host_import_error};
use wasi_http::HttpHooks;
pub(crate) use wasi_http_sender::HttpPool;

// Compute-bound guests yield at this fuel granularity so wall-clock deadlines can preempt
// them; smaller values tighten deadline adherence at the cost of slightly more overhead.
const DEADLINE_YIELD_FUEL: u64 = 10_000;

/// Wasmtime instance allocation strategy used by a Lockgate host.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum InstanceAllocation {
    /// Allocate instance resources for each invocation and release them afterward.
    #[default]
    OnDemand,
    /// Reuse preallocated instance resources sized by the supplied configuration.
    Pooling(PoolingAllocationConfig),
}

/// Capacity reserved by Wasmtime's pooling instance allocator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PoolingAllocationConfig {
    /// Maximum number of concurrent linear memories.
    pub total_memories: u32,
    /// Maximum number of concurrent tables.
    pub total_tables: u32,
    /// Maximum number of concurrent core Wasm instances.
    pub total_core_instances: u32,
    /// Maximum number of concurrent component instances.
    pub total_component_instances: u32,
    /// Maximum byte size of each pooled linear-memory slot.
    pub max_memory_size: usize,
}

impl Default for PoolingAllocationConfig {
    fn default() -> Self {
        Self {
            total_memories: 64,
            total_tables: 64,
            total_core_instances: 64,
            total_component_instances: 64,
            max_memory_size: 64 * 1024 * 1024,
        }
    }
}

pub(crate) struct ExecEngine {
    engine: Engine,
    pooling_memory_slot_size: Option<usize>,
    #[cfg(test)]
    cache_hits: AtomicU64,
    #[cfg(test)]
    cache_writes: AtomicU64,
}

impl ExecEngine {
    #[allow(
        dead_code,
        reason = "direct execution tests exercise the on-demand engine"
    )]
    pub(crate) fn new() -> WasmtimeResult<Self> {
        Self::with_allocation(InstanceAllocation::default())
    }

    pub(crate) fn with_allocation(allocation: InstanceAllocation) -> WasmtimeResult<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .wasm_component_model_fixed_length_lists(true)
            .wasm_component_model_map(true)
            .concurrency_support(true)
            .consume_fuel(true);
        // Preserve shareable compiled memory images across fresh per-call instances.
        config.memory_init_cow(true);

        let pooling_memory_slot_size = match allocation {
            InstanceAllocation::OnDemand => None,
            InstanceAllocation::Pooling(settings) => {
                let mut pooling = WasmtimePoolingAllocationConfig::default();
                pooling
                    .total_memories(settings.total_memories)
                    .total_tables(settings.total_tables)
                    .total_stacks(settings.total_component_instances)
                    .total_core_instances(settings.total_core_instances)
                    .total_component_instances(settings.total_component_instances)
                    .max_memory_size(settings.max_memory_size);
                config.allocation_strategy(WasmtimeInstanceAllocationStrategy::Pooling(pooling));
                Some(settings.max_memory_size)
            }
        };

        Ok(Self {
            engine: Engine::new(&config)?,
            pooling_memory_slot_size,
            #[cfg(test)]
            cache_hits: AtomicU64::new(0),
            #[cfg(test)]
            cache_writes: AtomicU64::new(0),
        })
    }

    #[cfg(test)]
    pub(crate) fn new_pooling() -> WasmtimeResult<Self> {
        Self::with_allocation(InstanceAllocation::Pooling(
            PoolingAllocationConfig::default(),
        ))
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn pooling_memory_slot_size(&self) -> Option<usize> {
        self.pooling_memory_slot_size
    }

    pub(crate) fn compile(&self, bytes: &[u8]) -> Result<Component, LoadError> {
        Component::new(&self.engine, bytes).map_err(LoadError::compile)
    }

    pub(crate) fn compile_cached(
        &self,
        bytes: &[u8],
        component_digest: &[u8; 32],
        cache: Option<&CompiledComponentCache>,
    ) -> Result<Component, LoadError> {
        if let Some(component) = cache.and_then(|cache| cache.load(&self.engine, component_digest))
        {
            #[cfg(test)]
            self.cache_hits.fetch_add(1, Ordering::Relaxed);
            return Ok(component);
        }

        let component = self.compile(bytes)?;
        if cache.is_some_and(|cache| cache.store(&self.engine, component_digest, &component)) {
            #[cfg(test)]
            self.cache_writes.fetch_add(1, Ordering::Relaxed);
        }
        Ok(component)
    }

    #[cfg(test)]
    pub(crate) fn cache_hits(&self) -> u64 {
        self.cache_hits.load(Ordering::Relaxed)
    }

    #[cfg(test)]
    pub(crate) fn cache_writes(&self) -> u64 {
        self.cache_writes.load(Ordering::Relaxed)
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
        let component = self.compile(bytes)?;
        let mut linker = Linker::new(&self.engine);
        add_settings_to_linker(&mut linker).map_err(LoadError::link)?;
        add_wasi_to_linker(&mut linker).map_err(LoadError::link)?;
        imports(&mut linker).map_err(LoadError::link)?;
        let instance_pre = linker
            .instantiate_pre(&component)
            .map_err(LoadError::link)?;

        Ok(LoadedComponent {
            instance_pre,
            imports: Arc::new(UnitImports),
            plugin: None,
            jobs: None,
            settings: SettingsState::NotReady,
            environment: None,
            http_pool: None,
        })
    }

    pub(crate) fn load_hosted_component<S: Send + Sync + 'static>(
        &self,
        component: &Component,
        imports: Arc<dyn ImportsFactory<S>>,
        interfaces: &[String],
        has_http_egress: bool,
    ) -> Result<LoadedComponent<S>, LoadError> {
        let mut linker = Linker::new(&self.engine);
        add_settings_to_linker(&mut linker).map_err(LoadError::link)?;
        add_wasi_to_linker(&mut linker).map_err(LoadError::link)?;
        add_http_to_linker(&mut linker, has_http_egress).map_err(LoadError::link)?;
        imports
            .register(&mut linker, interfaces)
            .map_err(LoadError::link)?;
        let instance_pre = linker.instantiate_pre(component).map_err(LoadError::link)?;

        Ok(LoadedComponent {
            instance_pre,
            imports,
            plugin: None,
            jobs: None,
            settings: SettingsState::NotReady,
            environment: None,
            http_pool: None,
        })
    }
}

pub(crate) struct LoadedComponent<S: 'static> {
    instance_pre: InstancePre<StoreCtx<S>>,
    imports: Arc<dyn ImportsFactory<S>>,
    plugin: Option<Arc<dyn Any + Send + Sync>>,
    jobs: Option<DetachedJobContext>,
    settings: SettingsState,
    environment: Option<EnvironmentGrants>,
    http_pool: Option<Arc<HttpPool>>,
}

#[derive(Clone, Debug)]
pub(crate) struct EnvironmentGrants {
    plugin_id: PluginId,
    required: Vec<String>,
    optional: Vec<String>,
}

impl EnvironmentGrants {
    pub(crate) fn new(plugin_id: PluginId, required: Vec<String>, optional: Vec<String>) -> Self {
        Self {
            plugin_id,
            required,
            optional,
        }
    }

    fn wasi_context(&self) -> Result<WasiCtx, EnvironmentError> {
        let mut wasi = WasiCtxBuilder::new();
        for name in &self.required {
            let value = match std::env::var(name) {
                Ok(value) => value,
                Err(std::env::VarError::NotPresent) => {
                    return Err(EnvironmentError::RequiredUnset {
                        plugin_id: self.plugin_id.clone(),
                        variable: name.clone(),
                    });
                }
                Err(std::env::VarError::NotUnicode(_)) => {
                    return Err(EnvironmentError::RequiredNotUnicode {
                        plugin_id: self.plugin_id.clone(),
                        variable: name.clone(),
                    });
                }
            };
            wasi.env(name, value);
        }
        for name in &self.optional {
            if let Ok(value) = std::env::var(name) {
                wasi.env(name, value);
            }
        }
        Ok(wasi.build())
    }
}

impl<S: Send + Sync + 'static> LoadedComponent<S> {
    pub(crate) fn set_plugin<P: Clone + Send + Sync + 'static>(
        &mut self,
        plugin: P,
        jobs: DetachedJobContext,
    ) {
        self.plugin = Some(Arc::new(plugin));
        self.jobs = Some(jobs);
    }

    pub(crate) fn set_environment_grants(&mut self, environment: EnvironmentGrants) {
        self.environment = Some(environment);
    }

    pub(crate) fn set_settings(&mut self, settings: ValidatedSettings) {
        self.settings = settings.into_state();
    }

    pub(crate) fn set_http_pool(&mut self, http_pool: Arc<HttpPool>) {
        self.http_pool = Some(http_pool);
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
        deadline: Duration,
    ) -> Result<Vec<Val>, ExecError> {
        let mut store = self
            .configured_store(data, limits)
            .map_err(ExecError::Environment)?;
        store
            .set_fuel(limits.instantiation_fuel)
            .map_err(map_instantiate_error)?;

        let instance = self.instance_pre.instantiate_async(&mut store).await;
        if let Some(panic) = store.data().take_host_panic() {
            return Err(panic);
        }
        let instance = instance.map_err(map_instantiate_error)?;

        store
            .set_fuel(invocation_fuel)
            .map_err(map_dispatch_error)?;
        store
            .fuel_async_yield_interval(Some(DEADLINE_YIELD_FUEL))
            .map_err(map_dispatch_error)?;
        let Some(func) = instance.get_func(&mut store, export.func) else {
            return Err(ExecError::Dispatch(anyhow::anyhow!(
                "resolved component export was not a function"
            )));
        };
        let mut results = vec![Val::Bool(false); export.result_count];

        let call = store.run_concurrent(async |accessor| {
            func.call_concurrent(accessor, args, &mut results).await
        });
        let call_result = tokio::time::timeout(deadline, call)
            .await
            .map_err(|_| ExecError::DeadlineExceeded(deadline))?;
        if let Some(panic) = store.data().take_host_panic() {
            return Err(panic);
        }
        let call_result = call_result.map_err(map_call_error)?;
        call_result.map_err(map_call_error)?;

        Ok(results)
    }

    /// Instantiates the component under its runtime limits, startup fuel, and deadline.
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
        deadline: Duration,
    ) -> Result<(), ExecError> {
        let store = self
            .configured_store(data, limits)
            .map_err(ExecError::Environment)?;
        // Admission charges constructor work to the app-chosen startup budget;
        // `HostBuilder::admit` documents the deliberate steady-state asymmetry.
        self.smoke_store(store, limits.instantiation_fuel.min(startup_fuel), deadline)
            .await
    }

    fn configured_store(
        &self,
        data: S,
        limits: ExecLimits,
    ) -> Result<Store<StoreCtx<S>>, EnvironmentError> {
        let wasi = match &self.environment {
            Some(environment) => environment.wasi_context()?,
            None => WasiCtxBuilder::new().build(),
        };
        let mut store = Store::new(
            self.instance_pre.engine(),
            StoreCtx::new(
                data,
                self.imports.create(),
                self.plugin.clone(),
                self.jobs.clone(),
                self.settings.clone(),
                wasi,
                StoreRuntimeConfig {
                    limits,
                    http_pool: self.http_pool.clone(),
                },
            ),
        );
        store.limiter(|ctx| &mut ctx.limiter);
        Ok(store)
    }

    async fn smoke_store(
        &self,
        mut store: Store<StoreCtx<S>>,
        fuel: u64,
        deadline: Duration,
    ) -> Result<(), ExecError> {
        store.set_fuel(fuel).map_err(map_instantiate_error)?;
        store
            .fuel_async_yield_interval(Some(DEADLINE_YIELD_FUEL))
            .map_err(map_instantiate_error)?;
        let instantiate = self.instance_pre.instantiate_async(&mut store);
        let result = tokio::time::timeout(deadline, instantiate)
            .await
            .map_err(|_| ExecError::DeadlineExceeded(deadline))?;
        if let Some(panic) = store.data().take_host_panic() {
            return Err(panic);
        }
        result.map_err(map_instantiate_error)?;
        Ok(())
    }

    #[cfg(test)]
    pub(crate) async fn smoke_observing_drop(
        &self,
        data: S,
        limits: ExecLimits,
        startup_fuel: u64,
        deadline: Duration,
        dropped: Arc<AtomicBool>,
    ) -> Result<(), ExecError> {
        let mut store = self
            .configured_store(data, limits)
            .map_err(ExecError::Environment)?;
        store.data_mut().observe_drop(dropped);
        self.smoke_store(store, limits.instantiation_fuel.min(startup_fuel), deadline)
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
    pub(crate) max_host_import_calls: u64,
    pub(crate) http_request_timeout_ceiling: Option<Duration>,
}

struct StoreRuntimeConfig {
    limits: ExecLimits,
    http_pool: Option<Arc<HttpPool>>,
}

#[doc(hidden)]
pub type HostParts<S, I, P> = (I, Arc<S>, Arc<P>, DetachedJobContext, ResourceStore);

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
    jobs: Option<DetachedJobContext>,
    resources: ResourceStore,
    host_import_calls: u64,
    max_host_import_calls: u64,
    settings: SettingsState,
    wasi: WasiCtx,
    wasi_http: WasiHttpCtx,
    http_hooks: HttpHooks,
    host_panics: HostPanicState,
    wasi_resources: ResourceTable,
    #[cfg(test)]
    drop_probe: Option<StoreDropProbe>,
}

impl<S> StoreCtx<S> {
    fn new(
        data: S,
        imports: Box<dyn Any + Send>,
        plugin: Option<Arc<dyn Any + Send + Sync>>,
        jobs: Option<DetachedJobContext>,
        settings: SettingsState,
        wasi: WasiCtx,
        runtime: StoreRuntimeConfig,
    ) -> Self {
        let host_panics = HostPanicState::default();
        let http_hooks = HttpHooks::with_host_panics(
            plugin.clone(),
            runtime.limits.http_request_timeout_ceiling,
            host_panics.clone(),
            runtime.http_pool,
        );
        Self {
            limiter: MemoryLimiter {
                max_memory_bytes: runtime.limits.max_memory_bytes,
            },
            data: Arc::new(data),
            imports,
            plugin,
            jobs,
            resources: ResourceStore::__new(),
            host_import_calls: 0,
            max_host_import_calls: runtime.limits.max_host_import_calls,
            settings,
            // The context retains its deny-by-default network policy and no
            // filesystem preopens. Raw WASI sockets and paths therefore cannot
            // bypass Lockgate's capability checks.
            wasi,
            wasi_http: WasiHttpCtx::new(),
            http_hooks,
            host_panics,
            wasi_resources: ResourceTable::new(),
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

    pub(crate) fn settings(&self) -> &SettingsState {
        &self.settings
    }

    fn take_host_panic(&self) -> Option<ExecError> {
        self.host_panics.take().map(|panic| ExecError::HostPanic {
            import: panic.import,
            message: panic.message,
        })
    }

    #[doc(hidden)]
    pub fn host_parts<I, P>(&mut self) -> WasmtimeResult<HostParts<S, I, P>>
    where
        I: Clone + 'static,
        P: Send + Sync + 'static,
    {
        // Generated capability adapters enter here exactly once per call;
        // framework and WASI implementations use their dedicated Store views.
        if self.max_host_import_calls != 0 {
            if self.host_import_calls >= self.max_host_import_calls {
                return Err(host_import_call_limit_error(self.max_host_import_calls));
            }
            self.host_import_calls += 1;
        }
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
        let jobs = self
            .jobs
            .clone()
            .expect("public plugin Stores must carry detached-job context");
        Ok((
            imports,
            Arc::clone(&self.data),
            plugin,
            jobs,
            self.resources.clone(),
        ))
    }

    #[cfg(test)]
    pub(crate) fn observe_drop(&mut self, dropped: Arc<AtomicBool>) {
        self.drop_probe = Some(StoreDropProbe(dropped));
    }
}

impl<S: Send + Sync> WasiView for StoreCtx<S> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.wasi_resources,
        }
    }
}

impl<S: Send + Sync> WasiHttpView for StoreCtx<S> {
    fn http(&mut self) -> WasiHttpCtxView<'_> {
        WasiHttpCtxView {
            ctx: &mut self.wasi_http,
            table: &mut self.wasi_resources,
            hooks: &mut self.http_hooks,
        }
    }
}

fn add_wasi_to_linker<S: Send + Sync + 'static>(
    linker: &mut Linker<StoreCtx<S>>,
) -> WasmtimeResult<()> {
    wasmtime_wasi::p2::add_to_linker_async(linker)?;
    wasmtime_wasi::p3::add_to_linker(linker)?;
    Ok(())
}

pub(crate) fn add_http_to_linker<S: Send + Sync + 'static>(
    linker: &mut Linker<StoreCtx<S>>,
    has_http_egress: bool,
) -> WasmtimeResult<()> {
    if has_http_egress {
        wasmtime_wasi_http::p3::add_to_linker(linker)?;
    }
    Ok(())
}

pub(crate) trait ImportsFactory<S>: Send + Sync {
    fn register(
        &self,
        linker: &mut Linker<StoreCtx<S>>,
        interfaces: &[String],
    ) -> WasmtimeResult<()>;
    fn create(&self) -> Box<dyn Any + Send>;
}

type RegisterImports<S, I> = fn(&I, &mut Linker<StoreCtx<S>>, &[String]) -> WasmtimeResult<()>;

pub(crate) struct TypedImports<S: 'static, I> {
    imports: I,
    register: RegisterImports<S, I>,
    marker: PhantomData<fn() -> S>,
}

impl<S: 'static, I> TypedImports<S, I> {
    pub(crate) fn new(imports: I, register: RegisterImports<S, I>) -> Self {
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
    fn register(
        &self,
        linker: &mut Linker<StoreCtx<S>>,
        interfaces: &[String],
    ) -> WasmtimeResult<()> {
        (self.register)(&self.imports, linker, interfaces)
    }

    fn create(&self) -> Box<dyn Any + Send> {
        Box::new(self.imports.clone())
    }
}

#[allow(dead_code, reason = "used by the direct execution-test linker seam")]
struct UnitImports;

impl<S> ImportsFactory<S> for UnitImports {
    fn register(
        &self,
        _linker: &mut Linker<StoreCtx<S>>,
        _interfaces: &[String],
    ) -> WasmtimeResult<()> {
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
