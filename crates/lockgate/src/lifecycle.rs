use std::{any::Any, collections::BTreeMap, error::Error, fmt, marker::PhantomData, path::PathBuf};

use crate::exec::{ExecEngine, LoadError};
use crate::inspection::{
    InspectError, Inspection, decode_exported_interfaces, decode_metadata, decode_needs,
    decode_sections,
};
use crate::validate::{ValidationError, validate_value_only_exports};

/// Symbolic root names and their host paths for later scope resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SymbolicRoots(BTreeMap<String, PathBuf>);

impl SymbolicRoots {
    /// Adds or replaces one symbolic root mapping.
    pub fn insert(&mut self, name: impl Into<String>, path: impl Into<PathBuf>) {
        self.0.insert(name.into(), path.into());
    }

    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
}

/// Symbolic grant limits reserved for the grant-system lifecycle step.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
#[non_exhaustive]
pub enum LimitSet {
    /// No symbolic limits are applied.
    #[default]
    Unconstrained,
    /// A constrained set whose contents become expressible with the grant system.
    Constrained,
}

/// Per-plugin configuration supplied at preparation time.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct PluginConfig {
    pub settings: Option<serde_json::Value>,
    pub roots: SymbolicRoots,
    pub limits: LimitSet,
}

impl PluginConfig {
    fn unavailable_field(&self) -> Option<&'static str> {
        if self.settings.is_some() {
            Some("settings")
        } else if !self.roots.is_empty() {
            Some("symbolic roots")
        } else if self.limits != LimitSet::Unconstrained {
            Some("grant limits")
        } else {
            None
        }
    }
}

/// Retained engine and linker state for preparing plugins with invocation data `S`.
pub struct HostBuilder<S: Send + 'static> {
    engine: ExecEngine,
    marker: PhantomData<fn() -> S>,
}

impl<S: Send + 'static> HostBuilder<S> {
    /// Creates a builder with Lockgate's pinned component-engine configuration.
    pub fn new() -> Result<Self, EngineError> {
        Ok(Self {
            engine: ExecEngine::new().map_err(EngineError::new)?,
            marker: PhantomData,
        })
    }

    /// Validates, compiles, and prelinks a plugin artifact for later admission.
    pub async fn prepare(
        &mut self,
        id: &str,
        bytes: &[u8],
        config: PluginConfig,
    ) -> Result<Prepared, AdmissionError> {
        let sections = decode_sections(bytes).map_err(AdmissionError::from_inspection)?;
        let metadata = decode_metadata(&sections).map_err(AdmissionError::from_inspection)?;
        if metadata.id() != id {
            return Err(AdmissionError::PluginIdMismatch {
                configured: id.to_string(),
                embedded: metadata.id().to_string(),
            });
        }
        let (needs, needs_digest) =
            decode_needs(&sections).map_err(AdmissionError::from_inspection)?;
        let exported_interfaces =
            decode_exported_interfaces(bytes).map_err(AdmissionError::from_inspection)?;
        validate_value_only_exports(bytes).map_err(AdmissionError::from_validation)?;
        if let Some(field) = config.unavailable_field() {
            return Err(AdmissionError::ConfigFeatureUnavailable { field });
        }

        let artifact = self
            .engine
            .load::<S>(bytes, |_| Ok(()))
            .map_err(AdmissionError::from_load)?;
        let inspection = Inspection::new(metadata, needs, needs_digest, exported_interfaces);
        Ok(Prepared {
            inspection,
            artifact: Box::new(artifact),
        })
    }
}

/// A validated, compiled, and prelinked plugin artifact.
pub struct Prepared {
    inspection: Inspection,
    #[allow(
        dead_code,
        reason = "retained for admit and invocation lifecycle steps"
    )]
    artifact: Box<dyn Any + Send>,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Prepared")
            .field("inspection", &self.inspection)
            .finish_non_exhaustive()
    }
}

impl Prepared {
    /// Returns the declarations produced by pure inspection of this artifact.
    pub fn inspection(&self) -> &Inspection {
        &self.inspection
    }
}

/// Failure to create Lockgate's retained Wasmtime engine.
#[derive(Debug)]
pub struct EngineError {
    message: String,
}

impl EngineError {
    fn new(error: wasmtime::Error) -> Self {
        Self {
            message: error.to_string(),
        }
    }
}

