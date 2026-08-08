//! Validated construction and isolated execution of capability-scoped components.
//! Each component receives its own store, WASI context, and fuel budget while sharing application state.

use crate::{
    Component,
    application::HostBindings,
    binding::Binding,
    catalog::{ApplicationError, Catalog, ComponentId},
    grants::{DirectoryAccess, DirectoryGrant, Grants},
    plan::{Plan, ResolvedImport, Target},
};
use lockgate_schema::PluginMetadata;
use std::{
    collections::HashMap,
    future::Future,
    pin::Pin,
    sync::{Arc, Mutex, Weak},
};
use thiserror::Error;
use tokio::sync::{Mutex as AsyncMutex, MutexGuard as AsyncMutexGuard};
use wasmtime::{
    Store,
    component::{Accessor, Instance, InstancePre, Linker, ResourceTable, Val},
};
use wasmtime_wasi::{FsPerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

pub(crate) const DEFAULT_FUEL_PER_CALL: u64 = 100_000;
const MAX_DEPTH: usize = 8;

type RuntimeCall<'a, R> = Pin<Box<dyn Future<Output = anyhow::Result<R>> + Send + 'a>>;

/// Runtime-owned component context and application-defined state exposed to host bindings.
pub struct HostContext<S = ()> {
    plugin: PluginMetadata,
    state: Arc<S>,
    resources: ResourceTable,
}

impl<S> HostContext<S> {
    fn new(plugin: PluginMetadata, state: Arc<S>) -> Self {
        Self {
            plugin,
            state,
            resources: ResourceTable::new(),
        }
    }

    /// Returns the metadata embedded by the plugin making host calls.
    pub fn plugin(&self) -> &PluginMetadata {
        &self.plugin
    }

    /// Returns the application-defined state associated with this component.
    pub fn state(&self) -> &S {
        self.state.as_ref()
    }

    /// Returns the resource table shared by application host bindings and WASI.
    pub fn resources_mut(&mut self) -> &mut ResourceTable {
        &mut self.resources
    }
}

impl<S: 'static> wasmtime::component::HasData for HostContext<S> {
    type Data<'a> = &'a mut Self;
}

/// Projects a plugin store into the application host context for generated bindings.
#[doc(hidden)]
pub fn host_context_data<S: Send + Sync + 'static>(
    store: &mut PluginStore<S>,
) -> <HostContext<S> as wasmtime::component::HasData>::Data<'_> {
    store.context_mut()
}

/// Per-component store data used by generated application bindings.
#[doc(hidden)]
pub struct PluginStore<S: Send + Sync + 'static = ()> {
    context: HostContext<S>,
    wasi: WasiCtx,
    call_path: Vec<ComponentId>,
}

impl<S: Send + Sync + 'static> PluginStore<S> {
    /// Returns the mutable component context projected into generated host bindings.
    #[doc(hidden)]
    pub fn context_mut(&mut self) -> &mut HostContext<S> {
        &mut self.context
    }
}

impl<S: Send + Sync + 'static> WasiView for PluginStore<S> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.context.resources,
        }
    }
}

struct ComponentRuntime<H: Send + Sync + 'static> {
    store: Store<PluginStore<H>>,
    instance: Instance,
    healthy: bool,
    fuel_per_call: u64,
}

struct RuntimeTable<H: Send + Sync + 'static> {
    components: HashMap<ComponentId, Arc<AsyncMutex<ComponentRuntime<H>>>>,
}

/// A fully instantiated set of isolated component stores.
pub struct Runtime<H: Send + Sync + 'static = ()> {
    plan: Plan,
    table: Arc<Mutex<RuntimeTable<H>>>,
    fuel_per_call: u64,
}

/// Runtime access retained by generated component clients.
#[doc(hidden)]
pub struct RuntimeComponent<'runtime, H: Send + Sync + 'static, B> {
    runtime: &'runtime Runtime<H>,
    component: Component<B>,
}

