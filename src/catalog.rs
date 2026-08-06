//! Component discovery and application-assigned identity.
//! A catalog hashes exact artifacts and exposes the imports and exports decoded from their WIT.

use crate::plugin::{
    DirectImport, Signature, interface_functions, type_name, validate_wit_interface,
};
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    fmt,
    sync::atomic::{AtomicU64, Ordering},
};
use thiserror::Error;
use wasmtime::{Config, Engine, component::Component};
use wit_component::DecodedWasm;
use wit_parser::{Resolve, WorldItem};

static NEXT_CATALOG: AtomicU64 = AtomicU64::new(1);

/// An opaque component handle that can only be created by a [`Catalog`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ComponentId {
    catalog: u64,
    index: usize,
}

/// An opaque handle for one exported function in a catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ExportId {
    catalog: u64,
    component: usize,
    export: usize,
}

/// The SHA-256 digest of the exact component bytes supplied to a catalog.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct ArtifactDigest([u8; 32]);

/// A discovered function signature rendered from Wasmtime's structural component types.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FunctionSignature {
    params: Vec<String>,
    results: Vec<String>,
}

/// Metadata for one exported component function.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportInfo {
    interface: String,
    function: String,
    target: String,
    signature: FunctionSignature,
    pub(crate) runtime_signature: Signature,
}

/// Read-only metadata for a cataloged component.
pub struct ComponentInfo<'a> {
    id: ComponentId,
    entry: &'a ComponentEntry,
}

pub(crate) struct ComponentEntry {
    pub(crate) name: String,
    digest: ArtifactDigest,
    pub(crate) imports: Vec<String>,
    pub(crate) direct_imports: Vec<DirectImport>,
    pub(crate) exports: Vec<ExportInfo>,
    pub(crate) component: Component,
}

struct InspectedComponent {
    imports: Vec<String>,
    direct_imports: Vec<DirectImport>,
    exports: Vec<ExportInfo>,
    component: Component,
}

/// A collection of decoded, compiled component artifacts.
pub struct Catalog {
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
    #[error("component `{component}` does not export `{target}`")]
    ExportNotFound { component: String, target: String },
}

impl Catalog {
    /// Creates an empty catalog with component-model support enabled.
    pub fn new() -> Result<Self, CatalogError> {
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

    /// Decodes and compiles an artifact under an application-assigned name.
    pub fn add(
        &mut self,
        name: impl Into<String>,
        bytes: impl AsRef<[u8]>,
    ) -> Result<ComponentId, CatalogError> {
        let name = name.into();
        if name.trim().is_empty() {
            return Err(CatalogError::EmptyName);
        }
        if self.names.contains_key(&name) {
            return Err(CatalogError::DuplicateName(name));
        }

        let bytes = bytes.as_ref();
        let inspected = inspect(&self.engine, &name, bytes)?;
        let id = ComponentId {
            catalog: self.identity,
            index: self.components.len(),
        };
        self.components.push(ComponentEntry {
            name: name.clone(),
            digest: ArtifactDigest(Sha256::digest(bytes).into()),
            imports: inspected.imports,
            direct_imports: inspected.direct_imports,
            exports: inspected.exports,
            component: inspected.component,
        });
        self.names.insert(name, id);
        Ok(id)
    }

    /// Returns metadata for a component handle from this catalog.
    pub fn component(&self, id: ComponentId) -> Result<ComponentInfo<'_>, CatalogError> {
        let entry = self.entry(id)?;
        Ok(ComponentInfo { id, entry })
    }

    /// Resolves an exact `package/interface@version#function` export.
    pub fn export(&self, component: ComponentId, target: &str) -> Result<ExportId, CatalogError> {
        let entry = self.entry(component)?;
        let export = entry
            .exports
            .iter()
            .position(|export| export.target == target)
            .ok_or_else(|| CatalogError::ExportNotFound {
                component: entry.name.clone(),
                target: target.into(),
            })?;
        Ok(ExportId {
            catalog: self.identity,
            component: component.index,
            export,
        })
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

    pub(crate) fn export_entry(&self, id: ExportId) -> Result<&ExportInfo, CatalogError> {
        if id.catalog != self.identity {
            return Err(CatalogError::ForeignComponent);
        }
        self.components
            .get(id.component)
            .and_then(|component| component.exports.get(id.export))
            .ok_or(CatalogError::ForeignComponent)
    }

    pub(crate) fn export_component(&self, id: ExportId) -> Result<ComponentId, CatalogError> {
        self.export_entry(id)?;
        Ok(ComponentId {
            catalog: self.identity,
            index: id.component,
        })
    }
}

impl ComponentInfo<'_> {
    pub fn id(&self) -> ComponentId {
        self.id
    }

    pub fn name(&self) -> &str {
        &self.entry.name
    }

    pub fn digest(&self) -> ArtifactDigest {
        self.entry.digest
    }

    pub fn imports(&self) -> &[String] {
        &self.entry.imports
    }

    pub fn exports(&self) -> &[ExportInfo] {
        &self.entry.exports
    }
}

impl ExportInfo {
    pub fn interface(&self) -> &str {
        &self.interface
    }

    pub fn function(&self) -> &str {
        &self.function
    }

    pub fn target(&self) -> &str {
        &self.target
    }

    pub fn signature(&self) -> &FunctionSignature {
        &self.signature
    }
}

impl FunctionSignature {
    pub fn params(&self) -> &[String] {
        &self.params
    }

    pub fn results(&self) -> &[String] {
        &self.results
    }
}

impl fmt::Display for ArtifactDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        for byte in self.0 {
            write!(formatter, "{byte:02x}")?;
        }
        Ok(())
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
    let component =
        Component::new(engine, bytes).map_err(|source| CatalogError::InvalidComponent {
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
        if interface.starts_with("wasi:") || interface == "tangent:core/registry@0.1.0" {
            continue;
        }
        validate_wit_interface(&resolve, *id).map_err(|source| CatalogError::InvalidComponent {
            name: name.into(),
            source,
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
        validate_wit_interface(&resolve, interface_id).map_err(|source| {
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
                signature: FunctionSignature {
                    params: signature.params.iter().map(type_name).collect(),
                    results: signature.results.iter().map(type_name).collect(),
                },
                runtime_signature: signature,
            }
        }));
    }
    Ok(InspectedComponent {
        imports,
        direct_imports,
        exports,
        component,
    })
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
            if imported && (name.starts_with("wasi:") || name == "tangent:core/registry@0.1.0") {
                continue;
            }
            validate_wit_interface(resolve, *id).map_err(|source| {
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
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    const WIT: &str = r#"package demo:catalog@0.1.0;

interface api { greet: func(name: string) -> string; }

world provider { export api; }
world caller { import api; }"#;

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
        let provider = catalog.add("greeter", component_bytes("provider")).unwrap();
        let info = catalog.component(provider).unwrap();
        assert_eq!(info.name(), "greeter");
        assert_eq!(info.digest().to_string().len(), 64);
        assert_eq!(info.exports()[0].target(), "demo:catalog/api@0.1.0#greet");
        assert_eq!(info.exports()[0].signature().params(), ["string"]);
        assert_eq!(info.exports()[0].signature().results(), ["string"]);
        assert!(catalog.add("greeter", component_bytes("provider")).is_err());
    }

    #[test]
    fn rejects_handles_from_another_catalog() {
        let mut first = Catalog::new().unwrap();
        let provider = first.add("greeter", component_bytes("provider")).unwrap();
        let second = Catalog::new().unwrap();
        assert!(matches!(
            second.component(provider),
            Err(CatalogError::ForeignComponent)
        ));
    }
}
