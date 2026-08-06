//! Enforcement-focused tests for loading, linking, recursion, and fuel exhaustion.
//! The component fixtures are built by the host build script and decoded where needed.

use crate::{
    manifest::{Capabilities, FsCapability, Manifest, NetCapability},
    plugin::{Signature, Target, decode_imports, interface_functions},
    runtime::{
        CALL_STACK, FUEL, MAX_DEPTH, PluginRuntime, PluginState, PluginTable, invoke_target,
        resolve_direct_target,
    },
};
use std::{
    collections::HashMap,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex},
};
use wasmtime::{
    Config, Engine, Store,
    component::{Component, Linker, ResourceTable, types::Type},
};
use wasmtime_wasi::WasiCtxBuilder;
use wit_component::{
    ComponentEncoder, DecodedWasm, StringEncoding, dummy_module, embed_component_metadata,
};
use wit_parser::{ManglingAndAbi, Resolve};

fn manifest(id: &str) -> Manifest {
    Manifest {
        id: id.into(),
        provides: vec![],
        invokes: vec![],
        capabilities: Capabilities {
            registry: false,
            fs: Some(FsCapability {
                read: vec![],
                write: vec![],
            }),
            net: Some(NetCapability { hosts: vec![] }),
        },
    }
}

fn target(plugin: &str) -> Target {
    Target {
        plugin: plugin.into(),
        interface: "demo:test/api@0.1.0".into(),
        function: "run".into(),
        signature: Signature {
            params: vec![],
            results: vec![],
        },
    }
}

fn decoded(id: &str) -> (Resolve, wit_parser::WorldId) {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let bytes = fs::read(root.join("plugins").join(id).join(format!("{id}.wasm"))).unwrap();
    match wit_component::decode(&bytes).unwrap() {
        DecodedWasm::Component(resolve, world) => (resolve, world),
        _ => panic!("not a component"),
    }
}

fn signature_from_wit(engine: &Engine, wit: &str) -> Signature {
    let component = component_from_wit(engine, wit);
    interface_functions(engine, &component, "demo:structural/api@0.1.0", false)
        .unwrap()
        .into_iter()
        .find_map(|(name, signature)| (name == "run").then_some(signature))
        .unwrap()
}

fn component_from_wit(engine: &Engine, wit: &str) -> Component {
    let mut resolve = Resolve::new();
    let package = resolve.push_str("fixture.wit", wit).unwrap();
    let world = resolve.packages[package].worlds["fixture"];
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    let bytes = ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap();
    Component::new(engine, bytes).unwrap()
}

fn structural_wit(
    variant_payload: &str,
    enum_case: &str,
    flag: &str,
    record_field: &str,
) -> String {
    format!(
        r#"package demo:structural@0.1.0;

interface api {{
  variant choice {{ text(string), number({variant_payload}) }}
  enum mode {{ fast, {enum_case} }}
  flags options {{ loud, {flag} }}
  record request {{ {record_field}: choice, mode: mode, options: options }}
  run: func(input: request) -> request;
}}

world fixture {{ export api; }}"#
    )
}

#[test]
fn unused_declared_capability_is_harmless() {
    let (resolve, world) = decoded("greeter");
    let mut manifest = manifest("greeter");
    manifest.capabilities.registry = true;
    assert!(
        decode_imports(&resolve, world, &manifest)
            .unwrap()
            .is_empty()
    );
}

#[test]
fn registry_import_requires_explicit_capability() {
    let (resolve, world) = decoded("dynamic");
    let error = decode_imports(&resolve, world, &manifest("dynamic")).unwrap_err();
    assert!(error.to_string().contains("tangent:core/registry"));
}

#[test]
fn fixed_length_lists_fail_cleanly_before_runtime_introspection() {
    let mut resolve = Resolve::new();
    let package = resolve
        .push_str(
            "fixed-list.wit",
            r#"package demo:fixed-list@0.1.0;

interface api {
  run: func(input: list<u32, 4>);
}

world fixture { import api; }"#,
        )
        .unwrap();
    let world = resolve.packages[package].worlds["fixture"];
    let error = decode_imports(&resolve, world, &manifest("fixed-list")).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("fixed-length lists are unsupported")
    );
}

