mod support;

use lockgate::{HostCtx, Resource};
use support::{Imports, TargetScope, permissions};

enum Session {}

mod sessions {
    pub struct __LockgateSessionBinding;

    impl __LockgateSessionBinding {
        pub const INTERFACE: lockgate::__private::InterfaceIdentity =
            lockgate::__private::InterfaceIdentity::__new("test:ui/sessions", None);
        pub const __LOCKGATE_METHOD_SEND: lockgate::__private::MethodIdentity =
            lockgate::__private::MethodIdentity::__new("send", "[method]session.send");
    }

    pub trait HostSession {
        const __LOCKGATE_POLICY_METHODS: &'static [lockgate::__private::PolicyMethod] = &[];

        fn send(
            &mut self,
            cx: HostCtx<'_, ()>,
            session: Resource<super::Session>,
        ) -> impl core::future::Future<Output = Result<(), support::Error>> + Send;
    }

    use super::{HostCtx, Resource, support};
}

#[lockgate::guarded]
impl sessions::HostSession for Imports {
    #[lockgate::requires(permission = permissions::READ, target = session.rep)]
    async fn send(
        &mut self,
        _cx: HostCtx<'_, ()>,
        session: Resource<Session>,
    ) -> Result<(), support::Error> {
        let _ = session;
        Ok(())
    }
}

fn main() {
    let _ = core::marker::PhantomData::<TargetScope>;
    let _ = core::marker::PhantomData::<Imports>;
    let _ = permissions::READ;
}
