mod common;

use std::sync::Arc;
use std::time::Duration;

use common::exec::{ExecEngine, StoreCtx};
use common::{INVOCATION_FUEL, LIMITS, TestState, wire_ready_wait};
use tokio::sync::Barrier;
use wasmtime::component::{Linker, Val};

#[tokio::test(flavor = "current_thread")]
async fn same_loaded_component_accepts_overlapping_invocations() {
    let engine = ExecEngine::new_pooling().unwrap();
    let barrier = Arc::new(Barrier::new(2));
    let loaded = engine
        .load::<TestState>(&common::EXEC_FIXTURE, {
            let barrier = Arc::clone(&barrier);
            move |linker| wire_barrier_wait(linker, barrier)
        })
        .unwrap();
    let suspend = loaded
        .export("test:exec/guest", "suspend")
        .expect("suspend export should resolve structurally");

    let pair = tokio::time::timeout(Duration::from_secs(5), async {
        tokio::join!(
            loaded.invoke(
                suspend,
                &[],
                TestState,
                LIMITS,
                INVOCATION_FUEL,
                common::INVOCATION_DEADLINE
            ),
            loaded.invoke(
                suspend,
                &[],
                TestState,
                LIMITS,
                INVOCATION_FUEL,
                common::INVOCATION_DEADLINE
            ),
        )
    })
    .await
    .expect("invocations did not overlap at the host barrier");

    for result in [pair.0.unwrap(), pair.1.unwrap()] {
        assert!(matches!(result.as_slice(), [Val::U32(7)]));
    }
}

#[tokio::test(flavor = "current_thread")]
async fn every_root_invocation_gets_fresh_guest_globals() {
    let engine = ExecEngine::new_pooling().unwrap();
    let loaded = engine
        .load::<TestState>(&common::EXEC_FIXTURE, wire_ready_wait)
        .unwrap();
    let pin = loaded
        .export("test:exec/guest", "pin")
        .expect("pin export should resolve structurally");

    for _ in 0..2 {
        let result = loaded
            .invoke(
                pin,
                &[],
                TestState,
                LIMITS,
                INVOCATION_FUEL,
                common::INVOCATION_DEADLINE,
            )
            .await
            .unwrap();
        assert!(matches!(result.as_slice(), [Val::U32(1)]));
    }
}

fn wire_barrier_wait(
    linker: &mut Linker<StoreCtx<TestState>>,
    barrier: Arc<Barrier>,
) -> wasmtime::Result<()> {
    linker
        .instance("test:exec/host")?
        .func_wrap_concurrent("wait", move |_, (): ()| {
            let barrier = Arc::clone(&barrier);
            Box::pin(async move {
                barrier.wait().await;
                Ok(())
            })
        })?;
    Ok(())
}
