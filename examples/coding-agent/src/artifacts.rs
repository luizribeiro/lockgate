//! Locates plugin artifacts staged by the example build script.

use anyhow::{Context, Result};
use std::{env, fs, path::Path};

pub(crate) fn provider_bytes() -> Result<Vec<u8>> {
    match env::var_os("LOCKGATE_PROVIDER_COMPONENT") {
        Some(path) => {
            let path = Path::new(&path);
            fs::read(path)
                .with_context(|| format!("failed to read provider plugin {}", path.display()))
        }
        None => component_bytes("inkling-provider"),
    }
}

pub(crate) fn component_bytes(name: &str) -> Result<Vec<u8>> {
    let path = Path::new(env!("LOCKGATE_CODING_AGENT_ROOT"))
        .join("plugins")
        .join(name)
        .join(format!("{name}.wasm"));
    fs::read(&path).with_context(|| format!("failed to read plugin {}", path.display()))
}