#[test]
fn direct_import_checks_policy_and_signature() {
    let plugins = Arc::new(Mutex::new(PluginTable::default()));
    let key = "demo:greeter/greeter@0.1.0#greet";
    let expected = Signature {
        params: vec![Type::String],
        results: vec![Type::String],
    };
    let provider = Target {
        plugin: "greeter".into(),
        interface: "demo:greeter/greeter@0.1.0".into(),
        function: "greet".into(),
        signature: expected.clone(),
    };
    plugins.lock().unwrap().targets.insert(key.into(), provider);

    let error = resolve_direct_target(&plugins, &manifest("caller"), key, &expected).unwrap_err();
    assert!(error.to_string().contains("not permitted"));

    let mut caller = manifest("caller");
    caller.invokes.push(key.into());
    let changed = Signature {
        params: vec![Type::S32],
        results: vec![Type::String],
    };
    let error = resolve_direct_target(&plugins, &caller, key, &changed).unwrap_err();
    assert!(error.to_string().contains("type mismatch"));
    assert!(error.to_string().contains("s32"));
}

#[test]
fn direct_import_compares_complete_structural_types() {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).unwrap();
    let baseline = signature_from_wit(&engine, &structural_wit("s32", "safe", "polite", "choice"));
    let key = "demo:structural/api@0.1.0#run";
    let plugins = Arc::new(Mutex::new(PluginTable::default()));
    plugins.lock().unwrap().targets.insert(
        key.into(),
        Target {
            plugin: "provider".into(),
            interface: "demo:structural/api@0.1.0".into(),
            function: "run".into(),
            signature: baseline,
        },
    );
    let mut caller = manifest("caller");
    caller.invokes.push(key.into());

    for changed in [
        structural_wit("u32", "safe", "polite", "choice"),
        structural_wit("s32", "careful", "polite", "choice"),
        structural_wit("s32", "safe", "quiet", "choice"),
        structural_wit("s32", "safe", "polite", "selection"),
    ] {
        let expected = signature_from_wit(&engine, &changed);
        let error = resolve_direct_target(&plugins, &caller, key, &expected).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("type mismatch"));
        assert!(message.contains("variant"));
        assert!(message.contains("enum"));
        assert!(message.contains("flags"));
        assert!(message.contains("record"));
    }
}

#[test]
fn direct_import_rejects_cross_store_resource_handles() {
    let mut config = Config::new();
    config.wasm_component_model(true);
    let engine = Engine::new(&config).unwrap();
    let component = component_from_wit(
        &engine,
        r#"package demo:structural@0.1.0;

interface api {
  resource file;
  run: func(input: borrow<file>);
}

world fixture { export api; }"#,
    );
    let error =
        interface_functions(&engine, &component, "demo:structural/api@0.1.0", false).unwrap_err();
    assert!(
        error
            .to_string()
            .contains("resource handles are unsupported")
    );
}

#[test]
fn rejects_depth_limit_and_cycles() {
    let plugins = Arc::new(Mutex::new(PluginTable::default()));
    CALL_STACK
        .with(|stack| *stack.borrow_mut() = (0..MAX_DEPTH).map(|n| format!("p{n}")).collect());
    let error = invoke_target(&plugins, &target("callee"), &[]).unwrap_err();
    assert!(error.to_string().contains("depth"));

    CALL_STACK.with(|stack| *stack.borrow_mut() = vec!["callee".into()]);
    let error = invoke_target(&plugins, &target("callee"), &[]).unwrap_err();
    assert!(error.to_string().contains("cycle"));
    CALL_STACK.with(|stack| stack.borrow_mut().clear());
}

#[test]
fn fuel_trap_marks_callee_unhealthy() {
    let mut config = Config::new();
    config.wasm_component_model(true).consume_fuel(true);
    let engine = Engine::new(&config).unwrap();
    let component = Component::new(
        &engine,
        r#"(component
            (core module $m (func (export "loop") (loop $again (br $again))))
            (core instance $i (instantiate $m))
            (func $loop (canon lift (core func $i "loop")))
            (instance $api (export "run" (func $loop)))
            (export "demo:test/api@0.1.0" (instance $api)))"#,
    )
    .unwrap();
    let plugins = Arc::new(Mutex::new(PluginTable::default()));
    let state = PluginState {
        manifest: manifest("loop"),
        wasi: WasiCtxBuilder::new().build(),
        table: ResourceTable::new(),
        plugins: Arc::downgrade(&plugins),
        handles: HashMap::new(),
        next_handle: 1,
    };
    let mut store = Store::new(&engine, state);
    store.set_fuel(FUEL).unwrap();
    let instance = Linker::new(&engine)
        .instantiate(&mut store, &component)
        .unwrap();
    let runtime = Arc::new(Mutex::new(PluginRuntime {
        store,
        instance,
        healthy: true,
    }));
    plugins
        .lock()
        .unwrap()
        .runtimes
        .insert("loop".into(), runtime.clone());

    let error = invoke_target(&plugins, &target("loop"), &[]).unwrap_err();
    assert!(format!("{error:#}").contains("fuel"));
    assert!(!runtime.lock().unwrap().healthy);
}
