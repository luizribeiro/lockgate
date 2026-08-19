#![allow(unused)]

mod support;

use lockgate::HostCtx;
use support::{
    Data, Imports, NoDenialError, Target, permissions, vm_no_denial_conversion as vm,
};

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = permissions::EXEC, target = target)]
    async fn action(
        &mut self,
        _cx: HostCtx<'_, Data>,
        target: Target,
    ) -> Result<(), NoDenialError> {
        Ok(())
    }
}

fn main() {}