impl fmt::Display for EngineError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "Lockgate engine initialization failed: {}",
            self.message
        )
    }
}

impl Error for EngineError {}

/// A typed failure while preparing a plugin for admission.
#[derive(Debug)]
#[non_exhaustive]
pub enum AdmissionError {
    ConfigFeatureUnavailable {
        field: &'static str,
    },
    NotComponent,
    InvalidComponent {
        message: String,
    },
    InvalidWit {
        message: String,
    },
    MissingMetadata,
    MissingNeeds,
    DuplicateSection {
        name: &'static str,
    },
    InvalidMetadata(lockgate_schema::sections::metadata::DecodeError),
    PluginIdMismatch {
        configured: String,
        embedded: String,
    },
    InvalidNeeds(lockgate_schema::sections::needs::DecodeError),
    NeedsDigest(lockgate_schema::sections::needs::EncodeError),
    UnsupportedExport(ValidationError),
    Compilation {
        message: String,
    },
    Preflight {
        message: String,
    },
}

impl AdmissionError {
    /// Returns this error's stable machine-readable code, when assigned.
    ///
    /// Codes are append-only: once assigned, a code is never changed or reused.
    pub fn code(&self) -> Option<&'static str> {
        match self {
            Self::UnsupportedExport(error) => error.code(),
            _ => None,
        }
    }

    fn from_inspection(error: InspectError) -> Self {
        match error {
            InspectError::NotComponent => Self::NotComponent,
            InspectError::InvalidComponent { message } => Self::InvalidComponent { message },
            InspectError::InvalidWit { message } => Self::InvalidWit { message },
            InspectError::MissingMetadata => Self::MissingMetadata,
            InspectError::MissingNeeds => Self::MissingNeeds,
            InspectError::DuplicateSection { name } => Self::DuplicateSection { name },
            InspectError::Metadata(error) => Self::InvalidMetadata(error),
            InspectError::Needs(error) => Self::InvalidNeeds(error),
            InspectError::NeedsDigest(error) => Self::NeedsDigest(error),
        }
    }

    fn from_validation(error: ValidationError) -> Self {
        match error {
            error @ ValidationError::UnsupportedExport { .. } => Self::UnsupportedExport(error),
            ValidationError::Decode { message } => Self::InvalidWit { message },
            ValidationError::NotComponent => Self::NotComponent,
        }
    }

    fn from_load(error: LoadError) -> Self {
        match error {
            LoadError::Compile(error) => Self::Compilation {
                message: error.to_string(),
            },
            LoadError::Link(error) => Self::Preflight {
                message: error.to_string(),
            },
        }
    }
}

impl fmt::Display for AdmissionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ConfigFeatureUnavailable { field } => write!(
                formatter,
                "plugin {field} configuration is not yet available in this build"
            ),
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
            Self::MissingMetadata => formatter
                .write_str("component is missing required `lockgate:plugin` metadata section"),
            Self::MissingNeeds => {
                formatter.write_str("component is missing required `lockgate:needs` needs section")
            }
            Self::DuplicateSection { name } => {
                write!(formatter, "component contains duplicate `{name}` sections")
            }
            Self::InvalidMetadata(error) => {
                write!(formatter, "plugin metadata is invalid: {error}")
            }
            Self::PluginIdMismatch {
                configured,
                embedded,
            } => write!(
                formatter,
                "configured plugin id `{configured}` does not match embedded id `{embedded}`"
            ),
            Self::InvalidNeeds(error) => {
                write!(formatter, "plugin needs manifest is invalid: {error}")
            }
            Self::NeedsDigest(error) => {
                write!(
                    formatter,
                    "plugin needs digest could not be computed: {error}"
                )
            }
            Self::UnsupportedExport(error) => error.fmt(formatter),
            Self::Compilation { message } => {
                write!(formatter, "component compilation failed: {message}")
            }
            Self::Preflight { message } => {
                write!(formatter, "component linker preflight failed: {message}")
            }
        }
    }
}

impl Error for AdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidMetadata(error) => Some(error),
            Self::InvalidNeeds(error) => Some(error),
            Self::NeedsDigest(error) => Some(error),
            Self::UnsupportedExport(error) => Some(error),
            _ => None,
        }
    }
}
