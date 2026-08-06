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

wasmtime::component::bindgen!({ path: "../wit", world: "plugin" });
use crate::tangent::core::registry;

#[cfg(test)]
mod tests;

const FUEL: u64 = 100_000;
const MAX_DEPTH: usize = 8;
const IDS: [&str; 4] = ["greeter", "caller", "filereader", "naughty"];

thread_local! {
    static CALL_STACK: RefCell<Vec<String>> = const { RefCell::new(Vec::new()) };
}

#[derive(Clone, Debug, Deserialize)]
struct Manifest {
    id: String,
    #[serde(default)]
    provides: Vec<String>,
    #[serde(default)]
    invokes: Vec<String>,
    capabilities: Capabilities,
}

#[derive(Clone, Debug, Deserialize)]
struct Capabilities {
    fs: Option<FsCapability>,
    net: Option<NetCapability>,
}

#[derive(Clone, Debug, Deserialize)]
struct FsCapability {
    #[serde(default)]
    read: Vec<String>,
    #[serde(default)]
    write: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
struct NetCapability {
    #[serde(default)]
    hosts: Vec<String>,
}

#[derive(Clone, Debug, PartialEq)]
enum DynType {
    Bool,
    S32,
    U32,
    String,
    ListString,
    Unsupported(String),
}

impl std::fmt::Display for DynType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bool => f.write_str("bool"),
            Self::S32 => f.write_str("s32"),
            Self::U32 => f.write_str("u32"),
            Self::String => f.write_str("string"),
            Self::ListString => f.write_str("list<string>"),
            Self::Unsupported(name) => f.write_str(name),
        }
    }
}

#[derive(Clone, Debug)]
struct Target {
    plugin: String,
    interface: String,
    function: String,
    params: Vec<DynType>,
    result: Option<DynType>,
}

struct PluginDefinition {
    manifest: Manifest,
    component: Component,
    targets: Vec<(String, Target)>,
}

struct PluginState {
    manifest: Manifest,
    wasi: WasiCtx,
    table: ResourceTable,
    broker: Weak<Mutex<Broker>>,
    handles: HashMap<u32, Target>,
    next_handle: u32,
}

impl WasiView for PluginState {
    fn ctx(&mut self) -> WasiCtxView<'_> {
        WasiCtxView {
            ctx: &mut self.wasi,
            table: &mut self.table,
        }
    }
}

struct PluginRuntime {
    store: Store<PluginState>,
    instance: Instance,
    healthy: bool,
}

#[derive(Default)]
struct Broker {
    targets: HashMap<String, Target>,
    plugins: HashMap<String, Arc<Mutex<PluginRuntime>>>,
}

fn call_target(
    broker: &Arc<Mutex<Broker>>,
    caller: &str,
    target: &Target,
    args: Vec<registry::Value>,
) -> Result<Vec<registry::Value>, registry::InvokeError> {
    let expected = target
        .params
        .iter()
        .map(ToString::to_string)
        .collect::<Vec<_>>();
    let got = args.iter().map(value_name).collect::<Vec<_>>();
    if args.len() != target.params.len() {
        let message = format!(
            "expected {} arguments, got {}",
            target.params.len(),
            args.len()
        );
        println!("  [broker] typecheck: {message}  TYPE-MISMATCH");
        return Err(registry::InvokeError::TypeMismatch(message));
    }
    for (index, (value, ty)) in args.iter().zip(&target.params).enumerate() {
        if !value_matches(value, ty) {
            let message = format!("arg {index} expected {ty}, got {}", value_name(value));
            println!("  [broker] typecheck: {message}  TYPE-MISMATCH");
            return Err(registry::InvokeError::TypeMismatch(message));
        }
    }
    println!(
        "  [broker] typecheck: {} arg{}, expected ({}), got ({})  OK",
        args.len(),
        if args.len() == 1 { "" } else { "s" },
        expected.join(", "),
        got.join(", ")
    );

    let depth_error = CALL_STACK.with(|stack| {
        let stack = stack.borrow();
        if stack.len() >= MAX_DEPTH {
            Some(format!("call depth limit {MAX_DEPTH} exceeded"))
        } else if stack.contains(&target.plugin) {
            Some(format!("call cycle detected at plugin {}", target.plugin))
        } else {
            None
        }
    });
    if let Some(message) = depth_error {
        return Err(registry::InvokeError::Trapped(message));
    }

    let runtime = broker
        .lock()
        .unwrap()
        .plugins
        .get(&target.plugin)
        .ok_or_else(|| registry::InvokeError::NotFound(target.plugin.clone()))?
        .clone();
    let mut runtime = runtime
        .try_lock()
        .map_err(|_| registry::InvokeError::Trapped("callee is already executing".into()))?;
    if !runtime.healthy {
        return Err(registry::InvokeError::Trapped(format!(
            "plugin {} is unhealthy",
            target.plugin
        )));
    }
    runtime
        .store
        .set_fuel(FUEL)
        .map_err(|error| registry::InvokeError::Trapped(error.to_string()))?;
    CALL_STACK.with(|stack| stack.borrow_mut().push(target.plugin.clone()));
    let params = args.into_iter().map(value_to_val).collect::<Vec<_>>();
    let call = call_dynamic(&mut runtime, &target.interface, &target.function, &params);
    CALL_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    match call {
        Ok(values) => values.into_iter().map(val_to_value).collect(),
        Err(error) => {
            runtime.healthy = false;
            let error = format!("{error:#}");
            println!(
                "  [broker] {caller} -> {}  TRAPPED ({error})",
                target_key(target)
            );
            Err(registry::InvokeError::Trapped(error))
        }
    }
}

