#![allow(unused)]

mod support;

use lockgate::HostCtx;
use support::{Data, Imports, Target, vm};

#[lockgate::guarded]
impl vm::Host for Imports {
    async fn action(&mut self, _cx: HostCtx<'_, Data>, _target: Target) {}
}

fn main() {}
