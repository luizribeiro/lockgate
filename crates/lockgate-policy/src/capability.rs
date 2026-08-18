//! Capability contracts and type-erased permission descriptor shapes.

use core::{any::TypeId, any::type_name, str::FromStr};

use crate::{Permission, Scope, ScopeError, ScopedPermission};

type ScopeTypeName = fn() -> &'static str;

/// The generated registration anchor for one capability family.
///
/// A contract binds one explicit stable capability ID to a static collection
/// of erased permission descriptors. This keeps registration deterministic
/// without runtime inventory or author-maintained registration closures.
pub trait CapabilityContract {
    /// The capability family's explicit stable wire ID.
    const ID: &'static str;

    /// Returns the permissions generated from the annotated capability module.
    #[doc(hidden)]
    fn permissions() -> &'static [ErasedPermission];
}

/// A declared permission with its scope generic erased for static storage.
///
/// The descriptor retains the stable permission ID and, for scoped
/// permissions, the concrete scope type identity. Its private fields ensure
/// descriptors can only be constructed from qualified permission handles.
#[derive(Clone, Copy)]
pub struct ErasedPermission {
    permission_id: &'static str,
    scope: Option<ErasedScope>,
}

impl ErasedPermission {
    /// Returns the explicit stable permission ID without the capability prefix.
    #[doc(hidden)]
    pub const fn permission_id(&self) -> &'static str {
        self.permission_id
    }

    /// Returns whether this permission requires scoped grants.
    #[doc(hidden)]
    pub const fn is_scoped(&self) -> bool {
        self.scope.is_some()
    }

    /// Returns the concrete scope type identity for a scoped permission.
    #[doc(hidden)]
    pub const fn scope_type_id(&self) -> Option<TypeId> {
        match self.scope {
            Some(scope) => Some(scope.type_id),
            None => None,
        }
    }

    /// Returns the diagnostic Rust type name for a scoped permission.
    #[doc(hidden)]
    pub fn scope_type_name(&self) -> Option<&'static str> {
        match self.scope {
            Some(scope) => Some((scope.type_name)()),
            None => None,
        }
    }
}

#[derive(Clone, Copy)]
struct ErasedScope {
    type_id: TypeId,
    type_name: ScopeTypeName,
}

/// Erases a qualified unscoped permission for generated capability metadata.
#[doc(hidden)]
pub const fn erase_permission(permission: Permission) -> ErasedPermission {
    ErasedPermission {
        permission_id: permission.atom().permission(),
        scope: None,
    }
}

/// Erases a qualified scoped permission while retaining its scope identity.
#[doc(hidden)]
pub const fn erase_scoped_permission<S>(permission: ScopedPermission<S>) -> ErasedPermission
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    ErasedPermission {
        permission_id: permission.atom().permission(),
        scope: Some(ErasedScope {
            type_id: TypeId::of::<S>(),
            type_name: type_name::<S>,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        __private::{
            PermissionDecl, ScopedPermissionDecl, qualify_permission, qualify_scoped_permission,
        },
        ScopeRepr,
    };
    use alloc::string::{String, ToString};

    #[derive(Clone, PartialEq, Eq)]
    enum PoolScope {
        Any,
        Named(String),
    }

    impl FromStr for PoolScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "*" => Ok(Self::Any),
                "" => Err(ScopeError::unknown(value)),
                name => Ok(Self::Named(name.to_string())),
            }
        }
    }

    impl ScopeRepr for PoolScope {
        fn canonical(&self) -> String {
            match self {
                Self::Any => "*".to_string(),
                Self::Named(name) => name.clone(),
            }
        }
    }

    impl Scope for PoolScope {
        fn contains(&self, inner: &Self) -> bool {
            self == inner || matches!(self, Self::Any)
        }
    }

    const EXEC_DECL: ScopedPermissionDecl<PoolScope> = ScopedPermission::<PoolScope>::new("exec");
    const EXEC: ScopedPermission<PoolScope> = qualify_scoped_permission("vm", EXEC_DECL);
    const ERASED_EXEC: ErasedPermission = erase_scoped_permission(EXEC);

    const LIST_DECL: PermissionDecl = Permission::new("list");
    const LIST: Permission = qualify_permission("vm", LIST_DECL);
    const ERASED_LIST: ErasedPermission = erase_permission(LIST);

    struct Contract;

    impl CapabilityContract for Contract {
        const ID: &'static str = "vm";

        fn permissions() -> &'static [ErasedPermission] {
            &[ERASED_EXEC, ERASED_LIST]
        }
    }

    #[test]
    fn construction_retains_permission_and_scope_identity() {
        assert_eq!(Contract::ID, "vm");
        assert_eq!(Contract::permissions().len(), 2);
        assert_eq!(ERASED_EXEC.permission_id(), "exec");
        assert!(ERASED_EXEC.is_scoped());
        assert_eq!(ERASED_EXEC.scope_type_id(), Some(TypeId::of::<PoolScope>()));
        assert_eq!(
            ERASED_EXEC.scope_type_name(),
            Some(type_name::<PoolScope>())
        );

        assert_eq!(ERASED_LIST.permission_id(), "list");
        assert!(!ERASED_LIST.is_scoped());
        assert_eq!(ERASED_LIST.scope_type_id(), None);
        assert_eq!(ERASED_LIST.scope_type_name(), None);
    }
}
