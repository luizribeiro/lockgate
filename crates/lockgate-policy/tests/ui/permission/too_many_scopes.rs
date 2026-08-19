use lockgate_policy::{Need, Scope, ScopeRef, ScopedPermission};

#[lockgate_policy::capability("documents")]
mod documents {
    use super::{Scope, ScopedPermission};

    #[derive(Clone, Copy, PartialEq, Eq, lockgate_policy::ScopeRepr)]
    pub enum DocumentScope {
        All,
    }

    impl Scope for DocumentScope {}

    pub const READ: ScopedPermission<DocumentScope> = ScopedPermission::new("read");
}

const SCOPES: &[ScopeRef] = &[ScopeRef::literal("project"); 129];
const TOO_MANY: Need = documents::READ.need(SCOPES);

fn main() {}

