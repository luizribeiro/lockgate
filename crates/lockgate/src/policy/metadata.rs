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
