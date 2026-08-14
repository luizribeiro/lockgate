use std::{
    any::Any,
    collections::BTreeMap,
    error::Error,
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use lockgate_schema::{AtomKey, GrantSet, NeedsManifest, PluginMetadata};

use crate::exec::{
    ExecEngine, ExecError, ExecLimits, ImportsFactory, LoadError, LoadedComponent, TypedImports,
};
use crate::inspection::{InspectError, Inspection, decode_metadata, decode_needs, decode_sections};
use crate::role::{Role, RoleError, RoleInvocation};
use crate::validate::{ValidationError, validate_and_collect_exported_interfaces};

static NEXT_HOST_ID: AtomicU64 = AtomicU64::new(1);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HostId(u64);

impl HostId {
    fn next() -> Self {
        Self(NEXT_HOST_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Per-invocation execution budget.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum BudgetClass {
    /// A deterministic fuel ceiling for one invocation.
    Bounded { fuel: u64 },
}

/// Application data and execution budget installed for one invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvocationCtx<S> {
    pub data: S,
    pub budget: BudgetClass,
}

impl InvocationCtx<()> {
    /// Creates a context with no application data and a bounded fuel budget.
    pub fn bounded(fuel: u64) -> Self {
        Self::new((), BudgetClass::Bounded { fuel })
    }
}

impl<S> InvocationCtx<S> {
    /// Creates an invocation context from application data and a budget.
    pub fn new(data: S, budget: BudgetClass) -> Self {
        Self { data, budget }
    }
}

/// Per-plugin limits applied to every fresh Store, including the smoke probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub instantiation_fuel: u64,
    pub max_memory_bytes: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            instantiation_fuel: 10_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
        }
    }
}

impl From<RuntimeLimits> for ExecLimits {
    fn from(limits: RuntimeLimits) -> Self {
        Self {
            instantiation_fuel: limits.instantiation_fuel,
            max_memory_bytes: limits.max_memory_bytes,
        }
    }
}

/// The application consent accepted for a prepared plugin.
#[derive(Clone, Debug)]
pub struct Acceptance(AcceptanceKind);

#[derive(Clone, Debug)]
enum AcceptanceKind {
    AllDeclared,
    Accepted(GrantSet),
}

impl Acceptance {
    /// Accepts every atom declared by the prepared plugin.
    pub fn all_declared() -> Self {
        Self(AcceptanceKind::AllDeclared)
    }

    /// Uses grants selected by an application consent flow.
    pub fn accepted(grants: GrantSet) -> Self {
        Self(AcceptanceKind::Accepted(grants))
    }
}

fn validate_acceptance(
    needs: &NeedsManifest,
    acceptance: &Acceptance,
) -> Result<(), AdmissionError> {
    // Intentionally a no-op: non-empty needs are rejected before this point,
    // and accepted-but-never-declared grants remain inert by design.
    // FIXME(grant-system): the grant-algebra join computes effective = declared
    // ∩ accepted ∩ limits, parses accepted values through registered scope
    // types, and changes this return type to the effective-grants value.
    let _ = needs;
    match &acceptance.0 {
        AcceptanceKind::AllDeclared => {}
        AcceptanceKind::Accepted(grants) => {
            let _ = grants;
        }
    }
    Ok(())
}

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
pub struct HostBuilder<S: Send + Sync + 'static> {
    id: HostId,
    engine: ExecEngine,
    imports: std::sync::Arc<dyn ImportsFactory<S>>,
    admitted: Vec<AdmittedPlugin<S>>,
}

struct AdmittedPlugin<S: 'static> {
    handle: PluginHandle,
    artifact: LoadedComponent<S>,
    limits: RuntimeLimits,
}

/// Identity and display metadata for an admitted plugin.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PluginHandle {
    host: HostId,
    index: usize,
    metadata: PluginMetadata,
}

impl PluginHandle {
    /// Returns the stable plugin identifier.
    pub fn id(&self) -> &str {
        self.metadata.id()
    }

    /// Returns the plugin's validated display metadata.
    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }
}