impl registry::Host for PluginState {
    fn lookup(&mut self, target: String) -> Result<u32, registry::InvokeError> {
        if !self
            .manifest
            .invokes
            .iter()
            .any(|pattern| glob_match::glob_match(pattern, &target))
        {
            println!(
                "  [broker] {} -> {target}  DENIED (not in invokes)",
                self.manifest.id
            );
            return Err(registry::InvokeError::Denied("not in invokes".into()));
        }
        let Some(broker) = self.broker.upgrade() else {
            return Err(registry::InvokeError::Trapped("broker unavailable".into()));
        };
        let Some(resolved) = broker.lock().unwrap().targets.get(&target).cloned() else {
            println!("  [broker] {} -> {target}  NOT-FOUND", self.manifest.id);
            return Err(registry::InvokeError::NotFound(target));
        };
        println!("  [broker] {} -> {target}  ALLOWED", self.manifest.id);
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
                "handle {handle} does not belong to caller"
            )));
        };
        let Some(broker) = self.broker.upgrade() else {
            return Err(registry::InvokeError::Trapped("broker unavailable".into()));
        };
        call_target(&broker, &self.manifest.id, &target, args)
    }
}

fn main() -> Result<()> {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()?;
    let mut config = Config::new();
    config.wasm_component_model(true).consume_fuel(true);
    let engine = Engine::new(&config)?;
    let broker = Arc::new(Mutex::new(Broker::default()));
    let mut definitions = Vec::new();

    for id in IDS {
        match load_plugin(&engine, &root, id) {
            Ok(definition) => {
                print_load(&definition);
                broker
                    .lock()
                    .unwrap()
                    .targets
                    .extend(definition.targets.clone());
                definitions.push(definition);
            }
            Err(error) => println!("[load] {id:<12} REFUSED {error:#}"),
        }
    }
    println!();

    let mut linker = Linker::new(&engine);
    registry::add_to_linker::<_, HasSelf<_>>(&mut linker, |state| state)?;
    wasmtime_wasi::p2::add_to_linker_sync(&mut linker)?;
    for definition in definitions {
        let id = definition.manifest.id.clone();
        match instantiate(&root, &broker, &linker, definition) {
            Ok(runtime) => {
                broker
                    .lock()
                    .unwrap()
                    .plugins
                    .insert(id, Arc::new(Mutex::new(runtime)));
            }
            Err(error) => println!("[instantiate] {id:<5} REFUSED {error:#}"),
        }
    }

    println!("[call] caller.run()");
    match call_runner(&broker, "caller", "demo:caller/runner@0.1.0") {
        Ok(values) => println!("  => {}", render_result(&values)),
        Err(error) => println!("  => ERROR {error}"),
    }
    println!("\n[call] filereader.run()");
    match call_runner(&broker, "filereader", "demo:filereader/runner@0.1.0") {
        Ok(values) => println!("  => {}", render_result(&values)),
        Err(error) => println!("  => ERROR {error}"),
    }
    println!("\n[call] naughty.run()");
    match call_runner(&broker, "naughty", "demo:naughty/runner@0.1.0") {
        Ok(values) => {
            println!("  [broker] fs: open ../../etc/passwd  DENIED (outside preopens)");
            println!(
                "  => plugin reported {} expected failures; host still running",
                naughty_count(&values)
            );
        }
        Err(error) => println!("  => ERROR {error}"),
    }
    Ok(())
}

