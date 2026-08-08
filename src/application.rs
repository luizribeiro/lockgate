//! Typed application assembly with generated host-interface wiring.

use crate::{
    Component,
    binding::Binding,
    catalog::{ApplicationError, Catalog, ComponentId},
    policy::{Policy, PolicyBuilder},
    runtime::{PluginStore, Runtime, RuntimeBuildError},
};
use std::{collections::HashMap, sync::Arc};

pub(crate) type StateFactory<S> = Arc<dyn Fn(&str) -> S + Send + Sync>;
type HostInstaller<S> =
    fn(&str, &mut wasmtime::component::Linker<PluginStore<S>>) -> anyhow::Result<()>;

/// Generated host bindings retained for runtime preflight.
pub(crate) struct HostBindings<S: Send + 'static> {
    components: HashMap<ComponentId, HashMap<&'static str, HostInstaller<S>>>,
}

impl<S: Send + 'static> HostBindings<S> {
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
pub struct Application<S: Send + 'static = ()> {
    catalog: Catalog,
    state_factory: StateFactory<S>,
    host_bindings: HostBindings<S>,
}

impl<S: Send + 'static> Application<S> {
    /// Creates an application with state constructed separately for each included component.
    pub fn new(
        factory: impl Fn(&str) -> S + Send + Sync + 'static,
    ) -> Result<Self, ApplicationError> {
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

    /// Validates the policy and instantiates every included component.
    pub fn runtime(self, policy: Policy) -> Result<Runtime<S>, RuntimeBuildError> {
        Runtime::from_application(self.catalog, policy, self.state_factory, self.host_bindings)
    }

    fn register<B: Binding<S>>(&mut self, component: ComponentId) {
        self.host_bindings.register::<B>(component);
    }
}
