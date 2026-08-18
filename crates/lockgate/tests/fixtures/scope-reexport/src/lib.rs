use lg::ScopeRepr as _;

#[derive(Clone, Copy, PartialEq, Eq, lg::ScopeRepr)]
enum FixtureScope {
    All,
    Current,
}

impl lg::Scope for FixtureScope {}

pub fn canonical_all() -> String {
    FixtureScope::All.canonical()
}

pub fn check_laws() -> Result<(), lg::ScopeError> {
    lg::check_scope_laws([FixtureScope::All, FixtureScope::Current])
}
