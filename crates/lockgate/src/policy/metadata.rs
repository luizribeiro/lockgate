use std::{error::Error, fmt};

use lockgate_policy::{Permission, Scope, ScopeError, ScopedPermission};

/// Stable identity of one generated imported WIT interface.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InterfaceIdentity {
    name: &'static str,
    version: Option<&'static str>,
}

impl InterfaceIdentity {
    /// Reserved for generated host bindings.
    #[doc(hidden)]
    pub const fn __new(name: &'static str, version: Option<&'static str>) -> Self {
        Self { name, version }
    }

    #[doc(hidden)]
    pub const fn name(self) -> &'static str {
        self.name
    }

    #[doc(hidden)]
    pub const fn version(self) -> Option<&'static str> {
        self.version
    }

    pub(crate) fn imported_name(self) -> String {
        match self.version {
            Some(version) => format!("{}@{version}", self.name),
            None => self.name.to_owned(),
        }
    }

    #[doc(hidden)]
    pub fn matches_import(self, imported: &str) -> bool {
        self.imported_name() == imported
    }
}

/// Rust and WIT names of one generated imported method.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MethodIdentity {
    rust_name: &'static str,
    wit_name: &'static str,
}

impl MethodIdentity {
    /// Reserved for generated host bindings.
    #[doc(hidden)]
    pub const fn __new(rust_name: &'static str, wit_name: &'static str) -> Self {
        Self {
            rust_name,
            wit_name,
        }
    }

    #[doc(hidden)]
    pub const fn rust_name(self) -> &'static str {
        self.rust_name
    }

    #[doc(hidden)]
    pub const fn wit_name(self) -> &'static str {
        self.wit_name
    }
}

/// Stable permission identity copied from a typed permission constant.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyPermission {
    capability: &'static str,
    permission: &'static str,
}

impl PolicyPermission {
    const fn unscoped(permission: Permission) -> Self {
        let (capability, permission) = lockgate_policy::__private::permission_ids(permission);
        Self {
            capability,
            permission,
        }
    }

    const fn scoped<S: Scope>(permission: ScopedPermission<S>) -> Self
    where
        <S as core::str::FromStr>::Err: Into<ScopeError>,
    {
        let (capability, permission) =
            lockgate_policy::__private::scoped_permission_ids(permission);
        Self {
            capability,
            permission,
        }
    }

    #[doc(hidden)]
    pub const fn capability(self) -> &'static str {
        self.capability
    }

    #[doc(hidden)]
    pub const fn permission(self) -> &'static str {
        self.permission
    }
}

/// Classification generated for one imported WIT method.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MethodClassification {
    Requires {
        permission: PolicyPermission,
        target: Option<&'static str>,
    },
    NoCapabilityRequired {
        reason: &'static str,
    },
}

impl MethodClassification {
    #[doc(hidden)]
    pub const fn permission(self) -> Option<PolicyPermission> {
        match self {
            Self::Requires { permission, .. } => Some(permission),
            Self::NoCapabilityRequired { .. } => None,
        }
    }

    #[doc(hidden)]
    pub const fn target(self) -> Option<&'static str> {
        match self {
            Self::Requires { target, .. } => target,
            Self::NoCapabilityRequired { .. } => None,
        }
    }

    #[doc(hidden)]
    pub const fn reason(self) -> Option<&'static str> {
        match self {
            Self::Requires { .. } => None,
            Self::NoCapabilityRequired { reason } => Some(reason),
        }
    }
}

/// One generated method identity and its application classification.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PolicyMethod {
    interface: InterfaceIdentity,
    method: MethodIdentity,
    classification: MethodClassification,
}

impl PolicyMethod {
    /// Reserved for `#[lockgate::guarded]` output.
    #[doc(hidden)]
    pub const fn __requires_unscoped(
        interface: InterfaceIdentity,
        method: MethodIdentity,
        permission: Permission,
    ) -> Self {
        Self {
            interface,
            method,
            classification: MethodClassification::Requires {
                permission: PolicyPermission::unscoped(permission),
                target: None,
            },
        }
    }

    /// Reserved for `#[lockgate::guarded]` output.
    #[doc(hidden)]
    pub const fn __requires_scoped<S: Scope>(
        interface: InterfaceIdentity,
        method: MethodIdentity,
        permission: ScopedPermission<S>,
        target: &'static str,
    ) -> Self
    where
        <S as core::str::FromStr>::Err: Into<ScopeError>,
    {
        Self {
            interface,
            method,
            classification: MethodClassification::Requires {
                permission: PolicyPermission::scoped(permission),
                target: Some(target),
            },
        }
    }

