use lockgate_policy::{Scope, ScopedPermission};

#[derive(Clone, PartialEq, Eq, lockgate_policy::ScopeRepr)]
enum Project {
    All,
}

impl Scope for Project {}

fn main() {
    let _ = ScopedPermission::<Project> {};
}

