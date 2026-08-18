use core::str::FromStr;

use lockgate_policy::{Scope, ScopeError, ScopeRepr, check_scope_laws_for_registration};

#[allow(clippy::upper_case_acronyms)]
#[derive(Clone, Copy, Debug, PartialEq, Eq, lockgate_policy::ScopeRepr)]
enum WireScope {
    ReadOnly,
    HTTPServer,
    XMLHttpRequest,
    UserID,
    #[scope(rename = "stable-name")]
    RenamedInRust,
}

impl Scope for WireScope {}

#[test]
fn derived_wire_names_match_the_compatibility_snapshot() {
    let snapshot = WireScope::exhaustive_domain()
        .unwrap()
        .into_iter()
        .map(|scope| format!("{scope:?} = {}", scope.canonical()))
        .collect::<Vec<_>>()
        .join("\n");

    assert_eq!(
        format!("{snapshot}\n"),
        include_str!("snapshots/scope_repr_wire_names.snap")
    );
}

#[test]
fn derived_parsing_round_trips_every_enumerated_value() {
    for scope in WireScope::exhaustive_domain().unwrap() {
        assert_eq!(WireScope::from_str(&scope.canonical()).unwrap(), scope);
    }

    let error = WireScope::from_str("renamed-in-rust").unwrap_err();
    assert_eq!(error, ScopeError::unknown("renamed-in-rust"));
}

#[test]
fn derived_exhaustive_domains_pass_registration_law_checks() {
    check_scope_laws_for_registration::<WireScope>().unwrap();
}
