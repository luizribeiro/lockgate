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

#[cfg(test)]
mod tests;
