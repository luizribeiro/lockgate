//! Component discovery and application-assigned identity.
//! A catalog hashes exact artifacts and exposes the imports and exports decoded from their WIT.

use crate::{
    binding::{Binding, BindingExport},
    plugin::{DirectImport, Signature, interface_functions, validate_introspectable_interface},
};
use std::{
    collections::{HashMap, HashSet},
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;
use wasmtime::{Config, Engine, component::Component as WasmtimeComponent};
use wit_component::DecodedWasm;
use wit_parser::{Resolve, WorldItem};

static NEXT_CATALOG: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ComponentId {
    catalog: u64,
    index: usize,
}

/// A component handle whose artifact was admitted against generated application bindings.
///
/// The binding parameter is preserved so [`crate::Runtime::component`] can construct the correct
/// generated client without exposing a raw Wasmtime instance.
pub struct Component<B> {
    id: ComponentId,
    binding: PhantomData<fn() -> B>,
}

pub(crate) trait ComponentRef: Copy {
    fn id(self) -> ComponentId;
}

pub(crate) struct ExportInfo {
    interface: String,
    function: String,
    target: String,
    pub(crate) runtime_signature: Signature,
}

pub(crate) struct ComponentEntry {
    pub(crate) name: String,
    binding_exports: Vec<lockgate_schema::Export>,
    pub(crate) imports: Vec<String>,
    pub(crate) direct_imports: Vec<DirectImport>,
    pub(crate) exports: Vec<ExportInfo>,
    pub(crate) host_imports: HashSet<&'static str>,
    pub(crate) component: WasmtimeComponent,
}

struct InspectedComponent {
    binding_exports: Vec<lockgate_schema::Export>,
    imports: Vec<String>,
    direct_imports: Vec<DirectImport>,
    exports: Vec<ExportInfo>,
    component: WasmtimeComponent,
}

pub(crate) struct Catalog {
    identity: u64,
    engine: Engine,
    components: Vec<ComponentEntry>,
    names: HashMap<String, ComponentId>,
}

/// An error produced while cataloging or resolving a component.
#[derive(Debug, Error)]
pub enum CatalogError {
    #[error("component name must not be empty")]
    EmptyName,
    #[error("component name `{0}` is already registered")]
    DuplicateName(String),
    #[error("failed to create the Wasmtime engine")]
    Engine(#[source] anyhow::Error),
    #[error("component `{name}` is not a valid supported component")]
    InvalidComponent {
        name: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("component handle does not belong to this catalog")]
    ForeignComponent,
    #[error("component `{name}` does not implement world `{world}`")]
    WorldMismatch {
        name: String,
        world: String,
        #[source]
        source: anyhow::Error,
    },
}

impl Catalog {
    pub(crate) fn new() -> Result<Self, CatalogError> {
        let mut config = Config::new();
        config.wasm_component_model(true).consume_fuel(true);
        let engine = Engine::new(&config).map_err(|error| CatalogError::Engine(error.into()))?;
        Ok(Self {
            identity: NEXT_CATALOG.fetch_add(1, Ordering::Relaxed),
            engine,
            components: Vec::new(),
            names: HashMap::new(),
        })
    }

    /// Decodes, compiles, and admits an artifact against generated application bindings.
    ///
    /// Admission requires every export described by `B` to exist in the component with the same
    /// WIT type. Additional component exports are allowed.
    pub(crate) fn add<S: Send + 'static, B: Binding<S>>(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<Component<B>, CatalogError> {
        let name = name.into();
        let inspected = self.inspect_new(&name, bytes.as_ref())?;
        validate_binding(B::EXPORTS, &inspected.binding_exports).map_err(|source| {
            CatalogError::WorldMismatch {
                name: name.clone(),
                world: B::WORLD.into(),
                source,
            }
        })?;
        let id = self.insert(name, inspected);
        self.components[id.index]
            .host_imports
            .extend(B::HOST_IMPORTS);
        Ok(Component {
            id,
            binding: PhantomData,
        })
    }

    /// Admits an existing artifact against an additional generated application binding.
    ///
    /// The returned handle refers to the same catalog entry and compiled component. Admission
    /// requires every export described by `B` to exist with the same WIT type; a failed check
    /// leaves the catalog unchanged.
    pub(crate) fn admit<S: Send + 'static, B: Binding<S>>(
        &mut self,
        component: impl ComponentRef,
    ) -> Result<Component<B>, CatalogError> {
        let id = component.id();
        {
            let entry = self.entry(id)?;
            validate_binding(B::EXPORTS, &entry.binding_exports).map_err(|source| {
                CatalogError::WorldMismatch {
                    name: entry.name.clone(),
                    world: B::WORLD.into(),
                    source,
                }
            })?;
        }
        self.components[id.index]
            .host_imports
            .extend(B::HOST_IMPORTS);
        Ok(Component {
            id,
            binding: PhantomData,
        })
    }

    #[cfg(test)]
    pub(crate) fn add_untyped(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ComponentId, CatalogError> {
        let name = name.into();
        let inspected = self.inspect_new(&name, bytes.as_ref())?;
        Ok(self.insert(name, inspected))
    }

    #[cfg(test)]
    pub(crate) fn register_host_imports(
        &mut self,
        component: ComponentId,
        interfaces: &'static [&'static str],
    ) {
        let entry = self
            .components
            .get_mut(component.index)
            .expect("test component belongs to this catalog");
        entry.host_imports.extend(interfaces);
    }

    fn inspect_new(&self, name: &str, bytes: &[u8]) -> Result<InspectedComponent, CatalogError> {
        if name.trim().is_empty() {
            return Err(CatalogError::EmptyName);
        }
        if self.names.contains_key(name) {
            return Err(CatalogError::DuplicateName(name.into()));
        }
        inspect(&self.engine, name, bytes)
    }

    fn insert(&mut self, name: String, inspected: InspectedComponent) -> ComponentId {
        let id = ComponentId {
            catalog: self.identity,
            index: self.components.len(),
        };
        self.components.push(ComponentEntry {
            name: name.clone(),
            binding_exports: inspected.binding_exports,
            imports: inspected.imports,
            direct_imports: inspected.direct_imports,
            exports: inspected.exports,
            component: inspected.component,
            host_imports: HashSet::new(),
        });
        self.names.insert(name, id);
        id
    }

    pub(crate) fn identity(&self) -> u64 {
        self.identity
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn entry(&self, id: ComponentId) -> Result<&ComponentEntry, CatalogError> {
        if id.catalog != self.identity {
            return Err(CatalogError::ForeignComponent);
        }
        self.components
            .get(id.index)
            .ok_or(CatalogError::ForeignComponent)
    }
}

impl<B> Copy for Component<B> {}

impl<B> Clone for Component<B> {
    fn clone(&self) -> Self {
        *self
    }
}

impl<B> fmt::Debug for Component<B> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("Component")
    }
}

