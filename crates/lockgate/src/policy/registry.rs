use std::{collections::BTreeMap, error::Error, fmt};

use lockgate_policy::{__private::ErasedPermission, CapabilityContract, ScopeError};
use lockgate_schema::AtomKey;

/// A capability contract could not be registered safely.
#[derive(Debug)]
#[non_exhaustive]
pub enum CapabilityRegistrationError {
    /// The contract's capability ID does not satisfy the authoring grammar.
    MalformedCapabilityId { capability: &'static str },
    /// A contract with the same capability ID is already registered.
    DuplicateCapability { capability: &'static str },
    /// A permission ID does not satisfy the authoring grammar.
    MalformedPermissionId {
        capability: &'static str,
        permission: &'static str,
    },
    /// A contract declares the same permission ID more than once.
    DuplicatePermission {
        capability: &'static str,
        permission: &'static str,
    },
    /// A descriptor carries a different capability ID than its contract.
    InconsistentDescriptor {
        contract_capability: &'static str,
        descriptor_capability: &'static str,
        permission: &'static str,
    },
    /// A scoped permission's exhaustive domain violates the scope laws.
    ScopeLawViolation {
        capability: &'static str,
        permission: &'static str,
        scope_type: &'static str,
        source: ScopeError,
    },
}

impl fmt::Display for CapabilityRegistrationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MalformedCapabilityId { capability } => write!(
                formatter,
                "capability ID `{capability}` is malformed; expected lowercase ASCII kebab-case such as `virtual-machines`"
            ),
            Self::DuplicateCapability { capability } => write!(
                formatter,
                "capability `{capability}` is registered more than once; register each capability contract once"
            ),
            Self::MalformedPermissionId {
                capability,
                permission,
            } => write!(
                formatter,
                "capability `{capability}` declares malformed permission ID `{permission}`; expected lowercase ASCII kebab-case such as `list-pools`"
            ),
            Self::DuplicatePermission {
                capability,
                permission,
            } => write!(
                formatter,
                "capability `{capability}` declares permission `{permission}` more than once; permission IDs must be unique within a capability"
            ),
            Self::InconsistentDescriptor {
                contract_capability,
                descriptor_capability,
                permission,
            } => write!(
                formatter,
                "contract `{contract_capability}` returned permission descriptor `{descriptor_capability}.{permission}`; descriptors must be qualified with their contract's capability ID"
            ),
            Self::ScopeLawViolation {
                capability,
                permission,
                scope_type,
                source,
            } => write!(
                formatter,
                "capability `{capability}` permission `{permission}` uses scope type `{scope_type}` that violates Lockgate's scope laws: {source}"
            ),
        }
    }
}

impl Error for CapabilityRegistrationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::ScopeLawViolation { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[derive(Clone, Default)]
pub(crate) struct CapabilityRegistry {
    capabilities: BTreeMap<&'static str, BTreeMap<&'static str, ErasedPermission>>,
}

impl CapabilityRegistry {
    pub(crate) fn contains(&self, capability: &str, permission: &str) -> bool {
        self.capabilities
            .get(capability)
            .is_some_and(|permissions| permissions.contains_key(permission))
    }

    pub(crate) fn permission(&self, atom: &AtomKey) -> Option<ErasedPermission> {
        self.capabilities
            .get(atom.capability())
            .and_then(|permissions| permissions.get(atom.operation()))
            .copied()
    }

    pub(crate) fn register<C: CapabilityContract>(
        &mut self,
    ) -> Result<(), CapabilityRegistrationError> {
        let capability = C::ID;
        if !lockgate_policy::__private::is_valid_authoring_id(capability) {
            return Err(CapabilityRegistrationError::MalformedCapabilityId { capability });
        }
        if self.capabilities.contains_key(capability) {
            return Err(CapabilityRegistrationError::DuplicateCapability { capability });
        }

        let mut permissions = BTreeMap::new();
        for descriptor in C::permissions() {
            let permission = descriptor.permission_id();
            if descriptor.capability_id() != capability {
                return Err(CapabilityRegistrationError::InconsistentDescriptor {
                    contract_capability: capability,
                    descriptor_capability: descriptor.capability_id(),
                    permission,
                });
            }
            if !lockgate_policy::__private::is_valid_authoring_id(permission) {
                return Err(CapabilityRegistrationError::MalformedPermissionId {
                    capability,
                    permission,
                });
            }
            if permissions.contains_key(permission) {
                return Err(CapabilityRegistrationError::DuplicatePermission {
                    capability,
                    permission,
                });
            }
            if let Some(Err(source)) = descriptor.validate_scope_laws() {
                return Err(CapabilityRegistrationError::ScopeLawViolation {
                    capability,
                    permission,
                    scope_type: descriptor
                        .scope_type_name()
                        .expect("a scope-law hook implies a scoped descriptor"),
                    source,
                });
            }
            permissions.insert(permission, *descriptor);
        }
        self.capabilities.insert(capability, permissions);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use lockgate_policy::{
        __private::{ErasedPermission, erase_permission, qualify_permission},
        CapabilityContract, Permission,
    };

    use super::{CapabilityRegistrationError, CapabilityRegistry};

    const READ: Permission = qualify_permission("sessions", Permission::new("read"));
    const MALFORMED: Permission = qualify_permission("sessions", Permission::new("Read_All"));
    static PARTIALLY_INVALID: [ErasedPermission; 2] =
        [erase_permission(READ), erase_permission(MALFORMED)];
    static VALID: [ErasedPermission; 1] = [erase_permission(READ)];

    struct MidValidationFailure;

    impl CapabilityContract for MidValidationFailure {
        const ID: &'static str = "sessions";

        fn permissions() -> &'static [ErasedPermission] {
            &PARTIALLY_INVALID
        }
    }

    struct ValidContract;

    impl CapabilityContract for ValidContract {
        const ID: &'static str = "sessions";

        fn permissions() -> &'static [ErasedPermission] {
            &VALID
        }
    }

    #[test]
    fn failed_registration_does_not_reserve_the_capability_id() {
        let mut registry = CapabilityRegistry::default();

        assert!(matches!(
            registry.register::<MidValidationFailure>(),
            Err(CapabilityRegistrationError::MalformedPermissionId {
                capability: "sessions",
                permission: "Read_All",
            })
        ));
        registry.register::<ValidContract>().unwrap();
    }
}
