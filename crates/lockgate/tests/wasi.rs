mod common;

use common::exec::ExecEngine;
use common::{INVOCATION_FUEL, LIMITS, TestState};
use wasmtime::Result;
use wasmtime::component::Val;

#[tokio::test(flavor = "current_thread")]
async fn wasi_importing_guest_instantiates_and_runs() -> Result<()> {
    let engine = ExecEngine::new()?;
    let loaded = engine.load::<TestState>(&common::WASI_FIXTURE, |_| Ok(()))?;
    let clock = loaded
        .export("test:wasi/guest", "clock-seconds")
        .expect("clock export should resolve structurally");

    let results = loaded
        .invoke(clock, &[], TestState, LIMITS, INVOCATION_FUEL)
        .await?;

    assert!(matches!(results.as_slice(), [Val::U64(seconds)] if *seconds > 0));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn wasi_outbound_network_is_denied_at_runtime() -> Result<()> {
    let engine = ExecEngine::new()?;
    let loaded = engine.load::<TestState>(&common::WASI_FIXTURE, |_| Ok(()))?;
    let connect = loaded
        .export("test:wasi/guest", "network-denied")
        .expect("network denial export should resolve structurally");

    let results = loaded
        .invoke(connect, &[], TestState, LIMITS, INVOCATION_FUEL)
        .await?;

    assert!(matches!(results.as_slice(), [Val::Bool(true)]));
    Ok(())
}
