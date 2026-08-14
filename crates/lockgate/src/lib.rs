//! Host admission, permissions, and execution kernel.

#[cfg_attr(
    test,
    allow(
        dead_code,
        reason = "execution test hooks are exercised by the integration-test harness"
    )
)]
mod exec;
mod inspection;
mod lifecycle;
mod role;
mod validate;

pub use inspection::{InspectError, Inspection, inspect};
pub use lifecycle::{
    Acceptance, AdmissionError, BudgetClass, EngineError, Host, HostBuilder, InvocationCtx,
    LimitSet, PluginConfig, PluginHandle, Prepared, RuntimeLimits, SymbolicRoots,
};
pub use role::{CallError, Role, RoleError, RoleInvocation, Value};
pub use validate::ValidationError;

#[cfg(test)]
mod tests {
    use super::exec::ExecEngine;

    #[test]
    fn creates_engine_with_pinned_configuration() {
        ExecEngine::new().expect("the pinned Wasmtime configuration should be valid");
    }
}
