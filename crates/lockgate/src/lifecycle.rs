use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

use lockgate_schema::{AtomKey, NeedKind, NeedsManifest, PluginMetadata};

use crate::CallContext;
use crate::config::{SettingsValidationError, validate_settings};
use crate::exec::{
    ExecEngine, ExecError, ExecLimits, ImportsFactory, LoadError, LoadedComponent, TypedImports,
};
use crate::inspection::{
    InspectError, Inspection, decode_imported_interfaces, decode_metadata, decode_needs,
    decode_sections,
};
use crate::jobs::{DetachedJobContext, DetachedJobFailure, JobTracker};
use crate::policy::{
    CapabilityRegistrationError, CapabilityRegistry, EffectiveGrants, HostImportPolicyError,
    HostImportPolicyMetadata, PreparedNeedsDigest, ResolvedNeeds, ScopeResolutionError,
    resolve_needs,
};
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

/// A [`CallContext`] and execution budget installed for one invocation.
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
    /// Creates an invocation context with `data` as its [`CallContext`] and the
    /// supplied budget.
    pub fn new(data: S, budget: BudgetClass) -> Self {
        Self { data, budget }
    }
}

/// Per-plugin limits applied to every fresh Store, including the smoke probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub instantiation_fuel: u64,
    pub max_memory_bytes: usize,
    pub max_detached_jobs: usize,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            instantiation_fuel: 10_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_detached_jobs: 32,
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

/// Acceptance of every concrete atom in one prepared plugin request.
///
/// Acceptances can only be produced by [`Prepared::accept_all`]. They retain
/// the plugin identity and exact prepared-needs digest that admission checks.
#[derive(Clone, Debug)]
pub struct Acceptance {
    plugin_id: String,
    digest: PreparedNeedsDigest,
}

fn validate_acceptance(
    plugin_id: &str,
    digest: PreparedNeedsDigest,
    acceptance: &Acceptance,
) -> Result<(), AdmissionError> {
    if acceptance.plugin_id != plugin_id {
        return Err(AdmissionError::AcceptancePluginMismatch {
            prepared: plugin_id.to_owned(),
            acceptance: acceptance.plugin_id.clone(),
        });
    }
    if acceptance.digest != digest {
        return Err(AdmissionError::AcceptanceDigestMismatch {
            plugin: plugin_id.to_owned(),
            prepared: digest.to_string(),
            acceptance: acceptance.digest.to_string(),
        });
    }
    Ok(())
}

fn bind_effective_grants(
    plugin_id: &str,
    digest: PreparedNeedsDigest,
    resolved: ResolvedNeeds,
    acceptance: &Acceptance,
) -> Result<EffectiveGrants, AdmissionError> {
    validate_acceptance(plugin_id, digest, acceptance)?;
    Ok(EffectiveGrants::from_resolved(resolved))
}

/// Symbolic root names and their host paths for later scope resolution.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SymbolicRoots(BTreeMap<String, PathBuf>);

impl SymbolicRoots {
    /// Adds or replaces one symbolic root mapping.
    pub fn insert(&mut self, name: impl Into<String>, path: impl Into<PathBuf>) {
        self.0.insert(name.into(), path.into());
    }

    pub(crate) fn get(&self, name: &str) -> Option<&std::path::Path> {
        self.0.get(name).map(PathBuf::as_path)
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
        if self.limits != LimitSet::Unconstrained {
            Some("grant limits")
        } else {
            None
        }
    }
}

/// Retained engine and linker state for preparing plugins with invocation data `S`.
pub struct HostBuilder<S: CallContext> {
    id: HostId,
    engine: ExecEngine,
    imports: std::sync::Arc<dyn ImportsFactory<S>>,
    admitted: Vec<AdmittedPlugin<S>>,
    jobs: std::sync::Arc<JobTracker>,
    registry: CapabilityRegistry,
    policy_metadata: HostImportPolicyMetadata,
}

struct AdmittedPlugin<S: 'static> {
    handle: PluginHandle,
    artifact: LoadedComponent<S>,
    limits: RuntimeLimits,
}

/// Identity and display metadata for an admitted plugin.
#[derive(Clone)]
pub struct PluginHandle {
    host: HostId,
    index: usize,
    instance_id: String,
    metadata: PluginMetadata,
    effective_grants: EffectiveGrants,
    registry: std::sync::Arc<CapabilityRegistry>,
}

impl fmt::Debug for PluginHandle {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginHandle")
            .field("host", &self.host)
            .field("index", &self.index)
            .field("instance_id", &self.instance_id)
            .field("metadata", &self.metadata)
            .field("effective_grants", &self.effective_grants)
            .finish_non_exhaustive()
    }
}

impl PartialEq for PluginHandle {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
            && self.index == other.index
            && self.instance_id == other.instance_id
            && self.metadata == other.metadata
            && self.effective_grants == other.effective_grants
    }
}

impl Eq for PluginHandle {}

