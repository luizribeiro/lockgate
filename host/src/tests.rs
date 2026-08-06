//! Enforcement-focused tests for loading, linking, recursion, and fuel exhaustion.
//! The component fixtures are built by the host build script and decoded where needed.

use crate::{
    manifest::{Capabilities, FsCapability, Manifest, NetCapability},
    plugin::{Signature, Target, decode_imports},
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
    component::{Component, Linker, ResourceTable},
};
use wasmtime_wasi::WasiCtxBuilder;
use wit_component::DecodedWasm;
use wit_parser::Resolve;

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
            result: None,
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
fn direct_import_checks_policy_and_signature() {
    let plugins = Arc::new(Mutex::new(PluginTable::default()));
    let key = "demo:greeter/greeter@0.1.0#greet";
    let expected = Signature {
        params: vec!["string".into()],
        result: Some("string".into()),
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
        params: vec!["s32".into()],
        result: Some("string".into()),
    };
    let error = resolve_direct_target(&plugins, &caller, key, &changed).unwrap_err();
    assert!(error.to_string().contains("type mismatch"));
    assert!(error.to_string().contains("s32"));
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
