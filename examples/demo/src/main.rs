//! Runs the narrated end-to-end Lockgate prototype.

use anyhow::Result;
use std::path::Path;

fn main() -> Result<()> {
    lockgate::run_demo(Path::new(env!("LOCKGATE_DEMO_ROOT")))
}