impl PluginHandle {
    /// Returns the operator-assigned instance identifier.
    pub fn id(&self) -> &str {
        &self.instance_id
    }

    /// Returns the plugin's validated display metadata.
    pub fn metadata(&self) -> &PluginMetadata {
        &self.metadata
    }

    #[allow(
        dead_code,
        reason = "guard expansion consumes this immutable query surface in the next policy chunk"
    )]
    pub(crate) fn effective_grants(&self) -> &EffectiveGrants {
        &self.effective_grants
    }

    pub(crate) fn capability_registry(&self) -> &CapabilityRegistry {
        &self.registry
    }

    /// Reserved for framework-owned mediated capability adapters.
    #[doc(hidden)]
    pub fn __scoped_access_allowed<T>(
        &self,
        permission: lockgate_policy::ScopedPermission<T>,
        memberships: &[T],
    ) -> bool
    where
        T: lockgate_policy::Scope,
        <T as core::str::FromStr>::Err: Into<lockgate_policy::ScopeError>,
    {
        crate::policy::scoped_access_allowed(
            self.capability_registry(),
            self.effective_grants(),
            permission,
            memberships,
        )
    }

    #[cfg(test)]
    pub(crate) fn for_policy_test(instance_id: &str, effective_grants: EffectiveGrants) -> Self {
        Self::for_policy_test_with_registry(
            instance_id,
            effective_grants,
            CapabilityRegistry::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_policy_test_with_registry(
        instance_id: &str,
        effective_grants: EffectiveGrants,
        registry: CapabilityRegistry,
    ) -> Self {
        Self {
            host: HostId(0),
            index: 0,
            instance_id: instance_id.to_owned(),
            metadata: PluginMetadata::new(instance_id, "Policy test plugin", "1.0").unwrap(),
            effective_grants,
            registry: std::sync::Arc::new(registry),
        }
    }
}

impl<S: CallContext> HostBuilder<S> {
    /// Creates a builder with application host imports and Lockgate's pinned
    /// component-engine configuration.
    ///
    /// Generated bindings implement [`crate::HostImports`] for the supplied
    /// value. Pass `()` when admitted plugins import no application functions.
    pub fn new<I: crate::HostImports<S>>(imports: I) -> Result<Self, HostConstructionError> {
        let policy_metadata = I::policy_metadata().map_err(HostConstructionError::HostImports)?;
        let engine = ExecEngine::new()
            .map_err(EngineError::new)
            .map_err(HostConstructionError::Engine)?;
        let jobs = JobTracker::new()
            .map_err(EngineError::new)
            .map_err(HostConstructionError::Engine)?;
        let mut registry = CapabilityRegistry::default();
        registry
            .register::<lockgate_policy::http::Contract>()
            .expect("Lockgate's built-in HTTP capability must be valid");
        Ok(Self {
            id: HostId::next(),
            engine,
            imports: std::sync::Arc::new(TypedImports::new(
                imports,
                |imports, linker, interfaces| {
                    crate::HostImports::add_to_linker(imports, linker, interfaces)
                },
            )),
            admitted: Vec::new(),
            jobs,
            registry,
            policy_metadata,
        })
    }

    /// Registers one generated capability vocabulary with the host.
    ///
    /// Registration validates stable names, descriptor consistency, duplicate
    /// declarations, and every exhaustive scope domain before retaining any
    /// part of the contract.
    pub fn register<C: lockgate_policy::CapabilityContract>(
        mut self,
    ) -> Result<Self, CapabilityRegistrationError> {
        self.registry.register::<C>()?;
        Ok(self)
    }

    /// Installs the application callback for failed or panicked detached jobs.
    ///
    /// The default callback is a no-op because Lockgate has no logging
    /// dependency. Applications that detach work should install a sink before
    /// admitting plugins so asynchronous failures remain observable.
    pub fn on_detached_job_error(
        &mut self,
        sink: impl Fn(DetachedJobFailure) + Send + Sync + 'static,
    ) {
        self.jobs.set_error_sink(sink);
    }

    /// Validates and compiles the artifact, preflights its complete linker,
    /// then probes and validates its settings contract for later admission.
    ///
    /// A schema export is invoked once in a capped internal Store that has no
    /// application call context or application import implementations. The
    /// framework settings import reports `not-ready` during that probe. Once
    /// the supplied settings (or `{}` when absent) pass the plugin's Draft
    /// 2020-12 schema, their JSON is retained for smoke and steady-state Stores.
    pub async fn prepare(
        &mut self,
        id: &str,
        bytes: &[u8],
        config: PluginConfig,
    ) -> Result<Prepared, AdmissionError> {
        self.validate_policy_permissions()?;
        let unavailable_field = config.unavailable_field();
        let sections = decode_sections(bytes).map_err(AdmissionError::from_inspection)?;
        let metadata = decode_metadata(&sections).map_err(AdmissionError::from_inspection)?;
        let (needs, needs_digest) =
            decode_needs(&sections).map_err(AdmissionError::from_inspection)?;
        let exported_interfaces = validate_and_collect_exported_interfaces(bytes)
            .map_err(AdmissionError::from_validation)?;
        let imported_interfaces =
            decode_imported_interfaces(bytes).map_err(AdmissionError::from_inspection)?;
        let wired_interfaces = self.select_host_imports(&imported_interfaces, &needs)?;
        let has_http_egress =
            validate_http_egress_grant(id, &imported_interfaces, http_egress_origin_count(&needs))?;
        if let Some(field) = unavailable_field {
            return Err(AdmissionError::ConfigFeatureUnavailable { field });
        }
        let component = self
            .engine
            .compile(bytes)
            .map_err(AdmissionError::from_load)?;
        let mut artifact = self
            .engine
            .load_hosted_component::<S>(
                &component,
                std::sync::Arc::clone(&self.imports),
                &wired_interfaces,
                has_http_egress,
            )
            .map_err(AdmissionError::from_load)?;
        let schema = self
            .engine
            .fetch_settings_schema(&component, RuntimeLimits::default().into())
            .await
            .map_err(AdmissionError::from_schema_fetch)?;
        let settings = validate_settings(schema.as_deref(), config.settings)
            .map_err(AdmissionError::from_settings_validation)?;
        let resolved = resolve_needs(&needs, settings.value(), &config.roots, &self.registry)
            .map_err(AdmissionError::ScopeResolution)?;
        let prepared_digest = PreparedNeedsDigest::compute(needs_digest, &resolved);
        artifact.set_settings(settings);
        let inspection = Inspection::new(metadata, needs, needs_digest, exported_interfaces);
        Ok(Prepared {
            host: self.id,
            instance_id: id.to_owned(),
            inspection,
            resolved,
            prepared_digest,
            artifact: Box::new(artifact),
        })
    }

    fn validate_policy_permissions(&self) -> Result<(), AdmissionError> {
        for interface in self.policy_metadata.interfaces() {
            for method in interface.methods() {
                let Some(permission) = method.classification().permission() else {
                    continue;
                };
                if !self
                    .registry
                    .contains(permission.capability(), permission.permission())
                {
                    return Err(AdmissionError::UnregisteredGuardPermission {
                        atom: permission.atom(),
                        interface: interface.interface().imported_name(),
                        method: method.method().wit_name(),
                    });
                }
            }
        }
        Ok(())
    }

    fn select_host_imports(
        &self,
        imported_interfaces: &[String],
        needs: &NeedsManifest,
    ) -> Result<Vec<String>, AdmissionError> {
        let declared = needs
            .required()
            .iter()
            .chain(needs.optional())
            .map(|entry| entry.atom())
            .collect::<BTreeSet<_>>();
        let mut wired = Vec::new();
        for interface in self.policy_metadata.interfaces() {
            let identity = interface.interface();
            if !imported_interfaces
                .iter()
                .any(|imported| identity.matches_import(imported))
            {
                continue;
            }

            let mut permissions = BTreeSet::new();
            let mut has_capability_free_method = false;
            for method in interface.methods() {
                match method.classification().permission() {
                    Some(permission) => {
                        permissions.insert(permission.atom());
                    }
                    None => has_capability_free_method = true,
                }
            }
            if has_capability_free_method || permissions.iter().any(|atom| declared.contains(atom))
            {
                wired.push(identity.imported_name());
            } else {
                return Err(AdmissionError::HostImportManifestMismatch {
                    interface: identity.imported_name(),
                    permissions: permissions.into_iter().collect(),
                });
            }
        }
        Ok(wired)
    }

    /// Verifies prepared acceptance and smoke-instantiates the plugin.
    /// Smoke instantiation uses the smaller of `limits.instantiation_fuel` and
    /// `startup_ctx`'s fuel, so an application-chosen startup budget may reject
    /// a constructor that steady-state calls would instantiate under the full
    /// limit.
    pub async fn admit(
        &mut self,
        prepared: Prepared,
        acceptance: Acceptance,
        limits: RuntimeLimits,
        startup_ctx: InvocationCtx<S>,
    ) -> Result<PluginHandle, AdmissionError> {
        let Prepared {
            host,
            instance_id,
            inspection,
            resolved,
            prepared_digest,
            artifact,
        } = prepared;
        if host != self.id {
            return Err(AdmissionError::PreparedHostMismatch {
                plugin: instance_id,
            });
        }
        if self
            .admitted
            .iter()
            .any(|plugin| plugin.handle.id() == instance_id.as_str())
        {
            return Err(AdmissionError::DuplicateInstanceId { instance_id });
        }
        let effective_grants =
            bind_effective_grants(&instance_id, prepared_digest, resolved, &acceptance)?;

        let mut artifact = artifact
            .downcast::<LoadedComponent<S>>()
            .expect("prepared artifact type must match its originating HostBuilder");
        let handle = PluginHandle {
            host: self.id,
            index: self.admitted.len(),
            instance_id,
            metadata: inspection.metadata().clone(),
            effective_grants,
            registry: std::sync::Arc::new(self.registry.clone()),
        };
        artifact.set_plugin(
            handle.clone(),
            DetachedJobContext::new(
                std::sync::Arc::clone(&self.jobs),
                handle.id().to_owned(),
                limits.max_detached_jobs,
            ),
        );
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
    ///
    /// Dropping the returned Host blocks the calling thread until its detached
    /// jobs have been aborted and awaited. On an async runtime, perform that
    /// drop in a blocking-safe context such as [`tokio::task::spawn_blocking`].
    pub fn finish(self) -> Host<S> {
        Host {
            id: self.id,
            engine: self.engine,
            plugins: self.admitted,
            jobs: self.jobs,
            registry: self.registry,
        }
    }
}

fn http_egress_origin_count(needs: &NeedsManifest) -> Option<usize> {
    let (capability, permission) =
        lockgate_policy::__private::scoped_permission_ids(lockgate_policy::http::EGRESS);
    let atom = AtomKey::new(capability, permission)
        .expect("typed permissions always contain a valid wire atom");
    needs
        .required()
        .iter()
        .chain(needs.optional())
        .find_map(|entry| match entry.kind() {
            NeedKind::Scoped(origins) if entry.atom() == &atom => Some(origins.len()),
            NeedKind::Flag | NeedKind::Scoped(_) => None,
        })
}

fn validate_http_egress_grant(
    plugin: &str,
    imported_interfaces: &[String],
    origin_count: Option<usize>,
) -> Result<bool, AdmissionError> {
    let Some(origin_count) = origin_count else {
        return Ok(false);
    };
    // Needs decoding rejects empty scoped needs first; retain this as defense in depth.
    if origin_count == 0 {
        return Err(AdmissionError::EmptyHttpEgressOrigins {
            plugin: plugin.to_owned(),
        });
    }
    if !imported_interfaces
        .iter()
        .any(|interface| interface == "wasi:http/client@0.3.0")
    {
        return Err(AdmissionError::HttpEgressUnavailable {
            plugin: plugin.to_owned(),
        });
    }
    Ok(true)
}

/// Steady-state owner of the execution engine and admitted plugins.
///
/// Dropping a Host blocks the calling thread until all detached jobs have been
/// aborted and awaited. On an async runtime, drop it in a blocking-safe context
/// such as [`tokio::task::spawn_blocking`].
pub struct Host<S: CallContext> {
    id: HostId,
    #[allow(
        dead_code,
        reason = "retained as the steady-state engine owner alongside its prelinked artifacts"
    )]
    engine: ExecEngine,
    plugins: Vec<AdmittedPlugin<S>>,
    jobs: std::sync::Arc<JobTracker>,
    #[allow(
        dead_code,
        reason = "retained for prepared grants and guard lookup in later policy slices"
    )]
    registry: CapabilityRegistry,
}

