//! Component discovery and embedded plugin identity.
//! The catalog compiles artifacts and retains the component structure needed for enforcement.

use crate::{
    binding::{BindingExport, RoleSet},
    plugin::{DirectImport, Signature, interface_functions, validate_introspectable_interface},
};
use lockgate_schema::{PLUGIN_METADATA_SECTION, PluginMetadata, decode_plugin_metadata};
use std::{
    collections::HashSet,
    fmt,
    hash::{Hash, Hasher},
    marker::PhantomData,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;
use wasmtime::{Config, Engine, component::Component as WasmtimeComponent};
use wit_component::DecodedWasm;
use wit_parser::{Resolve, WorldItem};

static NEXT_CATALOG: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct ComponentId {
    pub(crate) catalog: u64,
    pub(crate) index: usize,
}

/// A component handle whose artifact was admitted against generated application bindings.
///
/// The binding parameter is preserved so [`crate::Runtime::component`] can construct the correct
/// generated client without exposing a raw Wasmtime instance.
pub struct Component<B> {
    id: ComponentId,
    binding: PhantomData<fn() -> B>,
}

pub(crate) struct ExportInfo {
    pub(crate) interface: String,
    pub(crate) function: String,
    pub(crate) target: String,
    pub(crate) runtime_signature: Signature,
}

pub(crate) struct ComponentEntry {
    pub(crate) metadata: PluginMetadata,
    binding_exports: Vec<lockgate_schema::Export>,
    pub(crate) direct_imports: Vec<DirectImport>,
    pub(crate) wasi_imports: HashSet<String>,
    pub(crate) exports: Vec<ExportInfo>,
    pub(crate) host_imports: HashSet<&'static str>,
    pub(crate) component: WasmtimeComponent,
}

pub(crate) struct Catalog {
    identity: u64,
    engine: Engine,
    components: Vec<ComponentEntry>,
}

/// An error produced while constructing an application or admitting a component.
#[derive(Debug, Error)]
pub enum ApplicationError {
    #[error("artifact does not contain required `{PLUGIN_METADATA_SECTION}` plugin metadata")]
    MissingPluginMetadata,
    #[error("artifact contains more than one `{PLUGIN_METADATA_SECTION}` metadata section")]
    DuplicatePluginMetadata,
    #[error("artifact contains invalid `{PLUGIN_METADATA_SECTION}` plugin metadata")]
    InvalidPluginMetadata(#[source] anyhow::Error),
    #[error("failed to create the Wasmtime engine")]
    Engine(#[source] anyhow::Error),
    #[error("plugin `{plugin}` is not a valid supported component")]
    InvalidComponent {
        plugin: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("component handle does not belong to this application")]
    ForeignComponent,
    #[error("plugin `{plugin}` does not implement world `{world}`")]
    WorldMismatch {
        plugin: String,
        world: String,
        #[source]
        source: anyhow::Error,
    },
    #[error("component `{caller}` imports no interface exported by `{provider}")]
    NoMatchingImport { caller: String, provider: String },
    #[error("host interface `{interface}` is unavailable for component `{component}`")]
    HostImportUnavailable {
        component: String,
        interface: String,
    },
    #[error("component `{component}` does not import `wasi:http/client@0.3.0`")]
    OutboundHttpUnavailable { component: String },
    #[error("component `{component}` must be granted at least one outbound HTTP origin")]
    EmptyOutboundHttpOrigins { component: String },
    #[error(
        "invalid outbound HTTP origin `{origin}` for component `{component}`; expected an http(s) URL without a path, query, or fragment"
    )]
    InvalidOutboundHttpOrigin { component: String, origin: String },
    #[error("guest directory path must be normalized absolute POSIX: `{0}")]
    RelativeGuestPath(PathBuf),
    #[error("host directory does not exist or is not a directory: `{0}")]
    InvalidHostDirectory(PathBuf),
}

impl Catalog {
    pub(crate) fn new() -> Result<Self, ApplicationError> {
        let mut config = Config::new();
        config
            .wasm_component_model(true)
            .wasm_component_model_async(true)
            .wasm_component_model_async_stackful(true)
            .wasm_component_model_threading(true)
            .wasm_component_model_fixed_length_lists(true)
            .consume_fuel(true);
        let engine =
            Engine::new(&config).map_err(|error| ApplicationError::Engine(error.into()))?;
        Ok(Self {
            identity: NEXT_CATALOG.fetch_add(1, Ordering::Relaxed),
            engine,
            components: Vec::new(),
        })
    }

    /// Decodes, compiles, and admits an artifact against generated application roles.
    ///
    /// Admission requires every requested role export to exist in the component with the same
    /// WIT type. Additional component exports are allowed.
    pub(crate) fn add<S: Send + Sync + 'static, R: RoleSet<S>>(
        &mut self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<Component<R>, ApplicationError> {
        let mut entry = inspect(&self.engine, bytes.as_ref())?;
        validate_roles::<S, R>(&entry)?;
        R::for_each_role(&mut |_, _, host_imports, _| {
            entry.host_imports.extend(host_imports);
        });
        let id = self.insert(entry);
        Ok(Component {
            id,
            binding: PhantomData,
        })
    }

    pub(crate) fn supports<S: Send + Sync + 'static, R: RoleSet<S>>(
        &self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<bool, ApplicationError> {
        let entry = inspect(&self.engine, bytes.as_ref())?;
        match validate_roles::<S, R>(&entry) {
            Ok(()) => Ok(true),
            Err(ApplicationError::WorldMismatch { .. }) => Ok(false),
            Err(error) => Err(error),
        }
    }

    #[cfg(test)]
    pub(crate) fn add_untyped(
        &mut self,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ComponentId, ApplicationError> {
        let entry = inspect(&self.engine, bytes.as_ref())?;
        Ok(self.insert(entry))
    }

    fn insert(&mut self, entry: ComponentEntry) -> ComponentId {
        let id = ComponentId {
            catalog: self.identity,
            index: self.components.len(),
        };
        self.components.push(entry);
        id
    }

    pub(crate) fn component_ids(&self) -> Vec<ComponentId> {
        (0..self.components.len())
            .map(|index| ComponentId {
                catalog: self.identity,
                index,
            })
            .collect()
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }

    pub(crate) fn entry(&self, id: ComponentId) -> Result<&ComponentEntry, ApplicationError> {
        if id.catalog != self.identity {
            return Err(ApplicationError::ForeignComponent);
        }
        self.components
            .get(id.index)
            .ok_or(ApplicationError::ForeignComponent)
    }
}

fn validate_roles<S: Send + Sync + 'static, R: RoleSet<S>>(
    entry: &ComponentEntry,
) -> Result<(), ApplicationError> {
    let mut mismatch = None;
    R::for_each_role(&mut |world, exports, _, _| {
        if mismatch.is_none()
            && let Err(source) = validate_binding(exports, &entry.binding_exports)
        {
            mismatch = Some(ApplicationError::WorldMismatch {
                plugin: entry.metadata.id().into(),
                world: world.into(),
                source,
            });
        }
    });
    if let Some(error) = mismatch {
        return Err(error);
    }
    Ok(())
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

impl<B> Component<B> {
    pub(crate) fn id(self) -> ComponentId {
        self.id
    }

    pub(crate) fn cast<R>(self) -> Component<R> {
        Component {
            id: self.id,
            binding: PhantomData,
        }
    }
}

fn inspect(engine: &Engine, bytes: &[u8]) -> Result<ComponentEntry, ApplicationError> {
    let decoded =
        wit_component::decode(bytes).map_err(|source| ApplicationError::InvalidComponent {
            plugin: "unknown".into(),
            source,
        })?;
    let metadata = plugin_metadata(bytes)?;
    let plugin = metadata.id();
    let DecodedWasm::Component(resolve, world) = decoded else {
        return Err(ApplicationError::InvalidComponent {
            plugin: plugin.into(),
            source: anyhow::anyhow!("artifact is a core module, not a component"),
        });
    };
    validate_world_interfaces(&resolve, world, plugin)?;
    let binding_exports = lockgate_schema::world_exports(&resolve, world).map_err(|source| {
        ApplicationError::InvalidComponent {
            plugin: plugin.into(),
            source,
        }
    })?;
    let component = WasmtimeComponent::new(engine, bytes).map_err(|source| {
        ApplicationError::InvalidComponent {
            plugin: plugin.into(),
            source: source.into(),
        }
    })?;
    let mut direct_imports = Vec::new();
    let mut wasi_imports = HashSet::new();
    for item in resolve.worlds[world].imports.values() {
        let WorldItem::Interface { id, .. } = item else {
            continue;
        };
        let interface = resolve.id_of(*id).expect("named interface was validated");
        if interface.starts_with("wasi:") {
            wasi_imports.insert(interface);
            continue;
        }
        validate_introspectable_interface(&resolve, *id).map_err(|source| {
            ApplicationError::InvalidComponent {
                plugin: plugin.into(),
                source,
            }
        })?;
        let functions =
            interface_functions(engine, &component, &interface, true).map_err(|source| {
                ApplicationError::InvalidComponent {
                    plugin: plugin.into(),
                    source,
                }
            })?;
        direct_imports.push(DirectImport {
            interface,
            functions,
        });
    }
    let exported = exported_interface_names(&resolve, world, plugin)?;
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
            ApplicationError::InvalidComponent {
                plugin: plugin.into(),
                source,
            }
        })?;
        let functions =
            interface_functions(engine, &component, &interface, false).map_err(|source| {
                ApplicationError::InvalidComponent {
                    plugin: plugin.into(),
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
    Ok(ComponentEntry {
        metadata,
        binding_exports,
        direct_imports,
        wasi_imports,
        exports,
        component,
        host_imports: HashSet::new(),
    })
}

fn plugin_metadata(bytes: &[u8]) -> Result<PluginMetadata, ApplicationError> {
    let sections = wasmparser::Parser::new(0)
        .parse_all(bytes)
        .filter_map(|payload| match payload {
            Ok(wasmparser::Payload::CustomSection(section))
                if section.name() == PLUGIN_METADATA_SECTION =>
            {
                Some(Ok(section.data()))
            }
            Ok(_) => None,
            Err(error) => Some(Err(error)),
        })
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| ApplicationError::InvalidPluginMetadata(error.into()))?;
    let [bytes] = sections.as_slice() else {
        return match sections.len() {
            0 => Err(ApplicationError::MissingPluginMetadata),
            _ => Err(ApplicationError::DuplicatePluginMetadata),
        };
    };
    decode_plugin_metadata(bytes).map_err(ApplicationError::InvalidPluginMetadata)
}

#[cfg(test)]
pub(crate) fn with_test_plugin_metadata(mut bytes: Vec<u8>, id: &str) -> Vec<u8> {
    let metadata = PluginMetadata::new(id, id, "0.1.0").unwrap();
    let metadata = lockgate_schema::encode_plugin_metadata(&metadata).unwrap();
    let mut payload = Vec::new();
    encode_u32(PLUGIN_METADATA_SECTION.len() as u32, &mut payload);
    payload.extend_from_slice(PLUGIN_METADATA_SECTION.as_bytes());
    payload.extend_from_slice(&metadata);
    bytes.push(0);
    encode_u32(payload.len() as u32, &mut bytes);
    bytes.extend(payload);
    bytes
}

#[cfg(test)]
fn encode_u32(mut value: u32, output: &mut Vec<u8>) {
    loop {
        let mut byte = (value & 0x7f) as u8;
        value >>= 7;
        if value != 0 {
            byte |= 0x80;
        }
        output.push(byte);
        if value == 0 {
            break;
        }
    }
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
    plugin_id: &str,
) -> Result<(), ApplicationError> {
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
                .ok_or_else(|| ApplicationError::InvalidComponent {
                    plugin: plugin_id.into(),
                    source: anyhow::anyhow!("component contains an unnamed interface"),
                })?;
            if imported && name.starts_with("wasi:") {
                continue;
            }
            validate_introspectable_interface(resolve, *id).map_err(|source| {
                ApplicationError::InvalidComponent {
                    plugin: plugin_id.into(),
                    source,
                }
            })?;
        }
    }
    Ok(())
}

