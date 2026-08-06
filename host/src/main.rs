//! Host binary entry point.
//! Declares the host modules and generates bindings for its one compile-time WIT dependency.

use anyhow::Result;

mod demo;
mod manifest;
mod plugin;
mod runtime;

wasmtime::component::bindgen!({ path: "../wit", world: "consumer" });

#[cfg(test)]
mod tests;

fn main() -> Result<()> {
    demo::run()
}
