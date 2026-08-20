use std::{collections::BTreeMap, error::Error, fmt};

use serde::Serialize;

use crate::needs::{NeedKind, NeedsManifest, NeedsManifestValidationError};

use super::{check_payload_size, format_payload_too_large};

mod decode_error;
mod decoder;

pub use decode_error::NeedsDecodeError;

/// Encodes a validated manifest as canonical sorted-key JSON.
fn encode(manifest: &NeedsManifest) -> Result<Vec<u8>, NeedsEncodeError> {
    manifest
        .validate()
        .map_err(NeedsEncodeError::InvalidManifest)?;
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
    let payload = serde_json::to_vec(&WireManifest {
        format: manifest.format,
        optional,
        reasons,
        required,
    })
    .map_err(NeedsEncodeError::Serialization)?;
    check_payload_size(payload.len()).map_err(|error| NeedsEncodeError::PayloadTooLarge {
        actual_bytes: error.actual_bytes,
        max_bytes: error.max_bytes,
    })?;
    Ok(payload)
}

#[derive(Serialize)]
struct WireManifest<'a> {
    // Alphabetical field order is load-bearing because the digest covers these exact JSON bytes.
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
pub enum NeedsEncodeError {
    InvalidManifest(NeedsManifestValidationError),
    Serialization(serde_json::Error),
    PayloadTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },
}

impl fmt::Display for NeedsEncodeError {
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
            Self::PayloadTooLarge {
                actual_bytes,
                max_bytes,
            } => format_payload_too_large(formatter, "needs manifest", *actual_bytes, *max_bytes),
        }
    }
}

impl Error for NeedsEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidManifest(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::PayloadTooLarge { .. } => None,
        }
    }
}

impl NeedsManifest {
    /// Encodes this manifest as canonical sorted-key section JSON.
    pub fn to_section_bytes(&self) -> Result<Vec<u8>, NeedsEncodeError> {
        encode(self)
    }

    /// Decodes and validates a `lockgate:needs` custom-section payload.
    pub fn from_section_bytes(bytes: &[u8]) -> Result<Self, NeedsDecodeError> {
        decoder::decode(bytes)
    }
}
