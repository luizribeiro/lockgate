use std::{
    any::Any,
    error::Error,
    fmt,
    future::{Future, poll_fn},
    panic::{AssertUnwindSafe, catch_unwind},
    sync::{Arc, Mutex},
    task::Poll,
    time::Duration,
};

use anyhow::Error as AnyError;
use lockgate_policy::PluginId;
use wasmtime::{Error as WasmtimeError, Result as WasmtimeResult, Trap};

#[derive(Debug)]
pub(crate) enum EnvironmentError {
    RequiredUnset {
        plugin_id: PluginId,
        variable: String,
    },
    RequiredNotUnicode {
        plugin_id: PluginId,
        variable: String,
    },
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequiredUnset {
                plugin_id,
                variable,
            } => write!(
                formatter,
                "plugin instance `{plugin_id}` requires host environment variable `{variable}`, but it is unset"
            ),
            Self::RequiredNotUnicode {
                plugin_id,
                variable,
            } => write!(
                formatter,
                "plugin instance `{plugin_id}` requires host environment variable `{variable}`, but its value is not valid Unicode"
            ),
        }
    }
}

impl Error for EnvironmentError {}

#[derive(Debug)]
pub(crate) enum LoadError {
    Compile(AnyError),
    Link(AnyError),
}

impl LoadError {
    pub(super) fn compile(error: WasmtimeError) -> Self {
        Self::Compile(error.into())
    }

    pub(super) fn link(error: WasmtimeError) -> Self {
        Self::Link(error.into())
    }
}

impl fmt::Display for LoadError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Compile(error) => write!(f, "component compilation failed: {error}"),
            Self::Link(error) => write!(f, "component linking failed: {error}"),
        }
    }
}

impl Error for LoadError {}

#[derive(Debug)]
pub(crate) enum ExecError {
    Environment(EnvironmentError),
    Instantiate(AnyError),
    Trap(TrapDetail),
    OutOfBudget,
    DeadlineExceeded(Duration),
    HostPanic { import: String, message: String },
    HostImport(AnyError),
    Dispatch(AnyError),
}

impl fmt::Display for ExecError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Environment(error) => error.fmt(f),
            Self::Instantiate(error) => write!(f, "component instantiation failed: {error}"),
            Self::Trap(detail) => write!(f, "component trapped: {detail}"),
            Self::OutOfBudget => f.write_str("component exhausted its invocation fuel"),
            Self::DeadlineExceeded(_) => f.write_str("component exceeded its invocation deadline"),
            Self::HostPanic { import, message } => {
                write!(f, "host import `{import}` panicked: {message}")
            }
            Self::HostImport(error) => write!(f, "host import failed: {error}"),
            Self::Dispatch(error) => write!(f, "component dispatch failed: {error}"),
        }
    }
}

impl Error for ExecError {}

#[derive(Debug)]
pub(crate) enum TrapDetail {
    Wasm {
        trap: Trap,
        #[allow(dead_code, reason = "retained as the causal error for diagnostics")]
        error: AnyError,
    },
    MemoryLimit(MemoryLimitExceeded),
}

impl fmt::Display for TrapDetail {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Wasm { trap, .. } => write!(f, "{trap}"),
            Self::MemoryLimit(error) => error.fmt(f),
        }
    }
}

#[derive(Debug)]
struct HostImportMarker(AnyError);

impl fmt::Display for HostImportMarker {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(f)
    }
}

impl Error for HostImportMarker {}

#[derive(Clone, Debug)]
pub(crate) struct HostPanic {
    pub(crate) import: String,
    pub(crate) message: String,
}

#[derive(Clone, Default)]
pub(crate) struct HostPanicState(Arc<Mutex<Option<HostPanic>>>);

impl HostPanicState {
    pub(crate) fn record(&self, import: impl Into<String>, payload: Box<dyn Any + Send>) -> String {
        let panic = HostPanic {
            import: import.into(),
            message: panic_payload_message(payload),
        };
        let message = panic.message.clone();
        let mut slot = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if slot.is_none() {
            *slot = Some(panic);
        }
        message
    }

    pub(crate) fn take(&self) -> Option<HostPanic> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
    }
}

#[derive(Debug)]
struct HostPanicMarker(HostPanic);

impl fmt::Display for HostPanicMarker {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "host import `{}` panicked: {}",
            self.0.import, self.0.message
        )
    }
}

