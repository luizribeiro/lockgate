#![allow(unused)]

mod support;

use lockgate::{HostCtx, PluginSubject, ResolveScopedResource};
use support::{Data, Imports, Target, TargetScope, permissions, vm};

struct ResolverFailure;

impl ResolveScopedResource<TargetScope, String> for Imports {
    type Resource = Target;
    type Error = ResolverFailure;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        target: &'a String,
    ) -> Result<Self::Resource, Self::Error> {
        Ok(Target { id: target.clone() })
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
