//! Host admission, permissions, and execution kernel.

#[allow(
    dead_code,
    reason = "the private execution core is wired in later steps"
)]
mod exec;

#[cfg(test)]
mod tests {
    use super::exec::ExecEngine;

    #[test]
    fn creates_engine_with_pinned_configuration() {
        ExecEngine::new().expect("the pinned Wasmtime configuration should be valid");
    }
}
