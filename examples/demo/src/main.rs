//! Runs the narrated end-to-end Lockgate prototype.

use anyhow::Result;

fn main() -> Result<()> {
    lockgate::run_demo()
}
