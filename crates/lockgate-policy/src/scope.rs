//! Typed scope representations and their narrowing policy.
//!
//! A scope vocabulary must be able to express sufficiently specific
//! membership witnesses for every authority distinction it exposes. Resources
//! report those concrete witnesses, and broader grants authorize them through
//! [`Scope::contains`]. An empty membership list is a deliberate fail-closed
//! exclusion, not a fallback for a resource that belongs to no narrower
//! category.

use alloc::{boxed::Box, format, string::String, vec::Vec};
use core::{error::Error, fmt, str::FromStr};

const MAX_EXHAUSTIVE_SCOPE_VALUES: usize = 256;

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

    fn law_violation(message: impl Into<String>) -> Self {
        Self {
            message: message.into().into(),
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

    fn values(&self) -> &[S] {
        &self.values
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

/// Checks the scope laws over representative values from an open scope type.
///
/// The supplied values must be unique. Every law is checked over their full
/// Cartesian product (and, for transitivity and associativity, product of
/// triples), so callers should keep sample sets intentionally small. The check
/// proves the laws only for the supplied values; open types remain trusted over
/// values outside the sample.
///
/// Closed types expose exhaustive-domain evidence through
/// [`ScopeRepr::exhaustive_domain`] and are checked by Lockgate when registered.
/// Applications can use this helper in their own test suites for open types:
///
/// ```
/// extern crate alloc;
///
/// use alloc::string::String;
/// use core::str::FromStr;
/// use lockgate_policy::{Scope, ScopeError, ScopeRepr, check_scope_laws};
///
/// #[derive(Clone, PartialEq, Eq)]
/// struct Tenant(String);
///
/// impl FromStr for Tenant {
///     type Err = ScopeError;
///
///     fn from_str(value: &str) -> Result<Self, Self::Err> {
///         if value.is_empty() {
///             Err(ScopeError::unknown(value))
///         } else {
///             Ok(Self(value.into()))
///         }
///     }
/// }
///
/// impl ScopeRepr for Tenant {
///     fn canonical(&self) -> String { self.0.clone() }
/// }
/// impl Scope for Tenant {}
///
/// check_scope_laws::<Tenant>([
///     Tenant("engineering".into()),
///     Tenant("support".into()),
/// ])?;
/// # Ok::<(), ScopeError>(())
/// ```
pub fn check_scope_laws<S>(samples: impl IntoIterator<Item = S>) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    check_scope_laws_inner(&samples.into_iter().collect::<Vec<_>>())
}

/// Checks every value of a closed scope domain before registration.
///
/// Open scope domains have no exhaustive evidence and return `Ok(())`; their
/// applications remain responsible for property testing representative values.
#[doc(hidden)]
pub fn check_scope_laws_for_registration<S>() -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    let Some(domain) = S::exhaustive_domain() else {
        return Ok(());
    };
    check_exhaustive_domain(&domain)
}

fn check_exhaustive_domain<S>(domain: &ExhaustiveScopeDomain<S>) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    let scopes = domain.values();
    if scopes.len() > MAX_EXHAUSTIVE_SCOPE_VALUES {
        return Err(ScopeError::law_violation(format!(
            "exhaustive scope domain has {} values, exceeding the maximum of {} checked before quadratic and cubic scope-law checks",
            scopes.len(),
            MAX_EXHAUSTIVE_SCOPE_VALUES
        )));
    }
    check_scope_laws_inner(scopes)?;
    for left in scopes {
        for right in scopes {
            let Some(meet) = left.intersect(right) else {
                continue;
            };
            if !scopes.contains(&meet) {
                return Err(ScopeError::law_violation(format!(
                    "intersection closure law violated: `intersect({}, {})` returned {}, which is absent from the exhaustive scope domain",
                    name(left),
                    name(right),
                    intersection_name(Some(&meet))
                )));
            }
        }
    }
    Ok(())
}

fn check_scope_laws_inner<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    check_sample_uniqueness(samples)?;
    check_canonical_round_trips(samples)?;
    check_reflexivity_and_idempotence(samples)?;
    check_symmetry(samples)?;
    check_contains_agreement(samples)?;
    check_antisymmetry(samples)?;
    check_transitivity(samples)?;
    check_unique_meets(samples)?;
    check_associativity(samples)
}

fn check_sample_uniqueness<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for (left_index, left) in samples.iter().enumerate() {
        for (right_index, right) in samples.iter().enumerate().skip(left_index + 1) {
            if left == right {
                return Err(ScopeError::law_violation(format!(
                    "enumeration uniqueness law violated: samples at indices {left_index} and {right_index} are both {}",
                    name(left)
                )));
            }
        }
    }
    Ok(())
}

