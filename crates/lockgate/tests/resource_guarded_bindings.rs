extern crate alloc;

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use lockgate::{
    HostCtx, InvocationCtx, PermissionDenied, PluginConfig, PluginSubject, ResolveCtx,
    ResolveScopedResourceHandle, Resource, ResourceLookupError, RuntimeLimits, Scope,
    ScopedResource,
};
use lockgate_schema::{NeedEntry, NeedsManifest, PluginMetadata, ScopeRefEntry};

mod common;

#[derive(Clone, Debug, PartialEq, Eq, lockgate::ScopeRepr)]
enum SessionScope {
    All,
    Current,
}

impl Scope for SessionScope {
    fn contains(&self, inner: &Self) -> bool {
        self == inner || matches!(self, Self::All)
    }
}

#[lockgate::capability("session")]
mod permissions {
    use super::SessionScope;
    use lockgate::ScopedPermission;

    pub const SEND: ScopedPermission<SessionScope> = ScopedPermission::new("send");
}

struct LiveSession {
    identity: String,
    current: AtomicBool,
}

impl ScopedResource<SessionScope> for LiveSession {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<SessionScope> {
        if self.current.load(Ordering::SeqCst) {
            vec![SessionScope::Current]
        } else {
            vec![SessionScope::All]
        }
    }
}

#[derive(Debug)]
enum ResolveError {
    Lookup(ResourceLookupError),
}

impl From<ResourceLookupError> for ResolveError {
    fn from(error: ResourceLookupError) -> Self {
        Self::Lookup(error)
    }
}

#[derive(Default)]
struct Calls {
    next_session: AtomicUsize,
    resolutions: AtomicUsize,
    sends: AtomicUsize,
    checked_sends: AtomicUsize,
}

#[derive(Clone, Default)]
struct Imports {
    calls: Arc<Calls>,
}

lockgate::host_bindings!({
    path: "tests/data/resource_guarded",
    world: "fixture",
    imports: Imports,
    data: (),
});

use guest::HostExt;

impl sessions::Host for Imports {}

impl From<ResolveError> for sessions::SessionError {
    fn from(error: ResolveError) -> Self {
        match error {
            ResolveError::Lookup(error) => {
                let _ = error;
                Self::NotFound
            }
        }
    }
}

impl From<ResourceLookupError> for sessions::SessionError {
    fn from(_: ResourceLookupError) -> Self {
        Self::NotFound
    }
}

impl From<PermissionDenied> for sessions::SessionError {
    fn from(_: PermissionDenied) -> Self {
        Self::Denied
    }
}

impl ResolveScopedResourceHandle<SessionScope, sessions::Session, ()> for Imports {
    type Representation = LiveSession;
    type Resource = Arc<LiveSession>;
    type Error = ResolveError;

    async fn resolve_scoped_resource_handle<'a>(
        &'a self,
        _context: &'a ResolveCtx<'_, ()>,
        representation: Arc<Self::Representation>,
    ) -> Result<Self::Resource, Self::Error> {
        self.calls.resolutions.fetch_add(1, Ordering::SeqCst);
        Ok(representation)
    }
}

#[lockgate::guarded]
impl sessions::HostSession for Imports {
    #[lockgate::no_capability_required(reason = "resource acquisition carries no authority")]
    async fn new(&mut self, cx: HostCtx<'_, ()>, current: bool) -> Resource<sessions::Session> {
        let identity = self.calls.next_session.fetch_add(1, Ordering::SeqCst);
        cx.resolve_context()
            .insert_resource(LiveSession {
                identity: format!("live-session-{identity}"),
                current: AtomicBool::new(current),
            })
            .expect("fixture resource table has capacity")
    }

    #[lockgate::requires(permission = permissions::SEND, target = session)]
    async fn send(
        &mut self,
        _cx: HostCtx<'_, ()>,
        session: Resource<sessions::Session>,
        message: String,
    ) -> Result<String, sessions::SessionError> {
        self.calls.sends.fetch_add(1, Ordering::SeqCst);
        Ok(message)
    }

    #[lockgate::requires(
        permission = permissions::SEND,
        target = session,
        wire_type = Resource<sessions::Session>
    )]
    async fn checked_send(
        &mut self,
        _cx: HostCtx<'_, ()>,
        session: Arc<LiveSession>,
        message: String,
    ) -> Result<String, sessions::SessionError> {
        self.calls.checked_sends.fetch_add(1, Ordering::SeqCst);
        Ok(format!("{}:{message}", session.identity))
    }

    #[lockgate::no_capability_required(reason = "test-only live membership mutation")]
    async fn set_current(
        &mut self,
        cx: HostCtx<'_, ()>,
        session: Resource<sessions::Session>,
        current: bool,
    ) -> Result<(), sessions::SessionError> {
        let session = cx.resolve_context().resource::<_, LiveSession>(&session)?;
        session.current.store(current, Ordering::SeqCst);
        Ok(())
    }

    #[lockgate::no_capability_required(reason = "test-only stale-handle fixture")]
    async fn invalidate(
        &mut self,
        cx: HostCtx<'_, ()>,
        session: Resource<sessions::Session>,
    ) -> Result<(), sessions::SessionError> {
        cx.resolve_context().delete_resource(&session)?;
        Ok(())
    }
}

