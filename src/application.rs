//! Typed application assembly with generated host-interface wiring.

use crate::{
    Component,
    binding::Binding,
    catalog::{ApplicationError, Catalog, ComponentId},
    policy::{
        DirectoryAccess, DirectoryGrant, HostImportGrant, LinkGrant, Policy, valid_guest_path,
    },
    runtime::{PluginStore, Runtime, RuntimeBuildError},
};
use std::{collections::HashMap, path::PathBuf, sync::Arc};
type HostInstaller<S> =
    fn(&str, &mut wasmtime::component::Linker<PluginStore<S>>) -> anyhow::Result<()>;

/// Generated host bindings retained for runtime preflight.
pub(crate) struct HostBindings<S: Send + Sync + 'static> {
    components: HashMap<ComponentId, HashMap<&'static str, HostInstaller<S>>>,
}

impl<S: Send + Sync + 'static> HostBindings<S> {
    pub(crate) fn new() -> Self {
        Self {
            components: HashMap::new(),
        }
    }

    fn register<B: Binding<S>>(&mut self, component: ComponentId) {
        let bindings = self.components.entry(component).or_default();
        for interface in B::HOST_IMPORTS {
            bindings.insert(interface, B::install_host_import);
        }
    }

    pub(crate) fn install(
        &self,
        component: ComponentId,
        interface: &str,
        linker: &mut wasmtime::component::Linker<PluginStore<S>>,
    ) -> Option<anyhow::Result<()>> {
        self.components
            .get(&component)
            .and_then(|bindings| bindings.get(interface))
            .map(|install| install(interface, linker))
    }

    #[cfg(test)]
    pub(crate) fn installer_count(&self) -> usize {
        self.components.values().map(HashMap::len).sum()
    }
}

/// A catalog assembled together with its application state and generated host bindings.
pub struct Application<S: Send + Sync + 'static = ()> {
    catalog: Catalog,
    policy: Policy,
    state: Arc<S>,
    host_bindings: HostBindings<S>,
}

