//! Locates and reads component artifacts staged by the demo's build script.

use anyhow::{Context, Result};
use std::{
    fs,
    path::{Path, PathBuf},
};

pub(crate) fn root() -> Result<PathBuf> {
    Path::new(env!("LOCKGATE_DEMO_ROOT"))
        .canonicalize()
        .context("failed to resolve the staged demo root")
}

pub(crate) fn component_bytes(name: &str) -> Result<Vec<u8>> {
    let path = root()?
        .join("components")
        .join(name)
        .join(format!("{name}.wasm"));
    fs::read(&path).with_context(|| format!("failed to read {}", path.display()))
}
