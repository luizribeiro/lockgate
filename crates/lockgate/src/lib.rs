//! Host admission, permissions, and execution kernel.

#[allow(
    dead_code,
    reason = "the private execution core is wired in later steps"
)]
mod exec;
mod inspection;
#[allow(
    dead_code,
    reason = "the private validator is wired into admission in the next lifecycle step"
)]
mod validate;

pub use inspection::{InspectError, Inspection, inspect};

#[cfg(test)]
mod tests {
    use super::exec::ExecEngine;

    #[test]
    fn creates_engine_with_pinned_configuration() {
        ExecEngine::new().expect("the pinned Wasmtime configuration should be valid");
    }
}
