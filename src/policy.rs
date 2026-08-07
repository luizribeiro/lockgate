//! Immutable, programmatic capability policy construction.
//! Policy grants refer to opaque catalog handles instead of names copied from configuration files.

use crate::catalog::{Catalog, CatalogError, ComponentId, ComponentRef};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// An immutable set of component grants ready for runtime validation.
pub struct Policy {
    catalog: u64,
    components: Vec<ComponentId>,
    links: Vec<LinkGrant>,
    host_imports: Vec<HostImportGrant>,
    directories: Vec<DirectoryGrant>,
}

/// A policy builder tied to one catalog.
pub struct PolicyBuilder<'a> {
    catalog: &'a Catalog,
    components: Vec<ComponentId>,
    links: Vec<LinkGrant>,
    host_imports: Vec<HostImportGrant>,
    directories: Vec<DirectoryGrant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkGrant {
    caller: ComponentId,
    provider: ComponentId,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HostImportGrant {
    component: ComponentId,
    interface: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirectoryGrant {
    component: ComponentId,
    host: PathBuf,
    guest: PathBuf,
    access: DirectoryAccess,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DirectoryAccess {
    ReadOnly,
    ReadWrite,
}

#[derive(Debug, Error)]
pub enum PolicyError {
    #[error(transparent)]
    Catalog(#[from] CatalogError),
    #[error("component `{caller}` imports no interface exported by `{provider}`")]
    NoMatchingImport { caller: String, provider: String },
    #[error("component `{component}` does not import host interface `{interface}`")]
    NoSuchHostImport {
        component: String,
        interface: String,
    },
    #[error("guest directory path must be normalized absolute POSIX: `{0}`")]
    RelativeGuestPath(PathBuf),
    #[error("host directory does not exist or is not a directory: `{0}`")]
    InvalidHostDirectory(PathBuf),
}

impl Policy {
    pub fn builder(catalog: &Catalog) -> PolicyBuilder<'_> {
        PolicyBuilder {
            catalog,
            components: Vec::new(),
            links: Vec::new(),
            host_imports: Vec::new(),
            directories: Vec::new(),
        }
    }

    pub fn links(&self) -> &[LinkGrant] {
        &self.links
    }

    pub fn host_imports(&self) -> &[HostImportGrant] {
        &self.host_imports
    }

    pub fn directories(&self) -> &[DirectoryGrant] {
        &self.directories
    }

    pub fn components(&self) -> &[ComponentId] {
        &self.components
    }

    pub(crate) fn catalog_identity(&self) -> u64 {
        self.catalog
    }
}

impl PolicyBuilder<'_> {
    /// Includes a component that needs no other grants in the runtime.
    pub fn include(mut self, component: impl ComponentRef) -> Result<Self, PolicyError> {
        let component = component.id();
        self.catalog.component(component)?;
        self.include_component(component);
        Ok(self)
    }

    /// Permits a caller's typed imports to be satisfied by a provider.
    pub fn link(
        mut self,
        caller: impl ComponentRef,
        provider: impl ComponentRef,
    ) -> Result<Self, PolicyError> {
        let caller = caller.id();
        let provider = provider.id();
        let caller_info = self.catalog.component(caller)?;
        let provider_info = self.catalog.component(provider)?;
        let matches = caller_info.imports().iter().any(|import| {
            provider_info
                .exports()
                .iter()
                .any(|export| export.interface() == import)
        });
        if !matches {
            return Err(PolicyError::NoMatchingImport {
                caller: caller_info.name().into(),
                provider: provider_info.name().into(),
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
        mut self,
        component: impl ComponentRef,
        interface: impl Into<String>,
    ) -> Result<Self, PolicyError> {
        let component = component.id();
        let interface = interface.into();
        let info = self.catalog.component(component)?;
        if !info.imports().iter().any(|import| import == &interface) {
            return Err(PolicyError::NoSuchHostImport {
                component: info.name().into(),
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

    pub fn read_only_dir(
        self,
        component: impl ComponentRef,
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

    pub fn read_write_dir(
        self,
        component: impl ComponentRef,
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
        self.catalog.component(component)?;
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

impl LinkGrant {
    pub fn caller(&self) -> ComponentId {
        self.caller
    }

    pub fn provider(&self) -> ComponentId {
        self.provider
    }
}

impl HostImportGrant {
    pub fn component(&self) -> ComponentId {
        self.component
    }

    pub fn interface(&self) -> &str {
        &self.interface
    }
}

impl DirectoryGrant {
    pub fn component(&self) -> ComponentId {
        self.component
    }

    pub fn host(&self) -> &Path {
        &self.host
    }

    pub fn guest(&self) -> &Path {
        &self.guest
    }

    pub fn access(&self) -> DirectoryAccess {
        self.access
    }
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
        let policy = Policy::builder(&catalog)
            .link(caller, provider)
            .unwrap()
            .read_only_dir(caller, std::env::temp_dir(), "/shared")
            .unwrap()
            .build();
        assert_eq!(policy.links().len(), 1);
        assert_eq!(policy.directories()[0].access(), DirectoryAccess::ReadOnly);
    }

    #[test]
    fn grants_only_real_host_imports() {
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        let policy = Policy::builder(&catalog)
            .allow_host_import(caller, "demo:policy/api@0.1.0")
            .unwrap()
            .build();
        assert_eq!(policy.host_imports().len(), 1);
        assert!(matches!(
            Policy::builder(&catalog).allow_host_import(caller, "demo:policy/missing@0.1.0"),
            Err(PolicyError::NoSuchHostImport { .. })
        ));
    }

    #[test]
    fn rejects_unrelated_links_and_foreign_handles() {
        let mut catalog = Catalog::new().unwrap();
        let provider = catalog
            .add_untyped("provider", component_bytes("provider"))
            .unwrap();
        assert!(matches!(
            Policy::builder(&catalog).link(provider, provider),
            Err(PolicyError::NoMatchingImport { .. })
        ));

        let mut other = Catalog::new().unwrap();
        let foreign = other
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            Policy::builder(&catalog).read_only_dir(foreign, "/tmp", "/tmp"),
            Err(PolicyError::Catalog(CatalogError::ForeignComponent))
        ));
    }

    #[test]
    fn rejects_invalid_directory_grants() {
        let mut catalog = Catalog::new().unwrap();
        let caller = catalog
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            Policy::builder(&catalog).read_only_dir(caller, std::env::temp_dir(), "/a/../b"),
            Err(PolicyError::RelativeGuestPath(_))
        ));
        assert!(matches!(
            Policy::builder(&catalog).read_only_dir(
                caller,
                std::env::temp_dir().join("lockgate-path-that-does-not-exist"),
                "/shared"
            ),
            Err(PolicyError::InvalidHostDirectory(_))
        ));
    }
}