impl<B> PartialEq for Component<B> {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl<B> Eq for Component<B> {}

impl<B> Hash for Component<B> {
    fn hash<S: Hasher>(&self, state: &mut S) {
        self.id.hash(state);
    }
}

impl ComponentRef for ComponentId {
    fn id(self) -> ComponentId {
        self
    }
}

impl<B> ComponentRef for Component<B> {
    fn id(self) -> ComponentId {
        self.id
    }
}

impl<B> Component<B> {
    pub(crate) fn id(self) -> ComponentId {
        self.id
    }
}

impl ExportInfo {
    pub(crate) fn interface(&self) -> &str {
        &self.interface
    }

    pub(crate) fn function(&self) -> &str {
        &self.function
    }

    pub(crate) fn target(&self) -> &str {
        &self.target
    }
}

fn inspect(engine: &Engine, name: &str, bytes: &[u8]) -> Result<InspectedComponent, CatalogError> {
    let decoded =
        wit_component::decode(bytes).map_err(|source| CatalogError::InvalidComponent {
            name: name.into(),
            source,
        })?;
    let DecodedWasm::Component(resolve, world) = decoded else {
        return Err(CatalogError::InvalidComponent {
            name: name.into(),
            source: anyhow::anyhow!("artifact is a core module, not a component"),
        });
    };
    validate_world_interfaces(&resolve, world, name)?;
    let binding_exports = lockgate_schema::world_exports(&resolve, world).map_err(|source| {
        CatalogError::InvalidComponent {
            name: name.into(),
            source,
        }
    })?;
    let component =
        WasmtimeComponent::new(engine, bytes).map_err(|source| CatalogError::InvalidComponent {
            name: name.into(),
            source: source.into(),
        })?;
    let imports = interface_names(&resolve, world, true, name)?;
    let mut direct_imports = Vec::new();
    for item in resolve.worlds[world].imports.values() {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        let interface = resolve.id_of(*id).expect("named interface was validated");
        if interface.starts_with("wasi:") {
            continue;
        }
        validate_introspectable_interface(&resolve, *id).map_err(|source| {
            CatalogError::InvalidComponent {
                name: name.into(),
                source,
            }
        })?;
        let functions =
            interface_functions(engine, &component, &interface, true).map_err(|source| {
                CatalogError::InvalidComponent {
                    name: name.into(),
                    source,
                }
            })?;
        direct_imports.push(DirectImport {
            interface,
            functions,
        });
    }
    let exported = interface_names(&resolve, world, false, name)?;
    let mut exports = Vec::new();
    for interface in exported {
        let interface_id = resolve.worlds[world]
            .exports
            .values()
            .find_map(|item| match item {
                WorldItem::Interface { id, .. }
                    if resolve.id_of(*id).as_deref() == Some(interface.as_str()) =>
                {
                    Some(*id)
                }
                _ => None,
            })
            .expect("exported interface was collected from this world");
        validate_introspectable_interface(&resolve, interface_id).map_err(|source| {
            CatalogError::InvalidComponent {
                name: name.into(),
                source,
            }
        })?;
        let functions =
            interface_functions(engine, &component, &interface, false).map_err(|source| {
                CatalogError::InvalidComponent {
                    name: name.into(),
                    source,
                }
            })?;
        exports.extend(functions.into_iter().map(|(function, signature)| {
            let target = format!("{interface}#{function}");
            ExportInfo {
                interface: interface.clone(),
                function,
                target,
                runtime_signature: signature,
            }
        }));
    }
    Ok(InspectedComponent {
        binding_exports,
        imports,
        direct_imports,
        exports,
        component,
    })
}

fn validate_binding(
    expected: &[BindingExport],
    actual: &[lockgate_schema::Export],
) -> Result<(), anyhow::Error> {
    for expected in expected {
        let Some(actual) = actual
            .iter()
            .find(|actual| actual.interface == expected.interface && actual.item == expected.item)
        else {
            anyhow::bail!(
                "missing required export {}#{}",
                expected.interface,
                expected.item
            );
        };
        if actual.signature != expected.signature {
            anyhow::bail!(
                "type mismatch for {}#{}: expected {}, found {}",
                expected.interface,
                expected.item,
                expected.signature,
                actual.signature
            );
        }
    }
    Ok(())
}

fn validate_world_interfaces(
    resolve: &Resolve,
    world: wit_parser::WorldId,
    component_name: &str,
) -> Result<(), CatalogError> {
    for (imported, items) in [
        (true, &resolve.worlds[world].imports),
        (false, &resolve.worlds[world].exports),
    ] {
        for item in items.values() {
            let WorldItem::Interface { id, .. } = item else {
                continue;
            };
            let name = resolve
                .id_of(*id)
                .ok_or_else(|| CatalogError::InvalidComponent {
                    name: component_name.into(),
                    source: anyhow::anyhow!("component contains an unnamed interface"),
                })?;
            if imported && name.starts_with("wasi:") {
                continue;
            }
            validate_introspectable_interface(resolve, *id).map_err(|source| {
                CatalogError::InvalidComponent {
                    name: component_name.into(),
                    source,
                }
            })?;
        }
    }
    Ok(())
}

fn interface_names(
    resolve: &Resolve,
    world: wit_parser::WorldId,
    imports: bool,
    component_name: &str,
) -> Result<Vec<String>, CatalogError> {
    let items = if imports {
        &resolve.worlds[world].imports
    } else {
        &resolve.worlds[world].exports
    };
    items
        .values()
        .map(|item| match item {
            WorldItem::Interface { id, .. } => {
                resolve
                    .id_of(*id)
                    .ok_or_else(|| CatalogError::InvalidComponent {
                        name: component_name.into(),
                        source: anyhow::anyhow!("component contains an unnamed interface"),
                    })
            }
            _ => Err(CatalogError::InvalidComponent {
                name: component_name.into(),
                source: anyhow::anyhow!("direct world functions and types are unsupported"),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wasmtime::{Store, component::Instance};
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    const WIT: &str = r#"package demo:catalog@0.1.0;

interface api { greet: func(name: string) -> string; }
interface health { ping: func() -> bool; }

world provider { export api; }
world multi-provider { export api; export health; }
world caller { import api; }"#;

    struct GreeterBinding;

    impl<S: Send + 'static> Binding<S> for GreeterBinding {
        const WORLD: &'static str = "greeter";
        const EXPORTS: &'static [BindingExport] = &[BindingExport {
            interface: "demo:catalog/api@0.1.0",
            item: "greet",
            signature: "freestanding(string)->string",
        }];
        const HOST_IMPORTS: &'static [&'static str] = &[];

        type Client<'runtime> = ();

        fn bind(
            _store: &mut Store<crate::__private::PluginStore<S>>,
            _instance: &Instance,
        ) -> anyhow::Result<Self> {
            Ok(Self)
        }

        fn install_host_import(
            _interface: &str,
            _linker: &mut wasmtime::component::Linker<crate::__private::PluginStore<S>>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("greeter binding has no host imports")
        }

        fn client<'runtime>(
            _component: crate::__private::RuntimeComponent<'runtime, S, Self>,
        ) -> Self::Client<'runtime>
        where
            S: 'runtime,
        {
        }
    }

