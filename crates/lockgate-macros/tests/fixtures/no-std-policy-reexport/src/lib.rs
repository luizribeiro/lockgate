#![no_std]

use policy::ScopeRepr as _;

#[derive(Clone, Copy, PartialEq, Eq, policy::ScopeRepr)]
enum FixtureScope {
    All,
    #[scope(rename = "caller-current")]
    Current,
}

pub fn is_all_canonical() -> bool {
    FixtureScope::All.canonical() == "all"
}
