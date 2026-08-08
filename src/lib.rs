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
pub use catalog::{CatalogError, Component};
pub use lockgate_macros::bindings;
pub use policy::{Policy, PolicyBuilder, PolicyError};
pub use runtime::{HostContext, Runtime, RuntimeBuildError, RuntimeError};

#[doc(hidden)]
pub mod __private {
    pub use crate::binding::{Binding, BindingExport};
    pub use crate::runtime::{PluginStore, RuntimeComponent};
    pub use anyhow::Result as AnyResult;
}

#[cfg(test)]
mod tests;