    struct HealthBinding;

    impl<S: Send + 'static> Binding<S> for HealthBinding {
        const WORLD: &'static str = "health-check";
        const EXPORTS: &'static [BindingExport] = &[BindingExport {
            interface: "demo:catalog/health@0.1.0",
            item: "ping",
            signature: "freestanding()->bool",
        }];
        const HOST_IMPORTS: &'static [&'static str] = &[];

        type Client<'runtime> = ();

        fn bind(
            _store: &mut Store<crate::__private::PluginStore<S>>,
            _instance: &Instance,
        ) -> anyhow::Result<Self> {
            Ok(Self)
        }

        fn install_host_import(
            _interface: &str,
            _linker: &mut wasmtime::component::Linker<crate::__private::PluginStore<S>>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("health binding has no host imports")
        }

        fn client<'runtime>(
            _component: crate::__private::RuntimeComponent<'runtime, S, Self>,
        ) -> Self::Client<'runtime>
        where
            S: 'runtime,
        {
        }
    }

    struct MissingBinding;

    impl<S: Send + 'static> Binding<S> for MissingBinding {
        const WORLD: &'static str = "missing";
        const EXPORTS: &'static [BindingExport] = &[BindingExport {
            interface: "demo:catalog/missing@0.1.0",
            item: "run",
            signature: "freestanding()->unit",
        }];
        const HOST_IMPORTS: &'static [&'static str] = &[];

        type Client<'runtime> = ();

        fn bind(
            _store: &mut Store<crate::__private::PluginStore<S>>,
            _instance: &Instance,
        ) -> anyhow::Result<Self> {
            Ok(Self)
        }

        fn install_host_import(
            _interface: &str,
            _linker: &mut wasmtime::component::Linker<crate::__private::PluginStore<S>>,
        ) -> anyhow::Result<()> {
            anyhow::bail!("missing binding has no host imports")
        }

        fn client<'runtime>(
            _component: crate::__private::RuntimeComponent<'runtime, S, Self>,
        ) -> Self::Client<'runtime>
        where
            S: 'runtime,
        {
        }
    }

    fn component_bytes(world_name: &str) -> Vec<u8> {
        let mut resolve = Resolve::new();
        let package = resolve.push_str("catalog.wit", WIT).unwrap();
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
    fn assigns_identity_and_discovers_wit() {
        let mut catalog = Catalog::new().unwrap();
        let provider = catalog
            .add_untyped("greeter", component_bytes("provider"))
            .unwrap();
        let _caller = catalog
            .add_untyped("caller", component_bytes("caller"))
            .unwrap();
        let entry = catalog.entry(provider).unwrap();
        assert_eq!(entry.name, "greeter");
        assert_eq!(entry.exports[0].target(), "demo:catalog/api@0.1.0#greet");
        assert_eq!(catalog.components.len(), 2);
        assert!(
            catalog
                .add_untyped("greeter", component_bytes("provider"))
                .is_err()
        );
    }

    #[test]
    fn rejects_handles_from_another_catalog() {
        let mut first = Catalog::new().unwrap();
        let provider = first
            .add_untyped("greeter", component_bytes("provider"))
            .unwrap();
        let second = Catalog::new().unwrap();
        assert!(matches!(
            second.entry(provider),
            Err(CatalogError::ForeignComponent)
        ));
    }

    #[test]
    fn admits_one_artifact_under_multiple_bindings() {
        let mut catalog = Catalog::new().unwrap();
        let greeter = catalog
            .add::<(), GreeterBinding>("combined", component_bytes("multi-provider"))
            .unwrap();

        let health = catalog.admit::<(), HealthBinding>(greeter).unwrap();

        assert_eq!(greeter.id(), health.id());
        assert_eq!(catalog.components.len(), 1);
        assert_eq!(catalog.entry(health.id()).unwrap().name, "combined");
    }

    #[test]
    fn failed_additional_admission_leaves_the_catalog_unchanged() {
        let mut catalog = Catalog::new().unwrap();
        let greeter = catalog
            .add::<(), GreeterBinding>("greeter", component_bytes("provider"))
            .unwrap();
        let error = catalog.admit::<(), MissingBinding>(greeter).unwrap_err();

        assert!(matches!(error, CatalogError::WorldMismatch { .. }));
        assert_eq!(catalog.components.len(), 1);
        assert_eq!(catalog.entry(greeter.id()).unwrap().name, "greeter");
    }
}
