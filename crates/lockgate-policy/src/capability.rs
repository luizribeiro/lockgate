//! Capability contracts and type-erased permission descriptors.

use alloc::{boxed::Box, string::String};
use core::{any::Any, any::TypeId, any::type_name, error::Error, fmt, str::FromStr};

use crate::{Permission, Scope, ScopeError, ScopedPermission, check_scope_laws_for_registration};

type ParseScope = fn(&str) -> Result<ErasedScopeValue, ScopeError>;
type ScopeTypeName = fn() -> &'static str;
type CanonicalizeScope = fn(&ErasedScopeValue) -> Result<String, ErasedScopeTypeError>;
type IntersectScopes = fn(
    &ErasedScopeValue,
    &ErasedScopeValue,
) -> Result<Option<ErasedScopeValue>, ErasedScopeTypeError>;
type ValidateScopeLaws = fn() -> Result<(), ScopeError>;

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
/// permissions, the concrete type identity and every behavior required to
/// validate untrusted scope strings and perform scope algebra. Erasure changes
/// storage shape only; it does not weaken validation.
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

    /// Parses one wire string through the concrete scope implementation.
    ///
    /// `None` means this is an unscoped permission. A scoped permission returns
    /// `Some`, whose inner result reports untrusted-input parse failures.
    #[doc(hidden)]
    pub fn parse_scope(&self, value: &str) -> Option<Result<ErasedScopeValue, ScopeError>> {
        self.scope.map(|scope| (scope.parse)(value))
    }

    /// Canonicalizes a value previously returned by an erased scope hook.
    ///
    /// `None` means this is an unscoped permission. Mixing values from
    /// different scope types produces an explicit type error.
    #[doc(hidden)]
    pub fn canonicalize_scope(
        &self,
        value: &ErasedScopeValue,
    ) -> Option<Result<String, ErasedScopeTypeError>> {
        self.scope.map(|scope| (scope.canonicalize)(value))
    }

    /// Intersects two values through the concrete scope implementation.
    ///
    /// `None` at the outer layer means this permission is unscoped. The inner
    /// `Option` is the scope algebra's disjoint result.
    #[doc(hidden)]
    pub fn intersect_scopes(
        &self,
        left: &ErasedScopeValue,
        right: &ErasedScopeValue,
    ) -> Option<Result<Option<ErasedScopeValue>, ErasedScopeTypeError>> {
        self.scope.map(|scope| (scope.intersect)(left, right))
    }

    /// Runs exhaustive scope-law validation when the scope type is closed.
    ///
    /// `None` means this permission is unscoped. Open scope types return
    /// `Some(Ok(()))` because they carry no exhaustive-domain evidence.
    #[doc(hidden)]
    pub fn validate_scope_laws(&self) -> Option<Result<(), ScopeError>> {
        self.scope.map(|scope| (scope.validate_laws)())
    }
}

#[derive(Clone, Copy)]
struct ErasedScope {
    type_id: TypeId,
    type_name: ScopeTypeName,
    parse: ParseScope,
    canonicalize: CanonicalizeScope,
    intersect: IntersectScopes,
    validate_laws: ValidateScopeLaws,
}

/// One parsed concrete scope value behind a no-`std` type-erasure boundary.
///
/// Values can only be produced by an [`ErasedPermission`] scope hook, which
/// records the concrete `TypeId` alongside the boxed `core::any::Any` value.
#[doc(hidden)]
pub struct ErasedScopeValue {
    type_id: TypeId,
    type_name: &'static str,
    value: Box<dyn Any + Send + Sync>,
}

impl ErasedScopeValue {
    /// Returns the concrete type identity retained across erasure.
    pub const fn type_id(&self) -> TypeId {
        self.type_id
    }

    /// Returns the diagnostic concrete type name retained across erasure.
    pub const fn type_name(&self) -> &'static str {
        self.type_name
    }
}

/// A scope value was passed to hooks for a different concrete scope type.
///
/// This is a programmer-facing boundary error rather than an untrusted scope
/// parse error. Keeping it distinct prevents a cross-descriptor mix-up from
/// being mistaken for malformed manifest input.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ErasedScopeTypeError {
    expected: &'static str,
    actual: &'static str,
}

impl ErasedScopeTypeError {
    /// Returns the scope type expected by the descriptor hook.
    pub const fn expected(&self) -> &'static str {
        self.expected
    }

    /// Returns the scope type carried by the supplied erased value.
    pub const fn actual(&self) -> &'static str {
        self.actual
    }
}

impl fmt::Display for ErasedScopeTypeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "erased scope value has type `{}`, expected `{}`",
            self.actual, self.expected
        )
    }
}

impl Error for ErasedScopeTypeError {}

/// Erases a qualified unscoped permission for generated capability metadata.
#[doc(hidden)]
pub const fn erase_permission(permission: Permission) -> ErasedPermission {
    ErasedPermission {
        permission_id: permission.atom().permission(),
        scope: None,
    }
}

/// Erases a qualified scoped permission while retaining all scope behavior.
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
            parse: parse_scope::<S>,
            canonicalize: canonicalize_scope::<S>,
            intersect: intersect_scopes::<S>,
            validate_laws: check_scope_laws_for_registration::<S>,
        }),
    }
}

fn parse_scope<S>(value: &str) -> Result<ErasedScopeValue, ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    let value = value.parse::<S>().map_err(Into::into)?;
    Ok(ErasedScopeValue {
        type_id: TypeId::of::<S>(),
        type_name: type_name::<S>(),
        value: Box::new(value),
    })
}

