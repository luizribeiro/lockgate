//! Validated construction and isolated execution of capability-scoped components.
//! Each component receives its own store, WASI context, application state, and fuel budget.

use crate::{
    Component,
    application::HostBindings,
    binding::Binding,
    catalog::{ApplicationError, Catalog, ComponentId},
    plan::{Plan, ResolvedImport, Target},
    policy::{DirectoryAccess, DirectoryGrant, Policy},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard, TryLockError, Weak},
};
use thiserror::Error;
use wasmtime::{
    Store,
    component::{Instance, InstancePre, Linker, ResourceTable, Val},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

const FUEL: u64 = 100_000;
const MAX_DEPTH: usize = 8;
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Frame {
    component: ComponentId,
}

thread_local! {
    static CALL_STACK: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

struct CallGuard(Frame);

impl Drop for CallGuard {
    fn drop(&mut self) {
        CALL_STACK.with(|stack| {
            let mut stack = stack.borrow_mut();
            if let Some(position) = stack.iter().rposition(|frame| *frame == self.0) {
                stack.remove(position);
            }
        });
    }
}

/// Runtime-owned component context and application-defined state exposed to host bindings.
pub struct HostContext<S = ()> {
    component_name: String,
    state: Arc<S>,
    resources: ResourceTable,
}

impl<S> HostContext<S> {
    fn new(component_name: String, state: Arc<S>) -> Self {
        Self {
            component_name,
            state,
            resources: ResourceTable::new(),
        }
    }

    /// Returns the application-assigned name of the component making host calls.
    pub fn component_name(&self) -> &str {
        &self.component_name
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

/// Per-component store data used by generated application bindings.
#[doc(hidden)]
pub struct PluginStore<S: Send + Sync + 'static = ()> {
    context: HostContext<S>,
    wasi: WasiCtx,
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
}

struct RuntimeTable<H: Send + Sync + 'static> {
    components: HashMap<ComponentId, Arc<Mutex<ComponentRuntime<H>>>>,
}

/// A fully instantiated set of isolated component stores.
pub struct Runtime<H: Send + Sync + 'static = ()> {
    plan: Plan,
    table: Arc<Mutex<RuntimeTable<H>>>,
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
    #[error("policy belongs to a different application")]
    ForeignPolicy,
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
    pub(crate) fn from_application(
        catalog: Catalog,
        policy: Policy,
        state: Arc<H>,
        host_bindings: HostBindings<H>,
    ) -> Result<Self, RuntimeBuildError> {
        let plan = Plan::new(catalog, policy)?;
        Self::build(plan, state, host_bindings)
    }

    fn build(
        plan: Plan,
        state: Arc<H>,
        host_bindings: HostBindings<H>,
    ) -> Result<Self, RuntimeBuildError> {
        let table = Arc::new(Mutex::new(RuntimeTable {
            components: HashMap::new(),
        }));
        let runtime = Self { plan, table };
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
            runtime.instantiate(component, &state, instance)?;
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
            .expect("runtime plan order contains only included components");
        let engine = self.plan.catalog.engine();
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        for interface in &component_plan.host_imports {
            host_bindings
                .install(component, interface, &mut linker)
                .expect("policy host grants contain only admitted host bindings")
                .map_err(|error| instantiate_error(&entry.name, error))?;
        }
        wire_direct_imports(
            &mut linker,
            &self.table,
            component_plan.direct_imports.iter(),
        )
        .map_err(|error| instantiate_error(&entry.name, error))?;
        linker
            .instantiate_pre(&entry.component)
            .map_err(|error| instantiate_error(&entry.name, error.into()))
    }

    fn instantiate(
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
            .expect("runtime plan order contains only included components");
        let engine = self.plan.catalog.engine();
        let mut wasi = WasiCtxBuilder::new();
        for directory in &component_plan.directories {
            preopen(&mut wasi, directory).map_err(|error| instantiate_error(&entry.name, error))?;
        }
        let state = PluginStore {
            context: HostContext::new(entry.name.clone(), Arc::clone(state)),
            wasi: wasi.build(),
        };
        let mut store = Store::new(engine, state);
        store
            .set_fuel(FUEL)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        let instance = instance
            .instantiate(&mut store)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        let runtime = Arc::new(Mutex::new(ComponentRuntime {
            store,
            instance,
            healthy: true,
        }));
        self.table
            .lock()
            .map_err(|_| {
                instantiate_error(
                    &entry.name,
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
    pub fn invoke<R>(
        &self,
        call: impl FnOnce(&mut Store<PluginStore<H>>, B) -> anyhow::Result<R>,
    ) -> Result<R, RuntimeError> {
        let id = self.component.id();
        let entry = self.runtime.plan.catalog.entry(id)?;
        invoke_component_runtime(&self.runtime.table, id, &entry.name, |runtime| {
            let ComponentRuntime {
                store, instance, ..
            } = runtime;
            let binding = B::bind(store, instance)?;
            call(store, binding)
        })
    }
}

impl<H: Send + Sync + 'static> ComponentRuntime<H> {
    fn call(&mut self, target: &Target, params: &[Val]) -> Result<Vec<Val>, anyhow::Error> {
        let interface = self
            .instance
            .get_export_index(&mut self.store, None, &target.interface)
            .ok_or_else(|| anyhow::anyhow!("interface is not exported"))?;
        let function = self
            .instance
            .get_export_index(&mut self.store, Some(&interface), &target.function)
            .ok_or_else(|| anyhow::anyhow!("function is not exported"))?;
        let function = self
            .instance
            .get_func(&mut self.store, function)
            .ok_or_else(|| anyhow::anyhow!("export is not a function"))?;
        let mut results = function
            .ty(&self.store)
            .results()
            .map(|_| Val::Bool(false))
            .collect::<Vec<_>>();
        function.call(&mut self.store, params, &mut results)?;
        Ok(results)
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
            instance.func_new(function, move |_store, _ty, params, results| {
                let values = invoke_target(&runtimes, &target, params)
                    .map_err(|error| wasmtime::Error::msg(error.to_string()))?;
                if values.len() != results.len() {
                    return Err(wasmtime::Error::msg("provider returned the wrong arity"));
                }
                for (result, value) in results.iter_mut().zip(values) {
                    *result = value;
                }
                Ok(())
            })?;
        }
    }
    Ok(())
}

fn invoke_target<H: Send + Sync + 'static>(
    runtimes: &Weak<Mutex<RuntimeTable<H>>>,
    target: &Target,
    params: &[Val],
) -> Result<Vec<Val>, RuntimeError> {
    let runtimes = runtimes
        .upgrade()
        .ok_or_else(|| RuntimeError::Unavailable {
            component: target.component_name.clone(),
        })?;
    invoke_component_runtime(
        &runtimes,
        target.component,
        &target.component_name,
        |runtime| runtime.call(target, params),
    )
}

fn invoke_component_runtime<H: Send + Sync + 'static, R>(
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    component: ComponentId,
    component_name: &str,
    call: impl FnOnce(&mut ComponentRuntime<H>) -> anyhow::Result<R>,
) -> Result<R, RuntimeError> {
    let _guard = enter_component(component, component_name)?;
    let runtime = runtime_for(runtimes, component, component_name)?;
    let mut runtime = try_runtime_lock(&runtime, component_name)?;
    if !runtime.healthy {
        return Err(RuntimeError::Unhealthy {
            component: component_name.into(),
        });
    }
    runtime
        .store
        .set_fuel(FUEL)
        .map_err(|source| RuntimeError::Trapped {
            component: component_name.into(),
            source: source.into(),
        })?;
    let result = call(&mut runtime);
    if result.is_err() {
        runtime.healthy = false;
    }
    result.map_err(|source| RuntimeError::Trapped {
        component: component_name.into(),
        source,
    })
}

fn runtime_for<H: Send + Sync + 'static>(
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    component: ComponentId,
    name: &str,
) -> Result<Arc<Mutex<ComponentRuntime<H>>>, RuntimeError> {
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
    runtime: &'a Mutex<ComponentRuntime<H>>,
    name: &str,
) -> Result<MutexGuard<'a, ComponentRuntime<H>>, RuntimeError> {
    match runtime.try_lock() {
        Ok(runtime) => Ok(runtime),
        Err(TryLockError::WouldBlock) => Err(RuntimeError::Busy {
            component: name.into(),
        }),
        Err(TryLockError::Poisoned(_)) => Err(RuntimeError::Poisoned),
    }
}

