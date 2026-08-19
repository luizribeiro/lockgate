//! Stateful host policy registration and enforcement.

mod grants;
mod metadata;
mod registry;
mod resolution;
mod resources;

pub use grants::EffectiveGrants;
pub(crate) use grants::PreparedNeedsDigest;
#[doc(hidden)]
pub use metadata::{
    HostImportPolicyError, HostImportPolicyMetadata, InterfaceIdentity, MethodClassification,
    MethodIdentity, PolicyMethod, PolicyPermission, ValidatedInterfacePolicy,
    validate_interface_policy, validate_interface_policy_parts,
};
pub use registry::CapabilityRegistrationError;
pub(crate) use registry::CapabilityRegistry;
pub use resolution::{
    InvalidScopeValue, JsonValueKind, NeedValueKind, ScopeReference, ScopeResolutionError,
};
pub(crate) use resolution::{ResolvedNeeds, resolve_needs};
pub(crate) use resources::scoped_access_allowed;
pub use resources::{
    PermissionDenied, PluginSubject, ResolveCtx, ResolveScopedResource,
    ResolveScopedResourceHandle, ResourceLookupError, ScopedResource,
};
#[doc(hidden)]
pub use resources::{ResourceStore, resolve_scoped_resource, resolve_scoped_resource_handle};
