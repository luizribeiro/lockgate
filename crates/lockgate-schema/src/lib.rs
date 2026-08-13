//! Shared no-wasmtime types for metadata, needs manifests, grant atoms, digests, and Serde.

use std::{error::Error, fmt};

use serde::{Deserialize, Serialize};

mod needs;
pub mod sections;

pub use needs::{
    AtomKey, AtomKeyError, EntryLocation, MAX_ATOMS_PER_MANIFEST, MAX_ROOT_NAME_BYTES,
    MAX_ROOT_SUBPATH_BYTES, MAX_ROOT_SUBPATH_SEGMENTS, MAX_SCOPE_VALUE_BYTES, MAX_SCOPES_PER_ENTRY,
    NeedEntry, NeedEntryError, NeedKind, NeedReasonError, NeedsDigest, NeedsManifest,
    NeedsManifestValidationError, Requirement, ScopeCharacterKind, ScopeRef, ScopeRefError,
    ScopeValueKind,
};

const PLUGIN_METADATA_FORMAT: u32 = 1;

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

    /// Returns the plugin implementation's SemVer version.
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
        validate_required(PluginMetadataField::Id, &self.id)?;
        if self.id.chars().any(char::is_whitespace) {
            return Err(PluginMetadataValidationError::IdContainsWhitespace);
        }
        validate_required(PluginMetadataField::Name, &self.name)?;
        validate_required(PluginMetadataField::Version, &self.version)?;
        semver::Version::parse(&self.version).map_err(|error| {
            PluginMetadataValidationError::InvalidVersion {
                reason: error.to_string(),
            }
        })?;
        for (field, value) in [
            (
                PluginMetadataField::Description,
                self.description.as_deref(),
            ),
            (PluginMetadataField::License, self.license.as_deref()),
            (PluginMetadataField::Repository, self.repository.as_deref()),
            (PluginMetadataField::Homepage, self.homepage.as_deref()),
        ] {
            if value.is_some_and(|value| value.trim().is_empty()) {
                return Err(PluginMetadataValidationError::EmptyOptionalField { field });
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
    /// A required field is empty or consists only of whitespace.
    EmptyRequiredField { field: PluginMetadataField },
    /// A required field has leading or trailing whitespace.
    SurroundingWhitespace { field: PluginMetadataField },
    /// The plugin identifier contains whitespace.
    IdContainsWhitespace,
    /// The plugin version is not valid semantic version syntax.
    InvalidVersion { reason: String },
    /// An optional field is present but empty or consists only of whitespace.
    EmptyOptionalField { field: PluginMetadataField },
}

impl fmt::Display for PluginMetadataValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { found } => {
                write!(formatter, "unsupported plugin metadata format {found}")
            }
            Self::EmptyRequiredField { field } => {
                write!(formatter, "plugin {field} must not be empty")
            }
            Self::SurroundingWhitespace { field } => write!(
                formatter,
                "plugin {field} must not have leading or trailing whitespace"
            ),
            Self::IdContainsWhitespace => {
                formatter.write_str("plugin id must not contain whitespace")
            }
            Self::InvalidVersion { reason } => {
                write!(formatter, "plugin version is not valid SemVer: {reason}")
            }
            Self::EmptyOptionalField { field } => {
                write!(formatter, "plugin {field} must not be empty when present")
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

fn validate_required(
    field: PluginMetadataField,
    value: &str,
) -> Result<(), PluginMetadataValidationError> {
    if value.trim().is_empty() {
        return Err(PluginMetadataValidationError::EmptyRequiredField { field });
    }
    if value.trim() != value {
        return Err(PluginMetadataValidationError::SurroundingWhitespace { field });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn metadata() -> PluginMetadata {
        PluginMetadata::new("com.example.greeter", "Greeter", "1.2.3").unwrap()
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
                PluginMetadataValidationError::EmptyRequiredField { field }
            );
        }
    }

    #[test]
    fn rejects_surrounding_whitespace_in_required_fields() {
        for (result, field) in [
            (
                PluginMetadata::new(" plugin", "Greeter", "1.2.3"),
                PluginMetadataField::Id,
            ),
            (
                PluginMetadata::new("plugin", "Greeter ", "1.2.3"),
                PluginMetadataField::Name,
            ),
            (
                PluginMetadata::new("plugin", "Greeter", "1.2.3\n"),
                PluginMetadataField::Version,
            ),
        ] {
            assert_eq!(
                result.unwrap_err(),
                PluginMetadataValidationError::SurroundingWhitespace { field }
            );
        }
    }

    #[test]
    fn rejects_whitespace_inside_the_plugin_id() {
        assert_eq!(
            PluginMetadata::new("com.example\u{2003}greeter", "Greeter", "1.2.3").unwrap_err(),
            PluginMetadataValidationError::IdContainsWhitespace
        );
    }

    #[test]
    fn rejects_versions_that_are_not_semver() {
        let error = PluginMetadata::new("com.example.greeter", "Greeter", "latest").unwrap_err();

        assert!(matches!(
            error,
            PluginMetadataValidationError::InvalidVersion { .. }
        ));
        assert!(error.to_string().contains("valid SemVer"));
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
                PluginMetadataValidationError::EmptyOptionalField { field }
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