impl Error for HostPanicMarker {}

#[allow(
    dead_code,
    reason = "capability adapters do not yet produce host-import failures; covered by the direct exec test"
)]
pub(crate) fn host_import_error(error: AnyError) -> WasmtimeError {
    WasmtimeError::new(HostImportMarker(error))
}

pub(super) fn host_panic_error(
    import: impl Into<String>,
    payload: Box<dyn Any + Send>,
) -> WasmtimeError {
    WasmtimeError::new(HostPanicMarker(HostPanic {
        import: import.into(),
        message: panic_payload_message(payload),
    }))
}

pub(crate) async fn catch_unwind_future<F>(future: F) -> Result<F::Output, Box<dyn Any + Send>>
where
    F: Future,
{
    let mut future = Box::pin(future);
    poll_fn(
        move |cx| match catch_unwind(AssertUnwindSafe(|| future.as_mut().poll(cx))) {
            Ok(Poll::Ready(output)) => Poll::Ready(Ok(output)),
            Ok(Poll::Pending) => Poll::Pending,
            Err(payload) => Poll::Ready(Err(payload)),
        },
    )
    .await
}

#[doc(hidden)]
pub async fn catch_host_panic<M, F>(
    import: &'static str,
    make_future: M,
) -> WasmtimeResult<F::Output>
where
    M: FnOnce() -> F,
    F: Future,
{
    let future = catch_unwind(AssertUnwindSafe(make_future))
        .map_err(|payload| host_panic_error(import, payload))?;
    catch_unwind_future(future)
        .await
        .map_err(|payload| host_panic_error(import, payload))
}

fn panic_payload_message(payload: Box<dyn Any + Send>) -> String {
    let payload = match payload.downcast::<String>() {
        Ok(message) => return *message,
        Err(payload) => payload,
    };
    match payload.downcast::<&'static str>() {
        Ok(message) => (*message).to_owned(),
        Err(_) => "non-string panic payload".to_owned(),
    }
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct MemoryLimitExceeded {
    pub(super) current: usize,
    pub(super) desired: usize,
    pub(super) limit: usize,
}

impl fmt::Display for MemoryLimitExceeded {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "linear memory growth from {} to {} bytes exceeds the {}-byte limit",
            self.current, self.desired, self.limit
        )
    }
}

impl Error for MemoryLimitExceeded {}

pub(super) fn map_instantiate_error(error: WasmtimeError) -> ExecError {
    if error.is::<HostPanicMarker>() {
        let marker = error
            .downcast::<HostPanicMarker>()
            .expect("host-panic marker type was checked before downcast");
        return ExecError::HostPanic {
            import: marker.0.import,
            message: marker.0.message,
        };
    }
    if error.is::<MemoryLimitExceeded>() {
        return ExecError::Trap(memory_limit_detail(error));
    }
    if error.downcast_ref::<Trap>() == Some(&Trap::OutOfFuel) {
        return ExecError::OutOfBudget;
    }
    ExecError::Instantiate(error.into())
}

pub(super) fn map_dispatch_error(error: WasmtimeError) -> ExecError {
    ExecError::Dispatch(error.into())
}

pub(super) fn map_call_error(error: WasmtimeError) -> ExecError {
    if error.is::<HostPanicMarker>() {
        let marker = error
            .downcast::<HostPanicMarker>()
            .expect("host-panic marker type was checked before downcast");
        return ExecError::HostPanic {
            import: marker.0.import,
            message: marker.0.message,
        };
    }
    if error.is::<HostImportMarker>() {
        let marker = error
            .downcast::<HostImportMarker>()
            .expect("host-import marker type was checked before downcast");
        return ExecError::HostImport(marker.0);
    }
    if error.is::<MemoryLimitExceeded>() {
        return ExecError::Trap(memory_limit_detail(error));
    }
    if let Some(trap) = error.downcast_ref::<Trap>().copied() {
        if trap == Trap::OutOfFuel {
            return ExecError::OutOfBudget;
        }
        return ExecError::Trap(TrapDetail::Wasm {
            trap,
            error: error.into(),
        });
    }
    ExecError::Dispatch(error.into())
}

fn memory_limit_detail(error: WasmtimeError) -> TrapDetail {
    TrapDetail::MemoryLimit(
        error
            .downcast::<MemoryLimitExceeded>()
            .expect("memory-limit marker type was checked before downcast"),
    )
}
