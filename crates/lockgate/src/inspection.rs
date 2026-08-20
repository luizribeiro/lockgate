use std::{error::Error, fmt};

use lockgate_schema::sections::metadata::MetadataDecodeError;
use lockgate_schema::sections::needs::{NeedsDecodeError, NeedsEncodeError};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{NeedsDigest, NeedsManifest, PluginMetadata};
use wasmparser::{Encoding, Parser, Payload};
use wit_parser::{Resolve, WorldId, WorldItem, WorldKey, decoding::DecodedWasm};

/// Decoded plugin declarations suitable for listing and admission displays.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Inspection {
    metadata: PluginMetadata,
    needs: NeedsManifest,
    needs_digest: NeedsDigest,
    exported_interfaces: Vec<String>,
}

impl Inspection {
    pub(crate) fn new(
        metadata: PluginMetadata,
        needs: NeedsManifest,
        needs_digest: NeedsDigest,
        exported_interfaces: Vec<String>,
    ) -> Self {
        Self {
            metadata,
            needs,
            needs_digest,
            exported_interfaces,
        }
    }

    /// Returns the plugin's validated identity and display metadata.
    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    /// Returns the plugin's symbolic permission declaration.
    pub fn needs(&self) -> &NeedsManifest {
        &self.needs
    }

    /// Returns the digest of the canonical needs declaration.
    pub fn needs_digest(&self) -> NeedsDigest {
        self.needs_digest
    }

    /// Returns every exported interface name in component WIT order.
    pub fn exported_interfaces(&self) -> &[String] {
        &self.exported_interfaces
    }
}

/// Inspects custom sections and WIT without creating a Wasmtime engine.
pub fn inspect(bytes: &[u8]) -> Result<Inspection, InspectError> {
    let sections = decode_sections(bytes)?;
    let metadata = decode_metadata(&sections)?;
    let (needs, needs_digest) = decode_needs(&sections)?;
    let exported_interfaces = decode_exported_interfaces(bytes)?;
    Ok(Inspection::new(
        metadata,
        needs,
        needs_digest,
        exported_interfaces,
    ))
}

pub(crate) struct Sections<'a> {
    metadata: Option<&'a [u8]>,
    needs: Option<&'a [u8]>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ContainerKind {
    Module,
    Component,
}

