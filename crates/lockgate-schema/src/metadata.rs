use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::text::{DisplayStringViolation, classify_disallowed_character, validate_display_string};

const PLUGIN_METADATA_FORMAT: u32 = 1;
const MAX_ID_BYTES: usize = 128;
const MAX_NAME_BYTES: usize = 128;
const MAX_VERSION_BYTES: usize = 128;
const MAX_LICENSE_BYTES: usize = 512;
const MAX_LONG_DISPLAY_FIELD_BYTES: usize = 2048;

/// Language-neutral identity and display metadata embedded in a plugin artifact.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PluginMetadata {
    format: u32,
    id: String,
    name: String,
    version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    description: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    license: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    repository: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    homepage: Option<String>,
}

impl PluginMetadata {
    /// Creates and validates required plugin metadata.
    pub fn new(
        id: impl Into<String>,
        name: impl Into<String>,
        version: impl Into<String>,
    ) -> Result<Self, PluginMetadataValidationError> {
        let metadata = Self {
            format: PLUGIN_METADATA_FORMAT,
            id: id.into(),
            name: name.into(),
            version: version.into(),
            description: None,
            license: None,
            repository: None,
            homepage: None,
        };
        metadata.validate()?;
        Ok(metadata)
    }

    /// Adds a human-readable description.
    pub fn with_description(mut self, value: impl Into<String>) -> Self {
        self.description = Some(value.into());
        self
    }

    /// Adds a license identifier or expression.
    pub fn with_license(mut self, value: impl Into<String>) -> Self {
        self.license = Some(value.into());
        self
    }

    /// Adds the plugin's source repository URL.
    pub fn with_repository(mut self, value: impl Into<String>) -> Self {
        self.repository = Some(value.into());
        self
    }

    /// Adds the plugin's homepage URL.
    pub fn with_homepage(mut self, value: impl Into<String>) -> Self {
        self.homepage = Some(value.into());
        self
    }

    /// Returns the stable machine-readable plugin identifier.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// Returns the human-readable plugin name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Returns the plugin implementation's opaque display version.
    pub fn version(&self) -> &str {
        &self.version
    }

    /// Returns the optional human-readable description.
    pub fn description(&self) -> Option<&str> {
        self.description.as_deref()
    }

    /// Returns the optional license identifier or expression.
    pub fn license(&self) -> Option<&str> {
        self.license.as_deref()
    }

    /// Returns the optional source repository URL.
    pub fn repository(&self) -> Option<&str> {
        self.repository.as_deref()
    }

    /// Returns the optional homepage URL.
    pub fn homepage(&self) -> Option<&str> {
        self.homepage.as_deref()
    }

    pub(crate) fn validate(&self) -> Result<(), PluginMetadataValidationError> {
        if self.format != PLUGIN_METADATA_FORMAT {
            return Err(PluginMetadataValidationError::UnsupportedFormat { found: self.format });
        }
        validate_display_field(PluginMetadataField::Id, &self.id, MAX_ID_BYTES)?;
        validate_display_field(PluginMetadataField::Name, &self.name, MAX_NAME_BYTES)?;
        validate_display_field(
            PluginMetadataField::Version,
            &self.version,
            MAX_VERSION_BYTES,
        )?;
        for (field, value, max_bytes) in [
            (
                PluginMetadataField::Description,
                self.description.as_deref(),
                MAX_LONG_DISPLAY_FIELD_BYTES,
            ),
            (
                PluginMetadataField::License,
                self.license.as_deref(),
                MAX_LICENSE_BYTES,
            ),
            (
                PluginMetadataField::Repository,
                self.repository.as_deref(),
                MAX_LONG_DISPLAY_FIELD_BYTES,
            ),
            (
                PluginMetadataField::Homepage,
                self.homepage.as_deref(),
                MAX_LONG_DISPLAY_FIELD_BYTES,
            ),
        ] {
            if let Some(value) = value {
                validate_display_field(field, value, max_bytes)?;
            }
        }
        Ok(())
    }
}

/// A semantic validation failure in plugin metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginMetadataValidationError {
    /// The wire format version is not supported.
    UnsupportedFormat { found: u32 },
    /// A field is empty or consists only of whitespace.
    EmptyField { field: PluginMetadataField },
    /// A field contains a character that is unsafe in consent displays.
    DisallowedCharacter {
        field: PluginMetadataField,
        byte_index: usize,
        character: char,
    },
    /// A field exceeds its UTF-8 byte limit.
    FieldTooLong {
        field: PluginMetadataField,
        max_bytes: usize,
    },
}

