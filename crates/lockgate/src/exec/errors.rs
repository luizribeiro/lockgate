use std::{error::Error, fmt, time::Duration};

use anyhow::Error as AnyError;
use wasmtime::{Error as WasmtimeError, Trap};

#[derive(Debug)]
pub(crate) enum EnvironmentError {
    RequiredUnset {
        instance_id: String,
        variable: String,
    },
    RequiredNotUnicode {
        instance_id: String,
        variable: String,
    },
}

impl fmt::Display for EnvironmentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RequiredUnset {
                instance_id,
                variable,
            } => write!(
                formatter,
                "plugin instance `{instance_id}` requires host environment variable `{variable}`, but it is unset"
            ),
            Self::RequiredNotUnicode {
                instance_id,
                variable,
            } => write!(
                formatter,
                "plugin instance `{instance_id}` requires host environment variable `{variable}`, but its value is not valid Unicode"
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
    HostImportCallLimitExceeded { limit: u64 },
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
            Self::HostImportCallLimitExceeded { limit } => write!(
                f,
                "component exceeded its per-invocation host-import call limit of {limit}"
            ),
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

#[derive(Clone, Copy, Debug)]
struct HostImportCallLimitExceeded {
    limit: u64,
}

impl fmt::Display for HostImportCallLimitExceeded {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "per-invocation host-import call limit of {} exceeded",
            self.limit
        )
    }
}

impl Error for HostImportCallLimitExceeded {}

pub(super) fn host_import_call_limit_error(limit: u64) -> WasmtimeError {
    WasmtimeError::new(HostImportCallLimitExceeded { limit })
}

#[allow(
    dead_code,
    reason = "capability adapters do not yet produce host-import failures; covered by the direct exec test"
)]
pub(crate) fn host_import_error(error: AnyError) -> WasmtimeError {
    WasmtimeError::new(HostImportMarker(error))
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
    if let Some(exceeded) = error.downcast_ref::<HostImportCallLimitExceeded>() {
        return ExecError::HostImportCallLimitExceeded {
            limit: exceeded.limit,
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
    if let Some(exceeded) = error.downcast_ref::<HostImportCallLimitExceeded>() {
        return ExecError::HostImportCallLimitExceeded {
            limit: exceeded.limit,
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
