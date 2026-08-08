//! Capability-scoped WebAssembly component applications with runtime-discovered linking.
//! Applications admit artifacts, policies grant authority, and runtime builders validate before instantiation.

extern crate self as lockgate;

mod application;
mod binding;
mod catalog;
mod plan;
mod plugin;
mod policy;
mod runtime;

pub use application::Application;
pub use catalog::{
    ArtifactDigest, CatalogError, Component, ComponentInfo, ExportInfo, FunctionSignature,
};
pub use lockgate_macros::bindings;
pub use policy::{Policy, PolicyBuilder, PolicyError};
pub use runtime::{Event, HostContext, Runtime, RuntimeBuildError, RuntimeBuilder, RuntimeError};

#[doc(hidden)]
pub mod __private {
    pub use crate::binding::{
        ApplicationBinding, BindingExport, BindingImport, ComponentBinding, HostImportBinding,
        RuntimeBinding,
    };
    pub use crate::runtime::{HostContextData, PluginStore, RuntimeComponent};
    pub use anyhow::Result as AnyResult;
}

#[cfg(test)]
mod tests;
