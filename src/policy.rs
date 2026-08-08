//! Immutable, programmatic capability policy construction.
//! Policy grants refer to typed application handles instead of names copied from configuration files.

use crate::catalog::{ApplicationError, Catalog, Component, ComponentId};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// An immutable set of component grants ready for runtime validation.
pub struct Policy {
    pub(crate) catalog: u64,
    pub(crate) components: Vec<ComponentId>,
    pub(crate) links: Vec<LinkGrant>,
    pub(crate) host_imports: Vec<HostImportGrant>,
    pub(crate) directories: Vec<DirectoryGrant>,
}

/// A policy builder tied to one application.
pub struct PolicyBuilder<'a> {
    catalog: &'a Catalog,
    components: Vec<ComponentId>,
    links: Vec<LinkGrant>,
    host_imports: Vec<HostImportGrant>,
    directories: Vec<DirectoryGrant>,
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

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error(transparent)]
    Application(#[from] ApplicationError),
    #[error("component `{caller}` imports no interface exported by `{provider}`")]
    NoMatchingImport { caller: String, provider: String },
    #[error("component `{component}` was not admitted with host interface `{interface}`")]
    HostImportUnavailable {
        component: String,
        interface: String,
    },
    #[error("guest directory path must be normalized absolute POSIX: `{0}`")]
    RelativeGuestPath(PathBuf),
    #[error("host directory does not exist or is not a directory: `{0}`")]
    InvalidHostDirectory(PathBuf),
}

impl<'a> PolicyBuilder<'a> {
    pub(crate) fn new(catalog: &'a Catalog) -> Self {
        PolicyBuilder {
            catalog,
            components: Vec::new(),
            links: Vec::new(),
            host_imports: Vec::new(),
            directories: Vec::new(),
        }
    }
}

impl PolicyBuilder<'_> {
    /// Includes a component that needs no other grants in the runtime.
    pub fn include<B>(self, component: Component<B>) -> Result<Self, PolicyError> {
        self.include_id(component.id())
    }

    pub(crate) fn include_id(mut self, component: ComponentId) -> Result<Self, PolicyError> {
        self.catalog.entry(component)?;
        self.include_component(component);
        Ok(self)
    }

    /// Permits a caller's typed imports to be satisfied by a provider.
    pub fn link<C, P>(
        self,
        caller: Component<C>,
        provider: Component<P>,
    ) -> Result<Self, PolicyError> {
        self.link_ids(caller.id(), provider.id())
    }

    pub(crate) fn link_ids(
        mut self,
        caller: ComponentId,
        provider: ComponentId,
    ) -> Result<Self, PolicyError> {
        let caller_entry = self.catalog.entry(caller)?;
        let provider_entry = self.catalog.entry(provider)?;
        let matches = caller_entry.direct_imports.iter().any(|import| {
            provider_entry
                .exports
                .iter()
                .any(|export| export.interface == import.interface)
        });
        if !matches {
            return Err(PolicyError::NoMatchingImport {
                caller: caller_entry.name.clone(),
                provider: provider_entry.name.clone(),
            });
        }
        let grant = LinkGrant { caller, provider };
        if !self.links.contains(&grant) {
            self.links.push(grant);
        }
        self.include_component(caller);
        self.include_component(provider);
        Ok(self)
    }

    /// Permits one component import to be implemented by the embedding host.
    pub fn allow_host_import(
        self,
        component: Component<impl Sized>,
        interface: impl Into<String>,
    ) -> Result<Self, PolicyError> {
        self.allow_host_import_id(component.id(), interface)
    }

    pub(crate) fn allow_host_import_id(
        mut self,
        component: ComponentId,
        interface: impl Into<String>,
    ) -> Result<Self, PolicyError> {
        let interface = interface.into();
        let entry = self.catalog.entry(component)?;
        let imported = entry
            .direct_imports
            .iter()
            .any(|import| import.interface == interface);
        if !imported || !entry.host_imports.contains(interface.as_str()) {
            return Err(PolicyError::HostImportUnavailable {
                component: entry.name.clone(),
                interface,
            });
        }
        self.include_component(component);
        let grant = HostImportGrant {
            component,
            interface,
        };
        if !self.host_imports.contains(&grant) {
            self.host_imports.push(grant);
        }
        Ok(self)
    }

    pub fn read_only_dir<B>(
        self,
        component: Component<B>,
        host: impl Into<PathBuf>,
        guest: impl Into<PathBuf>,
    ) -> Result<Self, PolicyError> {
        self.directory(
            component.id(),
            host.into(),
            guest.into(),
            DirectoryAccess::ReadOnly,
        )
    }

    pub fn read_write_dir<B>(
        self,
        component: Component<B>,
        host: impl Into<PathBuf>,
        guest: impl Into<PathBuf>,
    ) -> Result<Self, PolicyError> {
        self.directory(
            component.id(),
            host.into(),
            guest.into(),
            DirectoryAccess::ReadWrite,
        )
    }

    pub fn build(self) -> Policy {
        Policy {
            catalog: self.catalog.identity(),
            components: self.components,
            links: self.links,
            host_imports: self.host_imports,
            directories: self.directories,
        }
    }

    fn directory(
        mut self,
        component: ComponentId,
        host: PathBuf,
        guest: PathBuf,
        access: DirectoryAccess,
    ) -> Result<Self, PolicyError> {
        self.catalog.entry(component)?;
        self.include_component(component);
        if !valid_guest_path(&guest) {
            return Err(PolicyError::RelativeGuestPath(guest));
        }
        let host = host
            .canonicalize()
            .map_err(|_| PolicyError::InvalidHostDirectory(host.clone()))?;
        if !host.is_dir() {
            return Err(PolicyError::InvalidHostDirectory(host));
        }
        let grant = DirectoryGrant {
            component,
            host,
            guest,
            access,
        };
        if !self.directories.contains(&grant) {
            self.directories.push(grant);
        }
        Ok(self)
    }

    fn include_component(&mut self, component: ComponentId) {
        if !self.components.contains(&component) {
            self.components.push(component);
        }
    }
}

