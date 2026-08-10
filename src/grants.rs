//! Private capability grants retained by an application until runtime construction.

use crate::catalog::ComponentId;
use std::path::{Path, PathBuf};

#[derive(Default)]
pub(crate) struct Grants {
    pub(crate) links: Vec<LinkGrant>,
    pub(crate) host_imports: Vec<HostImportGrant>,
    pub(crate) directories: Vec<DirectoryGrant>,
    pub(crate) outbound_http: Vec<OutboundHttpGrant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct LinkGrant {
    pub(crate) caller: ComponentId,
    pub(crate) provider: ComponentId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HostImportGrant {
    pub(crate) component: ComponentId,
    pub(crate) interface: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct OutboundHttpGrant {
    pub(crate) component: ComponentId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DirectoryGrant {
    pub(crate) component: ComponentId,
    pub(crate) host: PathBuf,
    pub(crate) guest: PathBuf,
    pub(crate) access: DirectoryAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DirectoryAccess {
    ReadOnly,
    ReadWrite,
}

pub(crate) fn valid_guest_path(path: &Path) -> bool {
    let Some(path) = path.to_str() else {
        return false;
    };
    if path == "/" {
        return true;
    }
    path.starts_with('/')
        && !path.ends_with('/')
        && !path.contains('\0')
        && path[1..]
            .split('/')
            .all(|segment| !segment.is_empty() && segment != "." && segment != "..")
}
