#![allow(unused)]

mod support;

use core::convert::Infallible;
use lockgate::{HostCtx, PluginSubject, ResolveScopedResource};
use support::{Data, Error, Imports, Target, TargetScope, permissions, vm};

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = permissions::EXEC, target = __lockgate_subject)]
    async fn action(
        &mut self,
        __lockgate_resource: HostCtx<'_, Data>,
        __lockgate_subject: Target,
    ) -> Result<(), Error> {
        Ok(())
    }
}

#[derive(Clone)]
struct OtherImports;

impl ResolveScopedResource<TargetScope, Target> for OtherImports {
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

#[lockgate::guarded]
impl vm::Host for OtherImports {
    #[lockgate::requires(permission = permissions::EXEC, target = target)]
    async fn action(
        &mut self,
        __lockgate_target: HostCtx<'_, Data>,
        target: Target,
    ) -> Result<(), Error> {
        Ok(())
    }
}

fn main() {}
