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
