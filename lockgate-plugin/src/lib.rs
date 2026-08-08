//! Lightweight guest bindings and metadata emission for Rust Lockgate plugins.

pub use lockgate_plugin_macros::bindings;

#[doc(hidden)]
pub use wit_bindgen as __wit_bindgen;
