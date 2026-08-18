//! Typed declarations and qualified permission handles.

use core::fmt;

use crate::atom::{
    AtomValidationError, QualifiedAtom, validate_capability_id, validate_permission_id,
};

/// An unqualified declaration consumed by `#[lockgate::capability]`.
///
/// A declaration retains only a permission ID, so it cannot be confused with
/// a complete authority identity or used where a qualified [`Permission`] is
/// required.
#[doc(hidden)]
#[derive(Clone, Copy)]
pub struct PermissionDecl {
    permission: &'static str,
}

impl fmt::Debug for PermissionDecl {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PermissionDecl")
            .field("permission", &self.permission)
            .finish()
    }
}

/// A qualified unscoped permission handle.
///
/// Its private identity always contains explicit, validated capability and
/// permission IDs. Authors declare one with [`Permission::new`], and generated
/// capability code turns that declaration into this handle through a hidden
/// implementation hook.
///
/// An unqualified declaration cannot be passed where this type is required:
///
/// ```compile_fail,E0308
/// use lockgate_policy::Permission;
///
/// fn requires_qualified(_: Permission) {}
///
/// requires_qualified(Permission::new("read"));
/// ```
///
/// The qualified identity cannot be populated through public fields:
///
/// ```compile_fail
/// use lockgate_policy::Permission;
///
/// let _ = Permission {};
/// ```
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct Permission {
    atom: QualifiedAtom,
}

impl Permission {
    /// Declares an unscoped permission ID for `#[lockgate::capability]`.
    ///
    /// This returns an inert declaration rather than a partially qualified
    /// `Permission`.
    #[allow(
        clippy::new_ret_no_self,
        reason = "the normative API makes new an intentionally inert declaration constructor"
    )]
    pub const fn new(permission: &'static str) -> PermissionDecl {
        assert_valid_permission_id(permission);
        PermissionDecl { permission }
    }

    #[allow(
        dead_code,
        reason = "erased descriptors consume this identity in a later commit"
    )]
    pub(crate) const fn atom(self) -> QualifiedAtom {
        self.atom
    }
}

impl fmt::Debug for Permission {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Permission")
            .field("capability", &self.atom.capability())
            .field("permission", &self.atom.permission())
            .finish()
    }
}

/// Qualifies an unscoped declaration for generated capability code.
#[doc(hidden)]
pub const fn qualify_permission(
    capability: &'static str,
    declaration: PermissionDecl,
) -> Permission {
    assert_valid_capability_id(capability);
    Permission {
        atom: match QualifiedAtom::new(capability, declaration.permission) {
            Ok(atom) => atom,
            Err(_) => panic!("Lockgate generated an invalid qualified permission atom"),
        },
    }
}

const fn assert_valid_capability_id(capability: &str) {
    match validate_capability_id(capability) {
        Ok(()) => {}
        Err(AtomValidationError::EmptyCapability) => {
            panic!("Lockgate capability ID must not be empty")
        }
        Err(AtomValidationError::DotInSegment) => {
            panic!("Lockgate capability ID must not contain `.`")
        }
        Err(AtomValidationError::EmptyPermission) => {
            panic!("Lockgate capability validation returned an impossible permission error")
        }
    }
}

const fn assert_valid_permission_id(permission: &str) {
    match validate_permission_id(permission) {
        Ok(()) => {}
        Err(AtomValidationError::EmptyPermission) => {
            panic!("Lockgate permission ID must not be empty")
        }
        Err(AtomValidationError::DotInSegment) => {
            panic!("Lockgate permission ID must not contain `.`")
        }
        Err(AtomValidationError::EmptyCapability) => {
            panic!("Lockgate permission validation returned an impossible capability error")
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const LIST_DECL: PermissionDecl = Permission::new("list-pools");
    const LIST: Permission = qualify_permission("vm", LIST_DECL);

    #[test]
    fn qualification_retains_the_complete_stable_identity() {
        assert_eq!(LIST.atom().capability(), "vm");
        assert_eq!(LIST.atom().permission(), "list-pools");
    }

    #[test]
    #[should_panic(expected = "Lockgate permission ID must not contain `.`")]
    fn declarations_reject_ambiguous_permission_ids() {
        let _ = Permission::new("admin.read");
    }

    #[test]
    #[should_panic(expected = "Lockgate capability ID must not be empty")]
    fn qualification_rejects_an_empty_capability_id() {
        let _ = qualify_permission("", Permission::new("read"));
    }

    #[test]
    fn declarations_and_handles_support_derived_debug() {
        #[allow(
            dead_code,
            reason = "the fields exercise the permission types' Debug implementations"
        )]
        #[derive(Debug)]
        struct DebuggablePermissions {
            declaration: PermissionDecl,
            qualified: Permission,
        }

        let debug = alloc::format!(
            "{:?}",
            DebuggablePermissions {
                declaration: LIST_DECL,
                qualified: LIST,
            }
        );
        assert!(debug.contains("PermissionDecl"));
        assert!(debug.contains("Permission"));
    }
}
