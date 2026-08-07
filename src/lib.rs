//! Capability-scoped WebAssembly component runtime.
//! The initial public surface is minimal while the prototype is extracted into a library.

pub mod catalog;
pub mod plan;
mod plugin;
pub mod policy;
pub mod runtime;

pub use catalog::{ArtifactDigest, Catalog, CatalogError, ComponentId, ComponentInfo};
pub use plan::{Plan, PlanError};
pub use policy::{DirectoryAccess, Policy, PolicyBuilder, PolicyError};
pub use runtime::{Event, Runtime, RuntimeError};
pub use wasmtime::component::Val;

wasmtime::component::bindgen!({ path: "wit", world: "consumer" });

#[cfg(test)]
mod tests;
