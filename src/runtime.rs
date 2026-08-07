//! Validated construction and isolated execution of capability-scoped components.
//! Each component receives its own store, WASI context, application state, and fuel budget.

use crate::{
    Component, ComponentId, ComponentRef,
    binding::ComponentBinding,
    catalog::{Catalog, CatalogError},
    plan::{Plan, ResolvedImport, Target},
    policy::{DirectoryAccess, DirectoryGrant, Policy},
};
use std::{
    cell::RefCell,
    collections::HashMap,
    sync::{
        Arc, Mutex, MutexGuard, TryLockError, Weak,
        atomic::{AtomicU64, Ordering},
    },
};
use thiserror::Error;
use wasmtime::{
    Store,
    component::{HasData, Instance, InstancePre, Linker, ResourceTable, Val},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

const FUEL: u64 = 100_000;
const MAX_DEPTH: usize = 8;
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

type Observer = Arc<dyn Fn(Event) + Send + Sync>;
type StateFactory<S> = Arc<dyn Fn(ComponentId, &str) -> S + Send + Sync>;
type LinkerConfig<S> =
    Arc<dyn Fn(ComponentId, &mut Linker<PluginStore<S>>) -> anyhow::Result<()> + Send + Sync>;

/// Projects generated host bindings from a [`PluginStore`] to its host context.
pub struct HasHost<S>(std::marker::PhantomData<fn() -> S>);

impl<S: 'static> HasData for HasHost<S> {
    type Data<'a> = &'a mut HostContext<S>;
}

/// Internal projection used by generated application binding installers.
#[doc(hidden)]
pub struct HostContextData<S>(std::marker::PhantomData<fn() -> S>);

impl<S: 'static> HasData for HostContextData<S> {
    type Data<'a> = &'a mut HostContext<S>;
}

/// An observable cross-component broker event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    SiblingCall {
        caller: String,
        provider: String,
        target: String,
    },
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct Frame {
    runtime: u64,
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

/// The identity of the component associated with a host call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostComponent {
    id: ComponentId,
    name: String,
}

impl HostComponent {
    /// Returns the catalog identity of this component.
    pub fn id(&self) -> ComponentId {
        self.id
    }

    /// Returns the application-assigned component name.
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Runtime-owned component context and application-defined state exposed to host bindings.
pub struct HostContext<S = ()> {
    component: HostComponent,
    state: S,
}

impl<S> HostContext<S> {
    fn new(component: ComponentId, name: String, state: S) -> Self {
        Self {
            component: HostComponent {
                id: component,
                name,
            },
            state,
        }
    }

    /// Returns the identity of the component making host calls.
    pub fn component(&self) -> &HostComponent {
        &self.component
    }

    /// Returns the application-defined state associated with this component.
    pub fn state(&self) -> &S {
        &self.state
    }

    /// Returns mutable application-defined state associated with this component.
    pub fn state_mut(&mut self) -> &mut S {
        &mut self.state
    }
}

/// Per-component store data available to application-defined host bindings.
pub struct PluginStore<S: Send + 'static = ()> {
    context: HostContext<S>,
    wasi: WasiCtx,
    resources: ResourceTable,
}

impl<S: Send + 'static> PluginStore<S> {
    /// Returns the component context projected into generated host bindings.
    pub fn context(&self) -> &HostContext<S> {
        &self.context
    }

    /// Returns the mutable component context projected into generated host bindings.
    pub fn context_mut(&mut self) -> &mut HostContext<S> {
        &mut self.context
    }

    pub fn component_name(&self) -> &str {
        self.context.component.name()
    }

    pub fn resources_mut(&mut self) -> &mut ResourceTable {
        &mut self.resources
    }
}

impl<S: Send + 'static> WasiView for PluginStore<S> {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.resources,
        }
    }
}

struct ComponentRuntime<H: Send + 'static> {
    store: Store<PluginStore<H>>,
    instance: Instance,
    healthy: bool,
}

struct RuntimeTable<H: Send + 'static> {
    identity: u64,
    components: HashMap<ComponentId, Arc<Mutex<ComponentRuntime<H>>>>,
    observer: Option<Observer>,
}

/// A fully instantiated set of isolated component stores.
pub struct Runtime<H: Send + 'static = ()> {
    plan: Plan,
    table: Arc<Mutex<RuntimeTable<H>>>,
}

