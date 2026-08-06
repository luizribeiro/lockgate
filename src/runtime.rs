//! Isolated component execution for a validated [`Plan`](crate::Plan).
//! Each component receives its own store, WASI context, fuel budget, and dynamic handle table.

use crate::{
    ComponentId, ExportId, Plan,
    catalog::CatalogError,
    plan::{ResolvedImport, Target},
    policy::{DirectoryAccess, DirectoryGrant},
    tangent::core::registry,
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
    component::{HasSelf, Instance, Linker, ResourceTable, Val},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

mod dynamic;

const FUEL: u64 = 100_000;
const MAX_DEPTH: usize = 8;
const REGISTRY_INTERFACE: &str = "tangent:core/registry@0.1.0";
static NEXT_RUNTIME: AtomicU64 = AtomicU64::new(1);

type Observer = Arc<dyn Fn(Event) + Send + Sync>;

/// An observable cross-component broker event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Event {
    DirectCall {
        target: String,
    },
    DynamicLookup {
        caller: String,
        target: String,
        allowed: bool,
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

struct StoreState {
    component_name: String,
    wasi: WasiCtx,
    resources: ResourceTable,
    runtimes: Weak<Mutex<RuntimeTable>>,
    lookups: HashMap<String, Target>,
    handles: HashMap<u32, Target>,
    target_handles: HashMap<String, u32>,
}

impl WasiView for StoreState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.resources,
        }
    }
}

struct ComponentRuntime {
    store: Store<StoreState>,
    instance: Instance,
    healthy: bool,
}

struct RuntimeTable {
    identity: u64,
    components: HashMap<ComponentId, Arc<Mutex<ComponentRuntime>>>,
    observer: Option<Observer>,
}

/// A fully instantiated set of isolated component stores.
pub struct Runtime {
    plan: Plan,
    table: Arc<Mutex<RuntimeTable>>,
}

/// A typed failure while instantiating or calling a planned component.
#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("component `{component}` is not included in the plan")]
    NotPlanned { component: String },
    #[error("component `{component}` imports `{interface}` without an authorized provider")]
    ImportDenied {
        component: String,
        interface: String,
    },
    #[error("component `{component}` imports the dynamic registry without enabling it")]
    RegistryDenied { component: String },
    #[error("component `{component}` failed to instantiate")]
    Instantiation {
        component: String,
        #[source]
        source: anyhow::Error,
    },
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

impl Runtime {
    /// Instantiates every component included in a validated plan.
    pub fn new(plan: Plan) -> Result<Self, RuntimeError> {
        Self::build(plan, None)
    }

    /// Instantiates a plan and sends broker events to an application observer.
    pub fn with_observer(
        plan: Plan,
        observer: impl Fn(Event) + Send + Sync + 'static,
    ) -> Result<Self, RuntimeError> {
        Self::build(plan, Some(Arc::new(observer)))
    }

    fn build(plan: Plan, observer: Option<Observer>) -> Result<Self, RuntimeError> {
        let table = Arc::new(Mutex::new(RuntimeTable {
            identity: NEXT_RUNTIME.fetch_add(1, Ordering::Relaxed),
            components: HashMap::new(),
            observer,
        }));
        let runtime = Self { plan, table };
        for component in runtime.plan.order.clone() {
            runtime.instantiate(component)?;
        }
        Ok(runtime)
    }

    /// Calls an exact exported function with runtime component values.
    pub fn call(&self, export: ExportId, params: &[Val]) -> Result<Vec<Val>, RuntimeError> {
        let target = self.plan.target(export)?;
        invoke_target(&self.table, &target, params)
    }

    /// Reports whether a component has avoided a trapping call.
    pub fn is_healthy(&self, component: ComponentId) -> Result<bool, RuntimeError> {
        let name = self.plan.catalog.component(component)?.name().to_owned();
        let runtime = runtime_for(&self.table, component, &name)?;
        let runtime = runtime.lock().map_err(|_| RuntimeError::Poisoned)?;
        Ok(runtime.healthy)
    }

    pub fn plan(&self) -> &Plan {
        &self.plan
    }

    fn instantiate(&self, component: ComponentId) -> Result<(), RuntimeError> {
        let entry = self.plan.catalog.entry(component)?;
        let component_plan =
            self.plan
                .component(component)
                .map_err(|_| RuntimeError::NotPlanned {
                    component: entry.name.clone(),
                })?;
        for import in &entry.direct_imports {
            if !component_plan
                .direct_imports
                .contains_key(&import.interface)
            {
                return Err(RuntimeError::ImportDenied {
                    component: entry.name.clone(),
                    interface: import.interface.clone(),
                });
            }
        }
        if entry
            .imports
            .iter()
            .any(|import| import == REGISTRY_INTERFACE)
            && !component_plan.registry
        {
            return Err(RuntimeError::RegistryDenied {
                component: entry.name.clone(),
            });
        }

        let engine = self.plan.catalog.engine();
        let mut linker = Linker::new(engine);
        registry::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        wire_direct_imports(
            &mut linker,
            &self.table,
            component_plan.direct_imports.values(),
        )
        .map_err(|error| instantiate_error(&entry.name, error))?;

        let mut wasi = WasiCtxBuilder::new();
        for directory in &component_plan.directories {
            preopen(&mut wasi, directory).map_err(|error| instantiate_error(&entry.name, error))?;
        }
        let state = StoreState {
            component_name: entry.name.clone(),
            wasi: wasi.build(),
            resources: ResourceTable::new(),
            runtimes: Arc::downgrade(&self.table),
            lookups: component_plan.lookups.clone(),
            handles: HashMap::new(),
            target_handles: HashMap::new(),
        };
        let mut store = Store::new(engine, state);
        store
            .set_fuel(FUEL)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        let instance = linker
            .instantiate(&mut store, &entry.component)
            .map_err(|error| instantiate_error(&entry.name, error.into()))?;
        let runtime = Arc::new(Mutex::new(ComponentRuntime {
            store,
            instance,
            healthy: true,
        }));
        self.table
            .lock()
            .map_err(|_| RuntimeError::Poisoned)?
            .components
            .insert(component, runtime);
        Ok(())
    }
}

