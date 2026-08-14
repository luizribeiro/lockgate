use std::collections::HashMap;
use std::error::Error;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, RwLock};

use tokio::sync::mpsc;
use tokio::task::{Id as TaskId, JoinError, JoinSet};

type JobFuture = Pin<Box<dyn Future<Output = anyhow::Result<()>> + Send + 'static>>;
type ErrorSink = Arc<dyn Fn(DetachedJobFailure) + Send + Sync + 'static>;

/// An opaque identifier for a host-owned detached job.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct JobId(u64);

impl fmt::Display for JobId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// Failure to detach a host-owned job.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum DetachError {
    /// This admitted plugin already owns its maximum number of detached jobs.
    QuotaExceeded {
        plugin_id: String,
        max_detached_jobs: usize,
    },
    /// The owning host has begun shutting down.
    HostShuttingDown,
}

impl fmt::Display for DetachError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::QuotaExceeded {
                plugin_id,
                max_detached_jobs,
            } => write!(
                formatter,
                "plugin `{plugin_id}` reached its detached-job limit of {max_detached_jobs}"
            ),
            Self::HostShuttingDown => formatter.write_str("the Lockgate Host is shutting down"),
        }
    }
}

impl Error for DetachError {}

/// A failed or panicked detached job reported to the application.
#[derive(Debug)]
pub struct DetachedJobFailure {
    plugin_id: String,
    job_id: JobId,
    error: anyhow::Error,
}

impl DetachedJobFailure {
    /// Returns the admitted plugin that detached this job.
    pub fn plugin_id(&self) -> &str {
        &self.plugin_id
    }

    /// Returns the opaque identifier assigned when the job was detached.
    pub fn job_id(&self) -> JobId {
        self.job_id
    }

    /// Returns the job failure or synthesized panic error.
    pub fn error(&self) -> &(dyn Error + Send + Sync + 'static) {
        self.error.as_ref()
    }
}

#[doc(hidden)]
#[derive(Clone)]
pub struct DetachedJobContext {
    tracker: Arc<JobTracker>,
    plugin_id: String,
    max_detached_jobs: usize,
    active: Arc<AtomicUsize>,
}

impl DetachedJobContext {
    pub(crate) fn new(
        tracker: Arc<JobTracker>,
        plugin_id: String,
        max_detached_jobs: usize,
    ) -> Self {
        Self {
            tracker,
            plugin_id,
            max_detached_jobs,
            active: Arc::new(AtomicUsize::new(0)),
        }
    }

    pub(crate) fn detach(&self, future: JobFuture) -> Result<JobId, DetachError> {
        let permit = JobPermit::acquire(
            Arc::clone(&self.active),
            &self.plugin_id,
            self.max_detached_jobs,
        )?;
        self.tracker.submit(self.plugin_id.clone(), permit, future)
    }
}

struct JobPermit(Arc<AtomicUsize>);

impl JobPermit {
    fn acquire(
        active: Arc<AtomicUsize>,
        plugin_id: &str,
        limit: usize,
    ) -> Result<Self, DetachError> {
        let reserved = active.try_update(Ordering::AcqRel, Ordering::Acquire, |current| {
            (current < limit).then_some(current + 1)
        });
        match reserved {
            Ok(_) => Ok(Self(active)),
            Err(_) => Err(DetachError::QuotaExceeded {
                plugin_id: plugin_id.to_owned(),
                max_detached_jobs: limit,
            }),
        }
    }
}

impl Drop for JobPermit {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

pub(crate) struct JobTracker {
    sender: mpsc::UnboundedSender<JobCommand>,
    worker: Mutex<Option<std::thread::JoinHandle<()>>>,
    sink: Arc<RwLock<ErrorSink>>,
    next_id: AtomicU64,
    shutting_down: AtomicBool,
}

impl JobTracker {
    pub(crate) fn new() -> std::io::Result<Arc<Self>> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        let (sender, receiver) = mpsc::unbounded_channel();
        // Lockgate has no logging dependency, so the documented default sink
        // deliberately observes and drops failures without another side effect.
        let sink: Arc<RwLock<ErrorSink>> = Arc::new(RwLock::new(Arc::new(|_| {})));
        let worker_sink = Arc::clone(&sink);
        let worker = std::thread::Builder::new()
            .name("lockgate-detached-jobs".into())
            .spawn(move || runtime.block_on(supervise(receiver, worker_sink)))?;

        Ok(Arc::new(Self {
            sender,
            worker: Mutex::new(Some(worker)),
            sink,
            next_id: AtomicU64::new(1),
            shutting_down: AtomicBool::new(false),
        }))
    }

    pub(crate) fn set_error_sink(&self, sink: impl Fn(DetachedJobFailure) + Send + Sync + 'static) {
        *self.sink.write().expect("detached-job sink lock poisoned") = Arc::new(sink);
    }

    fn submit(
        &self,
        plugin_id: String,
        permit: JobPermit,
        future: JobFuture,
    ) -> Result<JobId, DetachError> {
        if self.shutting_down.load(Ordering::Acquire) {
            return Err(DetachError::HostShuttingDown);
        }
        let job_id = JobId(self.next_id.fetch_add(1, Ordering::Relaxed));
        self.sender
            .send(JobCommand::Start {
                metadata: JobMetadata { plugin_id, job_id },
                permit,
                future,
            })
            .map_err(|_| DetachError::HostShuttingDown)?;
        Ok(job_id)
    }