pub(crate) fn decode_sections(bytes: &[u8]) -> Result<Sections<'_>, InspectError> {
    let mut encoding = None;
    let mut containers = Vec::new();
    let mut metadata = None;
    let mut needs = None;

    for payload in Parser::new(0).parse_all(bytes) {
        match payload.map_err(|error| InspectError::InvalidComponent {
            message: error.to_string(),
        })? {
            Payload::Version {
                encoding: found, ..
            } if encoding.is_none() => encoding = Some(found),
            Payload::ModuleSection { .. } => containers.push(ContainerKind::Module),
            Payload::ComponentSection { .. } => containers.push(ContainerKind::Component),
            Payload::End(_) if !containers.is_empty() => {
                containers.pop();
            }
            Payload::CustomSection(section)
                if matches!(containers.as_slice(), [] | [ContainerKind::Module]) =>
            {
                match section.name() {
                    PLUGIN_METADATA_SECTION => {
                        set_section(&mut metadata, section.data(), PLUGIN_METADATA_SECTION)?
                    }
                    PLUGIN_NEEDS_SECTION => {
                        set_section(&mut needs, section.data(), PLUGIN_NEEDS_SECTION)?
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }

    if encoding != Some(Encoding::Component) {
        return Err(InspectError::NotComponent);
    }
    Ok(Sections { metadata, needs })
}

fn set_section<'a>(
    slot: &mut Option<&'a [u8]>,
    bytes: &'a [u8],
    name: &'static str,
) -> Result<(), InspectError> {
    if slot.replace(bytes).is_some() {
        return Err(InspectError::DuplicateSection { name });
    }
    Ok(())
}

pub(crate) fn decode_metadata(sections: &Sections<'_>) -> Result<PluginMetadata, InspectError> {
    let bytes = sections.metadata.ok_or(InspectError::MissingMetadata)?;
    PluginMetadata::from_section_bytes(bytes).map_err(InspectError::Metadata)
}

pub(crate) fn decode_needs(
    sections: &Sections<'_>,
) -> Result<(NeedsManifest, NeedsDigest), InspectError> {
    let bytes = sections.needs.ok_or(InspectError::MissingNeeds)?;
    let needs = NeedsManifest::from_section_bytes(bytes).map_err(InspectError::Needs)?;
    let digest = NeedsDigest::compute(&needs).map_err(InspectError::NeedsDigest)?;
    Ok((needs, digest))
}

pub(crate) fn decode_exported_interfaces(bytes: &[u8]) -> Result<Vec<String>, InspectError> {
    let decoded =
        wit_parser::decoding::decode(bytes).map_err(|error| InspectError::InvalidWit {
            message: error.to_string(),
        })?;
    let DecodedWasm::Component(resolve, world) = decoded else {
        return Err(InspectError::NotComponent);
    };
    Ok(exported_interface_names(&resolve, world))
}

pub(crate) fn decode_imported_interfaces(bytes: &[u8]) -> Result<Vec<String>, InspectError> {
    let decoded =
        wit_parser::decoding::decode(bytes).map_err(|error| InspectError::InvalidWit {
            message: error.to_string(),
        })?;
    let DecodedWasm::Component(resolve, world) = decoded else {
        return Err(InspectError::NotComponent);
    };
    Ok(resolve.worlds[world]
        .imports
        .iter()
        .filter_map(|(key, item)| match item {
            WorldItem::Interface { .. } => Some(world_key_name(&resolve, key)),
            WorldItem::Function(_) | WorldItem::Type { .. } => None,
        })
        .collect())
}

pub(crate) fn exported_interface_names(resolve: &Resolve, world: WorldId) -> Vec<String> {
    resolve.worlds[world]
        .exports
        .iter()
        .filter_map(|(key, item)| match item {
            WorldItem::Interface { .. } => Some(world_key_name(resolve, key)),
            WorldItem::Function(_) | WorldItem::Type { .. } => None,
        })
        .collect()
}

pub(crate) fn world_key_name(resolve: &Resolve, key: &WorldKey) -> String {
    match key {
        WorldKey::Name(name) => name.clone(),
        WorldKey::Interface(id) => resolve
            .id_of(*id)
            .unwrap_or_else(|| format!("interface-{}", id.index())),
    }
}

/// A pure inspection failure.
#[derive(Debug)]
#[non_exhaustive]
pub enum InspectError {
    NotComponent,
    InvalidComponent { message: String },
    InvalidWit { message: String },
    MissingMetadata,
    MissingNeeds,
    DuplicateSection { name: &'static str },
    Metadata(MetadataDecodeError),
    Needs(NeedsDecodeError),
    NeedsDigest(NeedsEncodeError),
}

impl fmt::Display for InspectError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::NotComponent => formatter.write_str("input is not a WebAssembly component"),
            Self::InvalidComponent { message } => {
                write!(
                    formatter,
                    "input is not a valid WebAssembly component: {message}"
                )
            }
            Self::InvalidWit { message } => {
                write!(formatter, "component WIT could not be decoded: {message}")
            }
            Self::MissingMetadata => write!(
                formatter,
                "component is missing required `{PLUGIN_METADATA_SECTION}` metadata section"
            ),
            Self::MissingNeeds => write!(
                formatter,
                "component is missing required `{PLUGIN_NEEDS_SECTION}` needs section"
            ),
            Self::DuplicateSection { name } => {
                write!(formatter, "component contains duplicate `{name}` sections")
            }
            Self::Metadata(error) => write!(formatter, "plugin metadata is invalid: {error}"),
            Self::Needs(error) => write!(formatter, "plugin needs manifest is invalid: {error}"),
            Self::NeedsDigest(error) => {
                write!(
                    formatter,
                    "plugin needs digest could not be computed: {error}"
                )
            }
        }
    }
}

impl Error for InspectError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Metadata(error) => Some(error),
            Self::Needs(error) => Some(error),
            Self::NeedsDigest(error) => Some(error),
            _ => None,
        }
    }
}
