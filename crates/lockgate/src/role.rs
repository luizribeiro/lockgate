use std::{error::Error, fmt};

use crate::exec::LoadedComponent;
use crate::lifecycle::RuntimeLimits;

/// A hand-written or generated view of one exported WIT interface.
///
/// Implementations name the interface and wrap the invocation capability in
/// their typed client. Generated clients and readable cast extensions use this
/// same trait in later code-generation steps.
pub trait Role: 'static {
    const INTERFACE: &'static str;

    type Client<'a, S>: 'a
    where
        S: Send + 'static,
        Self: 'a;

    fn client<'a, S>(invocation: RoleInvocation<'a, S>) -> Self::Client<'a, S>
    where
        S: Send + 'static;
}

/// Interface-scoped calling capability supplied to a role client at cast time.
#[allow(
    dead_code,
    reason = "the public invocation method lands in the next role-client commit"
)]
pub struct RoleInvocation<'a, S: Send + 'static> {
    pub(crate) artifact: &'a LoadedComponent<S>,
    pub(crate) limits: RuntimeLimits,
    pub(crate) interface: &'static str,
}

impl<S: Send + 'static> fmt::Debug for RoleInvocation<'_, S> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("RoleInvocation")
            .field("interface", &self.interface)
            .finish_non_exhaustive()
    }
}

impl<'a, S: Send + 'static> RoleInvocation<'a, S> {
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
}

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
