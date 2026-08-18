//! Typed scope representations and their narrowing policy.
//!
//! A scope vocabulary must be able to express sufficiently specific
//! membership witnesses for every authority distinction it exposes. Resources
//! report those concrete witnesses, and broader grants authorize them through
//! [`Scope::contains`]. An empty membership list is a deliberate fail-closed
//! exclusion, not a fallback for a resource that belongs to no narrower
//! category.

use alloc::{boxed::Box, string::String, vec::Vec};
use core::{error::Error, fmt, str::FromStr};

/// An invalid scope representation or scope-algebra implementation.
///
/// Parsing implementations can construct an unknown-value error with
/// [`ScopeError::unknown`]. Scope-law checking uses the same error type so
/// callers have one error boundary for scope definitions.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ScopeError {
    message: Box<str>,
}

impl ScopeError {
    /// Reports a wire value that this scope type does not recognize.
    pub fn unknown(value: impl Into<String>) -> Self {
        let value = value.into();
        Self {
            message: alloc::format!("unknown scope `{value}`").into(),
        }
    }
}

impl fmt::Display for ScopeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(&self.message)
    }
}

impl Error for ScopeError {}

/// Proof that the contained values exhaust a closed scope domain.
///
/// Closed domains are produced by [`ScopeRepr`] derives. Their private
/// representation prevents hand-written implementations from accidentally
/// claiming an incomplete domain through the supported API. The hidden
/// constructor is reserved for generated code.
#[doc(hidden)]
pub struct ExhaustiveScopeDomain<S> {
    values: Vec<S>,
}

impl<S> ExhaustiveScopeDomain<S> {
    /// Constructs exhaustive-domain evidence for generated `ScopeRepr` code.
    #[doc(hidden)]
    pub fn __from_derive(values: Vec<S>) -> Self {
        Self { values }
    }
}

impl<S> IntoIterator for ExhaustiveScopeDomain<S> {
    type Item = S;
    type IntoIter = alloc::vec::IntoIter<S>;

    fn into_iter(self) -> Self::IntoIter {
        self.values.into_iter()
    }
}

/// The representational half of a scoped permission's vocabulary.
///
/// A scope's canonical string is its stable wire representation in manifests,
/// grants, and limits. `canonical` deliberately does not require
/// [`Display`](core::fmt::Display): an application may already use `Display`
/// for human-readable text.
///
/// Closed enums should derive this trait with `#[derive(ScopeRepr)]` instead of
/// implementing it by hand. The derive owns both the variant mapping and
/// exhaustive-domain evidence, so adding a variant cannot silently leave the
/// domain stale. Open vocabularies implement the trait manually and retain the
/// default `None` evidence.
///
/// Derived wire names use lowercase ASCII kebab-case. Variant identifiers are
/// split at non-alphanumeric characters and at a lowercase-to-uppercase
/// transition. Consecutive uppercase characters remain one acronym, except
/// that the last uppercase character starts the next word when followed by
/// lowercase characters: `ReadOnly` becomes `read-only`, `UserID` becomes
/// `user-id`, and `XMLHttpRequest` becomes `xml-http-request`. This algorithm
/// is part of wire compatibility. Use `#[scope(rename = "stable-name")]` on a
/// variant before a Rust rename that would otherwise change its wire value.
pub trait ScopeRepr: Clone + Eq + FromStr + Send + Sync + 'static
where
    <Self as FromStr>::Err: Into<ScopeError>,
{
    /// Returns this value's stable wire representation.
    fn canonical(&self) -> String;

    /// Returns proof of every value for a closed scope type.
    ///
    /// Open types return `None`. Their implementations are trusted and should
    /// be validated over representative samples with `check_scope_laws`.
    #[doc(hidden)]
    fn exhaustive_domain() -> Option<ExhaustiveScopeDomain<Self>> {
        None
    }
}

/// The policy half of a scoped permission's vocabulary.
///
/// Scope policy forms a meet-semilattice with an implicit empty bottom:
///
/// - `intersect` is symmetric and idempotent;
/// - `Some(value)` is the unique greatest common subscope;
/// - `None` means that two values are disjoint;
/// - `outer.contains(inner)` is equivalent to
///   `outer.intersect(inner) == Some(inner)`;
/// - containment is reflexive, antisymmetric, and transitive; and
/// - parsing a canonical value returns the original value, while canonicalizing
///   parsed input produces its normalized wire form.
///
/// This is intentionally not a full lattice. Scope lists represent finite
/// unions, so a join operation on individual values would force every
/// vocabulary to invent values that duplicate the list wire format.
///
/// Implement this trait by hand whenever a scope type has policy beyond the
/// equal-or-disjoint default. Different semantics require distinct Rust types;
/// scope identity is the type's [`core::any::TypeId`].
pub trait Scope: ScopeRepr
where
    <Self as FromStr>::Err: Into<ScopeError>,
{
    /// Returns whether `self` contains `inner`.
    ///
    /// The default gives equal-or-disjoint semantics. Resource implementations
    /// must report membership witnesses specific enough to preserve every
    /// distinction this relation exposes. A broad scope authorizes a resource
    /// only when it contains at least one reported witness; empty membership
    /// therefore remains fail-closed.
    fn contains(&self, inner: &Self) -> bool {
        self == inner
    }

    /// Returns the greatest common subscope, or `None` when values are disjoint.
    ///
    /// This default is correct for nested-or-disjoint domains. A domain with
    /// diamond-shaped relationships must override it.
    fn intersect(&self, other: &Self) -> Option<Self> {
        if self.contains(other) {
            Some(other.clone())
        } else if other.contains(self) {
            Some(self.clone())
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use alloc::{
        string::{String, ToString},
        vec,
        vec::Vec,
    };
    use core::str::FromStr;

    use super::{Scope, ScopeError, ScopeRepr};

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ExactScope {
        Current,
        Created,
    }

    impl FromStr for ExactScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "current" => Ok(Self::Current),
                "created" => Ok(Self::Created),
                value => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for ExactScope {
        fn canonical(&self) -> String {
            match self {
                Self::Current => "current",
                Self::Created => "created",
            }
            .into()
        }
    }

    impl Scope for ExactScope {}

    #[test]
    fn exact_scopes_are_equal_or_disjoint_by_default() {
        assert!(ExactScope::Current.contains(&ExactScope::Current));
        assert!(!ExactScope::Current.contains(&ExactScope::Created));
        assert_eq!(
            ExactScope::Current.intersect(&ExactScope::Current),
            Some(ExactScope::Current)
        );
        assert_eq!(ExactScope::Current.intersect(&ExactScope::Created), None);
    }

    #[test]
    fn open_scope_representations_do_not_claim_exhaustive_domains() {
        assert!(ExactScope::exhaustive_domain().is_none());
    }

    #[test]
    fn unknown_scope_errors_name_the_wire_value() {
        let error = ExactScope::from_str("elsewhere").unwrap_err();
        assert_eq!(error.to_string(), "unknown scope `elsewhere`");
    }

    #[test]
    fn exhaustive_domains_preserve_generated_value_order() {
        let domain = super::ExhaustiveScopeDomain::__from_derive(vec![
            ExactScope::Current,
            ExactScope::Created,
        ]);
        assert_eq!(
            domain.into_iter().collect::<Vec<_>>(),
            vec![ExactScope::Current, ExactScope::Created]
        );
    }
}
