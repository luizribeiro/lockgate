extern crate alloc;

use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use lockgate::{
    BudgetClass, HostCtx, InvocationCtx, PermissionDenied, PluginConfig, PluginSubject,
    ResolveScopedResource, RuntimeLimits, Scope, ScopeRepr, ScopedResource,
};
use lockgate_schema::{NeedEntry, NeedsManifest, PluginMetadata, ScopeRefEntry};

mod common;

const PLUGIN_ID: &str = "session-context";

#[derive(Clone, Copy, Debug, PartialEq, Eq, lockgate::ScopeRepr)]
enum SessionScope {
    All,
    Current,
    Created,
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

    pub const READ: ScopedPermission<SessionScope> = ScopedPermission::new("read");
}

#[derive(Debug, PartialEq, Eq, PartialOrd, Ord)]
struct SessionId(Box<str>);

impl SessionId {
    fn new(id: &str) -> Self {
        Self(id.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

struct SageCall {
    // Intentionally non-Clone: generated authorization must borrow this target.
    session: Option<SessionId>,
}

#[derive(Clone)]
struct PluginId(Box<str>);

impl PluginId {
    fn new(id: &str) -> Self {
        Self(id.into())
    }

    fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Clone)]
struct StoredSession {
    created_by: PluginId,
    is_current: bool,
}

struct SessionAuthorizationSnapshot {
    created_by: PluginId,
    is_current: bool,
}

impl ScopedResource<SessionScope> for SessionAuthorizationSnapshot {
    fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<SessionScope> {
        let mut scopes = Vec::new();
        if self.is_current {
            scopes.push(SessionScope::Current);
        }
        if self.created_by.as_str() == subject.plugin_id() {
            scopes.push(SessionScope::Created);
        }
        if scopes.is_empty() {
            scopes.push(SessionScope::All);
        }
        scopes
    }
}

#[derive(Debug)]
enum SessionError {
    Gone,
}

#[derive(Default)]
struct Calls {
    resolutions: AtomicUsize,
    reads: AtomicUsize,
}

struct SessionState {
    sessions: BTreeMap<Box<str>, StoredSession>,
}

impl SessionState {
    fn authorization_snapshot(
        &self,
        session: &str,
    ) -> Result<SessionAuthorizationSnapshot, SessionError> {
        let session = self.sessions.get(session).ok_or(SessionError::Gone)?;
        Ok(SessionAuthorizationSnapshot {
            created_by: session.created_by.clone(),
            is_current: session.is_current,
        })
    }
}

#[derive(Clone)]
struct Imports {
    calls: Arc<Calls>,
    state: Arc<SessionState>,
}

impl Imports {
    fn from_golden_rows() -> Self {
        let sessions = GOLDEN_ROWS
            .iter()
            .map(|row| {
                let created_by = if row.created_by_caller {
                    PLUGIN_ID
                } else {
                    "another-plugin"
                };
                (
                    row.id.into(),
                    StoredSession {
                        created_by: PluginId::new(created_by),
                        is_current: row.current,
                    },
                )
            })
            .collect();
        Self {
            calls: Arc::new(Calls::default()),
            state: Arc::new(SessionState { sessions }),
        }
    }
}

impl ResolveScopedResource<SessionScope, Option<SessionId>> for Imports {
    type Resource = SessionAuthorizationSnapshot;
    type Error = SessionError;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        session: &'a Option<SessionId>,
    ) -> Result<Self::Resource, Self::Error> {
        self.calls.resolutions.fetch_add(1, Ordering::SeqCst);

        // Force a suspension before reading the borrowed call-context field.
        tokio::task::yield_now().await;
        let id = session.as_ref().ok_or(SessionError::Gone)?;
        self.state.authorization_snapshot(id.as_str())
    }
}

lockgate::host_bindings!({
    path: "tests/data/session_context",
    world: "fixture",
    imports: Imports,
    data: SageCall,
});

use guest::HostExt;

impl From<SessionError> for sessions::SessionError {
    fn from(_: SessionError) -> Self {
        Self::Gone
    }
}

impl From<PermissionDenied> for sessions::SessionError {
    fn from(_: PermissionDenied) -> Self {
        Self::Denied
    }
}

#[lockgate::guarded]
impl sessions::Host for Imports {
    #[lockgate::requires(permission = permissions::READ, target = cx.data().session)]
    async fn read(&mut self, cx: HostCtx<'_, SageCall>) -> Result<String, sessions::SessionError> {
        self.calls.reads.fetch_add(1, Ordering::SeqCst);
        let id = cx
            .data()
            .session
            .as_ref()
            .expect("resolved session remains in call context")
            .as_str();

        // Return reviewer-visible golden-table evidence after authorization.
        // This second snapshot cannot influence the generated guard decision.
        let snapshot = self
            .state
            .authorization_snapshot(id)
            .expect("the generated guard already resolved this session");
        Ok(snapshot
            .scopes_for(&cx.subject())
            .iter()
            .map(SessionScope::canonical)
            .collect::<Vec<_>>()
            .join(","))
    }
}

struct GoldenRow {
    id: &'static str,
    current: bool,
    created_by_caller: bool,
    memberships: &'static [SessionScope],
    allowed: [bool; 3],
}

const GOLDEN_ROWS: &[GoldenRow] = &[
    GoldenRow {
        id: "current-created",
        current: true,
        created_by_caller: true,
        memberships: &[SessionScope::Current, SessionScope::Created],
        allowed: [true, true, true],
    },
    GoldenRow {
        id: "current-other",
        current: true,
        created_by_caller: false,
        memberships: &[SessionScope::Current],
        allowed: [true, true, false],
    },
    GoldenRow {
        id: "past-created",
        current: false,
        created_by_caller: true,
        memberships: &[SessionScope::Created],
        allowed: [true, false, true],
    },
    GoldenRow {
        id: "past-other",
        current: false,
        created_by_caller: false,
        memberships: &[SessionScope::All],
        allowed: [true, false, false],
    },
];

const GRANTS: [(&str, usize); 3] = [("all", 0), ("current", 1), ("created", 2)];

fn needs(grant: &str) -> NeedsManifest {
    NeedsManifest::new(
        vec![
            NeedEntry::scoped(
                "session.read".parse().unwrap(),
                vec![ScopeRefEntry::literal(grant).unwrap()],
            )
            .unwrap(),
        ],
        vec![],
    )
    .unwrap()
}

fn call(session: Option<&str>) -> InvocationCtx<SageCall> {
    InvocationCtx::new(
        SageCall {
            session: session.map(SessionId::new),
        },
        BudgetClass::Bounded {
            fuel: 5_000_000,
            deadline: common::INVOCATION_DEADLINE,
        },
    )
}

async fn runtime_host(
    imports: Imports,
    grant: &str,
) -> (lockgate::Host<SageCall>, lockgate::PluginHandle) {
    let metadata = PluginMetadata::new(PLUGIN_ID, "Session context fixture", "1.0").unwrap();
    let bytes = common::policy_fixture(&common::SESSION_CONTEXT_FIXTURE, &metadata, &needs(grant));
    let mut builder = lockgate::HostBuilder::new(imports)
        .unwrap()
        .register::<permissions::Contract>()
        .unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder
        .admit(prepared, acceptance, RuntimeLimits::default(), call(None))
        .await
        .unwrap();
    (builder.finish(), plugin)
}

#[tokio::test]
async fn session_golden_table_and_grant_matrix_use_the_generated_guard_path() {
    for (grant, decision_column) in GRANTS {
        let imports = Imports::from_golden_rows();
        let (host, plugin) = runtime_host(imports.clone(), grant).await;
        let guest = host.guest(&plugin).unwrap();

        for row in GOLDEN_ROWS {
            let actual = guest.read(call(Some(row.id))).await.unwrap();
            let expected = if row.allowed[decision_column] {
                format!(
                    "ok:{}",
                    row.memberships
                        .iter()
                        .map(SessionScope::canonical)
                        .collect::<Vec<_>>()
                        .join(",")
                )
            } else {
                "denied".to_owned()
            };
            assert_eq!(actual, expected, "grant {grant}, session {}", row.id);
        }

        for row in GOLDEN_ROWS {
            assert!(
                !row.memberships.is_empty(),
                "ordinary resolved session {} must have a witness",
                row.id
            );
        }
        assert_eq!(
            imports.calls.resolutions.load(Ordering::SeqCst),
            GOLDEN_ROWS.len()
        );
        assert_eq!(
            imports.calls.reads.load(Ordering::SeqCst),
            GOLDEN_ROWS
                .iter()
                .filter(|row| row.allowed[decision_column])
                .count()
        );
        host.shutdown().await;
    }
}

#[tokio::test]
async fn absent_call_context_session_uses_the_async_resolution_error_path() {
    let imports = Imports::from_golden_rows();
    let (host, plugin) = runtime_host(imports.clone(), "all").await;

    assert_eq!(
        host.guest(&plugin).unwrap().read(call(None)).await.unwrap(),
        "gone"
    );
    assert_eq!(imports.calls.resolutions.load(Ordering::SeqCst), 1);
    assert_eq!(imports.calls.reads.load(Ordering::SeqCst), 0);
    host.shutdown().await;
}
