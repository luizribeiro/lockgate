use super::*;

fn manifest(id: &str) -> Manifest {
    Manifest {
        id: id.into(),
        provides: vec![],
        invokes: vec![],
        capabilities: Capabilities {
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
        params: vec![],
        result: None,
    }
}

#[test]
fn rejects_undeclared_import_by_name() {
    let mut resolve = Resolve::default();
    let package = resolve
        .push_str(
            "bad.wit",
            "package test:bad@0.1.0; interface secret { ping: func(); } world plugin { import secret; }",
        )
        .unwrap();
    let world = resolve.packages[package].worlds["plugin"];
    let error = validate_imports(&resolve, world, &manifest("bad")).unwrap_err();
    assert!(error.to_string().contains("test:bad/secret@0.1.0"));
}

#[test]
fn rejects_depth_limit_and_cycles() {
    let broker = Arc::new(Mutex::new(Broker::default()));
    CALL_STACK
        .with(|stack| *stack.borrow_mut() = (0..MAX_DEPTH).map(|n| format!("p{n}")).collect());
    let error = call_target(&broker, "caller", &target("callee"), vec![]).unwrap_err();
    assert!(matches!(error, registry::InvokeError::Trapped(message) if message.contains("depth")));

    CALL_STACK.with(|stack| *stack.borrow_mut() = vec!["callee".into()]);
    let error = call_target(&broker, "caller", &target("callee"), vec![]).unwrap_err();
    assert!(matches!(error, registry::InvokeError::Trapped(message) if message.contains("cycle")));
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
            (core module $m
                (func (export "loop") (loop $again (br $again))))
            (core instance $i (instantiate $m))
            (func $loop (canon lift (core func $i "loop")))
            (instance $api (export "run" (func $loop)))
            (export "demo:test/api@0.1.0" (instance $api)))"#,
    )
    .unwrap();
    let broker = Arc::new(Mutex::new(Broker::default()));
    let state = PluginState {
        manifest: manifest("loop"),
        wasi: WasiCtxBuilder::new().build(),
        table: ResourceTable::new(),
        broker: Arc::downgrade(&broker),
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
    broker
        .lock()
        .unwrap()
        .plugins
        .insert("loop".into(), runtime.clone());

    let error = call_target(&broker, "caller", &target("loop"), vec![]).unwrap_err();
    assert!(matches!(error, registry::InvokeError::Trapped(message) if message.contains("fuel")));
    assert!(!runtime.lock().unwrap().healthy);
}
