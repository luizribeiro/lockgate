//! Isolated plugin execution and cross-plugin call enforcement.
//! Builds stores and WASI contexts, wires typed imports, and implements the dynamic broker.

use crate::{
    manifest::Manifest,
    plugin::{PluginDefinition, Signature, Target, type_name},
    tangent::core::registry,
};
use anyhow::{Context, Result, bail};
use std::{
    cell::RefCell,
    collections::HashMap,
    path::Path,
    sync::{Arc, Mutex, Weak},
};
use wasmtime::{
    Engine, Store,
    component::{HasSelf, Instance, Linker, ResourceTable, Val, types::Type},
};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};

pub(crate) const FUEL: u64 = 100_000;
pub(crate) const MAX_DEPTH: usize = 8;

thread_local! {
    pub(crate) static CALL_STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

pub(crate) struct PluginState {
    pub(crate) manifest: Manifest,
    pub(crate) wasi: WasiCtx,
    pub(crate) table: ResourceTable,
    pub(crate) plugins: Weak<Mutex<PluginTable>>,
    pub(crate) handles: HashMap<u32, Target>,
    pub(crate) next_handle: u32,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

pub(crate) struct PluginRuntime {
    pub(crate) store: Store<PluginState>,
    pub(crate) instance: Instance,
    pub(crate) healthy: bool,
}

#[derive(Default)]
pub(crate) struct PluginTable {
    pub(crate) targets: HashMap<String, Target>,
    pub(crate) runtimes: HashMap<String, Arc<Mutex<PluginRuntime>>>,
}

impl registry::Host for PluginState {
    fn lookup(&mut self, target: String) -> Result<u32, registry::InvokeError> {
        if !self.manifest.permits(&target) {
            println!("  [dynamic] {} -> {target}  DENIED", self.manifest.id);
            return Err(registry::InvokeError::Denied("not in invokes".into()));
        }
        let Some(plugins) = self.plugins.upgrade() else {
            return Err(registry::InvokeError::Trapped(
                "plugin table unavailable".into(),
            ));
        };
        let Some(resolved) = plugins.lock().unwrap().targets.get(&target).cloned() else {
            println!("  [dynamic] {} -> {target}  NOT-FOUND", self.manifest.id);
            return Err(registry::InvokeError::NotFound(target));
        };
        println!("  [dynamic] {} -> {target}  ALLOWED", self.manifest.id);
        let handle = self.next_handle;
        self.next_handle += 1;
        self.handles.insert(handle, resolved);
        Ok(handle)
    }

    fn invoke(
        &mut self,
        handle: u32,
        args: Vec<registry::Value>,
    ) -> Result<Vec<registry::Value>, registry::InvokeError> {
        let Some(target) = self.handles.get(&handle).cloned() else {
            return Err(registry::InvokeError::Denied(format!(
                "unknown handle {handle}"
            )));
        };
        check_values(&target.signature, &args)?;
        let Some(plugins) = self.plugins.upgrade() else {
            return Err(registry::InvokeError::Trapped(
                "plugin table unavailable".into(),
            ));
        };
        let params = args.into_iter().map(value_to_val).collect::<Vec<_>>();
        invoke_target(&plugins, &target, &params)
            .map_err(|error| registry::InvokeError::Trapped(format!("{error:#}")))?
            .into_iter()
            .map(val_to_value)
            .collect()
    }
}

impl PluginRuntime {
    pub(crate) fn instantiate(
        root: &Path,
        plugins: &Arc<Mutex<PluginTable>>,
        engine: &Engine,
        definition: PluginDefinition,
    ) -> Result<Self> {
        let mut linker = Linker::new(engine);
        registry::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
        wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
        wire_direct_imports(&mut linker, plugins, &definition)?;

        let mut wasi = WasiCtxBuilder::new();
        if let Some(fs) = &definition.manifest.capabilities.fs {
            for path in &fs.read {
                preopen(root, &mut wasi, path, false)?;
            }
            for path in &fs.write {
                preopen(root, &mut wasi, path, true)?;
            }
        }
        let state = PluginState {
            manifest: definition.manifest,
            wasi: wasi.build(),
            table: ResourceTable::new(),
            plugins: Arc::downgrade(plugins),
            handles: HashMap::new(),
            next_handle: 1,
        };
        let mut store = Store::new(engine, state);
        store.set_fuel(FUEL)?;
        let instance = linker.instantiate(&mut store, &definition.component)?;
        Ok(Self {
            store,
            instance,
            healthy: true,
        })
    }

    fn call(&mut self, interface: &str, function: &str, params: &[Val]) -> Result<Vec<Val>> {
        let interface = self
            .instance
            .get_export_index(&mut self.store, None, interface)
            .context("interface not exported")?;
        let function = self
            .instance
            .get_export_index(&mut self.store, Some(&interface), function)
            .context("function not exported")?;
        let func = self
            .instance
            .get_func(&mut self.store, function)
            .context("export is not a function")?;
        let mut results = func
            .ty(&self.store)
            .results()
            .map(|_| Val::Bool(false))
            .collect::<Vec<_>>();
        func.call(&mut self.store, params, &mut results)?;
        Ok(results)
    }
}

fn wire_direct_imports(
    linker: &mut Linker<PluginState>,
    plugins: &Arc<Mutex<PluginTable>>,
    definition: &PluginDefinition,
) -> Result<()> {
    for import in &definition.direct_imports {
        let mut instance = linker.instance(&import.interface)?;
        for (function, expected) in &import.functions {
            let key = format!("{}#{function}", import.interface);
            let target = resolve_direct_target(plugins, &definition.manifest, &key, expected)?;
            if !plugins
                .lock()
                .unwrap()
                .runtimes
                .contains_key(&target.plugin)
            {
                bail!("provider {} for {key} is not instantiated", target.plugin);
            }
            let plugins = Arc::clone(plugins);
            instance.func_new(function, move |_store, _ty, params, results| {
                println!("  [direct] -> {}", target.key());
                let values = invoke_target(&plugins, &target, params)
                    .map_err(|error| wasmtime::Error::msg(format!("{error:#}")))?;
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

pub(crate) fn resolve_direct_target(
    plugins: &Arc<Mutex<PluginTable>>,
    manifest: &Manifest,
    key: &str,
    expected: &Signature,
) -> Result<Target> {
    if !manifest.permits(key) {
        bail!("direct import {key} is not permitted by manifest invokes");
    }
    let target = plugins
        .lock()
        .unwrap()
        .targets
        .get(key)
        .cloned()
        .with_context(|| format!("direct import {key} has no provider"))?;
    if &target.signature != expected {
        bail!(
            "type mismatch for {key}: caller expects {}, provider exports {}",
            expected,
            target.signature
        );
    }
    Ok(target)
}

pub(crate) fn invoke_target(
    plugins: &Arc<Mutex<PluginTable>>,
    target: &Target,
    params: &[Val],
) -> Result<Vec<Val>> {
    let stack_error = CALL_STACK.with(|stack| {
        let stack = stack.borrow();
        if stack.len() >= MAX_DEPTH {
            Some(format!("call depth limit {MAX_DEPTH} exceeded"))
        } else if stack.contains(&target.plugin) {
            Some(format!("call cycle detected at plugin {}", target.plugin))
        } else {
            None
        }
    });
    if let Some(error) = stack_error {
        bail!(error);
    }
    let runtime = plugins
        .lock()
        .unwrap()
        .runtimes
        .get(&target.plugin)
        .with_context(|| format!("plugin {} is unavailable", target.plugin))?
        .clone();
    let mut runtime = runtime
        .try_lock()
        .map_err(|_| anyhow::anyhow!("callee store is already borrowed"))?;
    if !runtime.healthy {
        bail!("plugin {} is unhealthy", target.plugin);
    }
    runtime.store.set_fuel(FUEL)?;
    CALL_STACK.with(|stack| stack.borrow_mut().push(target.plugin.clone()));
    let call = runtime.call(&target.interface, &target.function, params);
    CALL_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    if call.is_err() {
        runtime.healthy = false;
    }
    call
}

pub(crate) fn call_runner(
    plugins: &Arc<Mutex<PluginTable>>,
    id: &str,
    interface: &str,
) -> Result<Vec<Val>> {
    let runtime = plugins
        .lock()
        .unwrap()
        .runtimes
        .get(id)
        .context("plugin unavailable")?
        .clone();
    let mut runtime = runtime.lock().unwrap();
    runtime.store.set_fuel(FUEL)?;
    CALL_STACK.with(|stack| stack.borrow_mut().push(id.into()));
    let result = runtime.call(interface, "run", &[]);
    CALL_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    if result.is_err() {
        runtime.healthy = false;
    }
    result
}

fn preopen(root: &Path, wasi: &mut WasiCtxBuilder, path: &str, writable: bool) -> Result<()> {
    let host = root.join(path.strip_prefix("./").unwrap_or(path));
    let guest = format!("/{}", host.file_name().unwrap().to_string_lossy());
    let (dirs, files) = if writable {
        (DirPerms::all(), FilePerms::all())
    } else {
        (DirPerms::READ, FilePerms::READ)
    };
    wasi.preopened_dir(host, guest, dirs, files)?;
    Ok(())
}

fn check_values(
    signature: &Signature,
    args: &[registry::Value],
) -> Result<(), registry::InvokeError> {
    if args.len() != signature.params.len() {
        return Err(registry::InvokeError::TypeMismatch(format!(
            "expected {} arguments, got {}",
            signature.params.len(),
            args.len()
        )));
    }
    for (index, (value, expected)) in args.iter().zip(&signature.params).enumerate() {
        if !dynamic_value_matches(value, expected) {
            return Err(registry::InvokeError::TypeMismatch(format!(
                "arg {index} expected {}, got {}",
                type_name(expected),
                value_name(value)
            )));
        }
    }
    Ok(())
}

fn dynamic_value_matches(value: &registry::Value, expected: &Type) -> bool {
    match (value, expected) {
        (registry::Value::Bool(_), Type::Bool)
        | (registry::Value::S32(_), Type::S32)
        | (registry::Value::U32(_), Type::U32)
        | (registry::Value::String(_), Type::String) => true,
        (registry::Value::List(_), Type::List(list)) => list.ty() == Type::String,
        _ => false,
    }
}

fn value_name(value: &registry::Value) -> &str {
    match value {
        registry::Value::Bool(_) => "bool",
        registry::Value::S32(_) => "s32",
        registry::Value::U32(_) => "u32",
        registry::Value::String(_) => "string",
        registry::Value::List(_) => "list<string>",
    }
}

fn value_to_val(value: registry::Value) -> Val {
    match value {
        registry::Value::Bool(value) => Val::Bool(value),
        registry::Value::S32(value) => Val::S32(value),
        registry::Value::U32(value) => Val::U32(value),
        registry::Value::String(value) => Val::String(value),
        registry::Value::List(values) => Val::List(values.into_iter().map(Val::String).collect()),
    }
}

fn val_to_value(value: Val) -> Result<registry::Value, registry::InvokeError> {
    match value {
        Val::Bool(value) => Ok(registry::Value::Bool(value)),
        Val::S32(value) => Ok(registry::Value::S32(value)),
        Val::U32(value) => Ok(registry::Value::U32(value)),
        Val::String(value) => Ok(registry::Value::String(value)),
        Val::List(values) => values
            .into_iter()
            .map(|value| match value {
                Val::String(string) => Ok(string),
                other => Err(registry::InvokeError::Trapped(format!(
                    "unsupported list item {other:?}"
                ))),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(registry::Value::List),
        other => Err(registry::InvokeError::Trapped(format!(
            "unsupported result {other:?}"
        ))),
    }
}

pub(crate) fn render_values(values: &[Val]) -> String {
    match values {
        [Val::Result(Ok(Some(value)))] => match value.as_ref() {
            Val::String(value) => format!("{value:?}"),
            other => format!("{other:?}"),
        },
        [Val::List(values)] => format!("{values:?}"),
        other => format!("{other:?}"),
    }
}