impl fmt::Display for PluginMetadataValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { found } => {
                write!(formatter, "unsupported plugin metadata format {found}")
            }
            Self::EmptyField { field } => {
                write!(formatter, "plugin {field} must not be empty")
            }
            Self::DisallowedCharacter {
                field,
                byte_index,
                character,
            } if *field == PluginMetadataField::Id && character.is_whitespace() => {
                write!(
                    formatter,
                    "plugin id contains whitespace at byte {byte_index}"
                )
            }
            Self::DisallowedCharacter {
                field,
                byte_index,
                character,
            } => write!(
                formatter,
                "plugin {field} contains a {} at byte {byte_index}",
                classify_disallowed_character(*character)
                    .expect("stored metadata character must be disallowed")
            ),
            Self::FieldTooLong { field, max_bytes } => {
                write!(formatter, "plugin {field} exceeds {max_bytes} UTF-8 bytes")
            }
        }
    }
}

impl Error for PluginMetadataValidationError {}

/// A field in the plugin metadata wire schema.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum PluginMetadataField {
    Id,
    Name,
    Version,
    Description,
    License,
    Repository,
    Homepage,
}

impl fmt::Display for PluginMetadataField {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Id => "id",
            Self::Name => "name",
            Self::Version => "version",
            Self::Description => "description",
            Self::License => "license",
            Self::Repository => "repository",
            Self::Homepage => "homepage",
        })
    }
}

