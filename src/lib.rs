//! Capability-scoped WebAssembly component applications with runtime-discovered linking.
//! Catalogs discover artifacts, policies grant authority, and runtime builders validate before instantiation.

extern crate self as lockgate;

mod binding;
pub mod catalog;
mod plan;
mod plugin;
pub mod policy;
pub mod runtime;

pub use catalog::{
    ArtifactDigest, Catalog, CatalogError, Component, ComponentId, ComponentInfo, ComponentRef,
};
pub use lockgate_macros::{bindgen, bindings};
pub use policy::{DirectoryAccess, HostImportGrant, Policy, PolicyBuilder, PolicyError};
pub use runtime::{
    Event, HasHost, PluginStore, Runtime, RuntimeBuildError, RuntimeBuilder, RuntimeError,
};

#[doc(hidden)]
pub mod __private {
    pub use crate::binding::{BindingExport, BindingImport, ComponentBinding};
    pub use anyhow::Result as AnyResult;
    pub use wasmtime::Store as WasmtimeStore;
    pub use wasmtime::component::Instance as WasmtimeInstance;
}

#[cfg(test)]
mod tests;