    pub(crate) fn shutdown(&self) {
        if self.shutting_down.swap(true, Ordering::AcqRel) {
            return;
        }
        let _ = self.sender.send(JobCommand::Shutdown);
        if let Some(worker) = self
            .worker
            .lock()
            .expect("detached-job worker lock poisoned")
            .take()
        {
            let _ = worker.join();
        }
    }
}

impl Drop for JobTracker {
    fn drop(&mut self) {
        self.shutdown();
    }
}

enum JobCommand {
    Start {
        metadata: JobMetadata,
        permit: JobPermit,
        future: JobFuture,
    },
    Shutdown,
}

struct JobMetadata {
    plugin_id: String,
    job_id: JobId,
}

async fn supervise(
    mut receiver: mpsc::UnboundedReceiver<JobCommand>,
    sink: Arc<RwLock<ErrorSink>>,
) {
    let mut jobs = JoinSet::new();
    let mut metadata = HashMap::new();
    loop {
        tokio::select! {
            command = receiver.recv() => match command {
                Some(JobCommand::Start { metadata: job, permit, future }) => {
                    spawn_job(&mut jobs, &mut metadata, job, permit, future);
                }
                Some(JobCommand::Shutdown) => {
                    receiver.close();
                    while let Some(command) = receiver.recv().await {
                        if let JobCommand::Start { metadata: job, permit, future } = command {
                            spawn_job(&mut jobs, &mut metadata, job, permit, future);
                        }
                    }
                    break;
                }
                None => break,
            },
            completion = jobs.join_next_with_id(), if !jobs.is_empty() => {
                if let Some(completion) = completion {
                    report_completion(completion, &mut metadata, &sink);
                }
            }
        }
    }

    jobs.abort_all();
    while let Some(completion) = jobs.join_next_with_id().await {
        report_completion(completion, &mut metadata, &sink);
    }
}

fn spawn_job(
    jobs: &mut JoinSet<anyhow::Result<()>>,
    metadata: &mut HashMap<TaskId, JobMetadata>,
    job: JobMetadata,
    permit: JobPermit,
    future: JobFuture,
) {
    let task = jobs.spawn(async move {
        let _permit = permit;
        future.await
    });
    metadata.insert(task.id(), job);
}

fn report_completion(
    completion: Result<(TaskId, anyhow::Result<()>), JoinError>,
    metadata: &mut HashMap<TaskId, JobMetadata>,
    sink: &RwLock<ErrorSink>,
) {
    let (task_id, error) = match completion {
        Ok((task_id, Ok(()))) => {
            metadata.remove(&task_id);
            return;
        }
        Ok((task_id, Err(error))) => (task_id, error),
        Err(error) if error.is_cancelled() => {
            metadata.remove(&error.id());
            return;
        }
        Err(error) => {
            let task_id = error.id();
            (task_id, anyhow::anyhow!("detached job panicked: {error}"))
        }
    };
    let Some(job) = metadata.remove(&task_id) else {
        return;
    };
    let report = DetachedJobFailure {
        plugin_id: job.plugin_id,
        job_id: job.job_id,
        error,
    };
    let sink = Arc::clone(&sink.read().expect("detached-job sink lock poisoned"));
    let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| sink(report)));
}

#[cfg(test)]
pub(crate) async fn assert_queued_start_after_shutdown_is_aborted() {
    use std::task::{Context, Poll};

    struct PendingDrop(Arc<AtomicBool>);

    impl Future for PendingDrop {
        type Output = anyhow::Result<()>;

        fn poll(self: Pin<&mut Self>, _cx: &mut Context<'_>) -> Poll<Self::Output> {
            Poll::Pending
        }
    }

    impl Drop for PendingDrop {
        fn drop(&mut self) {
            self.0.store(true, Ordering::SeqCst);
        }
    }

    let (sender, receiver) = mpsc::unbounded_channel();
    let sink_calls = Arc::new(AtomicUsize::new(0));
    let sink: Arc<RwLock<ErrorSink>> = Arc::new(RwLock::new({
        let sink_calls = Arc::clone(&sink_calls);
        Arc::new(move |_| {
            sink_calls.fetch_add(1, Ordering::SeqCst);
        })
    }));
    let active = Arc::new(AtomicUsize::new(0));
    let permit = JobPermit::acquire(Arc::clone(&active), "race", 1).unwrap();
    let dropped = Arc::new(AtomicBool::new(false));

    sender.send(JobCommand::Shutdown).unwrap();
    sender
        .send(JobCommand::Start {
            metadata: JobMetadata {
                plugin_id: "race".into(),
                job_id: JobId(1),
            },
            permit,
            future: Box::pin(PendingDrop(Arc::clone(&dropped))),
        })
        .unwrap();
    drop(sender);

    supervise(receiver, sink).await;

    assert!(dropped.load(Ordering::SeqCst));
    assert_eq!(active.load(Ordering::SeqCst), 0);
    assert_eq!(sink_calls.load(Ordering::SeqCst), 0);
}
