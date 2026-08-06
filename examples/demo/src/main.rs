//! Runs the narrated end-to-end Lockgate prototype.

use anyhow::Result;
use std::path::Path;

mod demo;
mod policy;

fn main() -> Result<()> {
    demo::run(Path::new(env!("LOCKGATE_DEMO_ROOT")))
}