    /// Reserved for `#[lockgate::guarded]` output.
    #[doc(hidden)]
    pub const fn __no_capability_required(
        interface: InterfaceIdentity,
        method: MethodIdentity,
        reason: &'static str,
    ) -> Self {
        Self {
            interface,
            method,
            classification: MethodClassification::NoCapabilityRequired { reason },
        }
    }

    #[doc(hidden)]
    pub const fn interface(self) -> InterfaceIdentity {
        self.interface
    }

    #[doc(hidden)]
    pub const fn method(self) -> MethodIdentity {
        self.method
    }

    #[doc(hidden)]
    pub const fn classification(self) -> MethodClassification {
        self.classification
    }
}

/// A generated host-import policy slot does not match its binding companion.
#[doc(hidden)]
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum HostImportPolicyError {
    MissingGuardedImplementation {
        interface: InterfaceIdentity,
    },
    WrongInterface {
        expected: InterfaceIdentity,
        found: InterfaceIdentity,
        method: MethodIdentity,
    },
    UnknownMethod {
        interface: InterfaceIdentity,
        method: MethodIdentity,
    },
    DuplicateMethod {
        interface: InterfaceIdentity,
        method: MethodIdentity,
    },
    MissingMethod {
        interface: InterfaceIdentity,
        method: MethodIdentity,
    },
}

impl fmt::Display for HostImportPolicyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingGuardedImplementation { interface } => write!(
                formatter,
                "host import interface `{}` has no policy classifications; annotate its generated `Host` implementation with `#[lockgate::guarded]`",
                DisplayInterface(*interface),
            ),
            Self::WrongInterface {
                expected,
                found,
                method,
            } => write!(
                formatter,
                "host policy method `{}` names interface `{}` but binding construction expected `{}`",
                method.rust_name(),
                DisplayInterface(*found),
                DisplayInterface(*expected),
            ),
            Self::UnknownMethod { interface, method } => write!(
                formatter,
                "host policy for interface `{}` names unknown method `{}` (WIT `{}`)",
                DisplayInterface(*interface),
                method.rust_name(),
                method.wit_name(),
            ),
            Self::DuplicateMethod { interface, method } => write!(
                formatter,
                "host policy for interface `{}` classifies method `{}` more than once",
                DisplayInterface(*interface),
                method.rust_name(),
            ),
            Self::MissingMethod { interface, method } => write!(
                formatter,
                "host policy for interface `{}` does not classify method `{}` (WIT `{}`)",
                DisplayInterface(*interface),
                method.rust_name(),
                method.wit_name(),
            ),
        }
    }
}

impl Error for HostImportPolicyError {}

struct DisplayInterface(InterfaceIdentity);

impl fmt::Display for DisplayInterface {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.0.name())?;
        if let Some(version) = self.0.version() {
            write!(formatter, "@{version}")?;
        }
        Ok(())
    }
}

/// One interface slot validated against its generated binding companion.
#[doc(hidden)]
#[derive(Clone, Debug)]
pub struct ValidatedInterfacePolicy {
    interface: InterfaceIdentity,
    methods: Vec<PolicyMethod>,
}

/// Deterministic policy metadata retained by `HostBuilder` for later wiring.
#[doc(hidden)]
#[derive(Clone, Debug, Default)]
pub struct HostImportPolicyMetadata {
    interfaces: Vec<ValidatedInterfacePolicy>,
}

impl HostImportPolicyMetadata {
    /// Reserved for generated host bindings.
    #[doc(hidden)]
    pub fn __new(mut interfaces: Vec<ValidatedInterfacePolicy>) -> Self {
        interfaces
            .sort_by_key(|interface| (interface.interface.name(), interface.interface.version()));
        Self { interfaces }
    }

    pub(crate) fn interfaces(&self) -> &[ValidatedInterfacePolicy] {
        &self.interfaces
    }
}

impl ValidatedInterfacePolicy {
    pub(crate) const fn interface(&self) -> InterfaceIdentity {
        self.interface
    }

    pub(crate) fn methods(&self) -> &[PolicyMethod] {
        &self.methods
    }
}