/// Configures a runtime before any component is instantiated.
pub struct RuntimeBuilder<H: Send + 'static = ()> {
    catalog: Catalog,
    policy: Policy,
    observer: Option<Observer>,
    state_factory: StateFactory<H>,
    configure_linker: LinkerConfig<H>,
}

/// A typed failure while validating or instantiating a runtime.
#[derive(Debug, Error)]
pub enum RuntimeBuildError {
    #[error("policy belongs to a different catalog")]
    ForeignPolicy,
    #[error(transparent)]
    Catalog(#[from] CatalogError),
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
    #[error("component `{component}` is not included in the policy")]
    NotIncluded { component: String },
    #[error("component `{component}` imports `{interface}` without an authorized provider")]
    ImportDenied {
        component: String,
        interface: String,
    },
    #[error("component `{component}` failed to instantiate")]
    Instantiation {
        component: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("runtime synchronization state was poisoned during construction")]
    Poisoned,
}

/// A typed failure while calling an instantiated component.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
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

impl Runtime<()> {
    /// Begins configuring a runtime for a catalog and its policy.
    pub fn builder(catalog: Catalog, policy: Policy) -> RuntimeBuilder<()> {
        RuntimeBuilder {
            catalog,
            policy,
            observer: None,
            state_factory: Arc::new(|_, _| ()),
            configure_linker: Arc::new(|_, _| Ok(())),
        }
    }
}

impl<H: Send + 'static> Runtime<H> {
    fn build(
        plan: Plan,
        observer: Option<Observer>,
        state_factory: StateFactory<H>,
        configure_linker: LinkerConfig<H>,
    ) -> Result<Self, RuntimeBuildError> {
        let table = Arc::new(Mutex::new(RuntimeTable {
            identity: NEXT_RUNTIME.fetch_add(1, Ordering::Relaxed),
            components: HashMap::new(),
            observer,
        }));
        let runtime = Self { plan, table };
        let mut prepared = HashMap::new();
        for component in runtime.plan.order.clone() {
            let instance = runtime.prepare_instance(component, &configure_linker)?;
            prepared.insert(component, instance);
        }
        for component in runtime.plan.order.clone() {
            runtime.instantiate(
                component,
                &state_factory,
                prepared
                    .remove(&component)
                    .expect("every included component has a prepared instance"),
            )?;
        }
        Ok(runtime)
    }

    /// Runs an application-defined operation against a raw component instance.
    ///
    /// Prefer [`Self::with_component`] for artifacts admitted with [`Catalog::add`](crate::Catalog::add).
    pub fn with_instance<R>(
        &self,
        component: ComponentId,
        call: impl FnOnce(&mut Store<PluginStore<H>>, &Instance) -> anyhow::Result<R>,
    ) -> Result<R, RuntimeError> {
        let entry = self.plan.catalog.entry(component)?;
        with_component_runtime(&self.table, component, &entry.name, |runtime| {
            let ComponentRuntime {
                store, instance, ..
            } = runtime;
            call(store, instance)
        })
    }

    /// Runs a generated application binding against an admitted component.
    pub fn with_component<B: ComponentBinding, R>(
        &self,
        component: Component<B>,
        call: impl FnOnce(&mut Store<PluginStore<H>>, B) -> anyhow::Result<R>,
    ) -> Result<R, RuntimeError> {
        let id = component.id();
        let entry = self.plan.catalog.entry(id)?;
        with_component_runtime(&self.table, id, &entry.name, |runtime| {
            let ComponentRuntime {
                store, instance, ..
            } = runtime;
            let binding = B::bind(store, instance)?;
            call(store, binding)
        })
    }

    /// Reports whether a component has avoided a trapping call.
    pub fn is_healthy(&self, component: impl ComponentRef) -> Result<bool, RuntimeError> {
        let component = component.id();
        let name = self.plan.catalog.component(component)?.name().to_owned();
        let runtime = runtime_for(&self.table, component, &name)?;
        let runtime = runtime.lock().map_err(|_| RuntimeError::Poisoned)?;
        Ok(runtime.healthy)
    }

