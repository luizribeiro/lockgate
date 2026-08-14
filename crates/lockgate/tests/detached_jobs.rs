mod common;

use std::future::{Future, pending};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use lockgate::{
    Acceptance, CallError, DetachError, DetachedJobFailure, Host, HostBuilder, HostCtx,
    InvocationCtx, PluginConfig, PluginHandle, Role, RoleInvocation, RuntimeLimits, Value,
};
use tokio::sync::Semaphore;

const SUCCESS: u8 = 0;
const FAILURE: u8 = 1;
const NEVER: u8 = 2;
const PANIC: u8 = 3;
const TEST_TIMEOUT: Duration = Duration::from_secs(10);

type JobFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send>>;

#[derive(Clone)]
struct Imports {
    control: Control,
}

#[derive(Clone)]
struct Control {
    started: Arc<Semaphore>,
    release: Arc<Semaphore>,
    completed: Arc<Semaphore>,
    aborted: Arc<AtomicBool>,
    quota_errors: Arc<AtomicUsize>,
}

impl Control {
    fn new() -> Self {
        Self {
            started: Arc::new(Semaphore::new(0)),
            release: Arc::new(Semaphore::new(0)),
            completed: Arc::new(Semaphore::new(0)),
            aborted: Arc::new(AtomicBool::new(false)),
            quota_errors: Arc::new(AtomicUsize::new(0)),
        }
    }

    async fn wait_started(&self) {
        wait_for(&self.started, "detached job did not start").await;
    }

    async fn wait_completed(&self) {
        wait_for(&self.completed, "detached job did not complete").await;
    }

    fn release_one(&self) {
        self.release.add_permits(1);
    }
}

lockgate::host_bindings!({
    path: "tests/data/detached_jobs",
    world: "fixture",
    imports: Imports,
    data: (),
});

impl application::Host for Imports {
    async fn start(&mut self, cx: HostCtx<'_, ()>, kind: u8) -> Result<String, String> {
        let future = job(kind, self.control.clone());
        match cx.detach(future) {
            Ok(job_id) => Ok(job_id.to_string()),
            Err(error @ DetachError::QuotaExceeded { .. }) => {
                self.control.quota_errors.fetch_add(1, Ordering::SeqCst);
                Err(error.to_string())
            }
            Err(error) => Err(error.to_string()),
        }
    }
}

fn job(kind: u8, control: Control) -> JobFuture {
    Box::pin(async move {
        control.started.add_permits(1);
        match kind {
            SUCCESS => {
                take_permit(&control.release).await;
                control.completed.add_permits(1);
                Ok(())
            }
            FAILURE => {
                take_permit(&control.release).await;
                Err(anyhow::anyhow!("detached failure"))
            }
            NEVER => {
                let _drop_marker = DropMarker(Arc::clone(&control.aborted));
                pending::<()>().await;
                Ok(())
            }
            PANIC => panic!("detached panic"),
            _ => Err(anyhow::anyhow!("unknown detached job kind")),
        }
    })
}

struct DropMarker(Arc<AtomicBool>);

impl Drop for DropMarker {
    fn drop(&mut self) {
        self.0.store(true, Ordering::SeqCst);
    }
}

struct DetachedRole;

struct DetachedClient<'a, S: Send + Sync + 'static>(RoleInvocation<'a, S>);

impl Role for DetachedRole {
    const INTERFACE: &'static str = "test:detached-jobs/guest";

    type Client<'a, S>
        = DetachedClient<'a, S>
    where
        S: Send + Sync + 'static;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + Sync + 'static,
    {
        DetachedClient(invocation)
    }
}

impl DetachedClient<'_, ()> {
    async fn start(&self, kind: u8) -> Result<String, CallError> {
        let values = self
            .0
            .invoke(
                "start",
                &[Value::U8(kind)],
                InvocationCtx::bounded(1_000_000),
            )
            .await?;
        match values.as_slice() {
            [Value::String(value)] => Ok(value.clone()),
            _ => Err(CallError::shape("expected one string result")),
        }
    }
}

