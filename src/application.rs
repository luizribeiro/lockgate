//! Typed application assembly with generated host-interface wiring.

use crate::{
    Component,
    binding::RoleSet,
    catalog::{ApplicationError, Catalog, ComponentId},
    grants::{
        DirectoryAccess, DirectoryGrant, Grants, HostImportGrant, LinkGrant, valid_guest_path,
    },
    runtime::{PluginStore, Runtime, RuntimeBuildError},
};
use lockgate_schema::PluginMetadata;
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

    fn register<R: RoleSet<S>>(&mut self, component: ComponentId) {
        let bindings = self.components.entry(component).or_default();
        R::for_each_role(&mut |_, _, host_imports, install| {
            for interface in host_imports {
                bindings.insert(interface, install);
            }
        });
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
    grants: Grants,
    state: Arc<S>,
    host_bindings: HostBindings<S>,
    fuel_per_call: u64,
}

impl<S: Send + Sync + 'static> Application<S> {
    /// Creates an application whose state is shared by every component context.
    pub fn new(state: S) -> Result<Self, ApplicationError> {
        let catalog = Catalog::new()?;
        Ok(Self {
            catalog,
            grants: Grants::default(),
            state: Arc::new(state),
            host_bindings: HostBindings::new(),
            fuel_per_call: crate::runtime::DEFAULT_FUEL_PER_CALL,
        })
    }

    /// Sets the WebAssembly instruction budget restored before each component call.
    ///
    /// The same budget applies while instantiating each component. Applications should choose the
    /// smallest value that accommodates their plugins; the default is 100,000 units of Wasmtime
    /// fuel.
    pub fn fuel_per_call(mut self, fuel: u64) -> Self {
        self.fuel_per_call = fuel;
        self
    }

    #[cfg(test)]
    pub(crate) fn host_installer_count(&self) -> usize {
        self.host_bindings.installer_count()
    }

    /// Adds an artifact to the application and retains its generated host-interface installers.
    pub fn add<R: RoleSet<S>>(
        &mut self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<R::Handles, ApplicationError> {
        let component = self.catalog.add::<S, R>(bytes)?;
        self.register::<R>(component.id());
        Ok(R::handles(component))
    }

    /// Returns the metadata embedded by an admitted plugin.
    pub fn metadata<B>(
        &self,
        component: Component<B>,
    ) -> Result<&PluginMetadata, ApplicationError> {
        Ok(&self.catalog.entry(component.id())?.metadata)
    }

    #[cfg(test)]
    pub(crate) fn add_untyped(
        &mut self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ComponentId, ApplicationError> {
        self.catalog.add_untyped(bytes)
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

    /// Validates the retained grants and runs every added component.
    pub async fn run(self) -> Result<Runtime<S>, RuntimeBuildError> {
        Runtime::from_application(
            self.catalog,
            self.grants,
            self.state,
            self.host_bindings,
            self.fuel_per_call,
        )
        .await
    }

    fn register<R: RoleSet<S>>(&mut self, component: ComponentId) {
        self.host_bindings.register::<R>(component);
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
                caller: caller_entry.metadata.id().into(),
                provider: provider_entry.metadata.id().into(),
            });
        }
        let grant = LinkGrant { caller, provider };
        if !self.grants.links.contains(&grant) {
            self.grants.links.push(grant);
        }
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
                component: entry.metadata.id().into(),
                interface,
            });
        }
        let grant = HostImportGrant {
            component,
            interface,
        };
        if !self.grants.host_imports.contains(&grant) {
            self.grants.host_imports.push(grant);
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
        if !self.grants.directories.contains(&grant) {
            self.grants.directories.push(grant);
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
        let bytes = ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap();
        crate::catalog::with_test_plugin_metadata(bytes, world_name)
    }

    #[test]
    fn rejects_unrelated_links_and_unavailable_host_imports() {
        let mut app = Application::new(()).unwrap();
        let provider = app.add_untyped(component_bytes("provider")).unwrap();
        assert!(matches!(
            app.link_ids(provider, provider),
            Err(ApplicationError::NoMatchingImport { .. })
        ));

        let mut app = Application::new(()).unwrap();
        let caller = app.add_untyped(component_bytes("caller")).unwrap();
        assert!(matches!(
            app.allow_host_import_id(caller, "demo:application/api@0.1.0"),
            Err(ApplicationError::HostImportUnavailable { .. })
        ));
    }

    #[test]
    fn rejects_foreign_and_invalid_directory_grants() {
        let mut first = Application::new(()).unwrap();
        first.add_untyped(component_bytes("caller")).unwrap();
        let mut second = Application::new(()).unwrap();
        let foreign = second.add_untyped(component_bytes("caller")).unwrap();
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
        let caller = app.add_untyped(component_bytes("caller")).unwrap();
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
        let caller = app.add_untyped(component_bytes("caller")).unwrap();
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
