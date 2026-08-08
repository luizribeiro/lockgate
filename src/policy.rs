//! Private capability grants retained by an application until runtime construction.

use crate::catalog::ComponentId;
use std::path::{Path, PathBuf};

pub(crate) struct Policy {
    pub(crate) catalog: u64,
    pub(crate) components: Vec<ComponentId>,
    pub(crate) links: Vec<LinkGrant>,
    pub(crate) host_imports: Vec<HostImportGrant>,
    pub(crate) directories: Vec<DirectoryGrant>,
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

impl Policy {
    pub(crate) fn new(catalog: u64) -> Self {
        Self {
            catalog,
            components: Vec::new(),
            links: Vec::new(),
            host_imports: Vec::new(),
            directories: Vec::new(),
        }
    }

    pub(crate) fn include(&mut self, component: ComponentId) {
        if !self.components.contains(&component) {
            self.components.push(component);
        }
    }
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
