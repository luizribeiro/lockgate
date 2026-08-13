use std::{error::Error, fmt};

use wit_parser::{
    Resolve, WorldId, WorldItem, WorldKey,
    decoding::{DecodedWasm, decode},
};

/// Validates the value-only rule directly against a component's decoded WIT.
pub(crate) fn validate_value_only_exports(bytes: &[u8]) -> Result<(), ValidationError> {
    let decoded = decode(bytes).map_err(classify_decode_error)?;
    let DecodedWasm::Component(resolve, world) = decoded else {
        return Err(ValidationError::NotComponent);
    };
    validate_world(&resolve, world)
}

fn classify_decode_error(error: anyhow::Error) -> ValidationError {
    let message = error.to_string();
    if let Some(name) = message
        .strip_prefix("component export `")
        .and_then(|message| message.strip_suffix("` was not a function or instance"))
    {
        return ValidationError::unsupported(
            "root",
            format!("<type {name}>"),
            "world-level exported type",
        );
    }
    ValidationError::Decode { message }
}

fn validate_world(resolve: &Resolve, world: WorldId) -> Result<(), ValidationError> {
    let world = &resolve.worlds[world];
    for (key, item) in &world.exports {
        let export_name = match key {
            WorldKey::Name(name) => name.clone(),
            WorldKey::Interface(id) => resolve
                .id_of(*id)
                .unwrap_or_else(|| format!("interface-{}", id.index())),
        };
        match item {
            WorldItem::Interface { .. } => {}
            WorldItem::Function(_) => {
                return Err(ValidationError::unsupported(
                    &world.name,
                    export_name,
                    "world-level exported function",
                ));
            }
            WorldItem::Type { .. } => {
                return Err(ValidationError::unsupported(
                    &world.name,
                    format!("<type {export_name}>"),
                    "world-level exported type",
                ));
            }
        }
    }
    Ok(())
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum ValidationError {
    Decode {
        message: String,
    },
    NotComponent,
    UnsupportedExport {
        interface: String,
        function: String,
        offending_type: String,
    },
}

impl ValidationError {
    fn unsupported(
        interface: impl Into<String>,
        function: impl Into<String>,
        offending_type: impl Into<String>,
    ) -> Self {
        Self::UnsupportedExport {
            interface: interface.into(),
            function: function.into(),
            offending_type: offending_type.into(),
        }
    }
}

impl fmt::Display for ValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Decode { message } => {
                write!(formatter, "failed to decode component WIT: {message}")
            }
            Self::NotComponent => formatter.write_str("input is a WIT package, not a component"),
            Self::UnsupportedExport {
                interface,
                function,
                offending_type,
            } => write!(
                formatter,
                "unsupported export `{interface}#{function}`: offending type `{offending_type}` cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature"
            ),
        }
    }
}

impl Error for ValidationError {}

#[cfg(test)]
mod tests;
