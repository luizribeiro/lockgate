use std::{error::Error, fmt, time::Duration};

use wasmtime::component::Val;

use crate::CallContext;
use crate::exec::{ExecError, LoadedComponent};
use crate::lifecycle::{BudgetClass, InvocationCtx, RuntimeLimits};

/// A hand-written or generated view of one exported WIT interface.
///
/// Implementations name the interface and wrap the invocation capability in
/// their typed client. Generated clients and readable cast extensions use this
/// same trait.
pub trait Role: 'static {
    const INTERFACE: &'static str;

    type Client<'a, S>: 'a
    where
        S: CallContext,
        Self: 'a;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: CallContext;
}

/// Interface-scoped calling capability supplied to a role client at cast time.
pub struct RoleInvocation<'a, S: CallContext> {
    pub(crate) artifact: &'a LoadedComponent<S>,
    pub(crate) limits: RuntimeLimits,
    pub(crate) interface: &'static str,
}

impl<S: CallContext> fmt::Debug for RoleInvocation<'_, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoleInvocation")
            .field("interface", &self.interface)
            .finish_non_exhaustive()
    }
}

impl<'a, S: CallContext> RoleInvocation<'a, S> {
    pub(crate) fn new(
        artifact: &'a LoadedComponent<S>,
        limits: RuntimeLimits,
        interface: &'static str,
    ) -> Self {
        Self {
            artifact,
            limits,
            interface,
        }
    }

    /// Invokes one function in this role's interface using a fresh bounded Store.
    ///
    /// This is the narrow dynamic-value boundary for hand-written and generated
    /// role clients. Role clients lower their typed arguments to [`Value`]s and
    /// lift the returned values before exposing a typed method to applications.
    pub async fn invoke(
        &self,
        function: &str,
        arguments: &[Value],
        ctx: InvocationCtx<S>,
    ) -> Result<Vec<Value>, CallError> {
        let export = self
            .artifact
            .export(self.interface, function)
            .ok_or_else(|| CallError::Dispatch {
                message: format!(
                    "interface `{}` has no function `{function}`",
                    self.interface
                ),
            })?;
        let BudgetClass::Bounded { fuel, deadline } = ctx.budget;
        self.artifact
            .invoke(
                export,
                arguments,
                ctx.data,
                self.limits.into(),
                fuel,
                deadline,
            )
            .await
            .map_err(|error| CallError::from_exec(error, fuel))
    }
}

/// A dynamically lowered or lifted WIT value used inside role clients.
pub type Value = Val;

/// Failure of one plugin invocation.
#[derive(Debug)]
#[non_exhaustive]
pub enum CallError {
    /// A fresh plugin instance could not be created for the call.
    Instantiate { message: String },
    /// Guest execution trapped.
    Trap { detail: String },
    /// The invocation exhausted its bounded fuel allowance.
    OutOfBudget { fuel: u64 },
    /// The invocation exceeded its bounded wall-clock allowance.
    DeadlineExceeded { deadline: Duration },
    /// The invocation exceeded its per-invocation host-import call limit.
    HostImportCallLimitExceeded { limit: u64 },
    /// Dynamic function lookup, argument lowering, or result lifting failed.
    Dispatch { message: String },
}

impl CallError {
    /// Reports a mismatch while a role client lowers arguments or lifts results.
    pub fn shape(message: impl Into<String>) -> Self {
        Self::Dispatch {
            message: message.into(),
        }
    }

    fn from_exec(error: ExecError, fuel: u64) -> Self {
        match error {
            ExecError::Environment(error) => Self::Instantiate {
                message: error.to_string(),
            },
            ExecError::Instantiate(error) => Self::Instantiate {
                message: error.to_string(),
            },
            ExecError::Trap(detail) => Self::Trap {
                detail: detail.to_string(),
            },
            ExecError::OutOfBudget => Self::OutOfBudget { fuel },
            ExecError::DeadlineExceeded(deadline) => Self::DeadlineExceeded { deadline },
            ExecError::HostImportCallLimitExceeded { limit } => {
                Self::HostImportCallLimitExceeded { limit }
            }
            // Generated application imports have no outer failure channel yet:
            // WIT `result` values are guest data, while only future fallible
            // capability adapters can create the internal HostImport marker.
            // A public host-import variant belongs here once capability adapters
            // can actually produce host-import failures.
            ExecError::HostImport(error) | ExecError::Dispatch(error) => Self::Dispatch {
                message: error.to_string(),
            },
        }
    }
}

impl fmt::Display for CallError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Instantiate { message } => {
                write!(formatter, "plugin instance could not be created: {message}")
            }
            Self::Trap { detail } => write!(formatter, "plugin trapped: {detail}"),
            Self::OutOfBudget { fuel } => write!(
                formatter,
                "plugin exhausted its bounded call budget of {fuel} fuel units"
            ),
            Self::DeadlineExceeded { deadline } => write!(
                formatter,
                "plugin exceeded its bounded call deadline of {deadline:?}"
            ),
            Self::HostImportCallLimitExceeded { limit } => write!(
                formatter,
                "plugin exceeded its per-invocation host-import call limit of {limit}"
            ),
            Self::Dispatch { message } => {
                write!(formatter, "plugin call could not be dispatched: {message}")
            }
        }
    }
}

impl Error for CallError {}

/// Failure to cast an admitted plugin handle to a role.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum RoleError {
    RoleNotExported { interface: &'static str },
    WrongHost,
}

impl fmt::Display for RoleError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::RoleNotExported { interface } => {
                write!(
                    formatter,
                    "plugin does not export role interface `{interface}`"
                )
            }
            Self::WrongHost => {
                formatter.write_str("plugin handle belongs to a different Lockgate Host")
            }
        }
    }
}

impl Error for RoleError {}
