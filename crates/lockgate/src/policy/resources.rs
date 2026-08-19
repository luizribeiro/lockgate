//! Application-owned resource classification and target resolution.

use std::{
    any::{Any, TypeId},
    fmt,
    future::Future,
    str::FromStr,
    sync::{Arc, Mutex, MutexGuard},
};

use lockgate_policy::{Scope, ScopeError, ScopedPermission};
use lockgate_schema::AtomKey;
use wasmtime::component::{Resource, ResourceTable, ResourceTableError};

use super::{CapabilityRegistry, EffectiveGrants};
use crate::{CallContext, PluginHandle};

/// The admitted plugin identity making one host invocation.
///
/// Lockgate constructs this value from the invocation's [`PluginHandle`]
/// before resolving or classifying a scoped target. Keeping the subject as a
/// dedicated value leaves room for future stable subject attributes without
/// making application policy depend on Lockgate's full plugin handle.
#[derive(Clone, Copy)]
pub struct PluginSubject<'a> {
    plugin: &'a PluginHandle,
}

impl fmt::Debug for PluginSubject<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("PluginSubject")
            .field("plugin_id", &self.plugin_id())
            .finish()
    }
}

impl<'a> PluginSubject<'a> {
    pub(crate) const fn new(plugin: &'a PluginHandle) -> Self {
        Self { plugin }
    }

    /// Returns the stable ID of the plugin making this invocation.
    pub fn plugin_id(&self) -> &str {
        self.plugin.id()
    }
}

/// A host import was denied because its required permission was not granted.
///
/// Guarded host methods receive this error through [`crate::HostCtx::require`]
/// or [`crate::HostCtx::require_scoped`]. Their ordinary error type must
/// implement `From<PermissionDenied>` so generated enforcement can return the
/// denial through the WIT method's normal `Result` channel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PermissionDenied {
    capability: &'static str,
    permission: &'static str,
}

impl PermissionDenied {
    pub(crate) const fn new(capability: &'static str, permission: &'static str) -> Self {
        Self {
            capability,
            permission,
        }
    }

    /// Returns the stable capability ID of the denied permission.
    pub const fn capability(&self) -> &'static str {
        self.capability
    }

    /// Returns the stable permission ID within the capability.
    pub const fn permission(&self) -> &'static str {
        self.permission
    }
}

impl fmt::Display for PermissionDenied {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            formatter,
            "permission `{}.{}` is not granted for this host call",
            self.capability, self.permission
        )
    }
}

impl std::error::Error for PermissionDenied {}

/// Classifies an application-owned resource into scope membership witnesses.
///
/// Implementations must report every concrete witness needed to preserve each
/// authority distinction promised by `S`. For every grant meant to authorize
/// this resource, at least one returned witness must be contained by that
/// grant. In particular, a broad grant does not authorize an empty result:
/// returning an empty vector deliberately denies access (fail closed).
///
/// Classification is pure and synchronous. Perform fallible or asynchronous
/// lookup through [`ResolveScopedResource`] or
/// [`ResolveScopedResourceHandle`]. A resolver may retain a live reference or
/// return an owned authorization snapshot when classification needs facts
/// loaded from external state.
/// Duplicate witnesses are permitted and do not change authorization's
/// existential containment relation.
pub trait ScopedResource<S: Scope>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    /// Returns all scope membership witnesses for `self` and `subject`.
    fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<S>;
}

impl<S, R> ScopedResource<S> for Arc<R>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
    R: ScopedResource<S> + ?Sized,
{
    fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<S> {
        (**self).scopes_for(subject)
    }
}

/// Failure while accessing one live host representation by WIT resource handle.
///
/// Generated resource guards propagate this value through the application's
/// ordinary resolver-error conversion. A stale, fabricated, or already
/// deleted handle is therefore reported by the host method's normal error
/// channel rather than panicking.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ResourceLookupError {
    /// The handle has no live entry in this invocation's resource table.
    NotPresent,
    /// The handle's entry contains a different application representation.
    WrongRepresentation,
    /// The invocation resource table has reached its configured capacity.
    Full,
    /// The resource cannot be removed while child resources remain live.
    HasChildren,
    /// Another host call panicked while it held the resource table lock.
    TablePoisoned,
}