impl<S: Send + Sync + 'static> HostBuilder<S> {
    /// Creates a builder with application host imports and Lockgate's pinned
    /// component-engine configuration.
    ///
    /// Generated bindings implement [`crate::HostImports`] for the supplied
    /// value. Pass `()` when admitted plugins import no application functions.
    pub fn new<I: crate::HostImports<S>>(imports: I) -> Result<Self, EngineError> {
        Ok(Self {
            id: HostId::next(),
            engine: ExecEngine::new().map_err(EngineError::new)?,
            imports: std::sync::Arc::new(TypedImports::new(imports, |imports, linker| {
                crate::HostImports::add_to_linker(imports, linker)
            })),
            admitted: Vec::new(),
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
        let exported_interfaces = validate_and_collect_exported_interfaces(bytes)
            .map_err(AdmissionError::from_validation)?;
        if let Some(field) = config.unavailable_field() {
            return Err(AdmissionError::ConfigFeatureUnavailable { field });
        }

        let artifact = self
            .engine
            .load_hosted::<S>(bytes, std::sync::Arc::clone(&self.imports))
            .map_err(AdmissionError::from_load)?;
        let inspection = Inspection::new(metadata, needs, needs_digest, exported_interfaces);
        Ok(Prepared {
            inspection,
            artifact: Box::new(artifact),
        })
    }

    /// Rejects declared needs before consulting acceptance while no capability
    /// registry exists, then smoke-instantiates the prepared plugin.
    pub async fn admit(
        &mut self,
        prepared: Prepared,
        acceptance: Acceptance,
        limits: RuntimeLimits,
        startup_ctx: InvocationCtx<S>,
    ) -> Result<PluginHandle, AdmissionError> {
        let Prepared {
            inspection,
            artifact,
        } = prepared;
        if let Some(entry) = inspection
            .needs()
            .required()
            .iter()
            .chain(inspection.needs().optional())
            .next()
        {
            return Err(AdmissionError::UnregisteredCapability {
                atom: entry.atom().clone(),
            });
        }

        validate_acceptance(inspection.needs(), &acceptance)?;

        let mut artifact = artifact
            .downcast::<LoadedComponent<S>>()
            .expect("prepared artifact type must match its originating HostBuilder");
        let handle = PluginHandle {
            host: self.id,
            index: self.admitted.len(),
            metadata: inspection.metadata().clone(),
        };
        artifact.set_plugin(handle.clone());
        let BudgetClass::Bounded { fuel } = startup_ctx.budget;
        artifact
            .smoke(startup_ctx.data, limits.into(), fuel)
            .await
            .map_err(AdmissionError::from_smoke)?;

        self.admitted.push(AdmittedPlugin {
            handle: handle.clone(),
            artifact: *artifact,
            limits,
        });
        Ok(handle)
    }

    /// Finishes configuration and transfers admitted plugins into a steady-state Host.
    pub fn finish(self) -> Host<S> {
        Host {
            id: self.id,
            engine: self.engine,
            plugins: self.admitted,
        }
    }
}

/// Steady-state owner of the execution engine and admitted plugins.
pub struct Host<S: Send + Sync + 'static> {
    id: HostId,
    #[allow(
        dead_code,
        reason = "retained as the steady-state engine owner alongside its prelinked artifacts"
    )]
    engine: ExecEngine,
    plugins: Vec<AdmittedPlugin<S>>,
}

impl<S: Send + Sync + 'static> Host<S> {
    /// Iterates over admitted plugins in admission order.
    pub fn plugins(&self) -> impl Iterator<Item = &PluginHandle> {
        self.plugins.iter().map(|plugin| &plugin.handle)
    }

    /// Casts a plugin to an exported WIT role before any call is attempted.
    pub fn client<R: Role>(&self, plugin: &PluginHandle) -> Result<R::Client<'_, S>, RoleError> {
        if plugin.host != self.id {
            return Err(RoleError::WrongHost);
        }
        let admitted = self.plugins.get(plugin.index).ok_or(RoleError::WrongHost)?;
        if !admitted.artifact.exports_interface(R::INTERFACE) {
            return Err(RoleError::RoleNotExported {
                interface: R::INTERFACE,
            });
        }
        Ok(R::client(RoleInvocation::new(
            &admitted.artifact,
            admitted.limits,
            R::INTERFACE,
        )))
    }
}

/// A validated, compiled, and prelinked plugin artifact.
pub struct Prepared {
    inspection: Inspection,
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
    Inspection(InspectError),
    PluginIdMismatch {
        configured: String,
        embedded: String,
    },
    UnsupportedExport(ValidationError),
    Compilation {
        message: String,
    },
    Preflight {
        message: String,
    },
    UnregisteredCapability {
        atom: AtomKey,
    },
    SmokeOutOfBudget,
    SmokeFailure {
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
        Self::Inspection(error)
    }

    fn from_validation(error: ValidationError) -> Self {
        match error {
            error @ ValidationError::UnsupportedExport { .. } => Self::UnsupportedExport(error),
            ValidationError::Decode { message } => {
                Self::Inspection(InspectError::InvalidWit { message })
            }
            ValidationError::NotComponent => Self::Inspection(InspectError::NotComponent),
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

    fn from_smoke(error: ExecError) -> Self {
        match error {
            ExecError::OutOfBudget => Self::SmokeOutOfBudget,
            error => Self::SmokeFailure {
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
            Self::Inspection(error) => error.fmt(formatter),
            Self::PluginIdMismatch {
                configured,
                embedded,
            } => write!(
                formatter,
                "configured plugin id `{configured}` does not match embedded id `{embedded}`"
            ),
            Self::UnsupportedExport(error) => error.fmt(formatter),
            Self::Compilation { message } => {
                write!(formatter, "component compilation failed: {message}")
            }
            Self::Preflight { message } => {
                write!(formatter, "component linker preflight failed: {message}")
            }
            Self::UnregisteredCapability { atom } => write!(
                formatter,
                "plugin declares capability `{atom}` that the application never registered"
            ),
            Self::SmokeOutOfBudget => formatter
                .write_str("plugin exhausted its startup budget during smoke instantiation"),
            Self::SmokeFailure { message } => {
                write!(formatter, "plugin smoke instantiation failed: {message}")
            }
        }
    }
}

impl Error for AdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inspection(error) => Some(error),
            Self::UnsupportedExport(error) => Some(error),
            _ => None,
        }
    }
}