fn validate_display_field(
    field: PluginMetadataField,
    value: &str,
    max_bytes: usize,
) -> Result<(), PluginMetadataValidationError> {
    validate_display_string(value, max_bytes, |character| {
        field == PluginMetadataField::Id && character.is_whitespace()
    })
    .map_err(|violation| match violation {
        DisplayStringViolation::Empty => PluginMetadataValidationError::EmptyField { field },
        DisplayStringViolation::DisallowedCharacter {
            byte_index,
            character,
        } => PluginMetadataValidationError::DisallowedCharacter {
            field,
            byte_index,
            character,
        },
        DisplayStringViolation::TooLong { max_bytes } => {
            PluginMetadataValidationError::FieldTooLong { field, max_bytes }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> PluginMetadata {
        PluginMetadata::new("com.example.greeter", "Greeter", "1.2.3").unwrap()
    }

    fn metadata_with_field(field: PluginMetadataField, value: String) -> PluginMetadata {
        let mut metadata = metadata();
        match field {
            PluginMetadataField::Id => metadata.id = value,
            PluginMetadataField::Name => metadata.name = value,
            PluginMetadataField::Version => metadata.version = value,
            PluginMetadataField::Description => metadata.description = Some(value),
            PluginMetadataField::License => metadata.license = Some(value),
            PluginMetadataField::Repository => metadata.repository = Some(value),
            PluginMetadataField::Homepage => metadata.homepage = Some(value),
        }
        metadata
    }

    #[test]
    fn constructs_metadata_with_every_wire_field() {
        let metadata = metadata()
            .with_description("Returns greetings")
            .with_license("MIT OR Apache-2.0")
            .with_repository("https://example.com/repository")
            .with_homepage("https://example.com");

        assert_eq!(metadata.id(), "com.example.greeter");
        assert_eq!(metadata.name(), "Greeter");
        assert_eq!(metadata.version(), "1.2.3");
        assert_eq!(metadata.description(), Some("Returns greetings"));
        assert_eq!(metadata.license(), Some("MIT OR Apache-2.0"));
        assert_eq!(
            metadata.repository(),
            Some("https://example.com/repository")
        );
        assert_eq!(metadata.homepage(), Some("https://example.com"));
        assert_eq!(metadata.format, PLUGIN_METADATA_FORMAT);
        metadata.validate().unwrap();
    }

    #[test]
    fn rejects_empty_required_fields() {
        for (result, field) in [
            (
                PluginMetadata::new("", "Greeter", "1.2.3"),
                PluginMetadataField::Id,
            ),
            (
                PluginMetadata::new("plugin", " \t", "1.2.3"),
                PluginMetadataField::Name,
            ),
            (
                PluginMetadata::new("plugin", "Greeter", ""),
                PluginMetadataField::Version,
            ),
        ] {
            assert_eq!(
                result.unwrap_err(),
                PluginMetadataValidationError::EmptyField { field }
            );
        }
    }

    #[test]
    fn accepts_surrounding_whitespace_in_display_fields() {
        assert!(PluginMetadata::new("plugin", " Greeter ", " 1.2.3 ").is_ok());
        assert!(PluginMetadata::new("plugin", " postgres", "1.2.3").is_ok());
    }

    #[test]
    fn rejects_whitespace_inside_the_plugin_id() {
        assert_eq!(
            PluginMetadata::new("com.example\u{2003}greeter", "Greeter", "1.2.3").unwrap_err(),
            PluginMetadataValidationError::DisallowedCharacter {
                field: PluginMetadataField::Id,
                byte_index: 11,
                character: '\u{2003}',
            }
        );
    }

    #[test]
    fn rejects_multiline_versions() {
        let error = PluginMetadata::new("com.example.greeter", "Greeter", "release\ncandidate")
            .unwrap_err();
        assert_eq!(
            error,
            PluginMetadataValidationError::DisallowedCharacter {
                field: PluginMetadataField::Version,
                byte_index: 7,
                character: '\n',
            }
        );
        assert_eq!(
            error.to_string(),
            "plugin version contains a line break at byte 7"
        );
    }

    #[test]
    fn rejects_disallowed_characters_in_versions() {
        for (version, expected) in [
            (
                "release\u{7}candidate",
                PluginMetadataValidationError::DisallowedCharacter {
                    field: PluginMetadataField::Version,
                    byte_index: 7,
                    character: '\u{7}',
                },
            ),
            (
                "release\u{202e}candidate",
                PluginMetadataValidationError::DisallowedCharacter {
                    field: PluginMetadataField::Version,
                    byte_index: 7,
                    character: '\u{202e}',
                },
            ),
        ] {
            assert_eq!(
                PluginMetadata::new("com.example.greeter", "Greeter", version).unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn bounds_version_utf8_bytes() {
        assert!(PluginMetadata::new("plugin", "Plugin", "x".repeat(128)).is_ok());
        assert_eq!(
            PluginMetadata::new("plugin", "Plugin", "x".repeat(129)).unwrap_err(),
            PluginMetadataValidationError::FieldTooLong {
                field: PluginMetadataField::Version,
                max_bytes: 128,
            }
        );
    }

    #[test]
    fn rejects_format_characters_in_every_display_field() {
        for field in [
            PluginMetadataField::Id,
            PluginMetadataField::Name,
            PluginMetadataField::Version,
            PluginMetadataField::Description,
            PluginMetadataField::License,
            PluginMetadataField::Repository,
            PluginMetadataField::Homepage,
        ] {
            let error = metadata_with_field(field, "pre\u{202e}post".to_string())
                .validate()
                .unwrap_err();
            assert_eq!(
                error,
                PluginMetadataValidationError::DisallowedCharacter {
                    field,
                    byte_index: 3,
                    character: '\u{202e}',
                }
            );
            assert_eq!(
                error.to_string(),
                format!("plugin {field} contains a format character at byte 3")
            );
        }
    }

    #[test]
    fn bounds_every_display_field_by_utf8_bytes() {
        for (field, max_bytes) in [
            (PluginMetadataField::Id, MAX_ID_BYTES),
            (PluginMetadataField::Name, MAX_NAME_BYTES),
            (PluginMetadataField::Version, MAX_VERSION_BYTES),
            (
                PluginMetadataField::Description,
                MAX_LONG_DISPLAY_FIELD_BYTES,
            ),
            (PluginMetadataField::License, MAX_LICENSE_BYTES),
            (
                PluginMetadataField::Repository,
                MAX_LONG_DISPLAY_FIELD_BYTES,
            ),
            (PluginMetadataField::Homepage, MAX_LONG_DISPLAY_FIELD_BYTES),
        ] {
            metadata_with_field(field, "x".repeat(max_bytes))
                .validate()
                .unwrap();
            assert_eq!(
                metadata_with_field(field, "x".repeat(max_bytes + 1))
                    .validate()
                    .unwrap_err(),
                PluginMetadataValidationError::FieldTooLong { field, max_bytes }
            );
        }
    }

    #[test]
    fn rejects_whitespace_only_optional_fields() {
        let values = [
            (
                metadata().with_description(" "),
                PluginMetadataField::Description,
            ),
            (metadata().with_license("\t"), PluginMetadataField::License),
            (
                metadata().with_repository("\n"),
                PluginMetadataField::Repository,
            ),
            (
                metadata().with_homepage("  "),
                PluginMetadataField::Homepage,
            ),
        ];

        for (metadata, field) in values {
            assert_eq!(
                metadata.validate().unwrap_err(),
                PluginMetadataValidationError::EmptyField { field }
            );
        }
    }

    #[test]
    fn rejects_unknown_format_versions() {
        let mut metadata = metadata();
        metadata.format = 2;

        assert_eq!(
            metadata.validate().unwrap_err(),
            PluginMetadataValidationError::UnsupportedFormat { found: 2 }
        );
    }
}
