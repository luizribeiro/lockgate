use std::{
    any::Any,
    collections::{BTreeMap, BTreeSet},
    error::Error,
    fmt,
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use lockgate_policy::PluginId;
use lockgate_schema::{
    AtomKey, GrantSet, GrantValue, NeedKind, NeedsManifest, PluginMetadata, hex_encode,
};
use rustls::RootCertStore;
use sha2::{Digest, Sha256};

use crate::CallContext;
use crate::config::{SettingsValidationError, validate_settings};
use crate::exec::cache::CompiledComponentCache;
use crate::exec::{
    EnvironmentError, EnvironmentGrants, ExecEngine, ExecError, ExecLimits, HttpPool,
    ImportsFactory, InstanceAllocation, LoadError, LoadedComponent, TypedImports,
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

fn raw_component_sha256(bytes: &[u8]) -> [u8; 32] {
    Sha256::digest(bytes).into()
}

fn display_component_digest(digest: &[u8; 32]) -> String {
    format!("sha256:{}", hex_encode(digest))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct HostId(u64);

impl HostId {
    fn next() -> Self {
        Self(NEXT_HOST_ID.fetch_add(1, Ordering::Relaxed))
    }
}

/// Fuel and wall-clock bounds for one exported function call.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CallBudget {
    /// Maximum guest instructions available to the call.
    pub fuel: u64,
    /// Maximum elapsed time for guest execution and asynchronous host work.
    pub deadline: Duration,
}

impl CallBudget {
    pub(crate) fn validate(self, target: impl Into<String>) -> Result<(), InvalidCallBudget> {
        let target = target.into();
        if self.fuel == 0 {
            return Err(InvalidCallBudget {
                target,
                reason: "fuel must be greater than zero",
            });
        }
        if self.deadline.is_zero() {
            return Err(InvalidCallBudget {
                target,
                reason: "deadline must be greater than zero",
            });
        }
        Ok(())
    }
}

impl Default for CallBudget {
    fn default() -> Self {
        Self {
            fuel: 25_000_000,
            deadline: Duration::from_secs(30),
        }
    }
}

/// An invalid setup-time call budget.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidCallBudget {
    target: String,
    reason: &'static str,
}

impl fmt::Display for InvalidCallBudget {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "invalid budget for {}: {}",
            self.target, self.reason
        )
    }
}

impl Error for InvalidCallBudget {}

#[derive(Clone, Debug, Default)]
pub(crate) struct RegisteredCallBudgets(BTreeMap<(&'static str, &'static str), CallBudget>);

impl RegisteredCallBudgets {
    fn register<R: Role>(&mut self, budgets: R::Budgets) -> Result<(), InvalidCallBudget> {
        self.0
            .retain(|(interface, _), _| *interface != R::INTERFACE);
        for (function, budget) in crate::RoleBudgets::__into_entries(budgets) {
            budget.validate(format!("{}#{function}", R::INTERFACE))?;
            self.0.insert((R::INTERFACE, function), budget);
        }
        Ok(())
    }

    pub(crate) fn resolve(&self, interface: &'static str, function: &str) -> CallBudget {
        self.0
            .get(&(interface, function))
            .copied()
            .unwrap_or_default()
    }
}

/// Per-plugin limits applied to every fresh Store, including the smoke probe.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RuntimeLimits {
    pub instantiation_fuel: u64,
    pub max_memory_bytes: usize,
    pub max_detached_jobs: usize,
    /// Maximum guarded capability-import calls per invocation; `0` disables this limit.
    pub max_host_import_calls: u64,
    pub http_request_timeout_ceiling: Option<Duration>,
}

impl Default for RuntimeLimits {
    fn default() -> Self {
        Self {
            instantiation_fuel: 10_000_000,
            max_memory_bytes: 64 * 1024 * 1024,
            max_detached_jobs: 32,
            max_host_import_calls: 10_000,
            http_request_timeout_ceiling: None,
        }
    }
}

impl From<RuntimeLimits> for ExecLimits {
    fn from(limits: RuntimeLimits) -> Self {
        Self {
            instantiation_fuel: limits.instantiation_fuel,
            max_memory_bytes: limits.max_memory_bytes,
            max_host_import_calls: limits.max_host_import_calls,
            http_request_timeout_ceiling: limits.http_request_timeout_ceiling,
        }
    }
}

/// Acceptance of every concrete atom in one prepared plugin request.
///
/// Acceptances can only be produced by [`Prepared::accept_all`] or
/// [`Prepared::accept_reviewed`]. They retain the plugin identity and exact
/// prepared-needs digest that admission checks.
#[derive(Clone, Debug)]
pub struct Acceptance {
    plugin_id: PluginId,
    digest: PreparedNeedsDigest,
}

