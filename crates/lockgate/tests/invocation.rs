mod common;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::task::{Context, Poll};

use common::exec::{ExecEngine, StoreCtx};
use common::{INVOCATION_FUEL, LIMITS, TestState, wire_ready_wait};
use tokio::sync::Notify;
use wasmtime::Result;
use wasmtime::component::{Linker, Val};

#[tokio::test(flavor = "current_thread")]
async fn value_export_returns_expected_value() -> Result<()> {
    let engine = ExecEngine::new()?;
    let loaded = engine.load::<TestState>(&common::EXEC_FIXTURE, wire_ready_wait)?;
    let export = loaded
        .export("test:exec/guest", "value")
        .expect("value export should resolve structurally");

    let results = loaded
        .invoke(
            export,
            &[],
            TestState,
            LIMITS,
            INVOCATION_FUEL,
            common::INVOCATION_DEADLINE,
        )
        .await?;

    assert!(matches!(results.as_slice(), [Val::U32(42)]));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn smoke_probe_drops_its_store() -> Result<()> {
    let engine = ExecEngine::new()?;
    let loaded = engine.load::<TestState>(&common::EXEC_FIXTURE, wire_ready_wait)?;
    let dropped = Arc::new(AtomicBool::new(false));

    loaded
        .smoke_observing_drop(
            TestState,
            LIMITS,
            INVOCATION_FUEL,
            common::INVOCATION_DEADLINE,
            Arc::clone(&dropped),
        )
        .await?;

    assert!(dropped.load(Ordering::SeqCst));
    Ok(())
}

#[tokio::test(flavor = "current_thread")]
async fn dropping_invocation_drops_store_and_stops_guest() -> Result<()> {
    let engine = ExecEngine::new()?;
    let entered = Arc::new(Notify::new());
    let calls = Arc::new(AtomicUsize::new(0));
    let import_dropped = Arc::new(AtomicBool::new(false));
    let store_dropped = Arc::new(AtomicBool::new(false));

    let loaded = engine.load::<TestState>(&common::EXEC_FIXTURE, {
        let entered = Arc::clone(&entered);
        let calls = Arc::clone(&calls);
        let import_dropped = Arc::clone(&import_dropped);
        let store_dropped = Arc::clone(&store_dropped);
        move |linker| wire_cancellable_wait(linker, entered, calls, import_dropped, store_dropped)
    })?;
    let export = loaded
        .export("test:exec/guest", "suspend")
        .expect("suspend export should resolve structurally");

    let mut invocation = Box::pin(loaded.invoke(
        export,
        &[],
        TestState,
        LIMITS,
        INVOCATION_FUEL,
        common::INVOCATION_DEADLINE,
    ));
    tokio::select! {
        result = &mut invocation => panic!("invocation completed before cancellation: {result:?}"),
        () = entered.notified() => {}
    }

    assert!(!store_dropped.load(Ordering::SeqCst));
    assert!(!import_dropped.load(Ordering::SeqCst));
    drop(invocation);

    assert!(store_dropped.load(Ordering::SeqCst));
    assert!(import_dropped.load(Ordering::SeqCst));
    tokio::task::yield_now().await;
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let results = loaded
        .invoke(
            export,
            &[],
            TestState,
            LIMITS,
            INVOCATION_FUEL,
            common::INVOCATION_DEADLINE,
        )
        .await?;
    assert!(matches!(results.as_slice(), [Val::U32(7)]));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
    Ok(())
}

fn wire_cancellable_wait(
    linker: &mut Linker<StoreCtx<TestState>>,
    entered: Arc<Notify>,
    calls: Arc<AtomicUsize>,
    import_dropped: Arc<AtomicBool>,
    store_dropped: Arc<AtomicBool>,
) -> Result<()> {
    linker
        .instance("test:exec/host")?
        .func_wrap_concurrent("wait", move |accessor, (): ()| {
            if calls.fetch_add(1, Ordering::SeqCst) == 0 {
                accessor.with(|mut access| {
                    access.get().observe_drop(Arc::clone(&store_dropped));
                });
                entered.notify_one();
                Box::pin(DropTrackedPending {
                    dropped: Arc::clone(&import_dropped),
                }) as Pin<Box<dyn Future<Output = Result<()>> + Send>>
            } else {
                Box::pin(async { Ok(()) })
            }
        })?;
    Ok(())
}

struct DropTrackedPending {
    dropped: Arc<AtomicBool>,
}

impl Future for DropTrackedPending {
    type Output = Result<()>;

    fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
        Poll::Pending
    }
}

impl Drop for DropTrackedPending {
    fn drop(&mut self) {
        self.dropped.store(true, Ordering::SeqCst);
    }
}