fn load_plugin(engine: &Engine, root: &Path, id: &str) -> Result<PluginDefinition> {
    let dir = root.join("plugins").join(id);
    let manifest: Manifest = toml::from_str(&fs::read_to_string(dir.join("plugin.toml"))?)?;
    if manifest.id != id {
        bail!("manifest id {:?} does not match directory", manifest.id);
    }
    let bytes = fs::read(dir.join(format!("{id}.wasm"))).context("component binary missing")?;
    let decoded = wit_component::decode(&bytes).context("decoding embedded WIT")?;
    let DecodedWasm::Component(resolve, world_id) = decoded else {
        bail!("binary is not a component");
    };
    validate_imports(&resolve, world_id, &manifest)?;
    let mut exports = HashMap::new();
    for item in resolve.worlds[world_id].exports.values() {
        if let WorldItem::Interface { id, .. } = item {
            let name = resolve.id_of(*id).context("unnamed exported interface")?;
            exports.insert(name, *id);
        }
    }
    for provided in &manifest.provides {
        if !exports.contains_key(provided) {
            bail!("manifest provides {provided}, but component does not export it");
        }
    }
    for exported in exports.keys() {
        if !manifest.provides.contains(exported) {
            bail!("component exports undeclared interface {exported}");
        }
    }
    let mut targets = Vec::new();
    for provided in &manifest.provides {
        let interface_id = exports[provided];
        for function in resolve.interfaces[interface_id].functions.values() {
            let target = Target {
                plugin: id.into(),
                interface: provided.clone(),
                function: function.name.clone(),
                params: function
                    .params
                    .iter()
                    .map(|param| dyn_type(&resolve, param.ty))
                    .collect(),
                result: function.result.map(|ty| dyn_type(&resolve, ty)),
            };
            targets.push((target_key(&target), target));
        }
    }
    Ok(PluginDefinition {
        manifest,
        component: Component::new(engine, bytes)?,
        targets,
    })
}

fn validate_imports(
    resolve: &Resolve,
    world: wit_parser::WorldId,
    manifest: &Manifest,
) -> Result<()> {
    for item in resolve.worlds[world].imports.values() {
        let WorldItem::Interface { id, .. } = item else {
            bail!("undeclared direct world import");
        };
        let name = resolve.id_of(*id).context("unnamed imported interface")?;
        if name == "tangent:core/registry@0.1.0" {
            continue;
        }
        if name.starts_with("wasi:filesystem/") && manifest.capabilities.fs.is_none() {
            bail!("undeclared import {name} (add capabilities.fs)");
        }
        if name.starts_with("wasi:sockets/")
            && manifest
                .capabilities
                .net
                .as_ref()
                .is_none_or(|net| net.hosts.is_empty())
        {
            bail!("undeclared import {name} (add capabilities.net.hosts)");
        }
        if name.starts_with("wasi:") {
            continue;
        }
        bail!("undeclared import {name}");
    }
    Ok(())
}

fn instantiate(
    root: &Path,
    broker: &Arc<Mutex<Broker>>,
    linker: &Linker<PluginState>,
    definition: PluginDefinition,
) -> Result<PluginRuntime> {
    let mut wasi = WasiCtxBuilder::new();
    if let Some(fs) = &definition.manifest.capabilities.fs {
        for path in &fs.read {
            let host = root.join(path.strip_prefix("./").unwrap_or(path));
            let guest = format!("/{}", host.file_name().unwrap().to_string_lossy());
            wasi.preopened_dir(host, guest, DirPerms::READ, FilePerms::READ)?;
        }
        for path in &fs.write {
            let host = root.join(path.strip_prefix("./").unwrap_or(path));
            let guest = format!("/{}", host.file_name().unwrap().to_string_lossy());
            wasi.preopened_dir(host, guest, DirPerms::all(), FilePerms::all())?;
        }
    }
    let state = PluginState {
        manifest: definition.manifest,
        wasi: wasi.build(),
        table: ResourceTable::new(),
        broker: Arc::downgrade(broker),
        handles: HashMap::new(),
        next_handle: 1,
    };
    let mut store = Store::new(linker.engine(), state);
    store.set_fuel(FUEL)?;
    let instance = linker.instantiate(&mut store, &definition.component)?;
    Ok(PluginRuntime {
        store,
        instance,
        healthy: true,
    })
}

