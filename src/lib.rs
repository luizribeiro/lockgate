//! Capability-scoped WebAssembly component runtime.
//! The initial public surface is minimal while the prototype is extracted into a library.

use anyhow::Result;

mod demo;
mod manifest;
mod plugin;
mod runtime;

wasmtime::component::bindgen!({ path: "wit", world: "consumer" });

#[cfg(test)]
mod tests;

/// Runs the original narrated prototype during the library-layout transition.
#[doc(hidden)]
pub fn run_demo() -> Result<()> {
    demo::run()
}