impl<S: CallContext> Drop for Host<S> {
    fn drop(&mut self) {
        self.jobs.shutdown();
    }
}

impl<S: CallContext> Host<S> {
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

    /// Clients for every admitted plugin whose compiled component exports
    /// `R`'s interface, in admission order.
    ///
    /// Use this to invoke every plugin implementing a role. Admission order
    /// makes sequential fan-out deterministic; applications wanting another
    /// order should collect and sort the results. Each item is produced by the
    /// single-plugin [`Host::client`] cast.
    pub fn clients<R: Role>(&self) -> impl Iterator<Item = (&PluginHandle, R::Client<'_, S>)> + '_ {
        self.plugins.iter().filter_map(|plugin| {
            self.client::<R>(&plugin.handle)
                .ok()
                .map(|client| (&plugin.handle, client))
        })
    }
}

/// A validated, compiled, and prelinked plugin artifact.
pub struct Prepared {
    host: HostId,
    instance_id: String,
    inspection: Inspection,
    resolved: ResolvedNeeds,
    prepared_digest: PreparedNeedsDigest,
    artifact: Box<dyn Any + Send>,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Prepared")
            .field("instance_id", &self.instance_id)
            .field("inspection", &self.inspection)
            .finish_non_exhaustive()
    }
}

impl Prepared {
    /// Returns the declarations produced by pure inspection of this artifact.
    pub fn inspection(&self) -> &Inspection {
        &self.inspection
    }