    fn prepare_instance(
        &self,
        component: ComponentId,
        configure_linker: &LinkerConfig<H>,
    ) -> Result<InstancePre<PluginStore<H>>, RuntimeBuildError> {
        let entry = self.plan.catalog.entry(component)?;
        let component_plan =
            self.plan
                .component(component)
                .map_err(|_| RuntimeBuildError::NotIncluded {
                    component: entry.name.clone(),
                })?;
        for import in &entry.direct_imports {
            if !component_plan
                .direct_imports
                .contains_key(&import.interface)
                && !component_plan.host_imports.contains(&import.interface)
            {
                return Err(RuntimeBuildError::ImportDenied {
                    component: entry.name.clone(),
                    interface: import.interface.clone(),
                });
            }
        }
        let engine = self.plan.catalog.engine();
        let mut linker = Linker::new(engine);
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        configure_linker(component, &mut linker)
            .map_err(|error| instantiate_error(&entry.name, error))?;
        wire_direct_imports(
            &mut linker,
            &self.table,
            &entry.name,
            component_plan.direct_imports.values(),
        )
        .map_err(|error| instantiate_error(&entry.name, error))?;
        linker
            .instantiate_pre(&entry.component)
            .map_err(|error| instantiate_error(&entry.name, error.into()))
    }

    fn instantiate(
        &self,
        component: ComponentId,
        state_factory: &StateFactory<H>,
        instance: InstancePre<PluginStore<H>>,
    ) -> Result<(), RuntimeBuildError> {
        let entry = self.plan.catalog.entry(component)?;
        let component_plan =
            self.plan
                .component(component)
                .map_err(|_| RuntimeBuildError::NotIncluded {
                    component: entry.name.clone(),
                })?;
        let engine = self.plan.catalog.engine();
        let mut wasi = WasiCtxBuilder::new();
        for directory in &component_plan.directories {
            preopen(&mut wasi, directory).map_err(|error| instantiate_error(&entry.name, error))?;
        }
        let state = PluginStore {
            context: HostContext::new(
                component,
                entry.name.clone(),
                state_factory(component, &entry.name),
            ),
            wasi: wasi.build(),
            resources: ResourceTable::new(),
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
            .map_err(|_| RuntimeBuildError::Poisoned)?
            .components
            .insert(component, runtime);
        Ok(())
    }
}

impl RuntimeBuilder<()> {
    /// Supplies application state and generated host bindings for every component store.
    pub fn with_host<H: Send + 'static>(
        self,
        state: impl Fn(ComponentId, &str) -> H + Send + Sync + 'static,
        configure_linker: impl Fn(ComponentId, &mut Linker<PluginStore<H>>) -> anyhow::Result<()>
        + Send
        + Sync
        + 'static,
    ) -> RuntimeBuilder<H> {
        RuntimeBuilder {
            catalog: self.catalog,
            policy: self.policy,
            observer: self.observer,
            state_factory: Arc::new(state),
            configure_linker: Arc::new(configure_linker),
        }
    }
}

impl<H: Send + 'static> RuntimeBuilder<H> {
    /// Sends broker events to an application observer from the start of instantiation.
    pub fn with_observer(mut self, observer: impl Fn(Event) + Send + Sync + 'static) -> Self {
        self.observer = Some(Arc::new(observer));
        self
    }

    /// Validates the complete policy, then instantiates every included component.
    pub fn build(self) -> Result<Runtime<H>, RuntimeBuildError> {
        let plan = Plan::new(self.catalog, self.policy)?;
        Runtime::build(
            plan,
            self.observer,
            self.state_factory,
            self.configure_linker,
        )
    }
}

impl<H: Send + 'static> ComponentRuntime<H> {
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