impl<H: Send + Sync + 'static, B> Copy for RuntimeComponent<'_, H, B> {}

impl<H: Send + Sync + 'static, B> Clone for RuntimeComponent<'_, H, B> {
    fn clone(&self) -> Self {
        *self
    }
}

/// A typed failure while validating or instantiating a runtime.
#[derive(Debug, Error)]
pub enum RuntimeBuildError {
    #[error("component `{caller}` has multiple authorized providers for `{interface}`")]
    AmbiguousProvider { caller: String, interface: String },
    #[error("component `{caller}` has no authorized provider for `{interface}`")]
    MissingProvider { caller: String, interface: String },
    #[error("provider `{provider}` does not export `{target}` required by `{caller}`")]
    MissingFunction {
        caller: String,
        provider: String,
        target: String,
    },
    #[error("type mismatch for {target}: caller expects {expected}, provider exports {actual}")]
    TypeMismatch {
        target: String,
        expected: String,
        actual: String,
    },
    #[error("sibling target `{target}` uses unsupported cross-store types: {reason}")]
    UnsupportedSiblingType { target: String, reason: String },
    #[error("component `{component}` has multiple grants for guest directory `{guest}`")]
    DuplicateGuestDirectory { component: String, guest: String },
    #[error("component `{component}` failed to instantiate")]
    Instantiation {
        component: String,
        #[source]
        source: anyhow::Error,
    },
}

/// A typed failure while calling an instantiated component.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Application(#[from] ApplicationError),
    #[error("component `{component}` is unavailable")]
    Unavailable { component: String },
    #[error("component `{component}` is unhealthy")]
    Unhealthy { component: String },
    #[error("component `{component}` store is already borrowed")]
    Busy { component: String },
    #[error("call depth limit {MAX_DEPTH} exceeded")]
    DepthLimit,
    #[error("call cycle detected at component `{component}`")]
    Cycle { component: String },
    #[error("component `{component}` trapped")]
    Trapped {
        component: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("runtime synchronization state was poisoned")]
    Poisoned,
}

