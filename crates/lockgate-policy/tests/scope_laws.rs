use core::str::FromStr;

use lockgate_policy::{
    Scope, ScopeError, ScopeRepr, check_scope_laws, check_scope_laws_for_registration,
};
use proptest::{collection::btree_set, prelude::*};

/// Policy categories are disjoint below the top scope, even if runtime session
/// membership can independently satisfy both narrower categories.
#[derive(Clone, Copy, Debug, PartialEq, Eq, lockgate_policy::ScopeRepr)]
enum SessionScope {
    All,
    Current,
    Created,
}

impl Scope for SessionScope {
    fn contains(&self, inner: &Self) -> bool {
        self == inner || matches!(self, Self::All)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, lockgate_policy::ScopeRepr)]
enum ExactClosedScope {
    Alpha,
    #[scope(rename = "stable-beta")]
    Beta,
}

impl Scope for ExactClosedScope {}

#[derive(Clone, Copy, Debug, PartialEq, Eq, lockgate_policy::ScopeRepr)]
enum DiamondScope {
    All,
    Left,
    Right,
    Bottom,
}

impl Scope for DiamondScope {
    fn contains(&self, inner: &Self) -> bool {
        self == inner
            || matches!(self, Self::All)
            || matches!((self, inner), (Self::Left | Self::Right, Self::Bottom))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
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
            value => Ok(Self::Named(value.to_owned())),
        }
    }
}

impl ScopeRepr for PoolScope {
    fn canonical(&self) -> String {
        match self {
            Self::Any => "*".to_owned(),
            Self::Named(name) => name.clone(),
        }
    }
}

impl Scope for PoolScope {
    fn contains(&self, inner: &Self) -> bool {
        self == inner || matches!(self, Self::Any)
    }
}

fn pool_name() -> impl Strategy<Value = String> {
    any::<String>().prop_filter("pool names are non-empty and `*` is reserved", |name| {
        !name.is_empty() && name != "*"
    })
}

#[test]
fn hierarchical_session_scopes_satisfy_every_scope_law() {
    let samples = SessionScope::exhaustive_domain().unwrap();
    check_scope_laws::<SessionScope>(samples).unwrap();
    check_scope_laws_for_registration::<SessionScope>().unwrap();

    assert_eq!(
        SessionScope::All.intersect(&SessionScope::Current),
        Some(SessionScope::Current)
    );
    assert_eq!(
        SessionScope::All.intersect(&SessionScope::Created),
        Some(SessionScope::Created)
    );
    assert_eq!(
        SessionScope::Current.intersect(&SessionScope::Created),
        None
    );
}

#[test]
fn derived_exact_enum_satisfies_every_scope_law() {
    check_scope_laws_for_registration::<ExactClosedScope>().unwrap();
}

#[test]
fn derived_diamond_proves_the_full_suite_checks_unique_meets() {
    let error = check_scope_laws_for_registration::<DiamondScope>().unwrap_err();
    assert_eq!(
        error.to_string(),
        "unique greatest-common-subscope law violated: `intersect(left, right)` returned `None`, but bottom is a common subscope — `None` must mean disjoint"
    );
}

#[test]
fn open_pool_scope_rejects_the_empty_name() {
    assert_eq!(
        PoolScope::from_str("").unwrap_err(),
        ScopeError::unknown("")
    );
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(128))]

    #[test]
    fn open_pool_scope_parses_and_canonicalizes_arbitrary_names(name in pool_name()) {
        let scope = PoolScope::from_str(&name).unwrap();
        prop_assert_eq!(&scope, &PoolScope::Named(name.clone()));
        prop_assert_eq!(scope.canonical(), name);
    }

    #[test]
    fn open_pool_scope_generated_values_satisfy_laws_and_round_trips(
        names in btree_set(pool_name(), 1..12)
    ) {
        let mut samples = vec![PoolScope::Any];
        samples.extend(names.into_iter().map(PoolScope::Named));

        check_scope_laws(samples.clone()).unwrap();
        for scope in samples {
            let canonical = scope.canonical();
            prop_assert_eq!(PoolScope::from_str(&canonical).unwrap(), scope);
        }
    }
}