impl fmt::Display for ResourceLookupError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::NotPresent => "resource handle is not present in this invocation",
            Self::WrongRepresentation => {
                "resource handle refers to a different host representation"
            }
            Self::Full => "invocation resource table has no free entries",
            Self::HasChildren => "resource still has live child resources",
            Self::TablePoisoned => "invocation resource table is unavailable",
        })
    }
}

impl std::error::Error for ResourceLookupError {}

impl From<ResourceTableError> for ResourceLookupError {
    fn from(error: ResourceTableError) -> Self {
        match error {
            ResourceTableError::Full => Self::Full,
            ResourceTableError::NotPresent => Self::NotPresent,
            ResourceTableError::WrongType => Self::WrongRepresentation,
            ResourceTableError::HasChildren => Self::HasChildren,
        }
    }
}

struct ResourceEntry(Option<Arc<dyn Any + Send + Sync>>);

/// One Store-owned resource table shared only with host calls in that Store.
///
/// The `Arc` here lets overlapping host-call adapters retain the table handle;
/// the table itself is created afresh for each invocation and is deliberately
/// not stored in the per-call cloned `HostImports` value.
#[doc(hidden)]
#[derive(Clone)]
pub struct ResourceStore {
    table: Arc<Mutex<ResourceTable>>,
}

impl ResourceStore {
    #[doc(hidden)]
    pub fn __new() -> Self {
        Self {
            table: Arc::new(Mutex::new(ResourceTable::new())),
        }
    }

    /// Reserved for generated WIT resource destructors.
    #[doc(hidden)]
    pub fn __delete_resource<H: 'static>(
        &self,
        handle: &Resource<H>,
    ) -> Result<(), ResourceLookupError> {
        self.delete(handle)
    }

    fn lock(&self) -> Result<MutexGuard<'_, ResourceTable>, ResourceLookupError> {
        self.table
            .lock()
            .map_err(|_| ResourceLookupError::TablePoisoned)
    }

    fn insert<H: 'static, R: Any + Send + Sync>(
        &self,
        resource: R,
    ) -> Result<Resource<H>, ResourceLookupError> {
        let mut table = self.lock()?;
        let entry = table.push(ResourceEntry(Some(Arc::new(resource))))?;
        Ok(Resource::new_own(entry.rep()))
    }

    fn get<H: 'static, R: Any + Send + Sync>(
        &self,
        handle: &Resource<H>,
    ) -> Result<Arc<R>, ResourceLookupError> {
        let table = self.lock()?;
        let key = Resource::<ResourceEntry>::new_borrow(handle.rep());
        let entry = table.get(&key)?;
        Arc::clone(entry.0.as_ref().ok_or(ResourceLookupError::NotPresent)?)
            .downcast::<R>()
            .map_err(|_| ResourceLookupError::WrongRepresentation)
    }

    fn delete<H: 'static>(&self, handle: &Resource<H>) -> Result<(), ResourceLookupError> {
        let mut table = self.lock()?;
        // Keep an empty entry instead of returning its numeric slot to
        // ResourceTable's free list. Guest-visible handles carry that numeric
        // representation, so non-reuse prevents an invalidated stale handle
        // from ever naming a later resource (the ABA case).
        let key = Resource::<ResourceEntry>::new_borrow(handle.rep());
        let entry = table.get_mut(&key)?;
        entry.0.take().ok_or(ResourceLookupError::NotPresent)?;
        Ok(())
    }
}

/// Narrow context for resolving one resource-method authorization target.
///
/// This context exposes the calling [`PluginSubject`], immutable invocation
/// data, and typed operations on the invocation's resource table. It does not
/// expose effective grants or any way to mutate them; authorization remains a
/// separate generated step after resolution and classification.
///
/// Lockgate's default table shape stores an [`Arc`] to the live host
/// representation. Repeated lookups of one handle therefore reuse the live
/// reference, but generated guards still call the resolver, `scopes_for`, and
/// the effective-grant check on every method call. Applications can instead
/// store a stable identity and load current state in
/// [`ResolveScopedResourceHandle`]. Lockgate does not cache membership-fact
/// snapshots. An application that chooses to return such a snapshot owns the
/// required lifetime, epoch, or invalidation check and must perform it on
/// every call.
#[derive(Clone, Copy)]
pub struct ResolveCtx<'a, C> {
    data: &'a C,
    subject: PluginSubject<'a>,
    resources: &'a ResourceStore,
}

