//! Stateful host policy registration and enforcement.

mod metadata;
mod registry;

#[doc(hidden)]
pub use metadata::{
    InterfaceIdentity, MethodClassification, MethodIdentity, PolicyMethod, PolicyPermission,
};
pub use registry::CapabilityRegistrationError;
pub(crate) use registry::CapabilityRegistry;