#[cfg(test)]
fn enter_call(target: &Target) -> Result<CallGuard, RuntimeError> {
    enter_component(target.component, &target.component_name)
}

fn enter_component(
    component: ComponentId,
    component_name: &str,
) -> Result<CallGuard, RuntimeError> {
    let frame = Frame { component };
    CALL_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let runtime_depth = stack
            .iter()
            .filter(|item| item.component.catalog == component.catalog)
            .count();
        if runtime_depth >= MAX_DEPTH {
            return Err(RuntimeError::DepthLimit);
        }
        if stack.contains(&frame) {
            return Err(RuntimeError::Cycle {
                component: component_name.into(),
            });
        }
        stack.push(frame);
        Ok(CallGuard(frame))
    })
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
    let (dirs, files) = match grant.access {
        DirectoryAccess::ReadOnly => (DirPerms::READ, FilePerms::READ),
        DirectoryAccess::ReadWrite => (DirPerms::all(), FilePerms::all()),
    };
    let guest = grant
        .guest
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("guest path is not UTF-8"))?;
    wasi.preopened_dir(&grant.host, guest, dirs, files)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Application, binding::Binding, catalog::Catalog, policy::Policy};
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    fn provider_bytes() -> Vec<u8> {
        let wit = r#"package demo:stack@0.1.0;
interface api { run: func(); }
world provider { export api; }"#;
        let mut resolve = Resolve::new();
        let package = resolve.push_str("stack.wit", wit).unwrap();
        let world = resolve.packages[package].worlds["provider"];
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap()
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
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap()
    }

    struct FailingHostBinding;

    impl Binding<()> for FailingHostBinding {
        const WORLD: &'static str = "consumer";
        const EXPORTS: &'static [crate::binding::BindingExport] = &[];
        const HOST_IMPORTS: &'static [&'static str] = &["demo:stack/api@0.1.0"];

        type Client<'runtime> = ();

        fn bind(_store: &mut Store<PluginStore<()>>, _instance: &Instance) -> anyhow::Result<Self> {
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

    #[test]
    fn host_contexts_share_application_state() {
        let state = Arc::new(vec![1]);
        let mut first = HostContext::new("first".into(), Arc::clone(&state));
        let second = HostContext::new("second".into(), state);

        assert_eq!(first.component_name(), "first");
        assert_eq!(second.component_name(), "second");
        assert!(std::ptr::eq(first.state(), second.state()));
        assert!(first.resources_mut().is_empty());
    }

    #[test]
    fn call_guards_reject_cycles_and_depth_without_leaking_frames() {
        let mut catalog = Catalog::new().unwrap();
        let mut components = Vec::new();
        for index in 0..=MAX_DEPTH {
            let component = catalog
                .add_untyped(format!("provider-{index}"), provider_bytes())
                .unwrap();
            components.push(component);
        }
        let mut policy = Policy::new(catalog.identity());
        for component in &components {
            policy.include(*component);
        }
        let plan = Plan::new(catalog, policy).unwrap();
        let targets = components
            .into_iter()
            .map(|component| {
                let entry = plan.catalog.entry(component).unwrap();
                let export = &entry.exports[0];
                Target {
                    component,
                    component_name: entry.name.clone(),
                    interface: export.interface.clone(),
                    function: export.function.clone(),
                }
            })
            .collect::<Vec<_>>();

        let first = enter_call(&targets[0]).unwrap();
        assert!(matches!(
            enter_call(&targets[0]),
            Err(RuntimeError::Cycle { .. })
        ));
        drop(first);

        let guards = targets[..MAX_DEPTH]
            .iter()
            .map(|target| enter_call(target).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            enter_call(&targets[MAX_DEPTH]),
            Err(RuntimeError::DepthLimit)
        ));
        drop(guards);
        assert!(enter_call(&targets[0]).is_ok());
    }

    #[test]
    fn all_linkers_are_preflighted_before_any_store_is_created() {
        let mut app = Application::new(()).unwrap();
        let first = app.add_untyped("first", provider_bytes()).unwrap();
        let second = app
            .add::<FailingHostBinding>("second", consumer_bytes())
            .unwrap();
        let result = app
            .include_id(first)
            .unwrap()
            .allow_host_import(second, "demo:stack/api@0.1.0")
            .unwrap()
            .runtime();

        assert!(matches!(
            result,
            Err(RuntimeBuildError::Instantiation { component, .. }) if component == "second"
        ));
    }
}