#[test]
fn generated_resource_policy_uses_resource_companion_identity() {
    let methods = <Imports as sessions::HostSession>::__LOCKGATE_POLICY_METHODS;
    assert_eq!(methods.len(), 5);
    assert_eq!(sessions::__LockgateBinding::METHODS.len(), 5);
    assert_eq!(methods[1].method().rust_name(), "send");
    assert_eq!(methods[1].method().wit_name(), "[method]session.send");
    assert_eq!(methods[1].classification().target(), Some("session"));
    assert_eq!(methods[2].method().rust_name(), "checked_send");
    assert_eq!(
        methods[2].method().wit_name(),
        "[method]session.checked-send"
    );
    assert_eq!(methods[2].classification().target(), Some("session"));

    lockgate::HostBuilder::new(Imports::default())
        .unwrap()
        .register::<permissions::Contract>()
        .unwrap();
}

fn needs_current() -> NeedsManifest {
    NeedsManifest::new(
        vec![
            NeedEntry::scoped(
                "session.send".parse().unwrap(),
                vec![ScopeRefEntry::literal("current").unwrap()],
            )
            .unwrap(),
        ],
        vec![],
    )
    .unwrap()
}

async fn runtime_host(
    imports: Imports,
    needs: &NeedsManifest,
) -> (lockgate::Host<()>, lockgate::PluginHandle) {
    let metadata =
        PluginMetadata::new("resource-guarded", "Resource guard fixture", "1.0").unwrap();
    let bytes = common::policy_fixture(&common::RESOURCE_GUARDED_FIXTURE, &metadata, needs);
    let mut builder = lockgate::HostBuilder::new(imports)
        .unwrap()
        .register::<permissions::Contract>()
        .unwrap();
    let prepared = builder
        .prepare("resource-guarded", &bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(
            prepared,
            acceptance,
            RuntimeLimits::default(),
            InvocationCtx::bounded(1_000_000, common::INVOCATION_DEADLINE),
        )
        .await
        .unwrap();
    (builder.finish(), plugin)
}

fn call() -> InvocationCtx<()> {
    InvocationCtx::bounded(5_000_000, common::INVOCATION_DEADLINE)
}

#[tokio::test]
async fn session_send_authorizes_the_live_host_representation() {
    let imports = Imports::default();
    let (host, plugin) = runtime_host(imports.clone(), &needs_current()).await;

    assert_eq!(
        host.guest(&plugin)
            .unwrap()
            .live_send(call())
            .await
            .unwrap(),
        "ok:hello"
    );
    assert_eq!(imports.calls.resolutions.load(Ordering::SeqCst), 1);
    assert_eq!(imports.calls.sends.load(Ordering::SeqCst), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn one_handle_reevaluates_membership_on_the_second_guarded_call() {
    let imports = Imports::default();
    let (host, plugin) = runtime_host(imports.clone(), &needs_current()).await;

    assert_eq!(
        host.guest(&plugin)
            .unwrap()
            .repeated_call(call())
            .await
            .unwrap(),
        "ok:first,denied"
    );
    assert_eq!(imports.calls.resolutions.load(Ordering::SeqCst), 2);
    assert_eq!(imports.calls.sends.load(Ordering::SeqCst), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn invalidated_handle_uses_the_method_error_path_and_drops_without_trapping() {
    let imports = Imports::default();
    let (host, plugin) = runtime_host(imports.clone(), &needs_current()).await;

    assert_eq!(
        host.guest(&plugin)
            .unwrap()
            .stale_handle(call())
            .await
            .unwrap(),
        "not-found"
    );
    assert_eq!(imports.calls.resolutions.load(Ordering::SeqCst), 0);
    assert_eq!(imports.calls.sends.load(Ordering::SeqCst), 0);
    host.shutdown().await;
}

#[tokio::test]
async fn acquiring_a_handle_grants_no_authority_to_later_methods() {
    let imports = Imports::default();
    let (host, plugin) = runtime_host(imports.clone(), &NeedsManifest::empty()).await;

    assert_eq!(
        host.guest(&plugin)
            .unwrap()
            .acquire_without_authority(call())
            .await
            .unwrap(),
        "denied"
    );
    assert_eq!(imports.calls.resolutions.load(Ordering::SeqCst), 1);
    assert_eq!(imports.calls.sends.load(Ordering::SeqCst), 0);
    host.shutdown().await;
}

#[tokio::test]
async fn resource_wire_guard_executes_the_checked_live_resource() {
    let imports = Imports::default();
    let (host, plugin) = runtime_host(imports.clone(), &needs_current()).await;

    assert_eq!(
        host.guest(&plugin)
            .unwrap()
            .checked_resource(call())
            .await
            .unwrap(),
        "ok:live-session-0:hello,denied"
    );
    assert_eq!(imports.calls.resolutions.load(Ordering::SeqCst), 2);
    assert_eq!(imports.calls.checked_sends.load(Ordering::SeqCst), 1);
    assert_eq!(imports.calls.sends.load(Ordering::SeqCst), 0);
    host.shutdown().await;
}