    /// Accepts every required and optional atom in this resolved request.
    pub fn accept_all(&self) -> Acceptance {
        // TODO: Add operator review of declared egress origins during install as
        // part of future consent and drift handling; for now accept them as-is.
        Acceptance {
            plugin_id: self.instance_id.clone(),
            digest: self.prepared_digest,
        }
    }
}

/// Failure to create Lockgate's retained Wasmtime engine.
#[derive(Debug)]
pub struct EngineError {
    message: String,
}

impl EngineError {
    fn new(error: impl fmt::Display) -> Self {
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

/// A typed failure while constructing a [`HostBuilder`].
#[derive(Debug)]
#[non_exhaustive]
pub enum HostConstructionError {
    /// Lockgate could not initialize its pinned execution engine.
    Engine(EngineError),
    /// Generated host-import policy metadata did not match its binding companion.
    HostImports(HostImportPolicyError),
}

impl fmt::Display for HostConstructionError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Engine(error) => error.fmt(formatter),
            Self::HostImports(error) => write!(formatter, "invalid host-import policy: {error}"),
        }
    }
}

impl Error for HostConstructionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Engine(error) => Some(error),
            Self::HostImports(error) => Some(error),
        }
    }
}

/// A typed failure while preparing a plugin for admission.
#[derive(Debug)]
#[non_exhaustive]
pub enum AdmissionError {
    ConfigFeatureUnavailable {
        field: &'static str,
    },
    Inspection(InspectError),
    UnsupportedExport(ValidationError),
    Compilation {
        message: String,
    },
    Preflight {
        message: String,
    },
    UnregisteredGuardPermission {
        atom: AtomKey,
        interface: String,
        method: &'static str,
    },
    HostImportManifestMismatch {
        interface: String,
        permissions: Vec<AtomKey>,
    },
    HttpEgressUnavailable {
        plugin: String,
    },
    EmptyHttpEgressOrigins {
        plugin: String,
    },
    SmokeOutOfBudget,
    SmokeFailure {
        message: String,
    },
    SchemaFetchOutOfBudget,
    SchemaFetchFailure {
        message: String,
    },
    SettingsWithoutSchema,
    SchemaTooLarge {
        actual: usize,
        maximum: usize,
    },
    SchemaTooDeep {
        maximum: usize,
    },
    SchemaMalformed {
        message: String,
    },
    SchemaReferenceNotLocal {
        reference: String,
    },
    InvalidSettingsSchema {
        message: String,
    },
    SettingsValidation {
        message: String,
    },
    ScopeResolution(ScopeResolutionError),
    PreparedHostMismatch {
        plugin: String,
    },
    DuplicateInstanceId {
        instance_id: String,
    },
    AcceptancePluginMismatch {
        prepared: String,
        acceptance: String,
    },
    AcceptanceDigestMismatch {
        plugin: String,
        prepared: String,
        acceptance: String,
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

    fn from_schema_fetch(error: wasmtime::Error) -> Self {
        if error.downcast_ref::<wasmtime::Trap>() == Some(&wasmtime::Trap::OutOfFuel) {
            Self::SchemaFetchOutOfBudget
        } else {
            Self::SchemaFetchFailure {
                message: error.to_string(),
            }
        }
    }

    fn from_settings_validation(error: SettingsValidationError) -> Self {
        match error {
            SettingsValidationError::SettingsWithoutSchema => Self::SettingsWithoutSchema,
            SettingsValidationError::SchemaTooLarge { actual, maximum } => {
                Self::SchemaTooLarge { actual, maximum }
            }
            SettingsValidationError::SchemaTooDeep { maximum } => Self::SchemaTooDeep { maximum },
            SettingsValidationError::SchemaMalformed { message } => {
                Self::SchemaMalformed { message }
            }
            SettingsValidationError::NonLocalReference { reference } => {
                Self::SchemaReferenceNotLocal { reference }
            }
            SettingsValidationError::InvalidSchema { message } => {
                Self::InvalidSettingsSchema { message }
            }
            SettingsValidationError::InvalidSettings { message } => {
                Self::SettingsValidation { message }
            }
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
            Self::UnsupportedExport(error) => error.fmt(formatter),
            Self::Compilation { message } => {
                write!(formatter, "component compilation failed: {message}")
            }
            Self::Preflight { message } => {
                write!(formatter, "component linker preflight failed: {message}")
            }
            Self::UnregisteredGuardPermission {
                atom,
                interface,
                method,
            } => write!(
                formatter,
                "host import `{interface}.{method}` requires permission `{atom}`, but the application never registered that permission"
            ),
            Self::HostImportManifestMismatch {
                interface,
                permissions,
            } => write!(
                formatter,
                "plugin imports host interface `{interface}` but declares none of its permissions [{}]; add at least one as a required or optional need, or remove the interface import",
                permissions
                    .iter()
                    .map(ToString::to_string)
                    .collect::<Vec<_>>()
                    .join(", "),
            ),
            Self::HttpEgressUnavailable { plugin } => write!(
                formatter,
                "plugin `{plugin}` has an HTTP egress grant but does not import `wasi:http/client@0.3.0`"
            ),
            Self::EmptyHttpEgressOrigins { plugin } => write!(
                formatter,
                "plugin `{plugin}` cannot hold an HTTP egress grant with no origins"
            ),
            Self::SmokeOutOfBudget => formatter
                .write_str("plugin exhausted its startup budget during smoke instantiation"),
            Self::SmokeFailure { message } => {
                write!(formatter, "plugin smoke instantiation failed: {message}")
            }
            Self::SchemaFetchOutOfBudget => {
                formatter.write_str("plugin exhausted the internal schema-fetch budget")
            }
            Self::SchemaFetchFailure { message } => {
                write!(formatter, "plugin settings schema fetch failed: {message}")
            }
            Self::SettingsWithoutSchema => formatter.write_str(
                "plugin settings were supplied, but the component exports no settings schema",
            ),
            Self::SchemaTooLarge { actual, maximum } => write!(
                formatter,
                "plugin settings schema is {actual} bytes; the maximum is {maximum} bytes"
            ),
            Self::SchemaTooDeep { maximum } => write!(
                formatter,
                "plugin settings schema exceeds the maximum JSON depth of {maximum}"
            ),
            Self::SchemaMalformed { message } => {
                write!(
                    formatter,
                    "plugin settings schema is not valid JSON: {message}"
                )
            }
            Self::SchemaReferenceNotLocal { reference } => write!(
                formatter,
                "plugin settings schema reference `{reference}` is not local; only fragment references are allowed"
            ),
            Self::InvalidSettingsSchema { message } => write!(
                formatter,
                "plugin settings schema is not valid JSON Schema Draft 2020-12: {message}"
            ),
            Self::SettingsValidation { message } => {
                write!(
                    formatter,
                    "plugin settings do not match their schema: {message}"
                )
            }
            Self::ScopeResolution(error) => error.fmt(formatter),
            Self::PreparedHostMismatch { plugin } => write!(
                formatter,
                "prepared plugin `{plugin}` belongs to another HostBuilder and cannot be admitted here; prepare it with this builder"
            ),
            Self::DuplicateInstanceId { instance_id } => write!(
                formatter,
                "plugin instance id `{instance_id}` is already admitted to this HostBuilder"
            ),
            Self::AcceptancePluginMismatch {
                prepared,
                acceptance,
            } => write!(
                formatter,
                "prepared plugin `{prepared}` cannot use an acceptance bound to plugin `{acceptance}`"
            ),
            Self::AcceptanceDigestMismatch {
                plugin,
                prepared,
                acceptance,
            } => write!(
                formatter,
                "prepared plugin `{plugin}` has needs digest `{prepared}`, but the acceptance is bound to needs digest `{acceptance}`; accept this prepared request again"
            ),
        }
    }
}

impl Error for AdmissionError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Inspection(error) => Some(error),
            Self::UnsupportedExport(error) => Some(error),
            Self::ScopeResolution(error) => Some(error),
            _ => None,
        }
    }
}