fn exported_interface_names(
    resolve: &Resolve,
    world: wit_parser::WorldId,
    plugin_id: &str,
) -> Result<Vec<String>, ApplicationError> {
    resolve.worlds[world]
        .exports
        .values()
        .map(|item| match item {
            WorldItem::Interface { id, .. } => {
                resolve
                    .id_of(*id)
                    .ok_or_else(|| ApplicationError::InvalidComponent {
                        plugin: plugin_id.into(),
                        source: anyhow::anyhow!("component contains an unnamed interface"),
                    })
            }
            _ => Err(ApplicationError::InvalidComponent {
                plugin: plugin_id.into(),
                source: anyhow::anyhow!("direct world functions and types are unsupported"),
            }),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::binding::{Binding, RoleSet};
    use wasmtime::component::Instance;
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    const WIT: &str = r#"package demo:catalog@0.1.0;

interface api { greet: func(name: string) -> string; }
interface health { ping: func() -> bool; }

world provider { export api; }
world multi-provider { export api; export health; }
world caller { import api; }"#;

    struct GreeterBinding;

    impl<S: Send + Sync + 'static> Binding<S> for GreeterBinding {
        const WORLD: &'static str = "greeter";
        const EXPORTS: &'static [BindingExport] = &[BindingExport {
            interface: "demo:catalog/api@0.1.0",
            item: "greet",
            signature: "freestanding(string)->string",
        }];
        const HOST_IMPORTS: &'static [&'static str] = &[];

        type Client<'runtime> = ();

        fn bind(
            _accessor: &wasmtime::component::Accessor<crate::__private::PluginStore<S>>,
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

    impl<S: Send + Sync + 'static> Binding<S> for HealthBinding {
        const WORLD: &'static str = "health-check";
        const EXPORTS: &'static [BindingExport] = &[BindingExport {
            interface: "demo:catalog/health@0.1.0",
            item: "ping",
            signature: "freestanding()->bool",
        }];
        const HOST_IMPORTS: &'static [&'static str] = &[];

        type Client<'runtime> = ();

        fn bind(
            _accessor: &wasmtime::component::Accessor<crate::__private::PluginStore<S>>,
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

    impl<S: Send + Sync + 'static> Binding<S> for MissingBinding {
        const WORLD: &'static str = "missing";
        const EXPORTS: &'static [BindingExport] = &[BindingExport {
            interface: "demo:catalog/missing@0.1.0",
            item: "run",
            signature: "freestanding()->unit",
        }];
        const HOST_IMPORTS: &'static [&'static str] = &[];

        type Client<'runtime> = ();

        fn bind(
            _accessor: &wasmtime::component::Accessor<crate::__private::PluginStore<S>>,
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

    macro_rules! impl_test_role_set {
        ($binding:ty) => {
            impl<S: Send + Sync + 'static> RoleSet<S> for $binding {
                type Handles = Component<Self>;

                #[allow(clippy::type_complexity)]
                fn for_each_role(
                    visitor: &mut dyn FnMut(
                        &'static str,
                        &'static [BindingExport],
                        &'static [&'static str],
                        fn(
                            &str,
                            &mut wasmtime::component::Linker<crate::__private::PluginStore<S>>,
                        ) -> anyhow::Result<()>,
                    ),
                ) {
                    visitor(
                        <Self as Binding<S>>::WORLD,
                        <Self as Binding<S>>::EXPORTS,
                        <Self as Binding<S>>::HOST_IMPORTS,
                        <Self as Binding<S>>::install_host_import,
                    );
                }

                fn handles(component: Component<Self>) -> Self::Handles {
                    component
                }
            }
        };
    }

    impl_test_role_set!(GreeterBinding);
    impl_test_role_set!(HealthBinding);
    impl_test_role_set!(MissingBinding);

    fn component_bytes(world_name: &str, plugin_id: &str) -> Vec<u8> {
        with_test_plugin_metadata(component_without_metadata(world_name), plugin_id)
    }

    fn component_without_metadata(world_name: &str) -> Vec<u8> {
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
            .add_untyped(component_bytes("provider", "greeter"))
            .unwrap();
        let _caller = catalog
            .add_untyped(component_bytes("caller", "caller"))
            .unwrap();
        let entry = catalog.entry(provider).unwrap();
        assert_eq!(entry.metadata.id(), "greeter");
        assert_eq!(entry.exports[0].target, "demo:catalog/api@0.1.0#greet");
        assert_eq!(catalog.components.len(), 2);
        catalog
            .add_untyped(component_bytes("provider", "greeter"))
            .unwrap();
        assert_eq!(catalog.components.len(), 3);
    }

    #[test]
    fn requires_embedded_plugin_metadata() {
        let mut catalog = Catalog::new().unwrap();
        let error = catalog
            .add_untyped(component_without_metadata("provider"))
            .unwrap_err();

        assert!(matches!(error, ApplicationError::MissingPluginMetadata));
    }

    #[test]
    fn rejects_handles_from_another_catalog() {
        let mut first = Catalog::new().unwrap();
        let provider = first
            .add_untyped(component_bytes("provider", "greeter"))
            .unwrap();
        let second = Catalog::new().unwrap();
        assert!(matches!(
            second.entry(provider),
            Err(ApplicationError::ForeignComponent)
        ));
    }

    #[test]
    fn adds_one_artifact_under_multiple_roles() {
        let mut catalog = Catalog::new().unwrap();
        let component = catalog
            .add::<(), (GreeterBinding, HealthBinding)>(component_bytes(
                "multi-provider",
                "combined",
            ))
            .unwrap();
        let (greeter, health) =
            <(GreeterBinding, HealthBinding) as RoleSet<()>>::handles(component);

        assert_eq!(greeter.id(), health.id());
        assert_eq!(catalog.components.len(), 1);
        assert_eq!(
            catalog.entry(health.id()).unwrap().metadata.id(),
            "combined"
        );
    }

    #[test]
    fn failed_role_set_addition_leaves_the_catalog_unchanged() {
        let mut catalog = Catalog::new().unwrap();
        let error = catalog
            .add::<(), (GreeterBinding, MissingBinding)>(component_bytes("provider", "greeter"))
            .unwrap_err();

        assert!(matches!(error, ApplicationError::WorldMismatch { .. }));
        assert!(catalog.components.is_empty());
    }
}
