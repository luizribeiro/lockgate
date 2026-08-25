mod common;

use common::exec::{ExecEngine, ExecError, LoadError, TrapDetail, host_import_error};
use common::{INVOCATION_FUEL, LIMITS, TestState, wire_ready_wait};
use wasmtime::Trap;

#[test]
fn load_errors_distinguish_compile_from_link() {
    let engine = ExecEngine::new().unwrap();

    assert!(matches!(
        engine.load::<TestState>(b"not a component", |_| Ok(())),
        Err(LoadError::Compile(_))
    ));
    assert!(matches!(
        engine.load::<TestState>(&common::EXEC_FIXTURE, |_| Ok(())),
        Err(LoadError::Link(_))
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn trap_is_isolated_from_the_next_invocation() {
    let engine = ExecEngine::new().unwrap();
    let loaded = engine
        .load::<TestState>(&common::EXEC_FIXTURE, wire_ready_wait)
        .unwrap();
    let trap = loaded
        .export("test:exec/guest", "trap")
        .expect("trap export should resolve structurally");
    let value = loaded
        .export("test:exec/guest", "value")
        .expect("value export should resolve structurally");

    let error = loaded
        .invoke(trap, &[], TestState, LIMITS, INVOCATION_FUEL, None)
        .await
        .expect_err("guest unreachable should trap");
    assert!(matches!(
        error,
        ExecError::Trap(TrapDetail::Wasm {
            trap: Trap::UnreachableCodeReached,
            ..
        })
    ));

    let results = loaded
        .invoke(value, &[], TestState, LIMITS, INVOCATION_FUEL, None)
        .await
        .expect("a trap must not poison later invocations");
    assert!(matches!(
        results.as_slice(),
        [wasmtime::component::Val::U32(42)]
    ));
}

#[tokio::test(flavor = "current_thread")]
async fn marked_host_failure_maps_to_host_import() {
    let engine = ExecEngine::new().unwrap();
    let loaded = engine
        .load::<TestState>(&common::EXEC_FIXTURE, |linker| {
            linker
                .instance("test:exec/host")?
                .func_wrap_concurrent("wait", |_, (): ()| {
                    Box::pin(async {
                        Err::<(), _>(host_import_error(anyhow::anyhow!(
                            "instrumented host failure"
                        )))
                    })
                })?;
            Ok(())
        })
        .unwrap();
    let suspend = loaded
        .export("test:exec/guest", "suspend")
        .expect("suspend export should resolve structurally");

    let error = loaded
        .invoke(suspend, &[], TestState, LIMITS, INVOCATION_FUEL, None)
        .await
        .expect_err("marked host failure should fail the invocation");
    let ExecError::HostImport(error) = error else {
        panic!("expected HostImport, got {error:?}");
    };
    assert_eq!(error.to_string(), "instrumented host failure");
}

#[tokio::test(flavor = "current_thread")]
async fn guest_trap_after_successful_import_stays_a_trap() {
    let engine = ExecEngine::new().unwrap();
    let loaded = engine
        .load::<TestState>(&common::EXEC_FIXTURE, wire_ready_wait)
        .unwrap();
    let export = loaded
        .export("test:exec/guest", "import-then-trap")
        .expect("import-then-trap export should resolve structurally");

    let error = loaded
        .invoke(export, &[], TestState, LIMITS, INVOCATION_FUEL, None)
        .await
        .expect_err("guest unreachable should trap after its import returns");
    assert!(matches!(
        error,
        ExecError::Trap(TrapDetail::Wasm {
            trap: Trap::UnreachableCodeReached,
            ..
        })
    ));
}
