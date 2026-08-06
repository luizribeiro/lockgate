use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{
    cell::RefCell,
    collections::HashMap,
    fs,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, Weak},
};
use wasmtime::component::{Component, HasSelf, Instance, Linker, ResourceTable, Val};
use wasmtime::{Config, Engine, Store};
use wasmtime_wasi::{DirPerms, FilePerms, WasiCtx, WasiCtxBuilder, WasiCtxView, WasiView};
use wit_component::DecodedWasm;
use wit_parser::{Resolve, Type, TypeDefKind, WorldItem};

wasmtime::component::bindgen!({ path: "../wit", world: "consumer" });
use crate::tangent::core::registry;

#[cfg(test)]
mod tests;

const FUEL: u64 = 100_000;
const MAX_DEPTH: usize = 8;
const IDS: [&str; 5] = ["greeter", "caller", "filereader", "naughty", "dynamic"];

thread_local! {
    static CALL_STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Debug, Deserialize)]
#[rustfmt::skip]
struct Manifest {
    id: String, #[serde(default)] provides: Vec<String>, #[serde(default)] invokes: Vec<String>,
    capabilities: Capabilities,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[rustfmt::skip]
struct Capabilities {
    #[serde(default)] registry: bool, fs: Option<FsCapability>, net: Option<NetCapability>,
}

#[derive(Clone, Debug, Deserialize)]
#[rustfmt::skip]
struct FsCapability {
    #[serde(default)] read: Vec<String>, #[serde(default)] write: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
#[rustfmt::skip]
struct NetCapability {
    #[serde(default)] hosts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
#[rustfmt::skip]
struct Signature {
    params: Vec<String>, result: Option<String>,
}

#[derive(Clone, Debug)]
#[rustfmt::skip]
struct Target {
    plugin: String, interface: String, function: String, signature: Signature,
}

#[derive(Clone, Debug)]
#[rustfmt::skip]
struct DirectImport {
    interface: String, functions: Vec<(String, Signature)>,
}

#[rustfmt::skip]
struct PluginDefinition {
    manifest: Manifest, component: Component, targets: Vec<(String, Target)>,
    direct_imports: Vec<DirectImport>,
}

#[rustfmt::skip]
struct PluginState {
    manifest: Manifest, wasi: WasiCtx, table: ResourceTable, plugins: Weak<Mutex<PluginTable>>,
    handles: HashMap<u32, Target>, next_handle: u32,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

#[rustfmt::skip]
struct PluginRuntime { store: Store<PluginState>, instance: Instance, healthy: bool }

#[derive(Default)]
#[rustfmt::skip]
struct PluginTable { targets: HashMap<String, Target>, runtimes: HashMap<String, Arc<Mutex<PluginRuntime>>> }

impl registry::Host for PluginState {
    fn lookup(&mut self, target: String) -> Result<u32, registry::InvokeError> {
        if !permitted(&self.manifest, &target) {
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

    #[rustfmt::skip]
    fn invoke(&mut self, handle: u32, args: Vec<registry::Value>)
        -> Result<Vec<registry::Value>, registry::InvokeError> {
        let Some(target) = self.handles.get(&handle).cloned() else {
            return Err(registry::InvokeError::Denied(format!("unknown handle {handle}")));
        };
        check_values(&target.signature, &args)?;
        let Some(plugins) = self.plugins.upgrade() else {
            return Err(registry::InvokeError::Trapped("plugin table unavailable".into()));
        };
        let params = args.into_iter().map(value_to_val).collect::<Vec<_>>();
        invoke_target(&plugins, &target, &params)
            .map_err(|error| registry::InvokeError::Trapped(format!("{error:#}")))?
            .into_iter().map(val_to_value).collect()
    }
}

#[rustfmt::skip]
fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").canonicalize()?;
    let mut config = Config::new();
    config.wasm_component_model(true).consume_fuel(true);
    let engine = Engine::new(&config)?;
    let plugins = Arc::new(Mutex::new(PluginTable::default()));
    let mut definitions = Vec::new();

    for id in IDS {
        match load_plugin(&engine, &root, id) {
            Ok(definition) => {
                print_load(&definition);
                plugins.lock().unwrap().targets.extend(definition.targets.clone());
                definitions.push(definition);
            }
            Err(error) => println!("[load] {id:<12} REFUSED {error:#}"),
        }
    }
    println!();

    for definition in definitions {
        let id = definition.manifest.id.clone();
        match instantiate(&root, &plugins, &engine, definition) {
            Ok(runtime) => { plugins.lock().unwrap().runtimes.insert(id, Arc::new(Mutex::new(runtime))); }
            Err(error) => println!("[instantiate] {id:<8} REFUSED {error:#}"),
        }
    }

    run_and_print(&plugins, "caller", "demo:caller/runner@0.1.0", "caller.run()")?;
    run_and_print(&plugins, "filereader", "demo:filereader/runner@0.1.0", "filereader.run()")?;
    println!("\n[call] naughty.run()\n  => unavailable (undeclared direct import was refused)");
    run_and_print(&plugins, "dynamic", "demo:dynamic/runner@0.1.0", "dynamic.run()")?;
    println!("\n[host] still running");
    Ok(())
}

#[rustfmt::skip]
fn load_plugin(engine: &Engine, root: &Path, id: &str) -> Result<PluginDefinition> {
    let dir = root.join("plugins").join(id);
    let manifest: Manifest = toml::from_str(&fs::read_to_string(dir.join("plugin.toml"))?)?;
    if manifest.id != id { bail!("manifest id {:?} does not match directory", manifest.id); }
    let bytes = fs::read(dir.join(format!("{id}.wasm"))).context("component binary missing")?;
    let DecodedWasm::Component(resolve, world) = wit_component::decode(&bytes)? else {
        bail!("binary is not a component");
    };
    let direct_imports = decode_imports(&resolve, world, &manifest)?;
    let exports = exported_interfaces(&resolve, world)?;
    for provided in &manifest.provides {
        if !exports.contains_key(provided) { bail!("manifest provides {provided}, but component does not export it"); }
    }
    let mut targets = Vec::new();
    for provided in &manifest.provides {
        for function in resolve.interfaces[exports[provided]].functions.values() {
            let target = Target {
                plugin: id.into(),
                interface: provided.clone(),
                function: function.name.clone(),
                signature: signature(&resolve, function),
            };
            targets.push((target_key(&target), target));
        }
    }
    Ok(PluginDefinition {
        manifest,
        component: Component::new(engine, bytes)?,
        targets,
        direct_imports,
    })
}

fn decode_imports(
    resolve: &Resolve,
    world: wit_parser::WorldId,
    manifest: &Manifest,
) -> Result<Vec<DirectImport>> {
    let mut direct = Vec::new();
    for item in resolve.worlds[world].imports.values() {
        let WorldItem::Interface { id, .. } = item else {
            bail!("direct world functions are unsupported");
        };
        let name = resolve.id_of(*id).context("unnamed imported interface")?;
        if name == "tangent:core/registry@0.1.0" {
            if !manifest.capabilities.registry {
                bail!("undeclared import {name}");
            }
        } else if name.starts_with("wasi:filesystem/") {
            if manifest.capabilities.fs.is_none() {
                bail!("undeclared import {name}");
            }
        } else if name.starts_with("wasi:sockets/") {
            if manifest
                .capabilities
                .net
                .as_ref()
                .is_none_or(|net| net.hosts.is_empty())
            {
                bail!("undeclared import {name}");
            }
        } else if !name.starts_with("wasi:") {
            let functions = resolve.interfaces[*id]
                .functions
                .values()
                .map(|function| (function.name.clone(), signature(resolve, function)))
                .collect();
            direct.push(DirectImport {
                interface: name,
                functions,
            });
        }
    }
    Ok(direct)
}

fn exported_interfaces(
    resolve: &Resolve,
    world: wit_parser::WorldId,
) -> Result<HashMap<String, wit_parser::InterfaceId>> {
    let mut exports = HashMap::new();
    for item in resolve.worlds[world].exports.values() {
        if let WorldItem::Interface { id, .. } = item {
            exports.insert(
                resolve.id_of(*id).context("unnamed exported interface")?,
                *id,
            );
        }
    }
    Ok(exports)
}

fn instantiate(
    root: &Path,
    plugins: &Arc<Mutex<PluginTable>>,
    engine: &Engine,
    definition: PluginDefinition,
) -> Result<PluginRuntime> {
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
    Ok(PluginRuntime {
        store,
        instance,
        healthy: true,
    })
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
                println!("  [direct] -> {}", target_key(&target));
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

fn resolve_direct_target(
    plugins: &Arc<Mutex<PluginTable>>,
    manifest: &Manifest,
    key: &str,
    expected: &Signature,
) -> Result<Target> {
    if !permitted(manifest, key) {
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
            signature_text(expected),
            signature_text(&target.signature)
        );
    }
    Ok(target)
}

fn invoke_target(
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
    let call = call_dynamic(&mut runtime, &target.interface, &target.function, params);
    CALL_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    if call.is_err() {
        runtime.healthy = false;
    }
    call
}

fn call_dynamic(
    runtime: &mut PluginRuntime,
    interface: &str,
    function: &str,
    params: &[Val],
) -> Result<Vec<Val>> {
    let interface = runtime
        .instance
        .get_export_index(&mut runtime.store, None, interface)
        .context("interface not exported")?;
    let function = runtime
        .instance
        .get_export_index(&mut runtime.store, Some(&interface), function)
        .context("function not exported")?;
    let func = runtime
        .instance
        .get_func(&mut runtime.store, function)
        .context("export is not a function")?;
    let mut results = func
        .ty(&runtime.store)
        .results()
        .map(|_| Val::Bool(false))
        .collect::<Vec<_>>();
    func.call(&mut runtime.store, params, &mut results)?;
    Ok(results)
}

fn call_runner(plugins: &Arc<Mutex<PluginTable>>, id: &str, interface: &str) -> Result<Vec<Val>> {
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
    let result = call_dynamic(&mut runtime, interface, "run", &[]);
    CALL_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    if result.is_err() {
        runtime.healthy = false;
    }
    result
}

fn run_and_print(
    plugins: &Arc<Mutex<PluginTable>>,
    id: &str,
    interface: &str,
    label: &str,
) -> Result<()> {
    println!("\n[call] {label}");
    match call_runner(plugins, id, interface) {
        Ok(values) => println!("  => {}", render_values(&values)),
        Err(error) => println!("  => ERROR {error:#}"),
    }
    Ok(())
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

#[rustfmt::skip]
fn permitted(manifest: &Manifest, target: &str) -> bool {
    manifest.invokes.iter().any(|pattern| glob_match::glob_match(pattern, target))
}

fn signature(resolve: &Resolve, function: &wit_parser::Function) -> Signature {
    Signature {
        params: function
            .params
            .iter()
            .map(|param| type_shape(resolve, param.ty))
            .collect(),
        result: function.result.map(|ty| type_shape(resolve, ty)),
    }
}

#[rustfmt::skip]
fn type_shape(resolve: &Resolve, ty: Type) -> String {
    match ty {
        Type::Bool => "bool".into(), Type::U8 => "u8".into(), Type::U16 => "u16".into(),
        Type::U32 => "u32".into(), Type::U64 => "u64".into(), Type::S8 => "s8".into(),
        Type::S16 => "s16".into(), Type::S32 => "s32".into(), Type::S64 => "s64".into(),
        Type::F32 => "f32".into(), Type::F64 => "f64".into(), Type::Char => "char".into(),
        Type::String => "string".into(), Type::ErrorContext => "error-context".into(),
        Type::Id(id) => match &resolve.types[id].kind {
            TypeDefKind::Type(ty) => type_shape(resolve, *ty),
            TypeDefKind::List(ty) => format!("list<{}>", type_shape(resolve, *ty)),
            TypeDefKind::Option(ty) => format!("option<{}>", type_shape(resolve, *ty)),
            TypeDefKind::Tuple(t) => format!("tuple<{}>", t.types.iter().map(|ty| type_shape(resolve, *ty)).collect::<Vec<_>>().join(",")),
            TypeDefKind::Record(r) => format!("record{{{}}}", r.fields.iter().map(|f| format!("{}:{}", f.name, type_shape(resolve, f.ty))).collect::<Vec<_>>().join(",")),
            TypeDefKind::Result(r) => format!("result<{},{}>", r.ok.map(|ty| type_shape(resolve, ty)).unwrap_or_default(), r.err.map(|ty| type_shape(resolve, ty)).unwrap_or_default()),
            kind => kind.as_str().into(),
        },
    }
}

#[rustfmt::skip]
fn signature_text(s: &Signature) -> String { format!("({}) -> {}", s.params.join(", "), s.result.as_deref().unwrap_or("()")) }

#[rustfmt::skip]
fn target_key(target: &Target) -> String { format!("{}#{}", target.interface, target.function) }

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
        if value_name(value) != expected {
            return Err(registry::InvokeError::TypeMismatch(format!(
                "arg {index} expected {expected}, got {}",
                value_name(value)
            )));
        }
    }
    Ok(())
}

#[rustfmt::skip]
fn value_name(value: &registry::Value) -> &str {
    match value {
        registry::Value::Bool(_) => "bool",
        registry::Value::S32(_) => "s32",
        registry::Value::U32(_) => "u32",
        registry::Value::String(_) => "string",
        registry::Value::List(_) => "list<string>",
    }
}

#[rustfmt::skip]
fn value_to_val(value: registry::Value) -> Val {
    match value {
        registry::Value::Bool(v) => Val::Bool(v),
        registry::Value::S32(v) => Val::S32(v),
        registry::Value::U32(v) => Val::U32(v),
        registry::Value::String(v) => Val::String(v),
        registry::Value::List(v) => Val::List(v.into_iter().map(Val::String).collect()),
    }
}

#[rustfmt::skip]
fn val_to_value(value: Val) -> Result<registry::Value, registry::InvokeError> {
    match value {
        Val::Bool(v) => Ok(registry::Value::Bool(v)), Val::S32(v) => Ok(registry::Value::S32(v)),
        Val::U32(v) => Ok(registry::Value::U32(v)), Val::String(v) => Ok(registry::Value::String(v)),
        Val::List(v) => v.into_iter().map(|v| match v { Val::String(s) => Ok(s), other => Err(registry::InvokeError::Trapped(format!("unsupported list item {other:?}"))) }).collect::<Result<Vec<_>, _>>().map(registry::Value::List),
        other => Err(registry::InvokeError::Trapped(format!("unsupported result {other:?}"))),
    }
}

#[rustfmt::skip]
fn print_load(definition: &PluginDefinition) {
    let id = &definition.manifest.id;
    let targets = definition.targets.iter().map(|(_, t)| format!("{}{}", target_key(t), signature_text(&t.signature))).collect::<Vec<_>>().join(", ");
    println!("[load] {id:<12} ok   provides {targets}");
}

#[rustfmt::skip]
fn render_values(values: &[Val]) -> String {
    match values {
        [Val::Result(Ok(Some(value)))] => match value.as_ref() {
            Val::String(value) => format!("{value:?}"),
            other => format!("{other:?}"),
        },
        [Val::List(values)] => format!("{values:?}"),
        other => format!("{other:?}"),
    }
}