fn wire_direct_imports<'a, H: Send + 'static>(
    linker: &mut Linker<PluginStore<H>>,
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    caller: &str,
    imports: impl Iterator<Item = &'a ResolvedImport>,
) -> Result<(), anyhow::Error> {
    for import in imports {
        let mut instance = linker.instance(&import.interface)?;
        for (function, target) in &import.functions {
            let target = target.clone();
            let runtimes = Arc::clone(runtimes);
            let caller = caller.to_owned();
            instance.func_new(function, move |_store, _ty, params, results| {
                emit(
                    &Arc::downgrade(&runtimes),
                    Event::SiblingCall {
                        caller: caller.clone(),
                        provider: target.component_name.clone(),
                        target: target.key(),
                    },
                );
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

fn emit<H: Send + 'static>(runtimes: &Weak<Mutex<RuntimeTable<H>>>, event: Event) {
    let observer = runtimes
        .upgrade()
        .and_then(|runtimes| runtimes.lock().ok()?.observer.clone());
    if let Some(observer) = observer {
        observer(event);
    }
}

fn invoke_target<H: Send + 'static>(
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    target: &Target,
    params: &[Val],
) -> Result<Vec<Val>, RuntimeError> {
    with_component_runtime(
        runtimes,
        target.component,
        &target.component_name,
        |runtime| runtime.call(target, params),
    )
}

fn with_component_runtime<H: Send + 'static, R>(
    runtimes: &Arc<Mutex<RuntimeTable<H>>>,
    component: ComponentId,
    component_name: &str,
    call: impl FnOnce(&mut ComponentRuntime<H>) -> anyhow::Result<R>,
) -> Result<R, RuntimeError> {
    let runtime_identity = runtimes
        .lock()
        .map_err(|_| RuntimeError::Poisoned)?
        .identity;
    let _guard = enter_component(runtime_identity, component, component_name)?;
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

fn runtime_for<H: Send + 'static>(
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

fn try_runtime_lock<'a, H: Send + 'static>(
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
fn enter_call(runtime: u64, target: &Target) -> Result<CallGuard, RuntimeError> {
    enter_component(runtime, target.component, &target.component_name)
}

fn enter_component(
    runtime: u64,
    component: ComponentId,
    component_name: &str,
) -> Result<CallGuard, RuntimeError> {
    let frame = Frame { runtime, component };
    CALL_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let runtime_depth = stack.iter().filter(|item| item.runtime == runtime).count();
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
    if !grant.host().is_dir() {
        anyhow::bail!("host path {} is not a directory", grant.host().display());
    }
    let (dirs, files) = match grant.access() {
        DirectoryAccess::ReadOnly => (DirPerms::READ, FilePerms::READ),
        DirectoryAccess::ReadWrite => (DirPerms::all(), FilePerms::all()),
    };
    let guest = grant
        .guest()
        .to_str()
        .ok_or_else(|| anyhow::anyhow!("guest path is not UTF-8"))?;
    wasi.preopened_dir(grant.host(), guest, dirs, files)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Catalog, Policy};
    use std::sync::atomic::AtomicUsize;
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

    #[test]
    fn host_context_keeps_component_identity_separate_from_user_state() {
        let mut catalog = Catalog::new().unwrap();
        let component = catalog.add_untyped("provider", provider_bytes()).unwrap();
        let mut context = HostContext::new(component, "provider".into(), vec![1]);

        context.state_mut().push(2);

        assert_eq!(context.component().id(), component);
        assert_eq!(context.component().name(), "provider");
        assert_eq!(context.state(), &[1, 2]);
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
        let mut policy = Policy::builder(&catalog);
        for component in &components {
            policy = policy.include(*component).unwrap();
        }
        let policy = policy.build();
        let plan = Plan::new(catalog, policy).unwrap();
        let targets = components
            .into_iter()
            .map(|component| {
                let entry = plan.catalog.entry(component).unwrap();
                let export = &entry.exports[0];
                Target {
                    component,
                    component_name: entry.name.clone(),
                    interface: export.interface().into(),
                    function: export.function().into(),
                }
            })
            .collect::<Vec<_>>();

        let first = enter_call(7, &targets[0]).unwrap();
        assert!(matches!(
            enter_call(7, &targets[0]),
            Err(RuntimeError::Cycle { .. })
        ));
        drop(first);

        let guards = targets[..MAX_DEPTH]
            .iter()
            .map(|target| enter_call(7, target).unwrap())
            .collect::<Vec<_>>();
        assert!(matches!(
            enter_call(7, &targets[MAX_DEPTH]),
            Err(RuntimeError::DepthLimit)
        ));
        drop(guards);
        assert!(enter_call(7, &targets[0]).is_ok());
    }

    #[test]
    fn all_linkers_are_preflighted_before_any_store_is_created() {
        let mut catalog = Catalog::new().unwrap();
        let first = catalog.add_untyped("first", provider_bytes()).unwrap();
        let second = catalog.add_untyped("second", consumer_bytes()).unwrap();
        let policy = Policy::builder(&catalog)
            .include(first)
            .unwrap()
            .allow_host_import(second, "demo:stack/api@0.1.0")
            .unwrap()
            .build();
        let creations = Arc::new(AtomicUsize::new(0));
        let factory_creations = Arc::clone(&creations);
        let result = Runtime::builder(catalog, policy)
            .with_host(
                move |_, _| {
                    factory_creations.fetch_add(1, Ordering::Relaxed);
                },
                |_, _| Ok(()),
            )
            .build();

        assert!(matches!(
            result,
            Err(RuntimeBuildError::Instantiation { .. })
        ));
        assert_eq!(creations.load(Ordering::Relaxed), 0);
    }
}
