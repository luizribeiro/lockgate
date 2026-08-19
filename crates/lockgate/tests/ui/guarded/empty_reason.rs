#![allow(unused)]

mod support;

use lockgate::HostCtx;
use support::{Data, Imports, Target, vm};

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::no_capability_required(reason = "  ")]
    async fn action(
        &mut self,
        _cx: HostCtx<'_, Data>,
        _target: Target,
    ) -> Result<(), support::Error> {
        Ok(())
    }
}

fn main() {}
