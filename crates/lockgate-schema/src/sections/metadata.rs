use std::{error::Error, fmt};

use crate::{PluginMetadata, PluginMetadataValidationError};

use super::check_payload_size;

/// A failure while encoding plugin metadata for its custom section.
#[derive(Debug)]
#[non_exhaustive]
pub enum PluginMetadataEncodeError {
    /// The metadata does not satisfy the wire schema's semantic rules.
    InvalidMetadata(PluginMetadataValidationError),
    /// The validated metadata could not be serialized as JSON.
    Serialization(serde_json::Error),
    /// The encoded custom-section payload exceeds the wire ceiling.
    PayloadTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },
}

impl fmt::Display for PluginMetadataEncodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::InvalidMetadata(error) => {
                write!(formatter, "plugin metadata failed validation: {error}")
            }
            Self::Serialization(error) => write!(
                formatter,
                "plugin metadata could not be encoded as JSON: {error}"
            ),
            Self::PayloadTooLarge {
                actual_bytes,
                max_bytes,
            } => write!(
                formatter,
                "plugin metadata payload is {actual_bytes} bytes; maximum is {max_bytes} bytes"
            ),
        }
    }
}

impl Error for PluginMetadataEncodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidMetadata(error) => Some(error),
            Self::Serialization(error) => Some(error),
            Self::PayloadTooLarge { .. } => None,
        }
    }
}

/// A failure while decoding plugin metadata from its custom section.
#[derive(Debug)]
#[non_exhaustive]
pub enum PluginMetadataDecodeError {
    /// The custom-section payload exceeds the wire ceiling.
    PayloadTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },
    /// The section payload is not valid JSON for the metadata wire schema.
    InvalidJson(serde_json::Error),
    /// The decoded fields do not satisfy the wire schema's semantic rules.
    InvalidMetadata(PluginMetadataValidationError),
}

impl fmt::Display for PluginMetadataDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge {
                actual_bytes,
                max_bytes,
            } => write!(
                formatter,
                "plugin metadata payload is {actual_bytes} bytes; maximum is {max_bytes} bytes"
            ),
            Self::InvalidJson(error) => {
                write!(
                    formatter,
                    "plugin metadata is not valid schema JSON: {error}"
                )
            }
            Self::InvalidMetadata(error) => {
                write!(formatter, "plugin metadata failed validation: {error}")
            }
        }
    }
}

impl Error for PluginMetadataDecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PayloadTooLarge { .. } => None,
            Self::InvalidJson(error) => Some(error),
            Self::InvalidMetadata(error) => Some(error),
        }
    }
}

/// Encodes validated metadata as the JSON payload of `lockgate:plugin`.
pub fn encode_plugin_metadata(
    metadata: &PluginMetadata,
) -> Result<Vec<u8>, PluginMetadataEncodeError> {
    metadata
        .validate()
        .map_err(PluginMetadataEncodeError::InvalidMetadata)?;
    let payload = serde_json::to_vec(metadata).map_err(PluginMetadataEncodeError::Serialization)?;
    check_payload_size(payload.len()).map_err(|error| {
        PluginMetadataEncodeError::PayloadTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    Ok(payload)
}

/// Decodes and validates the JSON payload of `lockgate:plugin`.
pub fn decode_plugin_metadata(bytes: &[u8]) -> Result<PluginMetadata, PluginMetadataDecodeError> {
    check_payload_size(bytes.len()).map_err(|error| {
        PluginMetadataDecodeError::PayloadTooLarge {
            actual_bytes: error.actual_bytes,
            max_bytes: error.max_bytes,
        }
    })?;
    let metadata: PluginMetadata =
        serde_json::from_slice(bytes).map_err(PluginMetadataDecodeError::InvalidJson)?;
    metadata
        .validate()
        .map_err(PluginMetadataDecodeError::InvalidMetadata)?;
    Ok(metadata)
}
