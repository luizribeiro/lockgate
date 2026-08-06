//! Capability-scoped WebAssembly component runtime.
//! The initial public surface is minimal while the prototype is extracted into a library.

use anyhow::Result;
use std::path::Path;

pub mod catalog;
mod demo;
mod manifest;
mod plugin;
pub mod policy;
mod runtime;

pub use catalog::{ArtifactDigest, Catalog, CatalogError, ComponentId, ComponentInfo, ExportId};
pub use policy::{DirectoryAccess, Policy, PolicyBuilder, PolicyError};

wasmtime::component::bindgen!({ path: "wit", world: "consumer" });

#[cfg(test)]
mod tests;

/// Runs the original narrated prototype from a staged fixture directory.
#[doc(hidden)]
pub fn run_demo(root: impl AsRef<Path>) -> Result<()> {
    demo::run(root.as_ref())
}
