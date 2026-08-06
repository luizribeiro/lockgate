//! Immutable, programmatic capability policy construction.
//! Policy grants refer to opaque catalog handles instead of names copied from configuration files.

use crate::catalog::{Catalog, CatalogError, ComponentId, ExportId};
use std::path::{Path, PathBuf};
use thiserror::Error;

/// A fully validated and immutable set of component grants.
pub struct Policy {
    links: Vec<LinkGrant>,
    lookups: Vec<LookupGrant>,
    directories: Vec<DirectoryGrant>,
}

/// A policy builder tied to one catalog.
pub struct PolicyBuilder<'a> {
    catalog: &'a Catalog,
    links: Vec<LinkGrant>,
    lookups: Vec<LookupGrant>,
    directories: Vec<DirectoryGrant>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkGrant {
    caller: ComponentId,
    provider: ComponentId,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LookupGrant {
    caller: ComponentId,
    export: ExportId,
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
    #[error("guest directory path must be absolute: `{0}`")]
    RelativeGuestPath(PathBuf),
}

impl Policy {
    pub fn builder(catalog: &Catalog) -> PolicyBuilder<'_> {
        PolicyBuilder {
            catalog,
            links: Vec::new(),
            lookups: Vec::new(),
            directories: Vec::new(),
        }
    }

    pub fn links(&self) -> &[LinkGrant] {
        &self.links
    }

    pub fn lookups(&self) -> &[LookupGrant] {
        &self.lookups
    }

    pub fn directories(&self) -> &[DirectoryGrant] {
        &self.directories
    }
}

impl PolicyBuilder<'_> {
    /// Permits a caller's typed imports to be satisfied by a provider.
    pub fn link(mut self, caller: ComponentId, provider: ComponentId) -> Result<Self, PolicyError> {
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
        Ok(self)
    }

    /// Permits an exact export to be resolved through the optional dynamic registry.
    pub fn allow_lookup(
        mut self,
        caller: ComponentId,
        export: ExportId,
    ) -> Result<Self, PolicyError> {
        self.catalog.component(caller)?;
        self.catalog.export_entry(export)?;
        let grant = LookupGrant { caller, export };
        if !self.lookups.contains(&grant) {
            self.lookups.push(grant);
        }
        Ok(self)
    }

    pub fn read_only_dir(
        self,
        component: ComponentId,
        host: impl Into<PathBuf>,
        guest: impl Into<PathBuf>,
    ) -> Result<Self, PolicyError> {
        self.directory(
            component,
            host.into(),
            guest.into(),
            DirectoryAccess::ReadOnly,
        )
    }

    pub fn read_write_dir(
        self,
        component: ComponentId,
        host: impl Into<PathBuf>,
        guest: impl Into<PathBuf>,
    ) -> Result<Self, PolicyError> {
        self.directory(
            component,
            host.into(),
            guest.into(),
            DirectoryAccess::ReadWrite,
        )
    }

    pub fn build(self) -> Policy {
        Policy {
            links: self.links,
            lookups: self.lookups,
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
        if !guest.is_absolute() {
            return Err(PolicyError::RelativeGuestPath(guest));
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
}

impl LinkGrant {
    pub fn caller(&self) -> ComponentId {
        self.caller
    }

    pub fn provider(&self) -> ComponentId {
        self.provider
    }
}

impl LookupGrant {
    pub fn caller(&self) -> ComponentId {
        self.caller
    }

    pub fn export(&self) -> ExportId {
        self.export
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
        let caller = catalog.add("caller", component_bytes("caller")).unwrap();
        let provider = catalog
            .add("provider", component_bytes("provider"))
            .unwrap();
        let export = catalog
            .export(provider, "demo:policy/api@0.1.0#run")
            .unwrap();
        let policy = Policy::builder(&catalog)
            .link(caller, provider)
            .unwrap()
            .allow_lookup(caller, export)
            .unwrap()
            .read_only_dir(caller, "/tmp/shared", "/shared")
            .unwrap()
            .build();
        assert_eq!(policy.links().len(), 1);
        assert_eq!(policy.lookups().len(), 1);
        assert_eq!(policy.directories()[0].access(), DirectoryAccess::ReadOnly);
    }

    #[test]
    fn rejects_unrelated_links_and_foreign_handles() {
        let mut catalog = Catalog::new().unwrap();
        let provider = catalog
            .add("provider", component_bytes("provider"))
            .unwrap();
        assert!(matches!(
            Policy::builder(&catalog).link(provider, provider),
            Err(PolicyError::NoMatchingImport { .. })
        ));

        let mut other = Catalog::new().unwrap();
        let foreign = other.add("caller", component_bytes("caller")).unwrap();
        assert!(matches!(
            Policy::builder(&catalog).read_only_dir(foreign, "/tmp", "/tmp"),
            Err(PolicyError::Catalog(CatalogError::ForeignComponent))
        ));
    }
}
