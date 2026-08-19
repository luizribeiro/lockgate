#![allow(dead_code, unused_imports)]

use lockgate::{HostCtx, Scope, ScopeRepr};

#[derive(Clone, PartialEq, Eq, ScopeRepr)]
pub enum TargetScope {
    All,
}

impl Scope for TargetScope {}

#[lockgate::capability("vm")]
pub mod permissions {
    use super::TargetScope;
    use lockgate::{Permission, ScopedPermission};

    pub const EXEC: ScopedPermission<TargetScope> = ScopedPermission::new("exec");
    pub const LIST: Permission = Permission::new("list");
}

#[derive(Clone)]
pub struct Imports;

pub struct Data {
    pub target: Target,
}

pub struct Target {
    pub id: String,
}

pub mod vm {
    use super::{Data, Imports, Target};
    use lockgate::HostCtx;

    pub struct __LockgateBinding;

    impl __LockgateBinding {
        pub const INTERFACE: lockgate::__private::InterfaceIdentity =
            lockgate::__private::InterfaceIdentity::__new("test:guarded/vm", None);
        pub const __LOCKGATE_METHOD_ACTION: lockgate::__private::MethodIdentity =
            lockgate::__private::MethodIdentity::__new("action", "action");
    }

    pub trait Host: Send {
        const __LOCKGATE_POLICY_METHODS: &'static [lockgate::__private::PolicyMethod] = &[];

        fn action(
            &mut self,
            cx: HostCtx<'_, Data>,
            target: Target,
        ) -> impl core::future::Future<Output = ()> + Send;
    }

    const _: fn(Imports) = |_: Imports| {};
}