impl<H: Send + Sync + 'static> Runtime<H> {
    pub(crate) async fn from_application(
        catalog: Catalog,
        grants: Grants,
        state: Arc<H>,
        host_bindings: HostBindings<H>,
        fuel_per_call: u64,
    ) -> Result<Self, RuntimeBuildError> {
        let plan = Plan::new(catalog, grants)?;
        Self::build(plan, state, host_bindings, fuel_per_call).await
    }

    async fn build(
        plan: Plan,
        state: Arc<H>,
        host_bindings: HostBindings<H>,
        fuel_per_call: u64,
    ) -> Result<Self, RuntimeBuildError> {
        let table = Arc::new(Mutex::new(RuntimeTable {
            components: HashMap::new(),
        }));
        let runtime = Self {
            plan,
            table,
            fuel_per_call,
        };
        let prepared = runtime
            .plan
            .order
            .iter()
            .map(|component| {
                runtime
                    .prepare_instance(*component, &host_bindings)
                    .map(|instance| (*component, instance))
            })
            .collect::<Result<Vec<_>, _>>()?;
        for (component, instance) in prepared {
            runtime.instantiate(component, &state, instance).await?;
        }
        Ok(runtime)
    }

    /// Creates a lightweight generated client for an admitted component role.
    ///
    /// The client performs no work until one of its WIT methods is called.
    pub fn component<B: Binding<H>>(&self, component: Component<B>) -> B::Client<'_> {
        B::client(RuntimeComponent::new(self, component))
    }

    fn prepare_instance(
        &self,
        component: ComponentId,
        host_bindings: &HostBindings<H>,
    ) -> Result<InstancePre<PluginStore<H>>, RuntimeBuildError> {
        let entry = self
            .plan
            .catalog
            .entry(component)
            .expect("runtime plan order contains only cataloged components");
        let component_plan = self
            .plan
            .components
            .get(&component)
            .expect("runtime plan order contains only application components");
        let engine = self.plan.catalog.engine();
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p3::add_to_linker(&mut linker)
            .map_err(|error| instantiate_error(entry.metadata.id(), error.into()))?;
        for interface in &component_plan.host_imports {
            host_bindings
                .install(component, interface, &mut linker)
                .expect("host grants contain only admitted host bindings")
                .map_err(|error| instantiate_error(entry.metadata.id(), error))?;
        }
        wire_direct_imports(
            &mut linker,
            &self.table,
            component_plan.direct_imports.iter(),
        )
        .map_err(|error| instantiate_error(entry.metadata.id(), error))?;
        linker
            .instantiate_pre(&entry.component)
            .map_err(|error| instantiate_error(entry.metadata.id(), error.into()))
    }

    async fn instantiate(
        &self,
        component: ComponentId,
        state: &Arc<H>,
        instance: InstancePre<PluginStore<H>>,
    ) -> Result<(), RuntimeBuildError> {
        let entry = self
            .plan
            .catalog
            .entry(component)
            .expect("runtime plan order contains only cataloged components");
        let component_plan = self
            .plan
            .components
            .get(&component)
            .expect("runtime plan order contains only application components");
        let engine = self.plan.catalog.engine();
        let mut wasi = WasiCtxBuilder::new();
        for directory in &component_plan.directories {
            preopen(&mut wasi, directory)
                .map_err(|error| instantiate_error(entry.metadata.id(), error))?;
        }
        let state = PluginStore {
            context: HostContext::new(entry.metadata.clone(), Arc::clone(state)),
            wasi: wasi.build(),
            call_path: Vec::new(),
        };
        let mut store = Store::new(engine, state);
        store
            .set_fuel(self.fuel_per_call)
            .map_err(|error| instantiate_error(entry.metadata.id(), error.into()))?;
        let instance = instance
            .instantiate_async(&mut store)
            .await
            .map_err(|error| instantiate_error(entry.metadata.id(), error.into()))?;
        let runtime = Arc::new(AsyncMutex::new(ComponentRuntime {
            store,
            instance,
            healthy: true,
            fuel_per_call: self.fuel_per_call,
        }));
        self.table
            .lock()
            .map_err(|_| {
                instantiate_error(
                    entry.metadata.id(),
                    anyhow::anyhow!("runtime synchronization state was poisoned"),
                )
            })?
            .components
            .insert(component, runtime);
        Ok(())
    }
}

impl<'runtime, H: Send + Sync + 'static, B: Binding<H>> RuntimeComponent<'runtime, H, B> {
    /// Creates the internal runtime view used by generated clients.
    #[doc(hidden)]
    pub(crate) fn new(runtime: &'runtime Runtime<H>, component: Component<B>) -> Self {
        Self { runtime, component }
    }

    /// Invokes a generated binding while preserving Lockgate's call invariants.
    #[doc(hidden)]
    pub async fn invoke<R>(
        &self,
        call: impl for<'a> FnOnce(&'a Accessor<PluginStore<H>>, B) -> RuntimeCall<'a, R>
        + Send
        + 'static,
    ) -> Result<R, RuntimeError>
    where
        R: Send,
    {
        let id = self.component.id();
        let entry = self.runtime.plan.catalog.entry(id)?;
        invoke_component_runtime(
            &self.runtime.table,
            id,
            entry.metadata.id(),
            &[],
            |runtime| {
                Box::pin(async move {
                    let instance = runtime.instance;
                    runtime
                        .store
                        .run_concurrent(async |accessor| {
                            let binding = B::bind(accessor, &instance)?;
                            call(accessor, binding).await
                        })
                        .await?
                })
            },
        )
        .await
    }
}

