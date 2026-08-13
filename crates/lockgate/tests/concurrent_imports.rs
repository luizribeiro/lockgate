#[allow(
    dead_code,
    reason = "the concurrent-import test does not use the cancellation drop probe"
)]
#[path = "../src/exec/mod.rs"]
mod exec;

mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use exec::{ExecEngine, ExecLimits};
use tokio::sync::Barrier;
use tokio::time::timeout;
use wasmtime::Result;
use wasmtime::component::Val;

const LIMITS: ExecLimits = ExecLimits {
    instantiation_fuel: 1_000_000,
    max_memory_bytes: 16 * 1024 * 1024,
};
const INVOCATION_FUEL: u64 = 1_000_000;
const REGRESSION_TIMEOUT: Duration = Duration::from_secs(10);

struct TestState;

#[tokio::test(flavor = "current_thread")]
async fn guest_async_imports_run_concurrently() -> Result<()> {
    let engine = ExecEngine::new()?;
    let barrier = Arc::new(Barrier::new(2));
    let entries = Arc::new(AtomicUsize::new(0));
    let loaded = engine.load::<TestState>(common::exec_concurrent_fixture(), {
        let barrier = Arc::clone(&barrier);
        let entries = Arc::clone(&entries);
        move |linker| {
            let mut host = linker.instance("test:exec-concurrent/host")?;
            host.func_wrap_concurrent("first", {
                let barrier = Arc::clone(&barrier);
                let entries = Arc::clone(&entries);
                move |_, (): ()| {
                    let barrier = Arc::clone(&barrier);
                    let entries = Arc::clone(&entries);
                    Box::pin(async move {
                        entries.fetch_add(1, Ordering::SeqCst);
                        barrier.wait().await;
                        Ok(())
                    })
                }
            })?;
            host.func_wrap_concurrent("second", move |_, (): ()| {
                let barrier = Arc::clone(&barrier);
                let entries = Arc::clone(&entries);
                Box::pin(async move {
                    entries.fetch_add(1, Ordering::SeqCst);
                    barrier.wait().await;
                    Ok(())
                })
            })?;
            Ok(())
        }
    })?;
    let export = loaded
        .export("test:exec-concurrent/guest", "run")
        .expect("concurrent run export should resolve structurally");

    let results = timeout(
        REGRESSION_TIMEOUT,
        loaded.invoke(export, &[], LIMITS, INVOCATION_FUEL),
    )
    .await
    .expect("serialized async imports deadlocked at the barrier")?;

    assert!(matches!(results.as_slice(), [Val::U32(2)]));
    assert_eq!(entries.load(Ordering::SeqCst), 2);
    Ok(())
}