#[cfg(test)]
mod grant_tests {
    use std::str::FromStr;

    use lockgate_policy::{Scope, ScopeError, ScopeRepr};
    use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
    use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata, ScopeRef};
    use wasm_encoder::{ComponentSection, CustomSection};
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    use super::*;

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum SessionScope {
        All,
        Current,
    }

    impl FromStr for SessionScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "all" | "everything" => Ok(Self::All),
                "current" => Ok(Self::Current),
                _ => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for SessionScope {
        fn canonical(&self) -> String {
            match self {
                Self::All => "all",
                Self::Current => "current",
            }
            .to_owned()
        }
    }

    impl Scope for SessionScope {}

    struct Session;

    impl crate::ScopedResource<SessionScope> for Session {
        fn scopes_for(&self, _subject: &crate::PluginSubject<'_>) -> Vec<SessionScope> {
            vec![SessionScope::All]
        }
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    enum ConflictingSessionScope {
        Other,
    }

    impl FromStr for ConflictingSessionScope {
        type Err = ScopeError;

        fn from_str(value: &str) -> Result<Self, Self::Err> {
            match value {
                "other" => Ok(Self::Other),
                _ => Err(ScopeError::unknown(value)),
            }
        }
    }

    impl ScopeRepr for ConflictingSessionScope {
        fn canonical(&self) -> String {
            "other".to_owned()
        }
    }

    impl Scope for ConflictingSessionScope {}

    #[lockgate_policy::capability("sessions")]
    mod permissions {
        use super::SessionScope;
        use lockgate_policy::{Permission, ScopedPermission};

        pub const READ: ScopedPermission<SessionScope> = ScopedPermission::new("read");
        pub const SEND: Permission = Permission::new("send");
    }

    #[lockgate_policy::capability("sessions")]
    mod conflicting_permissions {
        use super::ConflictingSessionScope;
        use lockgate_policy::ScopedPermission;

        pub const READ: ScopedPermission<ConflictingSessionScope> = ScopedPermission::new("read");
    }

    fn atom(value: &str) -> AtomKey {
        value.parse().unwrap()
    }

    fn component() -> Vec<u8> {
        let mut resolve = Resolve::new();
        let package = resolve
            .push_str(
                "fixture.wit",
                "package test:effective-grants; world fixture {}",
            )
            .unwrap();
        let world = resolve.select_world(&[package], None).unwrap();
        let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
        embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
        ComponentEncoder::default()
            .module(&module)
            .unwrap()
            .encode()
            .unwrap()
    }

    fn settings_schema_component() -> Vec<u8> {
        let schema = r#"{"type":"object","required":["scope"],"properties":{"scope":{"type":"string"}},"additionalProperties":false}"#;
        let encoded = schema
            .as_bytes()
            .iter()
            .map(|byte| format!(r"\{byte:02x}"))
            .collect::<String>();
        wat::parse_str(format!(
            r#"(component
                (core module $guest
                    (memory (export "memory") 1)
                    (data (i32.const 64) "{encoded}")
                    (func (export "settings-schema") (result i32)
                        (i32.store (i32.const 8) (i32.const 64))
                        (i32.store offset=4 (i32.const 8) (i32.const {length}))
                        (i32.const 8)
                    )
                )
                (core instance $guest-instance (instantiate $guest))
                (func $settings-schema (result string)
                    (canon lift
                        (core func $guest-instance "settings-schema")
                        (memory (core memory $guest-instance "memory"))
                    )
                )
                (instance $schema
                    (export "settings-schema" (func $settings-schema))
                )
                (export "lockgate:config/schema" (instance $schema))
            )"#,
            length = schema.len(),
        ))
        .unwrap()
    }

    #[test]
    fn pure_declared_needs_resolve_accept_and_freeze_for_queries() {
        let metadata = PluginMetadata::new("pure-grants", "Pure grants", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("sessions.read"),
                    vec![
                        ScopeRef::literal("everything").unwrap(),
                        ScopeRef::literal("all").unwrap(),
                    ],
                )
                .unwrap(),
            ],
            vec![NeedEntry::flag(atom("sessions.send"))],
        )
        .unwrap();
        let needs_digest = lockgate_schema::NeedsDigest::compute(&needs).unwrap();
        let mut registry = CapabilityRegistry::default();
        registry.register::<permissions::Contract>().unwrap();
        let resolved = resolve_needs(
            &needs,
            &serde_json::json!({}),
            &SymbolicRoots::default(),
            &registry,
        )
        .unwrap();
        let prepared_digest = PreparedNeedsDigest::compute(needs_digest, &resolved);
        let prepared = Prepared {
            host: HostId::next(),
            instance_id: "pure-instance".to_owned(),
            inspection: Inspection::new(metadata, needs, needs_digest, Vec::new()),
            resolved,
            prepared_digest,
            artifact: Box::new(()),
        };
        let acceptance = prepared.accept_all();
        let Prepared {
            instance_id,
            resolved,
            prepared_digest,
            ..
        } = prepared;

        let grants =
            bind_effective_grants(&instance_id, prepared_digest, resolved, &acceptance).unwrap();

        assert!(grants.has_unscoped(&atom("sessions.send")));
        assert!(!grants.has_unscoped(&atom("sessions.missing")));
        assert_eq!(
            grants.scoped_values(&atom("sessions.read")),
            Some(["all".to_owned()].as_slice())
        );
    }

    #[tokio::test]
    async fn identical_component_bytes_admit_as_instances_with_distinct_grants() {
        let metadata = PluginMetadata::new("shared-code", "Shared code", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("sessions.read"),
                    vec![ScopeRef::setting("/scope").unwrap()],
                )
                .unwrap(),
            ],
            vec![],
        )
        .unwrap();
        let bytes = with_section(
            with_section(
                settings_schema_component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            &needs.to_section_bytes().unwrap(),
        );
        let mut builder = HostBuilder::new(())
            .unwrap()
            .register::<permissions::Contract>()
            .unwrap();
        let prod = builder
            .prepare(
                "openai-prod",
                &bytes,
                PluginConfig {
                    settings: Some(serde_json::json!({ "scope": "all" })),
                    ..PluginConfig::default()
                },
            )
            .await
            .unwrap();
        let staging = builder
            .prepare(
                "openai-staging",
                &bytes,
                PluginConfig {
                    settings: Some(serde_json::json!({ "scope": "current" })),
                    ..PluginConfig::default()
                },
            )
            .await
            .unwrap();
        let prod_acceptance = prod.accept_all();
        let staging_acceptance = staging.accept_all();

        let prod = builder
            .admit(
                prod,
                prod_acceptance,
                RuntimeLimits::default(),
                InvocationCtx::bounded(1_000_000),
            )
            .await
            .unwrap();
        let staging = builder
            .admit(
                staging,
                staging_acceptance,
                RuntimeLimits::default(),
                InvocationCtx::bounded(1_000_000),
            )
            .await
            .unwrap();

        assert_eq!(prod.id(), "openai-prod");
        assert_eq!(staging.id(), "openai-staging");
        assert_ne!(prod.effective_grants(), staging.effective_grants());
        assert_eq!(
            prod.effective_grants()
                .scoped_values(&atom("sessions.read")),
            Some(["all".to_owned()].as_slice())
        );
        assert_eq!(
            staging
                .effective_grants()
                .scoped_values(&atom("sessions.read")),
            Some(["current".to_owned()].as_slice())
        );
    }

    #[tokio::test]
    async fn http_egress_grant_requires_the_wasi_http_client_import() {
        let metadata = PluginMetadata::new("http-no-import", "HTTP no import", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("http.egress"),
                    vec![ScopeRef::literal("https://example.com").unwrap()],
                )
                .unwrap(),
            ],
            vec![],
        )
        .unwrap();
        let component = with_section(
            with_section(
                component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            &needs.to_section_bytes().unwrap(),
        );
        let mut builder = HostBuilder::new(()).unwrap();

        let error = builder
            .prepare("http-no-import", &component, PluginConfig::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AdmissionError::HttpEgressUnavailable { ref plugin } if plugin == "http-no-import"
        ));
    }

    #[tokio::test]
    async fn empty_http_egress_origin_set_is_rejected_during_inspection() {
        let metadata = PluginMetadata::new("empty-http", "Empty HTTP", "1.0").unwrap();
        let raw_needs = br#"{"format":1,"optional":{},"reasons":{},"required":{"http.egress":[]}}"#;
        let component = with_section(
            with_section(
                component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            raw_needs,
        );
        let mut builder = HostBuilder::new(()).unwrap();

        let error = builder
            .prepare("empty-http", &component, PluginConfig::default())
            .await
            .unwrap_err();
        let message = error.to_string();

        assert!(matches!(error, AdmissionError::Inspection(_)));
        assert!(message.contains("scoped need must contain at least one scope"));
    }

    fn with_section(mut component: Vec<u8>, name: &str, data: &[u8]) -> Vec<u8> {
        CustomSection {
            name: name.into(),
            data: data.into(),
        }
        .append_to_component(&mut component);
        component
    }

    #[tokio::test]
    async fn prepared_artifacts_cannot_cross_builder_registry_boundaries() {
        let metadata = PluginMetadata::new("host-bound", "Host bound", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("sessions.read"),
                    vec![ScopeRef::literal("everything").unwrap()],
                )
                .unwrap(),
            ],
            vec![],
        )
        .unwrap();
        let component = with_section(
            with_section(
                component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            &needs.to_section_bytes().unwrap(),
        );
        let mut originating = HostBuilder::new(())
            .unwrap()
            .register::<permissions::Contract>()
            .unwrap();
        let prepared = originating
            .prepare("host-bound", &component, PluginConfig::default())
            .await
            .unwrap();
        let acceptance = prepared.accept_all();
        let mut conflicting = HostBuilder::new(())
            .unwrap()
            .register::<conflicting_permissions::Contract>()
            .unwrap();

        let error = conflicting
            .admit(
                prepared,
                acceptance,
                RuntimeLimits::default(),
                InvocationCtx::bounded(1_000_000),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AdmissionError::PreparedHostMismatch { ref plugin } if plugin == "host-bound"
        ));
        let message = error.to_string();
        assert!(message.contains("host-bound"));
        assert!(message.contains("another HostBuilder"));
    }

    #[tokio::test]
    async fn admitted_handle_carries_canonical_immutable_effective_grants() {
        let metadata = PluginMetadata::new("grant-query", "Grant query", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("sessions.read"),
                    vec![
                        ScopeRef::literal("everything").unwrap(),
                        ScopeRef::literal("all").unwrap(),
                    ],
                )
                .unwrap(),
            ],
            vec![NeedEntry::flag(atom("sessions.send"))],
        )
        .unwrap();
        let component = with_section(
            with_section(
                component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            &needs.to_section_bytes().unwrap(),
        );
        let mut builder = HostBuilder::new(())
            .unwrap()
            .register::<permissions::Contract>()
            .unwrap();
        let prepared = builder
            .prepare("grant-query", &component, PluginConfig::default())
            .await
            .unwrap();
        let acceptance = prepared.accept_all();

        let handle = builder
            .admit(
                prepared,
                acceptance,
                RuntimeLimits::default(),
                InvocationCtx::bounded(1_000_000),
            )
            .await
            .unwrap();

        assert!(
            handle
                .effective_grants()
                .has_unscoped(&atom("sessions.send"))
        );
        assert!(
            !handle
                .effective_grants()
                .has_unscoped(&atom("sessions.missing"))
        );
        assert_eq!(
            handle
                .effective_grants()
                .scoped_values(&atom("sessions.read")),
            Some(["all".to_owned()].as_slice())
        );
        assert_eq!(
            crate::PluginSubject::new(&handle).plugin_id(),
            "grant-query"
        );

        let jobs = DetachedJobContext::new(
            std::sync::Arc::clone(&builder.jobs),
            handle.id().to_owned(),
            1,
        );
        let cx = crate::HostCtx::new(&(), &handle, jobs, crate::policy::ResourceStore::__new());
        assert_eq!(cx.require(permissions::SEND), Ok(()));
        assert_eq!(cx.require_scoped(permissions::READ, &Session), Ok(()));
    }
}
