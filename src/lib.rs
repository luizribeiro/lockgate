//! Capability-scoped WebAssembly component applications with runtime-discovered linking.
//! Catalogs discover artifacts, policies grant authority, and runtime builders validate before instantiation.

pub mod catalog;
mod plan;
mod plugin;
pub mod policy;
pub mod runtime;

pub use catalog::{ArtifactDigest, Catalog, CatalogError, ComponentId, ComponentInfo};
pub use policy::{DirectoryAccess, HostImportGrant, Policy, PolicyBuilder, PolicyError};
pub use runtime::{Event, PluginStore, Runtime, RuntimeBuildError, RuntimeBuilder, RuntimeError};
pub use wasmtime::component::Val;

pub(crate) const REGISTRY_INTERFACE: &str = "lockgate:core/registry@0.1.0";

wasmtime::component::bindgen!({ path: "wit", world: "consumer" });

#[cfg(test)]
mod tests;
