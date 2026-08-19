#![allow(unused)]

mod support;

use core::convert::Infallible;
use lockgate::{
    HostCtx, PluginSubject, ResolveScopedResource, Scope, ScopeRepr, ScopedResource,
};
use support::{Data, Imports, Target, TargetScope, permissions, vm};

#[derive(Clone, PartialEq, Eq, ScopeRepr)]
enum OtherScope {
    Other,
}

impl Scope for OtherScope {}

struct WrongResource;

impl ScopedResource<OtherScope> for WrongResource {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<OtherScope> {
        vec![OtherScope::Other]
    }
}

impl ResolveScopedResource<TargetScope, String> for Imports {
    type Resource = WrongResource;
    type Error = Infallible;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        _target: &'a String,
    ) -> Result<Self::Resource, Self::Error> {
        Ok(WrongResource)
    }
}

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = permissions::EXEC, target = target.id)]
    async fn action(
        &mut self,
        _cx: HostCtx<'_, Data>,
        target: Target,
    ) -> Result<(), support::Error> {
        Ok(())
    }
}

fn main() {}