impl<H: Send + Sync + 'static> ComponentRuntime<H> {
    async fn call(&mut self, target: &Target, params: &[Val]) -> Result<Vec<Val>, anyhow::Error> {
        let instance = self.instance;
        self.store
            .run_concurrent(async |accessor| {
                let (function, mut results) = accessor.with(|mut access| {
                    let interface = instance
                        .get_export_index(&mut access, None, &target.interface)
                        .ok_or_else(|| anyhow::anyhow!("interface is not exported"))?;
                    let function = instance
                        .get_export_index(&mut access, Some(&interface), &target.function)
                        .ok_or_else(|| anyhow::anyhow!("function is not exported"))?;
                    let function = instance
                        .get_func(&mut access, function)
                        .ok_or_else(|| anyhow::anyhow!("export is not a function"))?;
                    let results = function
                        .ty(&access)
                        .results()
                        .map(|_| Val::Bool(false))
                        .collect::<Vec<_>>();
                    Ok::<_, anyhow::Error>((function, results))
                })?;
                function
                    .call_concurrent(accessor, params, &mut results)
                    .await?;
                Ok(results)
            })
            .await?
    }
}

fn wire_direct_imports<'a, H: Send + Sync + 'static>(
    linker: &mut Linker<PluginStore<H>>,
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    imports: impl Iterator<Item = (&'a String, &'a ResolvedImport)>,
) -> Result<(), anyhow::Error> {
    for (interface, import) in imports {
        let mut instance = linker.instance(interface)?;
        for (function, target) in &import.functions {
            let target = target.clone();
            let runtimes = Arc::downgrade(runtimes);
            instance.func_new_concurrent(function, move |accessor, _ty, params, results| {
                let target = target.clone();
                let runtimes = runtimes.clone();
                Box::pin(async move {
                    let call_path = accessor.with(|mut access| access.data_mut().call_path.clone());
                    let params = params.to_vec();
                    let values = tokio::spawn(async move {
                        invoke_target(&runtimes, &target, &params, &call_path).await
                    })
                    .await
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))?
                    .map_err(wasmtime::Error::new)?;
                    if values.len() != results.len() {
                        return Err(wasmtime::Error::msg("provider returned the wrong arity"));
                    }
                    for (result, value) in results.iter_mut().zip(values) {
                        *result = value;
                    }
                    Ok(())
                })
            })?;
        }
    }
    Ok(())
}

async fn invoke_target<H: Send + Sync + 'static>(
    runtimes: &Weak<Mutex<RuntimeTable<H>>>,
    target: &Target,
    params: &[Val],
    call_path: &[ComponentId],
) -> Result<Vec<Val>, RuntimeError> {
    let runtimes = runtimes
        .upgrade()
        .ok_or_else(|| RuntimeError::Unavailable {
            component: target.plugin_id.clone(),
        })?;
    let target = target.clone();
    let plugin_id = target.plugin_id.clone();
    let params = params.to_vec();
    invoke_component_runtime(
        &runtimes,
        target.component,
        &plugin_id,
        call_path,
        |runtime| Box::pin(async move { runtime.call(&target, &params).await }),
    )
    .await
}

async fn invoke_component_runtime<H: Send + Sync + 'static, R>(
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    component: ComponentId,
    plugin_id: &str,
    call_path: &[ComponentId],
    call: impl for<'a> FnOnce(&'a mut ComponentRuntime<H>) -> RuntimeCall<'a, R>,
) -> Result<R, RuntimeError>
where
    R: Send,
{
    let call_path = enter_component(call_path, component, plugin_id)?;
    let runtime = runtime_for(runtimes, component, plugin_id)?;
    let mut runtime = try_runtime_lock(&runtime, plugin_id)?;
    if !runtime.healthy {
        return Err(RuntimeError::Unhealthy {
            component: plugin_id.into(),
        });
    }
    let fuel_per_call = runtime.fuel_per_call;
    runtime
        .store
        .set_fuel(fuel_per_call)
        .map_err(|source| RuntimeError::Trapped {
            component: plugin_id.into(),
            source: source.into(),
        })?;
    let previous_path = std::mem::replace(&mut runtime.store.data_mut().call_path, call_path);
    let result = call(&mut runtime).await;
    runtime.store.data_mut().call_path = previous_path;
    if result.is_err() {
        runtime.healthy = false;
    }
    result.map_err(|source| RuntimeError::Trapped {
        component: plugin_id.into(),
        source,
    })
}

