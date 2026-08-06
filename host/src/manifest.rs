use serde::Deserialize;

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct Manifest {
    pub(crate) id: String,
    #[serde(default)]
    pub(crate) provides: Vec<String>,
    #[serde(default)]
    pub(crate) invokes: Vec<String>,
    pub(crate) capabilities: Capabilities,
}

#[derive(Clone, Debug, Default, Deserialize)]
pub(crate) struct Capabilities {
    #[serde(default)]
    pub(crate) registry: bool,
    pub(crate) fs: Option<FsCapability>,
    pub(crate) net: Option<NetCapability>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct FsCapability {
    #[serde(default)]
    pub(crate) read: Vec<String>,
    #[serde(default)]
    pub(crate) write: Vec<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub(crate) struct NetCapability {
    #[serde(default)]
    pub(crate) hosts: Vec<String>,
}

pub(crate) fn permitted(manifest: &Manifest, target: &str) -> bool {
    manifest
        .invokes
        .iter()
        .any(|pattern| glob_match::glob_match(pattern, target))
}
