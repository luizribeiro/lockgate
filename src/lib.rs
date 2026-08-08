//! Capability-scoped WebAssembly component applications with runtime-discovered linking.
//! Applications admit artifacts, retain explicit grants, and validate before instantiation.

extern crate self as lockgate;

mod application;
mod binding;
mod catalog;
mod grants;
mod plan;
mod plugin;
mod runtime;

pub use application::Application;
pub use catalog::{ApplicationError, Component};
pub use lockgate_macros::bindings;
pub use runtime::{HostContext, Runtime, RuntimeBuildError, RuntimeError};

#[doc(hidden)]
pub mod __private {
    pub use crate::binding::{Binding, BindingExport};
    pub use crate::runtime::{PluginStore, RuntimeComponent};
    pub use anyhow::Result as AnyResult;
}

#[cfg(test)]
mod tests;