/// Validates and normalizes one generated interface policy slot.
#[doc(hidden)]
pub fn validate_interface_policy(
    interface: InterfaceIdentity,
    expected_methods: &[MethodIdentity],
    policy_methods: &[PolicyMethod],
) -> Result<ValidatedInterfacePolicy, HostImportPolicyError> {
    if policy_methods.is_empty() {
        return Err(HostImportPolicyError::MissingGuardedImplementation { interface });
    }

    for (index, policy) in policy_methods.iter().enumerate() {
        if policy.interface() != interface {
            return Err(HostImportPolicyError::WrongInterface {
                expected: interface,
                found: policy.interface(),
                method: policy.method(),
            });
        }
        if !expected_methods.contains(&policy.method()) {
            return Err(HostImportPolicyError::UnknownMethod {
                interface,
                method: policy.method(),
            });
        }
        if policy_methods[..index]
            .iter()
            .any(|earlier| earlier.method() == policy.method())
        {
            return Err(HostImportPolicyError::DuplicateMethod {
                interface,
                method: policy.method(),
            });
        }
    }

    let mut methods = Vec::with_capacity(expected_methods.len());
    for expected in expected_methods {
        let Some(policy) = policy_methods
            .iter()
            .find(|policy| policy.method() == *expected)
        else {
            return Err(HostImportPolicyError::MissingMethod {
                interface,
                method: *expected,
            });
        };
        methods.push(*policy);
    }
    Ok(ValidatedInterfacePolicy { interface, methods })
}

#[cfg(test)]
mod tests {
    use super::*;

    const INTERFACE: InterfaceIdentity = InterfaceIdentity::__new("test:policy/vm", Some("1.0.0"));
    const OTHER_INTERFACE: InterfaceIdentity = InterfaceIdentity::__new("test:policy/other", None);
    const FIRST: MethodIdentity = MethodIdentity::__new("first", "first");
    const SECOND: MethodIdentity = MethodIdentity::__new("second", "second");
    const UNKNOWN: MethodIdentity = MethodIdentity::__new("unknown", "unknown");
    const EXPECTED: &[MethodIdentity] = &[FIRST, SECOND];

    const fn free(interface: InterfaceIdentity, method: MethodIdentity) -> PolicyMethod {
        PolicyMethod::__no_capability_required(interface, method, "metadata validator fixture")
    }

    #[test]
    fn empty_and_partially_filled_slots_are_rejected() {
        assert!(matches!(
            validate_interface_policy(INTERFACE, EXPECTED, &[]),
            Err(HostImportPolicyError::MissingGuardedImplementation {
                interface: INTERFACE,
            })
        ));
        assert!(matches!(
            validate_interface_policy(INTERFACE, EXPECTED, &[free(INTERFACE, FIRST)]),
            Err(HostImportPolicyError::MissingMethod { method: SECOND, .. })
        ));
    }

    #[test]
    fn validation_names_wrong_unknown_and_duplicate_identities() {
        assert!(matches!(
            validate_interface_policy(
                INTERFACE,
                EXPECTED,
                &[free(OTHER_INTERFACE, FIRST), free(INTERFACE, SECOND)]
            ),
            Err(HostImportPolicyError::WrongInterface {
                found: OTHER_INTERFACE,
                method: FIRST,
                ..
            })
        ));
        assert!(matches!(
            validate_interface_policy(
                INTERFACE,
                EXPECTED,
                &[free(INTERFACE, FIRST), free(INTERFACE, UNKNOWN)]
            ),
            Err(HostImportPolicyError::UnknownMethod {
                method: UNKNOWN,
                ..
            })
        ));
        assert!(matches!(
            validate_interface_policy(
                INTERFACE,
                EXPECTED,
                &[free(INTERFACE, FIRST), free(INTERFACE, FIRST)]
            ),
            Err(HostImportPolicyError::DuplicateMethod { method: FIRST, .. })
        ));
    }

    #[test]
    fn validated_metadata_uses_companion_order() {
        let validated = validate_interface_policy(
            INTERFACE,
            EXPECTED,
            &[free(INTERFACE, SECOND), free(INTERFACE, FIRST)],
        )
        .unwrap();
        assert_eq!(validated.interface(), INTERFACE);
        assert_eq!(
            validated
                .methods()
                .iter()
                .map(|method| method.method())
                .collect::<Vec<_>>(),
            [FIRST, SECOND]
        );
    }
}