impl<'a, C> ResolveCtx<'a, C> {
    pub(crate) const fn new(
        data: &'a C,
        plugin: &'a PluginHandle,
        resources: &'a ResourceStore,
    ) -> Self {
        Self {
            data,
            subject: PluginSubject::new(plugin),
            resources,
        }
    }

    /// Returns the stable subject for the plugin making this invocation.
    pub const fn subject(&self) -> PluginSubject<'a> {
        self.subject
    }

    /// Returns immutable application call context for this invocation.
    pub const fn data(&self) -> &'a C {
        self.data
    }

    /// Inserts one live representation and returns its typed WIT handle.
    ///
    /// Resource constructors normally call this once. Insertion records no
    /// permission or membership facts and grants no authority to later
    /// methods on the returned handle.
    pub fn insert_resource<H: 'static, R: Any + Send + Sync>(
        &self,
        resource: R,
    ) -> Result<Resource<H>, ResourceLookupError> {
        self.resources.insert(resource)
    }

    /// Looks up and retains the live representation for `handle`.
    ///
    /// The returned `Arc` is a live reference, not an authorization result.
    /// Generated guards reevaluate classification and grants after every
    /// lookup.
    pub fn resource<H: 'static, R: Any + Send + Sync>(
        &self,
        handle: &Resource<H>,
    ) -> Result<Arc<R>, ResourceLookupError> {
        self.resources.get(handle)
    }

    /// Removes a live representation from this invocation's table.
    ///
    /// This is primarily a lifecycle operation. Any later method using the
    /// same handle observes [`ResourceLookupError::NotPresent`]. Generated WIT
    /// destructors also remove their entry through this path.
    pub fn delete_resource<H: 'static>(
        &self,
        handle: &Resource<H>,
    ) -> Result<(), ResourceLookupError> {
        self.resources.delete(handle)
    }
}

/// Resolves a host-call target expression to an application-owned resource.
///
/// Resolution is fallible and may be asynchronous; the returned resource's
/// [`ScopedResource`] implementation performs the separate, pure
/// classification step. Rust selects a resolver from both the permission's
/// scope type `S` and the target expression type `A`.
///
/// The explicit `impl Future + Send` return matches Lockgate's generated async
/// host boundary while allowing implementations to use native `async fn`
/// syntax without boxing.
pub trait ResolveScopedResource<S, A: ?Sized + Sync>: Send + Sync
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    /// The owned resource or authorization snapshot produced by resolution.
    type Resource: ScopedResource<S> + Send;

    /// The application's domain error for failed resolution.
    type Error: Send;

    /// Resolves `argument` for the calling plugin.
    fn resolve_scoped_resource<'a>(
        &'a self,
        subject: &'a PluginSubject<'_>,
        argument: &'a A,
    ) -> impl Future<Output = Result<Self::Resource, Self::Error>> + Send + 'a;
}

