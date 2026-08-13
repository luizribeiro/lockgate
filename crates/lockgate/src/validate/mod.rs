use std::{error::Error, fmt};

use wit_parser::decoding::{DecodedWasm, decode};

/// Validates the value-only rule directly against a component's decoded WIT.
pub(crate) fn validate_value_only_exports(bytes: &[u8]) -> Result<(), ValidationError> {
    let decoded = decode(bytes).map_err(|error| ValidationError::Decode {
        message: error.to_string(),
    })?;
    let DecodedWasm::Component(_resolve, _world) = decoded else {
        return Err(ValidationError::NotComponent);
    };
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ValidationError {
    Decode { message: String },
    NotComponent,
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode { message } => {
                write!(formatter, "failed to decode component WIT: {message}")
            }
            Self::NotComponent => formatter.write_str("input is a WIT package, not a component"),
        }
    }
}

impl Error for ValidationError {}

#[cfg(test)]
mod tests;
