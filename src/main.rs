//! Temporary binary entry point for the narrated Lockgate prototype.

use anyhow::Result;

fn main() -> Result<()> {
    lockgate::run_demo()
}