/// Resolves a live WIT resource representation for scoped authorization.
///
/// This is the resource-handle counterpart to [`ResolveScopedResource`]. The
/// separate entry point is necessary because the ordinary `&self` resolver
/// cannot reach the per-Store resource table: `HostImports` is cloned for each
/// host call, while resource handles belong to the invocation Store.
///
/// Lockgate validates `handle` and obtains `Arc<Self::Representation>` before
/// calling this trait. Implementations whose live representation itself
/// implements [`ScopedResource`] normally return that `Arc` unchanged. An
/// implementation may instead derive an owned authorization snapshot, but the
/// resolver is invoked anew on every resource-method call and Lockgate never
/// freezes the returned membership facts in the handle.
pub trait ResolveScopedResourceHandle<S, H: 'static, C: CallContext>: Send + Sync
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    /// The live application value stored for this WIT resource.
    type Representation: Any + Send + Sync;

    /// The live value or per-call authorization snapshot to classify.
    type Resource: ScopedResource<S> + Send;

    /// The application's normal resource-resolution error.
    ///
    /// `From<ResourceLookupError>` ensures stale and invalid handles use the
    /// same error path as other resolution failures.
    type Error: From<ResourceLookupError> + Send;

    /// Resolves the live representation for this call's authorization check.
    fn resolve_scoped_resource_handle<'a>(
        &'a self,
        context: &'a ResolveCtx<'_, C>,
        representation: Arc<Self::Representation>,
    ) -> impl Future<Output = Result<Self::Resource, Self::Error>> + Send + 'a;
}

/// Calls an application resolver with the scope type selected by a permission.
#[doc(hidden)]
pub async fn resolve_scoped_resource<'a, S, A, R>(
    resolver: &'a R,
    subject: &'a PluginSubject<'_>,
    argument: &'a A,
    _permission: ScopedPermission<S>,
) -> Result<R::Resource, R::Error>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
    A: ?Sized + Sync,
    R: ResolveScopedResource<S, A> + ?Sized,
{
    resolver.resolve_scoped_resource(subject, argument).await
}

/// Looks up a WIT handle and calls its per-call authorization resolver.
#[doc(hidden)]
pub async fn resolve_scoped_resource_handle<'a, S, H, C, R>(
    resolver: &'a R,
    context: &'a ResolveCtx<'_, C>,
    handle: &Resource<H>,
    _permission: ScopedPermission<S>,
) -> Result<R::Resource, R::Error>
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
    H: 'static,
    C: CallContext,
    R: ResolveScopedResourceHandle<S, H, C> + ?Sized,
{
    let representation = context
        .resource::<H, R::Representation>(handle)
        .map_err(R::Error::from)?;
    resolver
        .resolve_scoped_resource_handle(context, representation)
        .await
}

/// Applies the single scoped-authorization relation to concrete memberships.
///
/// Access is allowed exactly when some effective grant contains some resource
/// membership witness. Canonical strings frozen at admission are parsed again
/// through the registered permission descriptor, then its concrete `S`
/// implementation receives the containment call. Missing grants, malformed
/// invariant data, a mismatched registered scope type, and empty memberships
/// all fail closed.
pub(crate) fn scoped_access_allowed<S>(
    registry: &CapabilityRegistry,
    grants: &EffectiveGrants,
    permission: ScopedPermission<S>,
    memberships: &[S],
) -> bool
where
    S: Scope,
    <S as FromStr>::Err: Into<ScopeError>,
{
    if memberships.is_empty() {
        return false;
    }

    let (capability, operation) = lockgate_policy::__private::scoped_permission_ids(permission);
    let atom = AtomKey::new(capability, operation)
        .expect("typed permissions always contain a valid wire atom");
    let Some(values) = grants.scoped_values(&atom) else {
        return false;
    };
    let Some(descriptor) = registry.permission(&atom) else {
        return false;
    };
    if descriptor.scope_type_id() != Some(TypeId::of::<S>()) {
        return false;
    }

    values.iter().any(|value| {
        let Some(Ok(grant)) = descriptor.parse_scope(value) else {
            return false;
        };
        memberships.iter().any(|membership| {
            descriptor
                .contains_scope(&grant, membership)
                .and_then(Result::ok)
                .unwrap_or(false)
        })
    })
}

#[cfg(test)]
mod tests {
    use std::{
        collections::BTreeMap,
        sync::{
            Arc, Mutex,
            atomic::{AtomicU64, AtomicUsize, Ordering},
        },
    };

    use lockgate_policy::ScopeRepr;
    use lockgate_schema::GrantSet;

    use super::*;

