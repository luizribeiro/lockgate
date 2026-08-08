//! Typed application assembly with generated host-interface wiring.

use crate::{
    Component,
    binding::{ApplicationBinding, HostImportBinding},
    catalog::{Catalog, CatalogError, ComponentId},
    policy::{Policy, PolicyBuilder},
    runtime::{PluginStore, Runtime, RuntimeBuildError},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(crate) type StateFactory<S> = Arc<dyn Fn(&str) -> S + Send + Sync>;
type HostInstaller<S> = fn(&mut wasmtime::component::Linker<PluginStore<S>>) -> anyhow::Result<()>;

/// Generated host bindings retained for runtime preflight.
pub(crate) struct HostBindings<S: Send + 'static> {
    installers: HashMap<&'static str, HostInstaller<S>>,
    requirements: HashMap<ComponentId, HashSet<&'static str>>,
}

impl<S: Send + 'static> HostBindings<S> {
    pub(crate) fn new() -> Self {
        Self {
            installers: HashMap::new(),
            requirements: HashMap::new(),
        }
    }

    fn register(&mut self, component: ComponentId, bindings: Vec<HostImportBinding<S>>) {
        let requirements = self.requirements.entry(component).or_default();
        for binding in bindings {
            self.installers
                .entry(binding.interface)
                .or_insert(binding.install);
            requirements.insert(binding.interface);
        }
    }

    pub(crate) fn installer(
        &self,
        component: ComponentId,
        interface: &str,
    ) -> Option<HostInstaller<S>> {
        self.requirements
            .get(&component)
            .filter(|requirements| requirements.contains(interface))
            .and_then(|_| self.installers.get(interface))
            .copied()
    }

    #[cfg(test)]
    pub(crate) fn installer_count(&self) -> usize {
        self.installers.len()
    }
}

/// A catalog assembled together with its application state and generated host bindings.
pub struct Application<S: Send + 'static = ()> {
    catalog: Catalog,
    state_factory: StateFactory<S>,
    host_bindings: HostBindings<S>,
}

impl<S: Send + 'static> Application<S> {
    /// Creates an application with state constructed separately for each included component.
    pub fn new(factory: impl Fn(&str) -> S + Send + Sync + 'static) -> Result<Self, CatalogError> {
        Ok(Self {
            catalog: Catalog::new()?,
            state_factory: Arc::new(factory),
            host_bindings: HostBindings::new(),
        })
    }

    /// Begins capability policy construction for this application.
    pub fn policy(&self) -> PolicyBuilder<'_> {
        PolicyBuilder::new(&self.catalog)
    }

    #[cfg(test)]
    pub(crate) fn host_installer_count(&self) -> usize {
        self.host_bindings.installer_count()
    }

    /// Admits an artifact and retains its generated host-interface installers.
    pub fn add<B: ApplicationBinding<S>>(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<Component<B>, CatalogError> {
        let component = self.catalog.add::<B>(name, bytes)?;
        self.register::<B>(component.id());
        Ok(component)
    }

    /// Admits an existing artifact under an additional generated binding role.
    pub fn admit<B: ApplicationBinding<S>>(
        &mut self,
        component: Component<impl Sized>,
    ) -> Result<Component<B>, CatalogError> {
        let component = self.catalog.admit::<B>(component)?;
        self.register::<B>(component.id());
        Ok(component)
    }

    #[cfg(test)]
    pub(crate) fn add_untyped(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ComponentId, CatalogError> {
        self.catalog.add_untyped(name, bytes)
    }

    /// Validates the policy and instantiates every included component.
    pub fn runtime(self, policy: Policy) -> Result<Runtime<S>, RuntimeBuildError> {
        Runtime::from_application(self.catalog, policy, self.state_factory, self.host_bindings)
    }

    fn register<B: ApplicationBinding<S>>(&mut self, component: ComponentId) {
        self.host_bindings.register(component, B::host_imports());
    }
}