fn runtime_for<H: Send + Sync + 'static>(
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    component: ComponentId,
    name: &str,
) -> Result<Arc<AsyncMutex<ComponentRuntime<H>>>, RuntimeError> {
    runtimes
        .lock()
        .map_err(|_| RuntimeError::Poisoned)?
        .components
        .get(&component)
        .cloned()
        .ok_or_else(|| RuntimeError::Unavailable {
            component: name.into(),
        })
}

fn try_runtime_lock<'a, H: Send + Sync + 'static>(
    runtime: &'a AsyncMutex<ComponentRuntime<H>>,
    name: &str,
) -> Result<AsyncMutexGuard<'a, ComponentRuntime<H>>, RuntimeError> {
    match runtime.try_lock() {
        Ok(runtime) => Ok(runtime),
        Err(_) => Err(RuntimeError::Busy {
            component: name.into(),
        }),
    }
}

#[cfg(test)]
fn enter_call(
    call_path: &[ComponentId],
    target: &Target,
) -> Result<Vec<ComponentId>, RuntimeError> {
    enter_component(call_path, target.component, &target.plugin_id)
}

fn enter_component(
    call_path: &[ComponentId],
    component: ComponentId,
    plugin_id: &str,
) -> Result<Vec<ComponentId>, RuntimeError> {
    let runtime_depth = call_path
        .iter()
        .filter(|item| item.catalog == component.catalog)
        .count();
    if runtime_depth >= MAX_DEPTH {
        return Err(RuntimeError::DepthLimit);
    }
    if call_path.contains(&component) {
        return Err(RuntimeError::Cycle {
            component: plugin_id.into(),
        });
    }
    let mut next = call_path.to_vec();
    next.push(component);
    Ok(next)
}

fn instantiate_error(component: &str, source: anyhow::Error) -> RuntimeBuildError {
    RuntimeBuildError::Instantiation {
        component: component.into(),
        source,
    }
}