fn valid_guest_path(path: &Path) -> bool {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::Catalog;
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    const WIT: &str = r#"package demo:policy@0.1.0;

interface api { run: func(); }

world provider { export api; }
world caller { import api; }"#;

    fn component_bytes(world_name: &str) -> Vec<u8> {
        let mut resolve = Resolve::new();
        let package = resolve.push_str("policy.wit", WIT).unwrap();
        let world = resolve.packages[package].worlds[world_name];
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap()
    }

    #[test]
    fn builds_handle_based_grants() {
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        let provider = catalog
            .add_untyped("provider", component_bytes("provider"))
            .unwrap();
        let policy = PolicyBuilder::new(&catalog)
            .link_ids(caller, provider)
            .unwrap()
            .directory(
                caller,
                std::env::temp_dir(),
                PathBuf::from("/shared"),
                DirectoryAccess::ReadOnly,
            )
            .unwrap()
            .build();
        assert_eq!(policy.links.len(), 1);
        assert_eq!(policy.directories[0].access, DirectoryAccess::ReadOnly);
    }

    #[test]
    fn grants_only_admitted_host_imports() {
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            PolicyBuilder::new(&catalog).allow_host_import_id(caller, "demo:policy/api@0.1.0"),
            Err(PolicyError::HostImportUnavailable { .. })
        ));
        catalog.register_host_imports(caller, &["demo:policy/api@0.1.0"]);
        let policy = PolicyBuilder::new(&catalog)
            .allow_host_import_id(caller, "demo:policy/api@0.1.0")
            .unwrap()
            .build();
        assert_eq!(policy.host_imports.len(), 1);
        assert!(matches!(
            PolicyBuilder::new(&catalog).allow_host_import_id(caller, "demo:policy/missing@0.1.0"),
            Err(PolicyError::HostImportUnavailable { .. })
        ));
    }

    #[test]
    fn rejects_unrelated_links_and_foreign_handles() {
        let mut catalog = Catalog::new().unwrap();
        let provider = catalog
            .add_untyped("provider", component_bytes("provider"))
            .unwrap();
        assert!(matches!(
            PolicyBuilder::new(&catalog).link_ids(provider, provider),
            Err(PolicyError::NoMatchingImport { .. })
        ));

        let mut other = Catalog::new().unwrap();
        let foreign = other
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            PolicyBuilder::new(&catalog).directory(
                foreign,
                PathBuf::from("/tmp"),
                PathBuf::from("/tmp"),
                DirectoryAccess::ReadOnly,
            ),
            Err(PolicyError::Application(ApplicationError::ForeignComponent))
        ));
    }

    #[test]
    fn rejects_invalid_directory_grants() {
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            PolicyBuilder::new(&catalog).directory(
                caller,
                std::env::temp_dir(),
                PathBuf::from("/a/../b"),
                DirectoryAccess::ReadOnly,
            ),
            Err(PolicyError::RelativeGuestPath(_))
        ));
        assert!(matches!(
            PolicyBuilder::new(&catalog).directory(
                caller,
                std::env::temp_dir().join("lockgate-path-that-does-not-exist"),
                PathBuf::from("/shared"),
                DirectoryAccess::ReadOnly,
            ),
            Err(PolicyError::InvalidHostDirectory(_))
        ));
    }
}