fn canonicalize_scope<S>(value: &ErasedScopeValue) -> Result<String, ErasedScopeTypeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    value
        .value
        .downcast_ref::<S>()
        .map(crate::ScopeRepr::canonical)
        .ok_or_else(|| wrong_scope_type::<S>(value))
}

fn intersect_scopes<S>(
    left: &ErasedScopeValue,
    right: &ErasedScopeValue,
) -> Result<Option<ErasedScopeValue>, ErasedScopeTypeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    let left = left
        .value
        .downcast_ref::<S>()
        .ok_or_else(|| wrong_scope_type::<S>(left))?;
    let right = right
        .value
        .downcast_ref::<S>()
        .ok_or_else(|| wrong_scope_type::<S>(right))?;
    Ok(left.intersect(right).map(|value| ErasedScopeValue {
        type_id: TypeId::of::<S>(),
        type_name: type_name::<S>(),
        value: Box::new(value),
    }))
}

fn wrong_scope_type<S>(value: &ErasedScopeValue) -> ErasedScopeTypeError {
    ErasedScopeTypeError {
        expected: type_name::<S>(),
        actual: value.type_name,
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

    #[derive(Clone, PartialEq, Eq)]
    struct InstanceScope(String);

    impl FromStr for InstanceScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            Ok(Self(value.to_string()))
        }
    }

    impl ScopeRepr for InstanceScope {
        fn canonical(&self) -> String {
            self.0.clone()
        }
    }

    impl Scope for InstanceScope {}

    #[derive(Clone, PartialEq, Eq)]
    enum BrokenClosedScope {
        Only,
    }

    impl FromStr for BrokenClosedScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "only" => Ok(Self::Only),
                value => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for BrokenClosedScope {
        fn canonical(&self) -> String {
            "only".to_string()
        }

        fn exhaustive_domain() -> Option<crate::ExhaustiveScopeDomain<Self>> {
            Some(crate::ExhaustiveScopeDomain::__from_derive(alloc::vec![
                Self::Only,
            ]))
        }
    }

    impl Scope for BrokenClosedScope {
        fn contains(&self, _inner: &Self) -> bool {
            false
        }

        fn intersect(&self, _other: &Self) -> Option<Self> {
            None
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
    fn erased_metadata_retains_scope_identity_and_law_validation() {
        assert_eq!(Contract::ID, "vm");
        assert_eq!(Contract::permissions().len(), 2);
        assert_eq!(ERASED_EXEC.permission_id(), "exec");
        assert!(ERASED_EXEC.is_scoped());
        assert_eq!(ERASED_EXEC.scope_type_id(), Some(TypeId::of::<PoolScope>()));
        assert_eq!(
            ERASED_EXEC.scope_type_name(),
            Some(type_name::<PoolScope>())
        );
        assert_eq!(ERASED_EXEC.validate_scope_laws().unwrap(), Ok(()));

        assert_eq!(ERASED_LIST.permission_id(), "list");
        assert!(!ERASED_LIST.is_scoped());
        assert_eq!(ERASED_LIST.scope_type_id(), None);
        assert_eq!(ERASED_LIST.scope_type_name(), None);
        assert!(ERASED_LIST.parse_scope("anything").is_none());
    }

    #[test]
    fn erased_scope_hooks_round_trip_and_intersect_like_the_concrete_scope() {
        let descriptor = &Contract::permissions()[0];
        let broad = descriptor.parse_scope("*").unwrap().unwrap();
        let named = descriptor.parse_scope("gpu").unwrap().unwrap();

        assert_eq!(
            descriptor.canonicalize_scope(&named).unwrap().unwrap(),
            "gpu"
        );
        let erased_intersection = descriptor
            .intersect_scopes(&broad, &named)
            .unwrap()
            .unwrap()
            .unwrap();
        let concrete_intersection = PoolScope::Any
            .intersect(&PoolScope::Named("gpu".to_string()))
            .unwrap();
        assert_eq!(
            descriptor
                .canonicalize_scope(&erased_intersection)
                .unwrap()
                .unwrap(),
            concrete_intersection.canonical()
        );
    }

    #[test]
    fn erased_hooks_reject_values_from_another_scope_type() {
        const OTHER_DECL: ScopedPermissionDecl<InstanceScope> =
            ScopedPermission::<InstanceScope>::new("inspect");
        const OTHER: ScopedPermission<InstanceScope> = qualify_scoped_permission("vm", OTHER_DECL);
        const ERASED_OTHER: ErasedPermission = erase_scoped_permission(OTHER);

        let pool = ERASED_EXEC.parse_scope("gpu").unwrap().unwrap();
        let error = ERASED_OTHER.canonicalize_scope(&pool).unwrap().unwrap_err();

        assert_eq!(error.expected(), type_name::<InstanceScope>());
        assert_eq!(error.actual(), type_name::<PoolScope>());
    }

    #[test]
    fn erased_law_hook_propagates_closed_domain_failures() {
        const BROKEN_DECL: ScopedPermissionDecl<BrokenClosedScope> =
            ScopedPermission::<BrokenClosedScope>::new("broken");
        const BROKEN: ScopedPermission<BrokenClosedScope> =
            qualify_scoped_permission("vm", BROKEN_DECL);
        const ERASED_BROKEN: ErasedPermission = erase_scoped_permission(BROKEN);

        let error = ERASED_BROKEN.validate_scope_laws().unwrap().unwrap_err();

        assert!(
            error
                .to_string()
                .contains("containment reflexivity law violated")
        );
    }
}
