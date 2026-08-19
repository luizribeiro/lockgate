//! Stateful host policy registration and enforcement.

mod registry;

pub use registry::CapabilityRegistrationError;
pub(crate) use registry::CapabilityRegistry;