impl<S: Send + Sync + 'static> Application<S> {
    /// Creates an application whose state is shared by every component context.
    pub fn new(state: S) -> Result<Self, ApplicationError> {
        let catalog = Catalog::new()?;
        let policy = Policy::new(catalog.identity());
        Ok(Self {
            catalog,
            policy,
            state: Arc::new(state),
            host_bindings: HostBindings::new(),
        })
    }

    /// Includes a component that needs no other capability grants.
    pub fn include<B>(self, component: Component<B>) -> Result<Self, ApplicationError> {
        self.include_id(component.id())
    }

    #[cfg(test)]
    pub(crate) fn host_installer_count(&self) -> usize {
        self.host_bindings.installer_count()
    }

    /// Admits an artifact and retains its generated host-interface installers.
    pub fn add<B: Binding<S>>(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<Component<B>, ApplicationError> {
        let component = self.catalog.add::<S, B>(name, bytes)?;
        self.register::<B>(component.id());
        Ok(component)
    }

    /// Admits an existing artifact under an additional generated binding role.
    pub fn admit<B: Binding<S>>(
        &mut self,
        component: Component<impl Sized>,
    ) -> Result<Component<B>, ApplicationError> {
        let component = self.catalog.admit::<S, B>(component)?;
        self.register::<B>(component.id());
        Ok(component)
    }

    #[cfg(test)]
    pub(crate) fn add_untyped(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ComponentId, ApplicationError> {
        self.catalog.add_untyped(name, bytes)
    }

    /// Permits a caller's typed imports to be satisfied by a provider.
    pub fn link<C, P>(
        self,
        caller: Component<C>,
        provider: Component<P>,
    ) -> Result<Self, ApplicationError> {
        self.link_ids(caller.id(), provider.id())
    }

    /// Permits one component import to be implemented by the embedding host.
    pub fn allow_host_import(
        self,
        component: Component<impl Sized>,
        interface: impl Into<String>,
    ) -> Result<Self, ApplicationError> {
        self.allow_host_import_id(component.id(), interface)
    }

    /// Preopens a host directory for read-only component access.
    pub fn read_only_dir<B>(
        self,
        component: Component<B>,
        host: impl Into<PathBuf>,
        guest: impl Into<PathBuf>,
    ) -> Result<Self, ApplicationError> {
        self.directory(
            component.id(),
            host.into(),
            guest.into(),
            DirectoryAccess::ReadOnly,
        )
    }

    /// Preopens a host directory for read-write component access.
    pub fn read_write_dir<B>(
        self,
        component: Component<B>,
        host: impl Into<PathBuf>,
        guest: impl Into<PathBuf>,
    ) -> Result<Self, ApplicationError> {
        self.directory(
            component.id(),
            host.into(),
            guest.into(),
            DirectoryAccess::ReadWrite,
        )
    }

    /// Validates the retained grants and instantiates every included component.
    pub fn runtime(self) -> Result<Runtime<S>, RuntimeBuildError> {
        Runtime::from_application(self.catalog, self.policy, self.state, self.host_bindings)
    }

    fn register<B: Binding<S>>(&mut self, component: ComponentId) {
        self.host_bindings.register::<B>(component);
    }

    pub(crate) fn include_id(mut self, component: ComponentId) -> Result<Self, ApplicationError> {
        self.catalog.entry(component)?;
        self.policy.include(component);
        Ok(self)
    }

    pub(crate) fn link_ids(
        mut self,
        caller: ComponentId,
        provider: ComponentId,
    ) -> Result<Self, ApplicationError> {
        let caller_entry = self.catalog.entry(caller)?;
        let provider_entry = self.catalog.entry(provider)?;
        let matches = caller_entry.direct_imports.iter().any(|import| {
            provider_entry
                .exports
                .iter()
                .any(|export| export.interface == import.interface)
        });
        if !matches {
            return Err(ApplicationError::NoMatchingImport {
                caller: caller_entry.name.clone(),
                provider: provider_entry.name.clone(),
            });
        }
        let grant = LinkGrant { caller, provider };
        if !self.policy.links.contains(&grant) {
            self.policy.links.push(grant);
        }
        self.policy.include(caller);
        self.policy.include(provider);
        Ok(self)
    }

    pub(crate) fn allow_host_import_id(
        mut self,
        component: ComponentId,
        interface: impl Into<String>,
    ) -> Result<Self, ApplicationError> {
        let interface = interface.into();
        let entry = self.catalog.entry(component)?;
        let imported = entry
            .direct_imports
            .iter()
            .any(|import| import.interface == interface);
        if !imported || !entry.host_imports.contains(interface.as_str()) {
            return Err(ApplicationError::HostImportUnavailable {
                component: entry.name.clone(),
                interface,
            });
        }
        self.policy.include(component);
        let grant = HostImportGrant {
            component,
            interface,
        };
        if !self.policy.host_imports.contains(&grant) {
            self.policy.host_imports.push(grant);
        }
        Ok(self)
    }

    pub(crate) fn directory(
        mut self,
        component: ComponentId,
        host: PathBuf,
        guest: PathBuf,
        access: DirectoryAccess,
    ) -> Result<Self, ApplicationError> {
        self.catalog.entry(component)?;
        self.policy.include(component);
        if !valid_guest_path(&guest) {
            return Err(ApplicationError::RelativeGuestPath(guest));
        }
        let host = host
            .canonicalize()
            .map_err(|_| ApplicationError::InvalidHostDirectory(host.clone()))?;
        if !host.is_dir() {
            return Err(ApplicationError::InvalidHostDirectory(host));
        }
        let grant = DirectoryGrant {
            component,
            host,
            guest,
            access,
        };
        if !self.policy.directories.contains(&grant) {
            self.policy.directories.push(grant);
        }
        Ok(self)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    const WIT: &str = r#"package demo:application@0.1.0;

interface api { run: func(); }

world provider { export api; }
world caller { import api; }"#;

    fn component_bytes(world_name: &str) -> Vec<u8> {
        let mut resolve = Resolve::new();
        let package = resolve.push_str("application.wit", WIT).unwrap();
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
    fn rejects_unrelated_links_and_unadmitted_host_imports() {
        let mut app = Application::new(()).unwrap();
        let provider = app
            .add_untyped("provider", component_bytes("provider"))
            .unwrap();
        assert!(matches!(
            app.link_ids(provider, provider),
            Err(ApplicationError::NoMatchingImport { .. })
        ));

        let mut app = Application::new(()).unwrap();
        let caller = app
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            app.allow_host_import_id(caller, "demo:application/api@0.1.0"),
            Err(ApplicationError::HostImportUnavailable { .. })
        ));
    }

    #[test]
    fn rejects_foreign_and_invalid_directory_grants() {
        let mut first = Application::new(()).unwrap();
        first
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        let mut second = Application::new(()).unwrap();
        let foreign = second
            .add_untyped("foreign", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            first.directory(
                foreign,
                std::env::temp_dir(),
                PathBuf::from("/shared"),
                DirectoryAccess::ReadOnly,
            ),
            Err(ApplicationError::ForeignComponent)
        ));

        let mut app = Application::new(()).unwrap();
        let caller = app
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            app.directory(
                caller,
                std::env::temp_dir(),
                PathBuf::from("/a/../b"),
                DirectoryAccess::ReadOnly,
            ),
            Err(ApplicationError::RelativeGuestPath(_))
        ));

        let mut app = Application::new(()).unwrap();
        let caller = app
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        assert!(matches!(
            app.directory(
                caller,
                std::env::temp_dir().join("lockgate-path-that-does-not-exist"),
                PathBuf::from("/shared"),
                DirectoryAccess::ReadOnly,
            ),
            Err(ApplicationError::InvalidHostDirectory(_))
        ));
    }
}
