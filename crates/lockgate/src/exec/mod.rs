use anyhow::Result;
use wasmtime::{Config, Engine};

pub(crate) struct ExecEngine {
    engine: Engine,
}

impl ExecEngine {
    pub(crate) fn new() -> Result<Self> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .concurrency_support(true)
            .consume_fuel(true)
            .epoch_interruption(true);

        Ok(Self {
            engine: Engine::new(&config)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ExecEngine;

    #[test]
    fn creates_engine_with_pinned_configuration() {
        ExecEngine::new().expect("the pinned Wasmtime configuration should be valid");
    }
}