    mod vm_contract {
        include!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../lockgate-policy/tests/fixtures/vm_contract.rs"
        ));
    }

    use crate::policy::ResolvedNeeds;
    use vm_contract::permissions::vm::{self, InstanceScope};

    #[derive(Clone, Debug, PartialEq, Eq, lockgate_policy::ScopeRepr)]
    enum ConflictingScope {
        Other,
    }

    impl Scope for ConflictingScope {}

    #[lockgate_policy::capability("vm")]
    mod conflicting_vm {
        use super::ConflictingScope;
        use lockgate_policy::ScopedPermission;

        pub const EXEC: ScopedPermission<ConflictingScope> = ScopedPermission::new("exec");
    }

    #[derive(Clone, Debug, PartialEq, Eq)]
    struct MockVm {
        pool: String,
        created_by: Option<String>,
    }

    impl ScopedResource<InstanceScope> for MockVm {
        fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<InstanceScope> {
            let mut scopes = vec![InstanceScope::Pool(self.pool.clone())];
            if self.created_by.as_deref() == Some(subject.plugin_id()) {
                scopes.push(InstanceScope::CreatedByCaller);
            }
            scopes
        }
    }

    #[derive(Debug, PartialEq, Eq)]
    enum VmError {
        NotFound(String),
    }

    struct MockHost {
        vms: Mutex<BTreeMap<String, MockVm>>,
    }

    impl ResolveScopedResource<InstanceScope, String> for MockHost {
        type Resource = MockVm;
        type Error = VmError;

        async fn resolve_scoped_resource<'a>(
            &'a self,
            _subject: &'a PluginSubject<'_>,
            id: &'a String,
        ) -> Result<Self::Resource, Self::Error> {
            self.vms
                .lock()
                .unwrap()
                .get(id)
                .cloned()
                .ok_or_else(|| VmError::NotFound(id.clone()))
        }
    }

    fn registry() -> CapabilityRegistry {
        let mut registry = CapabilityRegistry::default();
        registry.register::<vm::Contract>().unwrap();
        registry
    }

    fn effective_grants(scopes: &[InstanceScope]) -> EffectiveGrants {
        let mut required = GrantSet::new();
        if !scopes.is_empty() {
            required
                .insert_scopes(
                    AtomKey::new("vm", "exec").unwrap(),
                    scopes.iter().map(ScopeRepr::canonical),
                )
                .unwrap();
        }
        EffectiveGrants::from_resolved(ResolvedNeeds {
            required,
            optional: GrantSet::new(),
        })
    }

    fn raw_effective_grants(scopes: &[&str]) -> EffectiveGrants {
        let mut required = GrantSet::new();
        required
            .insert_scopes(
                AtomKey::new("vm", "exec").unwrap(),
                scopes.iter().map(|scope| (*scope).to_owned()),
            )
            .unwrap();
        EffectiveGrants::from_resolved(ResolvedNeeds {
            required,
            optional: GrantSet::new(),
        })
    }

    fn subject(plugin_id: &str) -> PluginHandle {
        PluginHandle::for_policy_test(plugin_id, effective_grants(&[]))
    }

    #[test]
    fn golden_vm_memberships_and_grant_matrix_use_the_real_policy_relation() {
        struct GoldenRow {
            vm: MockVm,
            caller: &'static str,
            memberships: Vec<InstanceScope>,
            allowed: [bool; 4],
        }

        let rows = [
            GoldenRow {
                vm: MockVm {
                    pool: "gpu".to_owned(),
                    created_by: Some("A".to_owned()),
                },
                caller: "A",
                memberships: vec![
                    InstanceScope::Pool("gpu".to_owned()),
                    InstanceScope::CreatedByCaller,
                ],
                allowed: [true, true, false, true],
            },
            GoldenRow {
                vm: MockVm {
                    pool: "gpu".to_owned(),
                    created_by: Some("A".to_owned()),
                },
                caller: "B",
                memberships: vec![InstanceScope::Pool("gpu".to_owned())],
                allowed: [true, true, false, false],
            },
            GoldenRow {
                vm: MockVm {
                    pool: "cpu".to_owned(),
                    created_by: Some("B".to_owned()),
                },
                caller: "A",
                memberships: vec![InstanceScope::Pool("cpu".to_owned())],
                allowed: [true, false, true, false],
            },
        ];
        let grant_cases = [
            InstanceScope::Any,
            InstanceScope::Pool("gpu".to_owned()),
            InstanceScope::Pool("cpu".to_owned()),
            InstanceScope::CreatedByCaller,
        ];
        let registry = registry();

        for row in rows {
            let handle = subject(row.caller);
            let plugin_subject = PluginSubject::new(&handle);
            let memberships = row.vm.scopes_for(&plugin_subject);
            assert_eq!(memberships, row.memberships);

            for (grant, expected) in grant_cases.iter().zip(row.allowed) {
                let grants = effective_grants(std::slice::from_ref(grant));
                assert_eq!(
                    scoped_access_allowed(&registry, &grants, vm::EXEC, &memberships),
                    expected,
                    "caller {} with grant {} and memberships {:?}",
                    row.caller,
                    grant.canonical(),
                    memberships
                );
            }

            assert!(!scoped_access_allowed(
                &registry,
                &effective_grants(&[]),
                vm::EXEC,
                &memberships,
            ));
        }

        assert!(!scoped_access_allowed(
            &registry,
            &effective_grants(&[InstanceScope::Any]),
            vm::EXEC,
            &[],
        ));
    }

    #[test]
    fn scoped_decision_fails_closed_on_broken_internal_invariants() {
        let memberships = [InstanceScope::Pool("gpu".to_owned())];
        assert!(!scoped_access_allowed(
            &registry(),
            &raw_effective_grants(&["not-an-instance-scope"]),
            vm::EXEC,
            &memberships,
        ));

        let mut conflicting_registry = CapabilityRegistry::default();
        conflicting_registry
            .register::<conflicting_vm::Contract>()
            .unwrap();
        assert!(!scoped_access_allowed(
            &conflicting_registry,
            &effective_grants(&[InstanceScope::Any]),
            vm::EXEC,
            &memberships,
        ));
    }

    #[test]
    fn subject_debug_exposes_only_the_public_plugin_id() {
        let handle =
            PluginHandle::for_policy_test("A", effective_grants(&[InstanceScope::CreatedByCaller]));
        let subject = PluginSubject::new(&handle);

        assert_eq!(format!("{subject:?}"), "PluginSubject { plugin_id: \"A\" }");
    }

    #[tokio::test]
    async fn string_id_resolves_asynchronously_to_a_local_vm() {
        let vm = MockVm {
            pool: "gpu".to_owned(),
            created_by: Some("A".to_owned()),
        };
        let host = MockHost {
            vms: Mutex::new(BTreeMap::from([("vm-1".to_owned(), vm.clone())])),
        };
        let handle = subject("A");
        let plugin_subject = PluginSubject::new(&handle);

        assert_eq!(
            host.resolve_scoped_resource(&plugin_subject, &"vm-1".to_owned())
                .await,
            Ok(vm)
        );
        assert_eq!(
            host.resolve_scoped_resource(&plugin_subject, &"missing".to_owned())
                .await,
            Err(VmError::NotFound("missing".to_owned()))
        );
    }

    enum StableHandle {}
    enum LiveHandle {}
    enum SnapshotHandle {}

    struct CacheShapeHost {
        stable_state: Mutex<BTreeMap<u64, MockVm>>,
        stable_loads: AtomicUsize,
        live_resolutions: AtomicUsize,
    }

    impl ResolveScopedResourceHandle<InstanceScope, StableHandle, ()> for CacheShapeHost {
        type Representation = u64;
        type Resource = MockVm;
        type Error = ResourceLookupError;

        async fn resolve_scoped_resource_handle<'a>(
            &'a self,
            _context: &'a ResolveCtx<'_, ()>,
            identity: Arc<Self::Representation>,
        ) -> Result<Self::Resource, Self::Error> {
            self.stable_loads.fetch_add(1, Ordering::SeqCst);
            self.stable_state
                .lock()
                .map_err(|_| ResourceLookupError::TablePoisoned)?
                .get(identity.as_ref())
                .cloned()
                .ok_or(ResourceLookupError::NotPresent)
        }
    }

    impl ResolveScopedResourceHandle<InstanceScope, LiveHandle, ()> for CacheShapeHost {
        type Representation = MockVm;
        type Resource = Arc<MockVm>;
        type Error = ResourceLookupError;

        async fn resolve_scoped_resource_handle<'a>(
            &'a self,
            _context: &'a ResolveCtx<'_, ()>,
            representation: Arc<Self::Representation>,
        ) -> Result<Self::Resource, Self::Error> {
            self.live_resolutions.fetch_add(1, Ordering::SeqCst);
            Ok(representation)
        }
    }

    struct VersionedSnapshot {
        facts: MockVm,
        epoch: u64,
    }

    #[derive(Debug, PartialEq, Eq)]
    enum SnapshotError {
        Lookup(ResourceLookupError),
        Stale,
    }

    impl From<ResourceLookupError> for SnapshotError {
        fn from(error: ResourceLookupError) -> Self {
            Self::Lookup(error)
        }
    }

    struct SnapshotHost {
        live_epoch: AtomicU64,
    }

    impl ResolveScopedResourceHandle<InstanceScope, SnapshotHandle, ()> for SnapshotHost {
        type Representation = VersionedSnapshot;
        type Resource = MockVm;
        type Error = SnapshotError;

        async fn resolve_scoped_resource_handle<'a>(
            &'a self,
            _context: &'a ResolveCtx<'_, ()>,
            snapshot: Arc<Self::Representation>,
        ) -> Result<Self::Resource, Self::Error> {
            if snapshot.epoch != self.live_epoch.load(Ordering::SeqCst) {
                return Err(SnapshotError::Stale);
            }
            Ok(snapshot.facts.clone())
        }
    }

    #[tokio::test]
    async fn cache_shapes_contrast_fresh_loads_live_reuse_and_versioned_facts() {
        const CALLS: usize = 256;

        let plugin = subject("cache-probe");
        let resources = ResourceStore::__new();
        let data = ();
        let context = ResolveCtx::new(&data, &plugin, &resources);
        let stable = context.insert_resource::<StableHandle, _>(7_u64).unwrap();
        let live = context
            .insert_resource::<LiveHandle, _>(MockVm {
                pool: "gpu".to_owned(),
                created_by: None,
            })
            .unwrap();
        let host = CacheShapeHost {
            stable_state: Mutex::new(BTreeMap::from([(
                7,
                MockVm {
                    pool: "gpu".to_owned(),
                    created_by: None,
                },
            )])),
            stable_loads: AtomicUsize::new(0),
            live_resolutions: AtomicUsize::new(0),
        };

        let mut first_live = None;
        for _ in 0..CALLS {
            let freshly_loaded = resolve_scoped_resource_handle(&host, &context, &stable, vm::EXEC)
                .await
                .unwrap();
            assert_eq!(freshly_loaded.pool, "gpu");

            let reused = resolve_scoped_resource_handle(&host, &context, &live, vm::EXEC)
                .await
                .unwrap();
            if let Some(first) = &first_live {
                assert!(Arc::ptr_eq(first, &reused));
            } else {
                first_live = Some(reused);
            }
        }
        assert_eq!(host.stable_loads.load(Ordering::SeqCst), CALLS);
        assert_eq!(host.live_resolutions.load(Ordering::SeqCst), CALLS);

        let snapshot = context
            .insert_resource::<SnapshotHandle, _>(VersionedSnapshot {
                facts: MockVm {
                    pool: "gpu".to_owned(),
                    created_by: None,
                },
                epoch: 3,
            })
            .unwrap();
        let snapshot_host = SnapshotHost {
            live_epoch: AtomicU64::new(3),
        };
        assert!(
            resolve_scoped_resource_handle(&snapshot_host, &context, &snapshot, vm::EXEC,)
                .await
                .is_ok()
        );
        snapshot_host.live_epoch.store(4, Ordering::SeqCst);
        assert_eq!(
            resolve_scoped_resource_handle(&snapshot_host, &context, &snapshot, vm::EXEC,).await,
            Err(SnapshotError::Stale)
        );
    }
}
