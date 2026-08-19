//! Typed declarations and qualified permission handles.

use core::{fmt, hash::Hash, marker::PhantomData, str::FromStr};

use crate::{
    Need, Scope, ScopeError, ScopeRef,
    atom::{AtomValidationError, QualifiedAtom, validate_capability_id, validate_permission_id},
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

/// An unqualified scoped declaration consumed by `#[lockgate::capability]`.
///
/// The scope marker preserves the declared scope vocabulary without granting
/// the value a capability ID or making it usable as a qualified permission.
#[doc(hidden)]
pub struct ScopedPermissionDecl<S: Scope>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    permission: &'static str,
    marker: PhantomData<fn() -> S>,
}

impl<S: Scope> Clone for ScopedPermissionDecl<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Scope> Copy for ScopedPermissionDecl<S> where <S as FromStr>::Err: Into<ScopeError> {}

impl<S: Scope> fmt::Debug for ScopedPermissionDecl<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopedPermissionDecl")
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

    /// Requests this unscoped permission for a plugin.
    ///
    /// The qualified atom comes from the surrounding capability contract, so
    /// callers do not repeat its stable string identity.
    pub const fn need(self) -> Need {
        Need::flag(self.atom)
    }

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

/// A qualified permission handle with one specific scope vocabulary.
///
/// Its private identity is always fully qualified, while `S` makes the scope
/// values accepted by this permission visible to Rust. Different permissions
/// in one capability may therefore use unrelated scope types.
///
/// An unqualified declaration cannot be passed where this type is required:
///
/// ```compile_fail,E0308
/// extern crate alloc;
///
/// use lockgate_policy::{Scope, ScopeError, ScopeRepr, ScopedPermission};
///
/// #[derive(Clone, PartialEq, Eq)]
/// struct Project;
///
/// impl core::str::FromStr for Project {
///     type Err = ScopeError;
///     fn from_str(_: &str) -> Result<Self, Self::Err> { Ok(Self) }
/// }
/// impl ScopeRepr for Project {
///     fn canonical(&self) -> alloc::string::String { "project".into() }
/// }
/// impl Scope for Project {}
///
/// fn requires_qualified(_: ScopedPermission<Project>) {}
///
/// requires_qualified(ScopedPermission::<Project>::new("read"));
/// ```
pub struct ScopedPermission<S: Scope>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    atom: QualifiedAtom,
    marker: PhantomData<fn() -> S>,
}

impl<S: Scope> ScopedPermission<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    /// Declares a scoped permission ID for `#[lockgate::capability]`.
    ///
    /// This returns an inert declaration carrying `S`, not a permission with
    /// an empty or sentinel capability ID.
    #[allow(
        clippy::new_ret_no_self,
        reason = "the normative API makes new an intentionally inert declaration constructor"
    )]
    pub const fn new(permission: &'static str) -> ScopedPermissionDecl<S> {
        assert_valid_permission_id(permission);
        ScopedPermissionDecl {
            permission,
            marker: PhantomData,
        }
    }

    /// Requests this scoped permission over a non-empty union of references.
    ///
    /// References remain symbolic until host preparation. Their concrete
    /// values are deliberately not checked against `S` while authoring.
    pub const fn need(self, scopes: &'static [ScopeRef]) -> Need {
        Need::scoped(self.atom, scopes)
    }

    pub(crate) const fn atom(self) -> QualifiedAtom {
        self.atom
    }
}

impl<S: Scope> Clone for ScopedPermission<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    fn clone(&self) -> Self {
        *self
    }
}

impl<S: Scope> Copy for ScopedPermission<S> where <S as FromStr>::Err: Into<ScopeError> {}

impl<S: Scope> PartialEq for ScopedPermission<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    fn eq(&self, other: &Self) -> bool {
        self.atom == other.atom
    }
}

impl<S: Scope> Eq for ScopedPermission<S> where <S as FromStr>::Err: Into<ScopeError> {}

impl<S: Scope> Hash for ScopedPermission<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    fn hash<H: core::hash::Hasher>(&self, state: &mut H) {
        self.atom.hash(state);
    }
}

impl<S: Scope> fmt::Debug for ScopedPermission<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ScopedPermission")
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

