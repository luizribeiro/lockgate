extern crate alloc;
extern crate lockgate_policy as lockgate;

use lockgate_policy::{CapabilityContract, Permission, Scope, ScopedPermission, capability};

#[path = "fixtures/vm_contract.rs"]
mod vm_contract;

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

    #[cfg(any())]
    pub const DISABLED: Permission = Permission::new("disabled");

    #[cfg_attr(all(), deprecated)]
    pub const LEGACY: Permission = Permission::new("legacy");
}

#[capability("conditional")]
mod conditional {
    use super::Permission;

    #[cfg(any())]
    pub const FIRST: Permission = Permission::new("read");

    #[cfg(not(any()))]
    pub const SECOND: Permission = Permission::new("read");
}

#[test]
fn capability_rewrites_declarations_into_typed_qualified_handles() {
    fn accepts_scoped(_: ScopedPermission<documents::DocumentScope>) {}
    fn accepts_unscoped(_: Permission) {}

    accepts_scoped(documents::READ);
    accepts_unscoped(documents::LIST);

    assert_eq!(core::mem::size_of::<documents::Contract>(), 0);
    assert_eq!(documents::Contract::ID, "documents");
    let descriptors = documents::Contract::permissions();
    assert_eq!(descriptors.len(), 3);
    assert_eq!(descriptors[0].permission_id(), "read");
    assert!(descriptors[0].is_scoped());
    assert_eq!(descriptors[1].permission_id(), "list");
    assert!(!descriptors[1].is_scoped());
    assert_eq!(descriptors[2].permission_id(), "legacy");

    let conditional_descriptors = conditional::Contract::permissions();
    assert_eq!(conditional_descriptors.len(), 1);
    assert_eq!(conditional_descriptors[0].permission_id(), "read");
}

#[test]
fn canonical_vm_contract_compiles_through_the_policy_macro() {
    use vm_contract::permissions::vm;

    fn accepts_pool(_: ScopedPermission<vm::PoolScope>) {}
    fn accepts_instance(_: ScopedPermission<vm::InstanceScope>) {}

    accepts_pool(vm::CREATE);
    accepts_instance(vm::EXEC);
    accepts_instance(vm::DESTROY);
    assert_eq!(vm::Contract::ID, "vm");
    assert_eq!(vm::Contract::permissions().len(), 4);
    assert_eq!(vm::Contract::permissions()[3].permission_id(), "list-pools");
}
