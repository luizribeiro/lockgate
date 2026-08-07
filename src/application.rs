//! Typed application assembly with generated host-interface wiring.

use crate::{
    Component, ComponentId, ComponentRef,
    binding::{ApplicationBinding, HostImportBinding},
    catalog::{Catalog, CatalogError},
    policy::Policy,
    runtime::{PluginStore, RuntimeBuilder},
};
use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
};

pub(crate) type StateFactory<S> = Arc<dyn Fn(ComponentId, &str) -> S + Send + Sync>;
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

impl Application<()> {
    /// Creates a stateless application.
    pub fn new() -> Result<Self, CatalogError> {
        Self::with_state_factory(|_, _| ())
    }
}

impl<S: Clone + Send + Sync + 'static> Application<S> {
    /// Creates an application whose component stores clone the supplied state value.
    ///
    /// Use shared ownership such as [`Arc`] when every component should observe the same logical
    /// state.
    pub fn with_state(state: S) -> Result<Self, CatalogError> {
        Self::with_state_factory(move |_, _| state.clone())
    }
}

impl<S: Send + 'static> Application<S> {
    /// Creates an application with state constructed separately for each included component.
    pub fn with_state_factory(
        factory: impl Fn(ComponentId, &str) -> S + Send + Sync + 'static,
    ) -> Result<Self, CatalogError> {
        Ok(Self {
            catalog: Catalog::new()?,
            state_factory: Arc::new(factory),
            host_bindings: HostBindings::new(),
        })
    }

    /// Returns the catalog used to construct capability policy.
    pub fn catalog(&self) -> &Catalog {
        &self.catalog
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
        component: impl ComponentRef,
    ) -> Result<Component<B>, CatalogError> {
        let component = self.catalog.admit::<B>(component)?;
        self.register::<B>(component.id());
        Ok(component)
    }

    /// Adds an artifact without generated application bindings.
    pub fn add_untyped(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ComponentId, CatalogError> {
        self.catalog.add_untyped(name, bytes)
    }

    /// Begins runtime construction for this application and an immutable capability policy.
    pub fn runtime(self, policy: Policy) -> RuntimeBuilder<S> {
        RuntimeBuilder::from_application(
            self.catalog,
            policy,
            self.state_factory,
            self.host_bindings,
        )
    }

    fn register<B: ApplicationBinding<S>>(&mut self, component: ComponentId) {
        self.host_bindings.register(component, B::host_imports());
    }
}
