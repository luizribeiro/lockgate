mod common;

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use common::exec::{ExecEngine, ExecError, StoreCtx};
use common::{LIMITS, TestState};
use wasmtime::component::{Linker, Val};

const ITERATIONS: u32 = 1_000;
const LOW_FUEL: u64 = 100_000;
const HIGH_FUEL: u64 = 5_000_000;

#[tokio::test(flavor = "current_thread")]
async fn fixed_loop_exhausts_at_a_deterministic_iteration() {
    let engine = ExecEngine::new().unwrap();
    let reported = Arc::new(AtomicU32::new(0));
    let loaded = engine
        .load::<TestState>(&common::EXEC_FUEL_FIXTURE, {
            let reported = Arc::clone(&reported);
            move |linker| wire_report(linker, reported)
        })
        .unwrap();
    let run = loaded
        .export("test:exec-fuel/guest", "run")
        .expect("run export should resolve structurally");

    let mut exhausted_at = [0; 2];
    for iteration in &mut exhausted_at {
        reported.store(0, Ordering::SeqCst);
        let error = loaded
            .invoke(
                run,
                &[Val::U32(ITERATIONS)],
                TestState,
                LIMITS,
                LOW_FUEL,
                common::INVOCATION_DEADLINE,
            )
            .await
            .expect_err("the low fuel allowance should be exhausted");
        assert!(matches!(error, ExecError::OutOfBudget));
        *iteration = reported.load(Ordering::SeqCst);
    }

    assert!(exhausted_at[0] > 0, "the guest should report progress");
    assert_eq!(
        exhausted_at[0], exhausted_at[1],
        "equal fuel must stop the fixed loop at the same reported iteration"
    );

    reported.store(0, Ordering::SeqCst);
    let result = loaded
        .invoke(
            run,
            &[Val::U32(ITERATIONS)],
            TestState,
            LIMITS,
            HIGH_FUEL,
            common::INVOCATION_DEADLINE,
        )
        .await
        .expect("the higher fuel allowance should complete");
    assert!(matches!(result.as_slice(), [Val::U32(_)]));
    assert_eq!(reported.load(Ordering::SeqCst), ITERATIONS);
}

fn wire_report(
    linker: &mut Linker<StoreCtx<TestState>>,
    reported: Arc<AtomicU32>,
) -> wasmtime::Result<()> {
    linker.instance("test:exec-fuel/host")?.func_wrap(
        "report",
        move |_, (iteration, _checksum): (u32, u32)| {
            reported.store(iteration, Ordering::SeqCst);
            Ok(())
        },
    )?;
    Ok(())
}