fn check_canonical_round_trips<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for sample in samples {
        let canonical = sample.canonical();
        let parsed = canonical.parse::<S>().map_err(|error| {
            let error: ScopeError = error.into();
            ScopeError::law_violation(format!(
                "canonical round-trip law violated for {}: canonical value `{canonical}` failed to parse: {error}",
                name(sample)
            ))
        })?;
        if parsed != *sample {
            return Err(ScopeError::law_violation(format!(
                "canonical round-trip law violated for {}: parsing `{canonical}` produced {}",
                name(sample),
                name(&parsed)
            )));
        }
    }
    Ok(())
}

fn check_reflexivity_and_idempotence<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for sample in samples {
        if !sample.contains(sample) {
            return Err(ScopeError::law_violation(format!(
                "containment reflexivity law violated: `contains({0}, {0})` is false",
                name(sample)
            )));
        }

        let intersection = sample.intersect(sample);
        if intersection.as_ref() != Some(sample) {
            return Err(ScopeError::law_violation(format!(
                "intersection idempotence law violated: `intersect({0}, {0})` returned {1}, expected `Some({0})`",
                name(sample),
                intersection_name(intersection.as_ref())
            )));
        }
    }
    Ok(())
}

fn check_symmetry<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for left in samples {
        for right in samples {
            let forward = left.intersect(right);
            let reverse = right.intersect(left);
            if forward != reverse {
                return Err(ScopeError::law_violation(format!(
                    "intersection symmetry law violated: `intersect({}, {})` returned {} but `intersect({}, {})` returned {}",
                    name(left),
                    name(right),
                    intersection_name(forward.as_ref()),
                    name(right),
                    name(left),
                    intersection_name(reverse.as_ref())
                )));
            }
        }
    }
    Ok(())
}

fn check_contains_agreement<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for outer in samples {
        for inner in samples {
            let contains = outer.contains(inner);
            let intersection = outer.intersect(inner);
            let agrees = intersection.as_ref() == Some(inner);
            if contains != agrees {
                return Err(ScopeError::law_violation(format!(
                    "containment/intersection agreement law violated: `contains({}, {})` is {contains} but `intersect({}, {})` returned {} — these must agree",
                    name(outer),
                    name(inner),
                    name(outer),
                    name(inner),
                    intersection_name(intersection.as_ref())
                )));
            }
        }
    }
    Ok(())
}

fn check_antisymmetry<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for left in samples {
        for right in samples {
            if left != right && left.contains(right) && right.contains(left) {
                return Err(ScopeError::law_violation(format!(
                    "containment antisymmetry law violated: {} and {} contain each other but are distinct values",
                    name(left),
                    name(right)
                )));
            }
        }
    }
    Ok(())
}

fn check_transitivity<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for outer in samples {
        for middle in samples {
            for inner in samples {
                if outer.contains(middle) && middle.contains(inner) && !outer.contains(inner) {
                    return Err(ScopeError::law_violation(format!(
                        "containment transitivity law violated: {} contains {} and {} contains {}, but {} does not contain {}",
                        name(outer),
                        name(middle),
                        name(middle),
                        name(inner),
                        name(outer),
                        name(inner)
                    )));
                }
            }
        }
    }
    Ok(())
}

fn check_unique_meets<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for left in samples {
        for right in samples {
            let intersection = left.intersect(right);
            match intersection.as_ref() {
                Some(meet) => {
                    if !left.contains(meet) || !right.contains(meet) {
                        return Err(ScopeError::law_violation(format!(
                            "greatest-common-subscope law violated: `intersect({}, {})` returned {}, which is not a common subscope",
                            name(left),
                            name(right),
                            intersection_name(Some(meet))
                        )));
                    }

                    for candidate in samples {
                        if left.contains(candidate)
                            && right.contains(candidate)
                            && !meet.contains(candidate)
                        {
                            return Err(ScopeError::law_violation(format!(
                                "unique greatest-common-subscope law violated: `intersect({}, {})` returned {}, but common subscope {} is not contained by it",
                                name(left),
                                name(right),
                                intersection_name(Some(meet)),
                                name(candidate)
                            )));
                        }
                    }
                }
                None => {
                    if let Some(candidate) = samples
                        .iter()
                        .find(|candidate| left.contains(candidate) && right.contains(candidate))
                    {
                        return Err(ScopeError::law_violation(format!(
                            "unique greatest-common-subscope law violated: `intersect({}, {})` returned `None`, but {} is a common subscope — `None` must mean disjoint",
                            name(left),
                            name(right),
                            name(candidate)
                        )));
                    }
                }
            }
        }
    }
    Ok(())
}

