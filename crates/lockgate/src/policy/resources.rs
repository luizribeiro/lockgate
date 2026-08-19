//! Application-owned resource classification and target resolution.

use std::{any::TypeId, fmt, future::Future, str::FromStr};

use lockgate_policy::{Scope, ScopeError, ScopedPermission};
use lockgate_schema::AtomKey;

use super::{CapabilityRegistry, EffectiveGrants};
use crate::PluginHandle;

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
/// lookup through [`ResolveScopedResource`] and return an owned authorization
/// snapshot when classification needs facts loaded from external state.
/// Duplicate witnesses are permitted and do not change authorization's
/// existential containment relation.
pub trait ScopedResource<S: Scope>
where
    <S as FromStr>::Err: Into<ScopeError>,
{
    /// Returns all scope membership witnesses for `self` and `subject`.
    fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<S>;
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
    use std::{collections::BTreeMap, sync::Mutex};

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
}