async fn host(
    control: Control,
    failures: Arc<Mutex<Vec<DetachedJobFailure>>>,
) -> (Host<()>, PluginHandle) {
    let mut builder = HostBuilder::new(Imports { control }).unwrap();
    builder.on_detached_job_error(move |failure| failures.lock().unwrap().push(failure));
    let prepared = builder
        .prepare(
            "detached-jobs",
            &common::DETACHED_JOBS_FIXTURE,
            PluginConfig::default(),
        )
        .await
        .unwrap();
    let plugin = builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits {
                max_detached_jobs: 1,
                ..RuntimeLimits::default()
            },
            InvocationCtx::bounded(1_000_000),
        )
        .await
        .unwrap();
    (builder.finish(), plugin)
}

#[tokio::test]
async fn detached_job_quota_recovers_after_success() {
    let control = Control::new();
    let failures = Arc::new(Mutex::new(Vec::new()));
    let (host, plugin) = host(control.clone(), failures).await;
    let client = host.client::<DetachedRole>(&plugin).unwrap();

    let first = client.start(SUCCESS).await.unwrap();
    assert!(!first.starts_with("error:"));
    control.wait_started().await;
    assert!(client.start(SUCCESS).await.unwrap().starts_with("error:"));
    control.release_one();
    control.wait_completed().await;

    let recovered = tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            let result = client.start(NEVER).await.unwrap();
            if !result.starts_with("error:") {
                break result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached-job capacity did not return after success");
    assert!(!recovered.starts_with("error:"));
    control.wait_started().await;
    assert!(control.quota_errors.load(Ordering::SeqCst) >= 1);
}

#[tokio::test]
async fn failed_job_reports_attribution_and_releases_capacity() {
    let control = Control::new();
    let failures = Arc::new(Mutex::new(Vec::new()));
    let (host, plugin) = host(control.clone(), Arc::clone(&failures)).await;
    let client = host.client::<DetachedRole>(&plugin).unwrap();

    let job_id = client.start(FAILURE).await.unwrap();
    control.wait_started().await;
    assert!(client.start(NEVER).await.unwrap().starts_with("error:"));
    control.release_one();
    wait_for_failure(&failures).await;

    {
        let reports = failures.lock().unwrap();
        assert_eq!(reports.len(), 1);
        assert_eq!(reports[0].plugin_id(), "detached-jobs");
        assert_eq!(reports[0].job_id().to_string(), job_id);
        assert_eq!(reports[0].error().to_string(), "detached failure");
    }

    assert!(!client.start(NEVER).await.unwrap().starts_with("error:"));
    control.wait_started().await;
}

#[tokio::test]
async fn panicking_job_is_reported_to_the_error_sink() {
    let control = Control::new();
    let failures = Arc::new(Mutex::new(Vec::new()));
    let (host, plugin) = host(control.clone(), Arc::clone(&failures)).await;
    let client = host.client::<DetachedRole>(&plugin).unwrap();

    let job_id = client.start(PANIC).await.unwrap();
    control.wait_started().await;
    wait_for_failure(&failures).await;

    let reports = failures.lock().unwrap();
    assert_eq!(reports[0].plugin_id(), "detached-jobs");
    assert_eq!(reports[0].job_id().to_string(), job_id);
    assert!(reports[0].error().to_string().contains("panicked"));
}

#[tokio::test]
async fn host_drop_aborts_and_awaits_detached_jobs() {
    let control = Control::new();
    let failures = Arc::new(Mutex::new(Vec::new()));
    let (host, plugin) = host(control.clone(), failures).await;
    {
        let client = host.client::<DetachedRole>(&plugin).unwrap();
        assert!(!client.start(NEVER).await.unwrap().starts_with("error:"));
        control.wait_started().await;
    }

    tokio::time::timeout(
        TEST_TIMEOUT,
        tokio::task::spawn_blocking(move || drop(host)),
    )
    .await
    .expect("Host drop hung while awaiting an aborted detached job")
    .unwrap();
    assert!(control.aborted.load(Ordering::SeqCst));
}

async fn wait_for(semaphore: &Semaphore, message: &str) {
    let permit = tokio::time::timeout(TEST_TIMEOUT, semaphore.acquire())
        .await
        .expect(message)
        .unwrap();
    permit.forget();
}

async fn take_permit(semaphore: &Semaphore) {
    semaphore.acquire().await.unwrap().forget();
}

async fn wait_for_failure(failures: &Mutex<Vec<DetachedJobFailure>>) {
    tokio::time::timeout(TEST_TIMEOUT, async {
        loop {
            if !failures.lock().unwrap().is_empty() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("detached-job error sink was not called");
}
