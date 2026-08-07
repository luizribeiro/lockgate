//! Capability-scoped WebAssembly component applications with runtime-discovered linking.
//! Catalogs discover artifacts, policies grant authority, and runtime builders validate before instantiation.

pub mod catalog;
mod plan;
mod plugin;
pub mod policy;
pub mod runtime;

pub use catalog::{ArtifactDigest, Catalog, CatalogError, ComponentId, ComponentInfo};
pub use policy::{DirectoryAccess, Policy, PolicyBuilder, PolicyError};
pub use runtime::{Event, Runtime, RuntimeBuildError, RuntimeBuilder, RuntimeError};
pub use wasmtime::component::Val;

wasmtime::component::bindgen!({ path: "wit", world: "consumer" });

#[cfg(test)]
mod tests;