/// Qualifies a scoped declaration for generated capability code.
#[doc(hidden)]
pub const fn qualify_scoped_permission<S: Scope>(
    capability: &'static str,
    declaration: ScopedPermissionDecl<S>,
) -> ScopedPermission<S>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    assert_valid_capability_id(capability);
    ScopedPermission {
        atom: match QualifiedAtom::new(capability, declaration.permission) {
            Ok(atom) => atom,
            Err(_) => panic!("Lockgate generated an invalid qualified scoped permission atom"),
        },
        marker: PhantomData,
    }
}

/// Copies the stable identity from a qualified unscoped permission.
#[doc(hidden)]
pub const fn permission_ids(permission: Permission) -> (&'static str, &'static str) {
    (permission.atom.capability(), permission.atom.permission())
}

/// Copies the stable identity from a qualified scoped permission.
#[doc(hidden)]
pub const fn scoped_permission_ids<S: Scope>(
    permission: ScopedPermission<S>,
) -> (&'static str, &'static str)
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    (permission.atom.capability(), permission.atom.permission())
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
    use alloc::{string::String, vec::Vec};

    #[derive(Clone, PartialEq, Eq)]
    struct PoolScope(String);

    impl FromStr for PoolScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            Ok(Self(value.into()))
        }
    }

    impl crate::ScopeRepr for PoolScope {
        fn canonical(&self) -> String {
            self.0.clone()
        }
    }

    impl Scope for PoolScope {}

    #[derive(Clone, PartialEq, Eq)]
    struct InstanceScope(String);

    impl FromStr for InstanceScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            Ok(Self(value.into()))
        }
    }

    impl crate::ScopeRepr for InstanceScope {
        fn canonical(&self) -> String {
            self.0.clone()
        }
    }

    impl Scope for InstanceScope {}

    const LIST_DECL: PermissionDecl = Permission::new("list-pools");
    const LIST: Permission = qualify_permission("vm", LIST_DECL);
    const CREATE_DECL: ScopedPermissionDecl<PoolScope> =
        ScopedPermission::<PoolScope>::new("create");
    const EXEC_DECL: ScopedPermissionDecl<InstanceScope> =
        ScopedPermission::<InstanceScope>::new("exec");
    const CREATE: ScopedPermission<PoolScope> = qualify_scoped_permission("vm", CREATE_DECL);
    const EXEC: ScopedPermission<InstanceScope> = qualify_scoped_permission("vm", EXEC_DECL);

    #[test]
    fn one_capability_keeps_each_permissions_scope_type() {
        fn accepts_pool(_: ScopedPermission<PoolScope>) {}
        fn accepts_instance(_: ScopedPermission<InstanceScope>) {}
        fn accepts_unscoped(_: Permission) {}

        accepts_pool(CREATE);
        accepts_instance(EXEC);
        accepts_unscoped(LIST);
    }

    #[test]
    fn qualification_retains_the_complete_stable_identity() {
        assert_eq!(LIST.atom().capability(), "vm");
        assert_eq!(LIST.atom().permission(), "list-pools");
        assert_eq!(CREATE.atom().permission(), "create");
        assert_eq!(EXEC.atom().permission(), "exec");
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

    #[test]
    fn scoped_types_support_derived_debug_without_debug_scopes() {
        #[allow(
            dead_code,
            reason = "the fields exercise every permission type's Debug implementation"
        )]
        #[derive(Debug)]
        struct DebuggablePermissions {
            unscoped_declaration: PermissionDecl,
            scoped_declaration: ScopedPermissionDecl<PoolScope>,
            unscoped: Permission,
            scoped: ScopedPermission<PoolScope>,
        }

        let debug = alloc::format!(
            "{:?}",
            DebuggablePermissions {
                unscoped_declaration: LIST_DECL,
                scoped_declaration: CREATE_DECL,
                unscoped: LIST,
                scoped: CREATE,
            }
        );
        assert!(debug.contains("ScopedPermissionDecl"));
        assert!(debug.contains("ScopedPermission"));
    }

    #[test]
    fn qualified_scoped_handles_are_copy_without_requiring_copy_scopes() {
        fn duplicate<T: Copy>(value: T) -> Vec<T> {
            alloc::vec![value; 2]
        }

        assert_eq!(duplicate(CREATE).len(), 2);
    }
}