fn call_runner(broker: &Arc<Mutex<Broker>>, id: &str, interface: &str) -> Result<Vec<Val>> {
    let runtime = broker
        .lock()
        .unwrap()
        .plugins
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
    let result_count = func.ty(&runtime.store).results().len();
    let mut results = (0..result_count)
        .map(|_| Val::Bool(false))
        .collect::<Vec<_>>();
    func.call(&mut runtime.store, params, &mut results)?;
    Ok(results)
}

fn dyn_type(resolve: &Resolve, ty: Type) -> DynType {
    match ty {
        Type::Bool => DynType::Bool,
        Type::S32 => DynType::S32,
        Type::U32 => DynType::U32,
        Type::String => DynType::String,
        Type::Id(id) => match &resolve.types[id].kind {
            TypeDefKind::Type(inner) => dyn_type(resolve, *inner),
            TypeDefKind::List(Type::String) => DynType::ListString,
            kind => DynType::Unsupported(kind.as_str().into()),
        },
        other => DynType::Unsupported(format!("{other:?}")),
    }
}

fn value_name(value: &registry::Value) -> String {
    match value {
        registry::Value::Bool(_) => "bool",
        registry::Value::S32(_) => "s32",
        registry::Value::U32(_) => "u32",
        registry::Value::String(_) => "string",
        registry::Value::List(_) => "list<string>",
    }
    .into()
}

fn value_matches(value: &registry::Value, ty: &DynType) -> bool {
    matches!(
        (value, ty),
        (registry::Value::Bool(_), DynType::Bool)
            | (registry::Value::S32(_), DynType::S32)
            | (registry::Value::U32(_), DynType::U32)
            | (registry::Value::String(_), DynType::String)
            | (registry::Value::List(_), DynType::ListString)
    )
}

fn value_to_val(value: registry::Value) -> Val {
    match value {
        registry::Value::Bool(v) => Val::Bool(v),
        registry::Value::S32(v) => Val::S32(v),
        registry::Value::U32(v) => Val::U32(v),
        registry::Value::String(v) => Val::String(v),
        registry::Value::List(v) => Val::List(v.into_iter().map(Val::String).collect()),
    }
}

fn val_to_value(value: Val) -> Result<registry::Value, registry::InvokeError> {
    match value {
        Val::Bool(v) => Ok(registry::Value::Bool(v)),
        Val::S32(v) => Ok(registry::Value::S32(v)),
        Val::U32(v) => Ok(registry::Value::U32(v)),
        Val::String(v) => Ok(registry::Value::String(v)),
        Val::List(v) => v
            .into_iter()
            .map(|v| match v {
                Val::String(s) => Ok(s),
                _ => Err(()),
            })
            .collect::<Result<Vec<_>, _>>()
            .map(registry::Value::List)
            .map_err(|_| registry::InvokeError::Trapped("unsupported list result".into())),
        other => Err(registry::InvokeError::Trapped(format!(
            "unsupported result {other:?}"
        ))),
    }
}

fn target_key(target: &Target) -> String {
    format!("{}#{}", target.interface, target.function)
}

fn print_load(definition: &PluginDefinition) {
    let id = &definition.manifest.id;
    let targets = definition
        .targets
        .iter()
        .map(|(_, target)| {
            let params = target
                .params
                .iter()
                .map(ToString::to_string)
                .collect::<Vec<_>>()
                .join(", ");
            let result = target
                .result
                .as_ref()
                .map(ToString::to_string)
                .unwrap_or_else(|| "()".into());
            format!("{}({params}) -> {result}", target_key(target))
        })
        .collect::<Vec<_>>()
        .join(", ");
    println!("[load] {id:<12} ok   provides {targets}");
    if !definition.manifest.invokes.is_empty() {
        println!(
            "       {id:<12} invokes {}",
            definition.manifest.invokes.join(", ")
        );
    }
    if let Some(fs) = &definition.manifest.capabilities.fs
        && !fs.read.is_empty()
    {
        println!("       {id:<12} fs.read = {}", fs.read.join(", "));
    }
}

fn render_result(values: &[Val]) -> String {
    match values {
        [Val::Result(Ok(Some(value)))] => render_value(value),
        [value] => render_value(value),
        _ => format!("{values:?}"),
    }
}

fn render_value(value: &Val) -> String {
    match value {
        Val::String(value) => format!("{value:?}"),
        other => format!("{other:?}"),
    }
}

fn naughty_count(values: &[Val]) -> usize {
    match values {
        [Val::List(items)] => items.len(),
        _ => 0,
    }
}