impl ComponentRuntime {
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

fn wire_direct_imports<'a>(
    linker: &mut Linker<StoreState>,
    runtimes: &Arc<Mutex<RuntimeTable>>,
    imports: impl Iterator<Item = &'a ResolvedImport>,
) -> Result<(), anyhow::Error> {
    for import in imports {
        let mut instance = linker.instance(&import.interface)?;
        for (function, target) in &import.functions {
            let target = target.clone();
            let runtimes = Arc::clone(runtimes);
            instance.func_new(function, move |_store, _ty, params, results| {
                emit(
                    &Arc::downgrade(&runtimes),
                    Event::DirectCall {
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

fn emit(runtimes: &Weak<Mutex<RuntimeTable>>, event: Event) {
    let observer = runtimes
        .upgrade()
        .and_then(|runtimes| runtimes.lock().ok()?.observer.clone());
    if let Some(observer) = observer {
        observer(event);
    }
}

fn invoke_target(
    runtimes: &Arc<Mutex<RuntimeTable>>,
    target: &Target,
    params: &[Val],
) -> Result<Vec<Val>, RuntimeError> {
    let runtime_identity = runtimes
        .lock()
        .map_err(|_| RuntimeError::Poisoned)?
        .identity;
    let _guard = enter_call(runtime_identity, target)?;
    let runtime = runtime_for(runtimes, target.component, &target.component_name)?;
    let mut runtime = try_runtime_lock(&runtime, &target.component_name)?;
    if !runtime.healthy {
        return Err(RuntimeError::Unhealthy {
            component: target.component_name.clone(),
        });
    }
    runtime
        .store
        .set_fuel(FUEL)
        .map_err(|source| RuntimeError::Trapped {
            component: target.component_name.clone(),
            source: source.into(),
        })?;
    let result = runtime.call(target, params);
    if result.is_err() {
        runtime.healthy = false;
    }
    result.map_err(|source| RuntimeError::Trapped {
        component: target.component_name.clone(),
        source,
    })
}

fn runtime_for(
    runtimes: &Arc<Mutex<RuntimeTable>>,
    component: ComponentId,
    name: &str,
) -> Result<Arc<Mutex<ComponentRuntime>>, RuntimeError> {
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

fn try_runtime_lock<'a>(
    runtime: &'a Mutex<ComponentRuntime>,
    name: &str,
) -> Result<MutexGuard<'a, ComponentRuntime>, RuntimeError> {
    match runtime.try_lock() {
        Ok(runtime) => Ok(runtime),
        Err(TryLockError::WouldBlock) => Err(RuntimeError::Busy {
            component: name.into(),
        }),
        Err(TryLockError::Poisoned(_)) => Err(RuntimeError::Poisoned),
    }
}

fn enter_call(runtime: u64, target: &Target) -> Result<CallGuard, RuntimeError> {
    let frame = Frame {
        runtime,
        component: target.component,
    };
    CALL_STACK.with(|stack| {
        let mut stack = stack.borrow_mut();
        let runtime_depth = stack.iter().filter(|item| item.runtime == runtime).count();
        if runtime_depth >= MAX_DEPTH {
            return Err(RuntimeError::DepthLimit);
        }
        if stack.contains(&frame) {
            return Err(RuntimeError::Cycle {
                component: target.component_name.clone(),
            });
        }
        stack.push(frame);
        Ok(CallGuard(frame))
    })
}

fn instantiate_error(component: &str, source: anyhow::Error) -> RuntimeError {
    RuntimeError::Instantiation {
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

    #[test]
    fn call_guards_reject_cycles_and_depth_without_leaking_frames() {
        let mut catalog = Catalog::new().unwrap();
        let mut components = Vec::new();
        let mut exports = Vec::new();
        for index in 0..=MAX_DEPTH {
            let component = catalog
                .add(format!("provider-{index}"), provider_bytes())
                .unwrap();
            exports.push(
                catalog
                    .export(component, "demo:stack/api@0.1.0#run")
                    .unwrap(),
            );
            components.push(component);
        }
        let mut policy = Policy::builder(&catalog);
        for component in components {
            policy = policy.include(component).unwrap();
        }
        let policy = policy.build();
        let plan = Plan::new(catalog, policy).unwrap();
        let targets = exports
            .into_iter()
            .map(|export| plan.target(export).unwrap())
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
}