fn check_associativity<S>(samples: &[S]) -> Result<(), ScopeError>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    for first in samples {
        for second in samples {
            for third in samples {
                let left_pair = first.intersect(second);
                let left = left_pair.as_ref().and_then(|meet| meet.intersect(third));
                let right_pair = second.intersect(third);
                let right = right_pair.as_ref().and_then(|meet| first.intersect(meet));
                if left != right {
                    return Err(ScopeError::law_violation(format!(
                        "intersection associativity law violated for {}, {}, {}: `(first ∩ second) ∩ third` returned {} but `first ∩ (second ∩ third)` returned {}",
                        name(first),
                        name(second),
                        name(third),
                        intersection_name(left.as_ref()),
                        intersection_name(right.as_ref())
                    )));
                }
            }
        }
    }
    Ok(())
}

fn name<S>(scope: &S) -> String
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    scope.canonical()
}

fn intersection_name<S>(scope: Option<&S>) -> String
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    match scope {
        Some(scope) => format!("`Some({})`", name(scope)),
        None => String::from("`None`"),
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

    use super::{
        ExhaustiveScopeDomain, Scope, ScopeError, ScopeRepr, check_scope_laws,
        check_scope_laws_for_registration,
    };

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

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum BrokenAgreementScope {
        All,
        Current,
    }

    impl FromStr for BrokenAgreementScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "all" => Ok(Self::All),
                "current" => Ok(Self::Current),
                value => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for BrokenAgreementScope {
        fn canonical(&self) -> String {
            match self {
                Self::All => "all",
                Self::Current => "current",
            }
            .into()
        }

        fn exhaustive_domain() -> Option<ExhaustiveScopeDomain<Self>> {
            Some(ExhaustiveScopeDomain::__from_derive(vec![
                Self::All,
                Self::Current,
            ]))
        }
    }

    impl Scope for BrokenAgreementScope {
        fn contains(&self, inner: &Self) -> bool {
            self == inner || matches!(self, Self::All)
        }

        fn intersect(&self, other: &Self) -> Option<Self> {
            (self == other).then(|| self.clone())
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum DiamondScope {
        All,
        Left,
        Right,
        Bottom,
    }

    impl FromStr for DiamondScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "all" => Ok(Self::All),
                "left" => Ok(Self::Left),
                "right" => Ok(Self::Right),
                "bottom" => Ok(Self::Bottom),
                value => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for DiamondScope {
        fn canonical(&self) -> String {
            match self {
                Self::All => "all",
                Self::Left => "left",
                Self::Right => "right",
                Self::Bottom => "bottom",
            }
            .into()
        }
    }

    impl Scope for DiamondScope {
        fn contains(&self, inner: &Self) -> bool {
            self == inner
                || matches!(self, Self::All)
                || matches!((self, inner), (Self::Left | Self::Right, Self::Bottom))
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum TransitivityScope {
        Outer,
        Middle,
        Inner,
    }

    impl FromStr for TransitivityScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "outer" => Ok(Self::Outer),
                "middle" => Ok(Self::Middle),
                "inner" => Ok(Self::Inner),
                value => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for TransitivityScope {
        fn canonical(&self) -> String {
            match self {
                Self::Outer => "outer",
                Self::Middle => "middle",
                Self::Inner => "inner",
            }
            .into()
        }
    }

    impl Scope for TransitivityScope {
        fn contains(&self, inner: &Self) -> bool {
            self == inner
                || matches!(
                    (self, inner),
                    (Self::Outer, Self::Middle) | (Self::Middle, Self::Inner)
                )
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct OversizedScope(u16);

    impl FromStr for OversizedScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            value
                .parse()
                .map(Self)
                .map_err(|_| ScopeError::unknown(value))
        }
    }

    impl ScopeRepr for OversizedScope {
        fn canonical(&self) -> String {
            panic!("the count guard must run before scope-law checks")
        }

        fn exhaustive_domain() -> Option<ExhaustiveScopeDomain<Self>> {
            Some(ExhaustiveScopeDomain::__from_derive(
                (0..=256).map(Self).collect(),
            ))
        }
    }

    impl Scope for OversizedScope {}

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct MaximumSizeScope(u16);

    impl FromStr for MaximumSizeScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            value
                .parse()
                .map(Self)
                .map_err(|_| ScopeError::unknown(value))
        }
    }

    impl ScopeRepr for MaximumSizeScope {
        fn canonical(&self) -> String {
            self.0.to_string()
        }

        fn exhaustive_domain() -> Option<ExhaustiveScopeDomain<Self>> {
            Some(ExhaustiveScopeDomain::__from_derive(
                (0..256).map(Self).collect(),
            ))
        }
    }

    impl Scope for MaximumSizeScope {}

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum EscapingMeetScope {
        Left,
        Right,
        Bottom,
    }

    impl FromStr for EscapingMeetScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "left" => Ok(Self::Left),
                "right" => Ok(Self::Right),
                "bottom" => Ok(Self::Bottom),
                value => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for EscapingMeetScope {
        fn canonical(&self) -> String {
            match self {
                Self::Left => "left",
                Self::Right => "right",
                Self::Bottom => "bottom",
            }
            .into()
        }

        fn exhaustive_domain() -> Option<ExhaustiveScopeDomain<Self>> {
            Some(ExhaustiveScopeDomain::__from_derive(vec![
                Self::Left,
                Self::Right,
            ]))
        }
    }

    impl Scope for EscapingMeetScope {
        fn contains(&self, inner: &Self) -> bool {
            self == inner || matches!((self, inner), (Self::Left | Self::Right, Self::Bottom))
        }

        fn intersect(&self, other: &Self) -> Option<Self> {
            if self == other {
                Some(self.clone())
            } else {
                Some(Self::Bottom)
            }
        }
    }

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

    #[test]
    fn exact_scope_samples_satisfy_the_full_law_suite() {
        check_scope_laws([ExactScope::Current, ExactScope::Created]).unwrap();
    }

    #[test]
    fn agreement_errors_explain_both_conflicting_answers() {
        let error = check_scope_laws([BrokenAgreementScope::All, BrokenAgreementScope::Current])
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "containment/intersection agreement law violated: `contains(all, current)` is true but `intersect(all, current)` returned `None` — these must agree"
        );
    }

    #[test]
    fn registration_runs_the_full_law_suite_for_exhaustive_domains() {
        let error = check_scope_laws_for_registration::<BrokenAgreementScope>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "containment/intersection agreement law violated: `contains(all, current)` is true but `intersect(all, current)` returned `None` — these must agree"
        );
    }

    #[test]
    fn unique_meet_errors_name_the_hidden_common_subscope() {
        let error = check_scope_laws([
            DiamondScope::All,
            DiamondScope::Left,
            DiamondScope::Right,
            DiamondScope::Bottom,
        ])
        .unwrap_err();
        let message = error.to_string();
        assert!(message.contains("unique greatest-common-subscope law violated"));
        assert!(message.contains("`intersect(left, right)` returned `None`"));
        assert!(message.contains("bottom is a common subscope"));
    }

    #[test]
    fn transitivity_errors_name_the_entire_broken_chain() {
        let error = check_scope_laws([
            TransitivityScope::Outer,
            TransitivityScope::Middle,
            TransitivityScope::Inner,
        ])
        .unwrap_err();
        assert_eq!(
            error.to_string(),
            "containment transitivity law violated: outer contains middle and middle contains inner, but outer does not contain inner"
        );
    }

    #[test]
    fn duplicate_sample_errors_name_both_indices_and_the_value() {
        let error = check_scope_laws([ExactScope::Current, ExactScope::Current]).unwrap_err();
        assert_eq!(
            error.to_string(),
            "enumeration uniqueness law violated: samples at indices 0 and 1 are both current"
        );
    }

    #[test]
    fn exhaustive_domain_size_is_bounded_before_law_checks() {
        let error = check_scope_laws_for_registration::<OversizedScope>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "exhaustive scope domain has 257 values, exceeding the maximum of 256 checked before quadratic and cubic scope-law checks"
        );
    }

    #[test]
    fn exhaustive_domain_at_the_size_limit_passes_registration() {
        check_scope_laws_for_registration::<MaximumSizeScope>().unwrap();
    }

    #[test]
    fn exhaustive_domains_must_include_every_pairwise_meet() {
        check_scope_laws([EscapingMeetScope::Left, EscapingMeetScope::Right]).unwrap();

        let error = check_scope_laws_for_registration::<EscapingMeetScope>().unwrap_err();
        assert_eq!(
            error.to_string(),
            "intersection closure law violated: `intersect(left, right)` returned `Some(bottom)`, which is absent from the exhaustive scope domain"
        );
    }
}
