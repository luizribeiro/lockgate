#![allow(unused)]

mod support;

use lockgate::HostCtx;
use support::{Data, Error, Imports, Target, permissions, vm};

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = permissions::EXEC, target = cx.data().target)]
    async fn action(
        &mut self,
        cx: HostCtx<'_, Data>,
        _target: Target,
    ) -> Result<(), Error> {
        Ok(())
    }
}

fn main() {}