fn validate_acceptance(
    plugin_id: &PluginId,
    digest: PreparedNeedsDigest,
    acceptance: &Acceptance,
) -> Result<(), AdmissionError> {
    if &acceptance.plugin_id != plugin_id {
        return Err(AdmissionError::AcceptancePluginMismatch {
            prepared_plugin_id: plugin_id.clone(),
            acceptance_plugin_id: acceptance.plugin_id.clone(),
        });
    }
    if acceptance.digest != digest {
        return Err(AdmissionError::AcceptanceDigestMismatch {
            plugin_id: plugin_id.clone(),
            prepared: digest.to_string(),
            acceptance: acceptance.digest.to_string(),
        });
    }
    Ok(())
}

fn bind_effective_grants(
    plugin_id: &PluginId,
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
    compiled_cache: Option<CompiledComponentCache>,
    http_pool: std::sync::Arc<HttpPool>,
    call_budgets: RegisteredCallBudgets,
    admission_budget: CallBudget,
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
    plugin_id: PluginId,
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
            .field("plugin_id", &self.plugin_id)
            .field("metadata", &self.metadata)
            .field("effective_grants", &self.effective_grants)
            .finish_non_exhaustive()
    }
}

impl PartialEq for PluginHandle {
    fn eq(&self, other: &Self) -> bool {
        self.host == other.host
            && self.index == other.index
            && self.plugin_id == other.plugin_id
            && self.metadata == other.metadata
            && self.effective_grants == other.effective_grants
    }
}

impl Eq for PluginHandle {}

impl PluginHandle {
    /// Returns the operator-assigned plugin identifier.
    pub fn id(&self) -> &PluginId {
        &self.plugin_id
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
    pub(crate) fn for_policy_test(plugin_id: PluginId, effective_grants: EffectiveGrants) -> Self {
        Self::for_policy_test_with_registry(
            plugin_id,
            effective_grants,
            CapabilityRegistry::default(),
        )
    }

    #[cfg(test)]
    pub(crate) fn for_policy_test_with_registry(
        plugin_id: PluginId,
        effective_grants: EffectiveGrants,
        registry: CapabilityRegistry,
    ) -> Self {
        Self {
            host: HostId(0),
            index: 0,
            metadata: PluginMetadata::new(plugin_id.as_str(), "Policy test plugin", "1.0").unwrap(),
            plugin_id,
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
        Self::with_allocation(imports, InstanceAllocation::default())
    }

    /// Creates a builder with an explicitly selected instance allocator.
    ///
    /// Pooling capacity is fixed for the lifetime of the host. Each admitted
    /// plugin's [`RuntimeLimits::max_memory_bytes`] must fit within the configured
    /// pooling memory-slot size.
    pub fn with_allocation<I: crate::HostImports<S>>(
        imports: I,
        allocation: InstanceAllocation,
    ) -> Result<Self, HostConstructionError> {
        let policy_metadata = I::policy_metadata().map_err(HostConstructionError::HostImports)?;
        let engine = ExecEngine::with_allocation(allocation)
            .map_err(EngineError::new)
            .map_err(HostConstructionError::Engine)?;
        let jobs = JobTracker::new()
            .map_err(EngineError::new)
            .map_err(HostConstructionError::Engine)?;
        let mut registry = CapabilityRegistry::default();
        registry
            .register::<lockgate_policy::net::Contract>()
            .expect("Lockgate's built-in HTTP capability must be valid");
        registry
            .register::<lockgate_policy::env::Contract>()
            .expect("Lockgate's built-in environment capability must be valid");
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
            compiled_cache: None,
            http_pool: std::sync::Arc::new(HttpPool::new()),
            call_budgets: RegisteredCallBudgets::default(),
            admission_budget: CallBudget::default(),
        })
    }

    /// Enables persistent reuse of compiled Wasmtime components from `directory`.
    ///
    /// The directory is created on the first successful compilation. Cache reads
    /// and writes are best-effort: missing, corrupt, or engine-incompatible entries
    /// fall back to compilation and replacement without failing preparation.
    ///
    /// # Trust boundary
    ///
    /// A `.cwasm` entry contains executable native code and is loaded through
    /// Wasmtime's unsafe deserialization API. `directory` must therefore be owned
    /// by the embedder and must not be writable by plugins or other untrusted
    /// principals. Place it under application state, not alongside plugin or
    /// project files.
    ///
    /// Lockgate does not currently evict cache entries or impose a size limit.
    /// Embedders that require a capacity bound should prune this directory; a
    /// built-in oldest-first limit is left as a follow-up.
    pub fn compiled_cache(mut self, directory: PathBuf) -> Self {
        self.compiled_cache = Some(CompiledComponentCache::new(directory));
        self
    }

