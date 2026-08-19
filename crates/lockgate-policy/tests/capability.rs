use lockgate_policy::{Permission, Scope, ScopedPermission, capability};

#[capability("documents")]
mod documents {
    use super::{Permission, Scope, ScopedPermission};

    #[derive(Clone, Copy, PartialEq, Eq, lockgate_policy::ScopeRepr)]
    pub enum DocumentScope {
        All,
    }

    impl Scope for DocumentScope {}

    /// Read documents in an allowed scope.
    pub const READ: ScopedPermission<DocumentScope> = ScopedPermission::new("read");

    /// List visible documents.
    pub const LIST: Permission = Permission::new("list");
}

#[test]
fn capability_rewrites_declarations_into_typed_qualified_handles() {
    fn accepts_scoped(_: ScopedPermission<documents::DocumentScope>) {}
    fn accepts_unscoped(_: Permission) {}

    accepts_scoped(documents::READ);
    accepts_unscoped(documents::LIST);
}
