#![allow(unused)]

mod support;

use lockgate::HostCtx;
use support::{Data, Imports, Target, permissions, vm};

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = permissions::EXEC)]
    async fn action(
        &mut self,
        _cx: HostCtx<'_, Data>,
        _target: Target,
    ) -> Result<(), support::Error> {
        Ok(())
    }
}

fn main() {}
