#![allow(dead_code, unused_imports)]

use core::convert::Infallible;

use lockgate::{
    HostCtx, PermissionDenied, PluginSubject, ResolveScopedResource, Scope, ScopeRepr,
    ScopedResource,
};

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

#[derive(Clone)]
pub struct Target {
    pub id: String,
}

impl ScopedResource<TargetScope> for Target {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<TargetScope> {
        vec![TargetScope::All]
    }
}

impl ResolveScopedResource<TargetScope, Target> for Imports {
    type Resource = Target;
    type Error = Infallible;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        target: &'a Target,
    ) -> Result<Self::Resource, Self::Error> {
        Ok(target.clone())
    }
}

#[derive(Debug)]
pub struct Error;

impl From<Infallible> for Error {
    fn from(error: Infallible) -> Self {
        match error {}
    }
}

impl From<PermissionDenied> for Error {
    fn from(_: PermissionDenied) -> Self {
        Self
    }
}

#[derive(Debug)]
pub struct NoDenialError;

impl From<Infallible> for NoDenialError {
    fn from(error: Infallible) -> Self {
        match error {}
    }
}

pub mod vm {
    use super::{Data, Error, Imports, Target};
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
        ) -> impl core::future::Future<Output = Result<(), Error>> + Send;
    }

    const _: fn(Imports) = |_: Imports| {};
}

pub mod vm_no_denial_conversion {
    use super::{Data, Imports, NoDenialError, Target};
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
        ) -> impl core::future::Future<Output = Result<(), NoDenialError>> + Send;
    }
}