fn preopen(wasi: &mut WasiCtxBuilder, grant: &DirectoryGrant) -> Result<(), anyhow::Error> {
    if !grant.host.is_dir() {
        anyhow::bail!("host path {} is not a directory", grant.host.display());
    }
    let perms = match grant.access {
        DirectoryAccess::ReadOnly => FsPerms::ReadOnly,
        DirectoryAccess::ReadWrite => FsPerms::ReadWrite,
    };
    let guest = grant
        .guest
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("guest path is not UTF-8"))?;
    wasi.preopened_dir(&grant.host, guest, perms)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Application, binding::Binding, catalog::Catalog, grants::Grants};
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    fn provider_bytes(plugin_id: &str) -> Vec<u8> {
        let wit = r#"package demo:stack@0.1.0;
interface api { run: func(); }
world provider { export api; }"#;
        let mut resolve = Resolve::new();
        let package = resolve.push_str("stack.wit", wit).unwrap();
        let world = resolve.packages[package].worlds["provider"];
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
        let bytes = ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap();
        crate::catalog::with_test_plugin_metadata(bytes, plugin_id)
    }

    fn consumer_bytes() -> Vec<u8> {
        let wit = r#"package demo:stack@0.1.0;
interface api { run: func(); }
world consumer { import api; }"#;
        let mut resolve = Resolve::new();
        let package = resolve.push_str("stack.wit", wit).unwrap();
        let world = resolve.packages[package].worlds["consumer"];
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
        let bytes = ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap();
        crate::catalog::with_test_plugin_metadata(bytes, "second")
    }

    struct FailingHostBinding;

    impl Binding<()> for FailingHostBinding {
        const WORLD: &'static str = "consumer";
        const EXPORTS: &'static [crate::binding::BindingExport] = &[];
        const HOST_IMPORTS: &'static [&'static str] = &["demo:stack/api@0.1.0"];

        type Client<'runtime> = ();

        fn bind(
            _accessor: &Accessor<PluginStore<()>>,
            _instance: &Instance,
        ) -> anyhow::Result<Self> {
            Ok(Self)
        }

        fn install_host_import(
            _interface: &str,
            _linker: &mut Linker<PluginStore<()>>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("deliberate test installer failure")
        }

        fn client<'runtime>(
            _component: RuntimeComponent<'runtime, (), Self>,
        ) -> Self::Client<'runtime>
        where
            (): 'runtime,
        {
        }
    }

    impl crate::binding::RoleSet<()> for FailingHostBinding {
        type Handles = Component<Self>;

        #[allow(clippy::type_complexity)]
        fn for_each_role(
            visitor: &mut dyn FnMut(
                &'static str,
                &'static [crate::binding::BindingExport],
                &'static [&'static str],
                fn(&str, &mut Linker<PluginStore<()>>) -> anyhow::Result<()>,
            ),
        ) {
            visitor(
                <Self as Binding<()>>::WORLD,
                <Self as Binding<()>>::EXPORTS,
                <Self as Binding<()>>::HOST_IMPORTS,
                <Self as Binding<()>>::install_host_import,
            );
        }

        fn handles(component: Component<Self>) -> Self::Handles {
            component
        }
    }

    #[test]
    fn host_contexts_share_application_state() {
        let state = Arc::new(vec![1]);
        let mut first = HostContext::new(
            PluginMetadata::new("first", "First", "0.1.0").unwrap(),
            Arc::clone(&state),
        );
        let second = HostContext::new(
            PluginMetadata::new("second", "Second", "0.1.0").unwrap(),
            state,
        );

        assert_eq!(first.plugin().id(), "first");
        assert_eq!(second.plugin().id(), "second");
        assert!(std::ptr::eq(first.state(), second.state()));
        assert!(first.resources_mut().is_empty());
    }

    #[test]
    fn call_paths_reject_cycles_and_excessive_depth() {
        let mut catalog = Catalog::new().unwrap();
        let mut components = Vec::new();
        for index in 0..=MAX_DEPTH {
            let plugin_id = format!("provider-{index}");
            let component = catalog.add_untyped(provider_bytes(&plugin_id)).unwrap();
            components.push(component);
        }
        let plan = Plan::new(catalog, Grants::default()).unwrap();
        let targets = components
            .into_iter()
            .map(|component| {
                let entry = plan.catalog.entry(component).unwrap();
                let export = &entry.exports[0];
                Target {
                    component,
                    plugin_id: entry.metadata.id().into(),
                    interface: export.interface.clone(),
                    function: export.function.clone(),
                }
            })
            .collect::<Vec<_>>();

        let first = enter_call(&[], &targets[0]).unwrap();
        assert!(matches!(
            enter_call(&first, &targets[0]),
            Err(RuntimeError::Cycle { .. })
        ));
        let path = targets[..MAX_DEPTH]
            .iter()
            .fold(Vec::new(), |path, target| {
                enter_call(&path, target).unwrap()
            });
        assert!(matches!(
            enter_call(&path, &targets[MAX_DEPTH]),
            Err(RuntimeError::DepthLimit)
        ));
        assert!(enter_call(&[], &targets[0]).is_ok());
    }

    #[tokio::test]
    async fn all_linkers_are_preflighted_before_any_store_is_created() {
        let mut app = Application::new(()).unwrap();
        app.add_untyped(provider_bytes("first")).unwrap();
        let second = app.add::<FailingHostBinding>(consumer_bytes()).unwrap();
        let result = app
            .allow_host_import(second, "demo:stack/api@0.1.0")
            .unwrap()
            .run()
            .await;

        assert!(matches!(
            result,
            Err(RuntimeBuildError::Instantiation { component, .. }) if component == "second"
        ));
    }
}