    /// Adds TLS trust anchors for every plugin admitted by this builder.
    ///
    /// The supplied anchors extend the WebPKI roots used by default; they do not
    /// replace public trust. The resulting store is shared by every outgoing
    /// `wasi:http` HTTPS connection made through the finished [`Host`]. Lockgate
    /// does not install a rustls crypto provider; the embedder remains responsible
    /// for doing so before the first HTTPS request.
    pub fn tls_roots(self, roots: RootCertStore) -> Self {
        let mut extended = RootCertStore {
            roots: webpki_roots::TLS_SERVER_ROOTS.into(),
        };
        extended.roots.extend(roots.roots);
        let extended = std::sync::Arc::new(extended);
        self.http_pool.set_tls_roots(extended);
        self
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

    /// Configures every exported function budget for one generated role.
    ///
    /// Roles without an explicit registration use [`CallBudget::default`] for
    /// each function. Registering the same role again replaces its prior budgets.
    pub fn budgets<R: Role>(mut self, budgets: R::Budgets) -> Result<Self, InvalidCallBudget> {
        self.call_budgets.register::<R>(budgets)?;
        Ok(self)
    }

    /// Configures the budget used by preflight and admission smoke instantiation.
    pub fn admission_budget(mut self, budget: CallBudget) -> Result<Self, InvalidCallBudget> {
        budget.validate("admission")?;
        self.admission_budget = budget;
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

    /// Inspects and compiles the artifact, then resolves its concrete consent
    /// surface for preflight and later admission.
    pub async fn prepare(
        &mut self,
        plugin_id: PluginId,
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
        let has_http_egress = validate_http_egress_grant(
            &plugin_id,
            &imported_interfaces,
            http_egress_origin_count(&needs),
        )?;
        if let Some(field) = unavailable_field {
            return Err(AdmissionError::ConfigFeatureUnavailable { field });
        }
        let component_sha256 = raw_component_sha256(bytes);
        let component = self
            .engine
            .compile_cached(bytes, &component_sha256, self.compiled_cache.as_ref())
            .map_err(AdmissionError::from_load)?;
        let empty_settings = serde_json::json!({});
        let settings = config.settings.as_ref().unwrap_or(&empty_settings);
        let resolved = resolve_needs(&needs, settings, &config.roots, &self.registry)
            .map_err(AdmissionError::ScopeResolution)?;
        let prepared_digest = PreparedNeedsDigest::compute(needs_digest, &resolved);
        let component_digest = display_component_digest(&component_sha256);
        let inspection = Inspection::new(metadata, needs, needs_digest, exported_interfaces);
        Ok(Prepared {
            host: self.id,
            plugin_id,
            inspection,
            resolved,
            prepared_digest,
            component_digest,
            artifact: Box::new(PreparedArtifact {
                component,
                settings: config.settings,
                roots: config.roots,
                wired_interfaces,
                has_http_egress,
            }),
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

    /// Validates a prepared plugin through linker wiring and smoke instantiation
    /// without requiring consent or minting a [`PluginHandle`].
    ///
    /// The returned report lists every required environment variable and whether
    /// it is currently present. Missing variables do not fail preflight; admission
    /// remains the readiness boundary that enforces them. The temporary Store and
    /// instance are dropped, and the plugin is not registered with this builder.
    pub async fn preflight_with_data(
        &mut self,
        prepared: &Prepared,
        limits: &RuntimeLimits,
        startup_data: S,
    ) -> Result<Preflight, AdmissionError> {
        let result = self
            .preflight_prepared(prepared, limits, startup_data, None)
            .await?;
        Ok(result.report)
    }

    /// Verifies prepared acceptance, preflights, and admits the plugin.
    ///
    /// Preflight establishes coherence: settings satisfy the plugin schema,
    /// imports wire, and a temporary instance starts successfully. Admission adds
    /// readiness by requiring consent and all required environment variables,
    /// then registers the retained artifact and returns its handle.
    ///
    /// Smoke instantiation uses the smaller of `limits.instantiation_fuel` and
    /// the configured admission budget, so an application-chosen
    /// startup budget may reject a constructor that steady-state calls would
    /// instantiate under the full limit.
    pub async fn admit_with_data(
        &mut self,
        prepared: Prepared,
        acceptance: Acceptance,
        limits: RuntimeLimits,
        startup_data: S,
    ) -> Result<PluginHandle, AdmissionError> {
        if prepared.host != self.id {
            return Err(AdmissionError::PreparedHostMismatch {
                plugin_id: prepared.plugin_id,
            });
        }
        if self
            .admitted
            .iter()
            .any(|plugin| plugin.handle.id() == &prepared.plugin_id)
        {
            return Err(AdmissionError::DuplicatePluginId {
                plugin_id: prepared.plugin_id,
            });
        }
        let effective_grants = bind_effective_grants(
            &prepared.plugin_id,
            prepared.prepared_digest,
            prepared.resolved.clone(),
            &acceptance,
        )?;
        let handle = PluginHandle {
            host: self.id,
            index: self.admitted.len(),
            plugin_id: prepared.plugin_id.clone(),
            metadata: prepared.inspection.metadata().clone(),
            effective_grants,
            registry: std::sync::Arc::new(self.registry.clone()),
        };
        let result = self
            .preflight_prepared(&prepared, &limits, startup_data, Some(&handle))
            .await?;

        self.admitted.push(AdmittedPlugin {
            handle: handle.clone(),
            artifact: result.artifact,
            limits,
        });
        Ok(handle)
    }

    async fn preflight_prepared(
        &self,
        prepared: &Prepared,
        limits: &RuntimeLimits,
        startup_data: S,
        admitted_handle: Option<&PluginHandle>,
    ) -> Result<PreflightResult<S>, AdmissionError> {
        if prepared.host != self.id {
            return Err(AdmissionError::PreparedHostMismatch {
                plugin_id: prepared.plugin_id.clone(),
            });
        }
        if let Some(max_memory_size) = self.engine.pooling_memory_slot_size()
            && limits.max_memory_bytes > max_memory_size
        {
            return Err(AdmissionError::PoolingMemoryLimitExceeded {
                max_memory_bytes: limits.max_memory_bytes,
                max_memory_size,
            });
        }
        let prepared_artifact = prepared
            .artifact
            .downcast_ref::<PreparedArtifact>()
            .expect("prepared artifact type must match its originating HostBuilder");
        let mut artifact = self
            .engine
            .load_hosted_component::<S>(
                &prepared_artifact.component,
                std::sync::Arc::clone(&self.imports),
                &prepared_artifact.wired_interfaces,
                prepared_artifact.has_http_egress,
            )
            .map_err(AdmissionError::from_load)?;
        let schema = self
            .engine
            .fetch_settings_schema(
                &prepared_artifact.component,
                RuntimeLimits::default().into(),
            )
            .await
            .map_err(AdmissionError::from_schema_fetch)?;
        let settings = validate_settings(schema.as_deref(), prepared_artifact.settings.clone())
            .map_err(AdmissionError::from_settings_validation)?;
        let resolved = resolve_needs(
            prepared.inspection.needs(),
            settings.value(),
            &prepared_artifact.roots,
            &self.registry,
        )
        .map_err(AdmissionError::ScopeResolution)?;
        debug_assert_eq!(resolved, prepared.resolved);
        artifact.set_settings(settings);
        artifact.set_http_pool(std::sync::Arc::clone(&self.http_pool));

        let report = environment_preflight(&prepared.resolved);
        match admitted_handle {
            Some(handle) => {
                artifact.set_environment_grants(environment_grants(
                    &prepared.plugin_id,
                    &prepared.resolved,
                ));
                artifact.set_plugin(
                    handle.clone(),
                    DetachedJobContext::new(
                        std::sync::Arc::clone(&self.jobs),
                        handle.id().clone(),
                        limits.max_detached_jobs,
                    ),
                );
            }
            None => artifact.set_environment_grants(preflight_environment_grants(
                &prepared.plugin_id,
                &prepared.resolved,
            )),
        }

        let CallBudget { fuel, deadline } = self.admission_budget;
        artifact
            .smoke(startup_data, (*limits).into(), fuel, deadline)
            .await
            .map_err(AdmissionError::from_smoke)?;
        Ok(PreflightResult { report, artifact })
    }

    /// Finishes configuration and transfers admitted plugins into a steady-state Host.
    ///
    /// Call [`Host::shutdown`] to abort and await detached jobs without blocking
    /// the calling thread. Dropping the returned Host only initiates best-effort
    /// shutdown and does not wait for detached jobs to finish.
    pub fn finish(self) -> Host<S> {
        Host {
            id: self.id,
            engine: self.engine,
            plugins: self.admitted,
            jobs: self.jobs,
            registry: self.registry,
            http_pool: self.http_pool,
            call_budgets: self.call_budgets,
        }
    }
}

impl HostBuilder<()> {
    /// Preflights a prepared plugin using the configured admission budget.
    pub async fn preflight(
        &mut self,
        prepared: &Prepared,
        limits: &RuntimeLimits,
    ) -> Result<Preflight, AdmissionError> {
        self.preflight_with_data(prepared, limits, ()).await
    }

    /// Admits a plugin using the configured admission budget.
    pub async fn admit(
        &mut self,
        prepared: Prepared,
        acceptance: Acceptance,
        limits: RuntimeLimits,
    ) -> Result<PluginHandle, AdmissionError> {
        self.admit_with_data(prepared, acceptance, limits, ()).await
    }
}

struct PreparedArtifact {
    component: wasmtime::component::Component,
    settings: Option<serde_json::Value>,
    roots: SymbolicRoots,
    wired_interfaces: Vec<String>,
    has_http_egress: bool,
}

struct PreflightResult<S: 'static> {
    report: Preflight,
    artifact: LoadedComponent<S>,
}

/// The consent-free coherence report produced by [`HostBuilder::preflight`].
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Preflight {
    /// Required environment variables and their current host presence.
    pub required_environment_variables: Vec<RequiredEnvironmentVariable>,
}

/// One required environment variable observed during preflight.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RequiredEnvironmentVariable {
    /// The exact host environment variable name requested by the plugin.
    pub name: String,
    /// Whether the variable is currently present, regardless of Unicode validity.
    pub present: bool,
}

fn environment_preflight(resolved: &ResolvedNeeds) -> Preflight {
    let required_environment_variables = environment_grant_values(&resolved.required)
        .into_iter()
        .map(|name| RequiredEnvironmentVariable {
            present: std::env::var_os(&name).is_some(),
            name,
        })
        .collect();
    Preflight {
        required_environment_variables,
    }
}

fn preflight_environment_grants(
    plugin_id: &PluginId,
    resolved: &ResolvedNeeds,
) -> EnvironmentGrants {
    let mut available = environment_grant_values(&resolved.required);
    available.extend(environment_grant_values(&resolved.optional));
    EnvironmentGrants::new(plugin_id.clone(), Vec::new(), available)
}

fn environment_grants(plugin_id: &PluginId, resolved: &ResolvedNeeds) -> EnvironmentGrants {
    EnvironmentGrants::new(
        plugin_id.clone(),
        environment_grant_values(&resolved.required),
        environment_grant_values(&resolved.optional),
    )
}

fn environment_grant_values(grants: &GrantSet) -> Vec<String> {
    let (capability, permission) =
        lockgate_policy::__private::scoped_permission_ids(lockgate_policy::env::READ);
    let atom = AtomKey::new(capability, permission)
        .expect("typed permissions always contain a valid wire atom");
    scoped_grant_values(grants, &atom)
}

fn scoped_grant_values(grants: &GrantSet, atom: &AtomKey) -> Vec<String> {
    match grants.get(atom) {
        Some(GrantValue::Scopes(values)) => values.clone(),
        Some(GrantValue::Flag) => {
            unreachable!("a resolved scoped permission cannot contain an unscoped grant")
        }
        None => Vec::new(),
    }
}

fn http_egress_origin_count(needs: &NeedsManifest) -> Option<usize> {
    let (capability, permission) =
        lockgate_policy::__private::scoped_permission_ids(lockgate_policy::net::EGRESS);
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
    plugin_id: &PluginId,
    imported_interfaces: &[String],
    origin_count: Option<usize>,
) -> Result<bool, AdmissionError> {
    let Some(origin_count) = origin_count else {
        return Ok(false);
    };
    // Needs decoding rejects empty scoped needs first; retain this as defense in depth.
    if origin_count == 0 {
        return Err(AdmissionError::EmptyHttpEgressOrigins {
            plugin_id: plugin_id.clone(),
        });
    }
    if !imported_interfaces
        .iter()
        .any(|interface| interface == "wasi:http/client@0.3.0")
    {
        return Err(AdmissionError::HttpEgressUnavailable {
            plugin_id: plugin_id.clone(),
        });
    }
    Ok(true)
}

/// Steady-state owner of the execution engine and admitted plugins.
///
/// Call [`Host::shutdown`] to abort and await all detached jobs and pooled HTTP
/// connection drivers without blocking the calling thread. Dropping a Host only
/// initiates best-effort shutdown and lets task cleanup finish in the background.
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
    http_pool: std::sync::Arc<HttpPool>,
    call_budgets: RegisteredCallBudgets,
}

impl<S: CallContext> Drop for Host<S> {
    fn drop(&mut self) {
        self.jobs.begin_shutdown();
        self.http_pool.begin_shutdown();
    }
}

impl<S: CallContext> Host<S> {
    /// Aborts and awaits all detached jobs and pooled HTTP connection drivers.
    pub async fn shutdown(self) {
        self.jobs.shutdown().await;
        self.http_pool.shutdown().await;
    }

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
            &self.call_budgets,
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

/// An inspected and compiled plugin request ready for consent and preflight.
pub struct Prepared {
    host: HostId,
    pub(crate) plugin_id: PluginId,
    pub(crate) inspection: Inspection,
    pub(crate) resolved: ResolvedNeeds,
    pub(crate) prepared_digest: PreparedNeedsDigest,
    pub(crate) component_digest: String,
    artifact: Box<dyn Any + Send>,
}

impl fmt::Debug for Prepared {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Prepared")
            .field("plugin_id", &self.plugin_id)
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
        Acceptance {
            plugin_id: self.plugin_id.clone(),
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
    /// A plugin's memory ceiling is larger than the host's pooled memory slots.
    PoolingMemoryLimitExceeded {
        /// Per-store memory ceiling requested for the plugin.
        max_memory_bytes: usize,
        /// Maximum memory size reserved in each pooling slot.
        max_memory_size: usize,
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
        plugin_id: PluginId,
    },
    EmptyHttpEgressOrigins {
        plugin_id: PluginId,
    },
    RequiredEnvironmentVariableUnset {
        plugin_id: PluginId,
        variable: String,
    },
    RequiredEnvironmentVariableNotUnicode {
        plugin_id: PluginId,
        variable: String,
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
        plugin_id: PluginId,
    },
    DuplicatePluginId {
        plugin_id: PluginId,
    },
    AcceptancePluginMismatch {
        prepared_plugin_id: PluginId,
        acceptance_plugin_id: PluginId,
    },
    AcceptanceDigestMismatch {
        plugin_id: PluginId,
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

    /// Returns developer-facing guidance for resolving this error, when available.
    pub fn hint(&self) -> Option<&'static str> {
        match self {
            Self::UnsupportedExport(error) => error.hint(),
            Self::HostImportManifestMismatch { .. } => Some(
                "add at least one as a required or optional need, or remove the interface import",
            ),
            Self::PoolingMemoryLimitExceeded { .. } => Some(
                "increase PoolingAllocationConfig::max_memory_size or lower RuntimeLimits::max_memory_bytes",
            ),
            Self::PreparedHostMismatch { .. } => Some("prepare it with this builder"),
            Self::AcceptanceDigestMismatch { .. } => Some("accept this prepared request again"),
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
            ExecError::Environment(EnvironmentError::RequiredUnset {
                plugin_id,
                variable,
            }) => Self::RequiredEnvironmentVariableUnset {
                plugin_id,
                variable,
            },
            ExecError::Environment(EnvironmentError::RequiredNotUnicode {
                plugin_id,
                variable,
            }) => Self::RequiredEnvironmentVariableNotUnicode {
                plugin_id,
                variable,
            },
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
            Self::PoolingMemoryLimitExceeded {
                max_memory_bytes,
                max_memory_size,
            } => write!(
                formatter,
                "plugin memory limit of {max_memory_bytes} bytes exceeds the pooling allocator slot size of {max_memory_size} bytes"
            ),
            Self::UnregisteredGuardPermission {
                atom,
                interface,
                method,
            } => write!(
                formatter,
                "host import `{interface}.{method}` requires permission `{atom}`, but the application never registered that permission"
            ),
            Self::HostImportManifestMismatch { interface, .. } => write!(
                formatter,
                "plugin imports host interface `{interface}` but declares none of its permissions"
            ),
            Self::HttpEgressUnavailable { plugin_id } => write!(
                formatter,
                "plugin `{plugin_id}` has an HTTP egress grant but does not import `wasi:http/client@0.3.0`"
            ),
            Self::EmptyHttpEgressOrigins { plugin_id } => write!(
                formatter,
                "plugin `{plugin_id}` cannot hold an HTTP egress grant with no origins"
            ),
            Self::RequiredEnvironmentVariableUnset {
                plugin_id,
                variable,
            } => write!(
                formatter,
                "plugin instance `{plugin_id}` requires host environment variable `{variable}`, but it is unset"
            ),
            Self::RequiredEnvironmentVariableNotUnicode {
                plugin_id,
                variable,
            } => write!(
                formatter,
                "plugin instance `{plugin_id}` requires host environment variable `{variable}`, but its value is not valid Unicode"
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
            Self::PreparedHostMismatch { plugin_id } => write!(
                formatter,
                "prepared plugin `{plugin_id}` belongs to another HostBuilder and cannot be admitted here"
            ),
            Self::DuplicatePluginId { plugin_id } => write!(
                formatter,
                "plugin instance id `{plugin_id}` is already admitted to this HostBuilder"
            ),
            Self::AcceptancePluginMismatch {
                prepared_plugin_id,
                acceptance_plugin_id,
            } => write!(
                formatter,
                "prepared plugin `{prepared_plugin_id}` cannot use an acceptance bound to plugin `{acceptance_plugin_id}`"
            ),
            Self::AcceptanceDigestMismatch {
                plugin_id,
                prepared,
                acceptance,
            } => write!(
                formatter,
                "prepared plugin `{plugin_id}` has needs digest `{prepared}`, but the acceptance is bound to needs digest `{acceptance}`"
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
    use std::{
        fs,
        path::{Path, PathBuf},
        str::FromStr,
        time::Instant,
    };

    use lockgate_policy::{Scope, ScopeError, ScopeRepr};
    use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
    use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata, ScopeRefEntry};
    use tempfile::tempdir;
    use wasmtime::{Config, Engine, OptLevel, Precompiled};
    use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
    use wit_parser::{ManglingAndAbi, Resolve};

    use super::*;
    use crate::test_support::{settings_schema_component, with_section};

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

    fn cache_fixture(plugin_id: &PluginId) -> Vec<u8> {
        let metadata =
            PluginMetadata::new(plugin_id.as_str(), "Compiled cache fixture", "1.0").unwrap();
        let needs = NeedsManifest::empty();
        with_section(
            with_section(
                component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            &needs.to_section_bytes().unwrap(),
        )
    }

    fn only_cache_entry(directory: &Path) -> PathBuf {
        let entries = fs::read_dir(directory)
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.extension()
                    .is_some_and(|extension| extension == "cwasm")
            })
            .collect::<Vec<_>>();
        assert_eq!(entries.len(), 1, "expected exactly one .cwasm entry");
        entries.into_iter().next().unwrap()
    }

    #[tokio::test]
    async fn prepare_records_the_raw_component_sha256() {
        let metadata = PluginMetadata::new("digest", "Digest", "1.0").unwrap();
        let needs = NeedsManifest::empty();
        let bytes = with_section(
            with_section(
                component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            &needs.to_section_bytes().unwrap(),
        );
        let expected = {
            let digest: [u8; 32] = Sha256::digest(&bytes).into();
            format!("sha256:{}", hex_encode(&digest))
        };
        let mut builder = HostBuilder::new(()).unwrap();

        let prepared = builder
            .prepare(
                PluginId::try_from("digest").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();

        assert_eq!(prepared.component_digest, expected);
    }

    #[tokio::test]
    async fn compiled_cache_hits_across_host_builders() {
        let directory = tempdir().unwrap();
        let bytes = cache_fixture(&PluginId::try_from("cache-hit").unwrap());
        let mut cold = HostBuilder::new(())
            .unwrap()
            .compiled_cache(directory.path().to_path_buf());

        let started = Instant::now();
        let prepared = cold
            .prepare(
                PluginId::try_from("cache-hit").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        let cold_elapsed = started.elapsed();
        assert_eq!(cold.engine.cache_hits(), 0);
        assert_eq!(cold.engine.cache_writes(), 1);
        assert!(only_cache_entry(directory.path()).is_file());
        drop(prepared);
        drop(cold);

        let mut cached = HostBuilder::new(())
            .unwrap()
            .compiled_cache(directory.path().to_path_buf());
        let started = Instant::now();
        cached
            .prepare(
                PluginId::try_from("cache-hit").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        let cached_elapsed = started.elapsed();
        assert_eq!(cached.engine.cache_hits(), 1);
        assert_eq!(cached.engine.cache_writes(), 0);

        eprintln!("fixture prepare timing: cold={cold_elapsed:?}, cached={cached_elapsed:?}");
    }

    #[tokio::test]
    async fn corrupt_compiled_cache_entry_is_recompiled_and_overwritten() {
        let directory = tempdir().unwrap();
        let bytes = cache_fixture(&PluginId::try_from("cache-corrupt").unwrap());
        let mut cold = HostBuilder::new(())
            .unwrap()
            .compiled_cache(directory.path().to_path_buf());
        cold.prepare(
            PluginId::try_from("cache-corrupt").unwrap(),
            &bytes,
            PluginConfig::default(),
        )
        .await
        .unwrap();
        drop(cold);

        let entry = only_cache_entry(directory.path());
        fs::write(&entry, b"not a compiled component").unwrap();

        let mut repaired = HostBuilder::new(())
            .unwrap()
            .compiled_cache(directory.path().to_path_buf());
        repaired
            .prepare(
                PluginId::try_from("cache-corrupt").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        assert_eq!(repaired.engine.cache_hits(), 0);
        assert_eq!(repaired.engine.cache_writes(), 1);
        assert_eq!(
            Engine::detect_precompiled_file(&entry).unwrap(),
            Some(Precompiled::Component)
        );
        drop(repaired);

        let mut cached = HostBuilder::new(())
            .unwrap()
            .compiled_cache(directory.path().to_path_buf());
        cached
            .prepare(
                PluginId::try_from("cache-corrupt").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        assert_eq!(cached.engine.cache_hits(), 1);
    }

    #[test]
    fn compiled_cache_key_changes_with_engine_configuration() {
        let mut speed = Config::new();
        speed.cranelift_opt_level(OptLevel::Speed);
        let speed = Engine::new(&speed).unwrap();
        let mut unoptimized = Config::new();
        unoptimized.cranelift_opt_level(OptLevel::None);
        let unoptimized = Engine::new(&unoptimized).unwrap();
        let digest = [0x5a; 32];

        assert_ne!(
            crate::exec::cache::cache_key(&speed, &digest),
            crate::exec::cache::cache_key(&unoptimized, &digest)
        );
    }

    #[tokio::test]
    async fn no_compiled_cache_configuration_performs_no_cache_writes() {
        let bytes = cache_fixture(&PluginId::try_from("cache-disabled").unwrap());
        let mut builder = HostBuilder::new(()).unwrap();

        builder
            .prepare(
                PluginId::try_from("cache-disabled").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();

        assert_eq!(builder.engine.cache_hits(), 0);
        assert_eq!(builder.engine.cache_writes(), 0);
    }

    #[test]
    fn pure_declared_needs_resolve_accept_and_freeze_for_queries() {
        let metadata = PluginMetadata::new("pure-grants", "Pure grants", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("sessions.read"),
                    vec![
                        ScopeRefEntry::literal("everything").unwrap(),
                        ScopeRefEntry::literal("all").unwrap(),
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
        let component_bytes = component();
        let component_digest = display_component_digest(&raw_component_sha256(&component_bytes));
        let prepared = Prepared {
            host: HostId::next(),
            plugin_id: PluginId::try_from("pure-instance").unwrap(),
            inspection: Inspection::new(metadata, needs, needs_digest, Vec::new()),
            resolved,
            prepared_digest,
            component_digest,
            artifact: Box::new(()),
        };
        let acceptance = prepared.accept_all();
        let Prepared {
            plugin_id,
            resolved,
            prepared_digest,
            ..
        } = prepared;

        let grants =
            bind_effective_grants(&plugin_id, prepared_digest, resolved, &acceptance).unwrap();

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
                    vec![ScopeRefEntry::setting("/scope").unwrap()],
                )
                .unwrap(),
            ],
            vec![],
        )
        .unwrap();
        let bytes = with_section(
            with_section(
                settings_schema_component(
                    r#"{"type":"object","required":["scope"],"properties":{"scope":{"type":"string"}},"additionalProperties":false}"#,
                ),
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
                PluginId::try_from("openai-prod").unwrap(),
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
                PluginId::try_from("openai-staging").unwrap(),
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
            .admit(prod, prod_acceptance, RuntimeLimits::default())
            .await
            .unwrap();
        let staging = builder
            .admit(staging, staging_acceptance, RuntimeLimits::default())
            .await
            .unwrap();

        assert_eq!(prod.id().as_str(), "openai-prod");
        assert_eq!(staging.id().as_str(), "openai-staging");
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
    async fn failed_admission_does_not_reserve_the_plugin_id() {
        let metadata = PluginMetadata::new("retry-code", "Retry code", "1.0").unwrap();
        let needs = NeedsManifest::empty();
        let bytes = with_section(
            with_section(
                component(),
                PLUGIN_METADATA_SECTION,
                &metadata.to_section_bytes().unwrap(),
            ),
            PLUGIN_NEEDS_SECTION,
            &needs.to_section_bytes().unwrap(),
        );
        let mut builder = HostBuilder::new(()).unwrap();
        let rejected = builder
            .prepare(
                PluginId::try_from("retry").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        let other = builder
            .prepare(
                PluginId::try_from("other").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        let mismatched_acceptance = other.accept_all();

        let error = builder
            .admit(rejected, mismatched_acceptance, RuntimeLimits::default())
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AdmissionError::AcceptancePluginMismatch {
                ref prepared_plugin_id,
                ref acceptance_plugin_id,
            } if prepared_plugin_id.as_str() == "retry"
                && acceptance_plugin_id.as_str() == "other"
        ));

        let retry = builder
            .prepare(
                PluginId::try_from("retry").unwrap(),
                &bytes,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        let acceptance = retry.accept_all();
        let handle = builder
            .admit(retry, acceptance, RuntimeLimits::default())
            .await
            .unwrap();

        assert_eq!(handle.id().as_str(), "retry");
    }

    #[tokio::test]
    async fn http_egress_grant_requires_the_wasi_http_client_import() {
        let metadata = PluginMetadata::new("http-no-import", "HTTP no import", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("net.egress"),
                    vec![ScopeRefEntry::literal("https://example.com").unwrap()],
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
            .prepare(
                PluginId::try_from("http-no-import").unwrap(),
                &component,
                PluginConfig::default(),
            )
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AdmissionError::HttpEgressUnavailable { ref plugin_id }
                if plugin_id.as_str() == "http-no-import"
        ));
    }

    #[tokio::test]
    async fn empty_http_egress_origin_set_is_rejected_during_inspection() {
        let metadata = PluginMetadata::new("empty-http", "Empty HTTP", "1.0").unwrap();
        let raw_needs = br#"{"format":1,"optional":{},"reasons":{},"required":{"net.egress":[]}}"#;
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
            .prepare(
                PluginId::try_from("empty-http").unwrap(),
                &component,
                PluginConfig::default(),
            )
            .await
            .unwrap_err();
        let message = error.to_string();

        assert!(matches!(error, AdmissionError::Inspection(_)));
        assert!(message.contains("scoped need must contain at least one scope"));
    }

    #[tokio::test]
    async fn prepared_artifacts_cannot_cross_builder_registry_boundaries() {
        let metadata = PluginMetadata::new("host-bound", "Host bound", "1.0").unwrap();
        let needs = NeedsManifest::new(
            vec![
                NeedEntry::scoped(
                    atom("sessions.read"),
                    vec![ScopeRefEntry::literal("everything").unwrap()],
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
            .prepare(
                PluginId::try_from("host-bound").unwrap(),
                &component,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        let acceptance = prepared.accept_all();
        let mut conflicting = HostBuilder::new(())
            .unwrap()
            .register::<conflicting_permissions::Contract>()
            .unwrap();

        let error = conflicting
            .admit(prepared, acceptance, RuntimeLimits::default())
            .await
            .unwrap_err();

        assert!(matches!(
            error,
            AdmissionError::PreparedHostMismatch { ref plugin_id }
                if plugin_id.as_str() == "host-bound"
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
                        ScopeRefEntry::literal("everything").unwrap(),
                        ScopeRefEntry::literal("all").unwrap(),
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
            .prepare(
                PluginId::try_from("grant-query").unwrap(),
                &component,
                PluginConfig::default(),
            )
            .await
            .unwrap();
        let acceptance = prepared.accept_all();

        let handle = builder
            .admit(prepared, acceptance, RuntimeLimits::default())
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
            crate::PluginSubject::new(&handle).plugin_id().as_str(),
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
