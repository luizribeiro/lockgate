//! Stateful host policy registration and enforcement.

mod metadata;
mod registry;
mod resolution;

#[doc(hidden)]
pub use metadata::{
    HostImportPolicyError, HostImportPolicyMetadata, InterfaceIdentity, MethodClassification,
    MethodIdentity, PolicyMethod, PolicyPermission, ValidatedInterfacePolicy,
    validate_interface_policy,
};
pub use registry::CapabilityRegistrationError;
pub(crate) use registry::CapabilityRegistry;
pub use resolution::{
    InvalidScopeValue, JsonValueKind, NeedValueKind, ScopeReference, ScopeResolutionError,
};
pub(crate) use resolution::{ResolvedNeeds, resolve_needs};
