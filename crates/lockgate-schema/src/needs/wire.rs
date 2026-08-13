use std::{collections::BTreeMap, error::Error, fmt};

use serde::Serialize;

use super::{NeedKind, NeedsManifest, NeedsManifestValidationError};

mod decode_error;

pub use decode_error::NeedsManifestDecodeError;

/// Encodes a validated manifest as canonical sorted-key JSON.
pub fn encode_needs_manifest(
    manifest: &NeedsManifest,
) -> Result<Vec<u8>, NeedsManifestEncodeError> {
    manifest
        .validate()
        .map_err(NeedsManifestEncodeError::InvalidManifest)?;
    let mut required = BTreeMap::new();
    let mut optional = BTreeMap::new();
    let mut reasons = BTreeMap::new();
    for (entries, output) in [
        (&manifest.required, &mut required),
        (&manifest.optional, &mut optional),
    ] {
        for entry in entries {
            let value = match &entry.kind {
                NeedKind::Flag => WireNeed::Flag(true),
                NeedKind::Scoped(scopes) => {
                    WireNeed::Scoped(scopes.iter().map(|scope| scope.to_wire()).collect())
                }
            };
            output.insert(entry.atom.to_string(), value);
            if let Some(reason) = entry.reason.as_deref() {
                reasons.insert(entry.atom.to_string(), reason);
            }
        }
    }
    serde_json::to_vec(&WireManifest {
        format: manifest.format,
        optional,
        reasons,
        required,
    })
    .map_err(NeedsManifestEncodeError::Serialization)
}

#[derive(Serialize)]
struct WireManifest<'a> {
    format: u32,
    optional: BTreeMap<String, WireNeed>,
    reasons: BTreeMap<String, &'a str>,
    required: BTreeMap<String, WireNeed>,
}

#[derive(Serialize)]
#[serde(untagged)]
enum WireNeed {
    Flag(bool),
    Scoped(Vec<String>),
}

/// A failure while encoding a needs manifest custom-section payload.
#[derive(Debug)]
#[non_exhaustive]
pub enum NeedsManifestEncodeError {
    InvalidManifest(NeedsManifestValidationError),
    Serialization(serde_json::Error),
}

impl fmt::Display for NeedsManifestEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidManifest(error) => {
                write!(formatter, "needs manifest failed validation: {error}")
            }
            Self::Serialization(error) => {
                write!(
                    formatter,
                    "needs manifest could not be encoded as JSON: {error}"
                )
            }
        }
    }
}

impl Error for NeedsManifestEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidManifest(error) => Some(error),
            Self::Serialization(error) => Some(error),
        }
    }
}
