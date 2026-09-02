extern crate alloc;

use std::collections::{BTreeMap, BTreeSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use lockgate::{
    AdmissionError, CallError, HostConstructionError, HostCtx, HostImportPolicyError,
    InstanceAllocation, PermissionDenied, PluginConfig, PluginId, PluginSubject,
    PoolingAllocationConfig, ResolveScopedResource, RuntimeLimits, ScopedResource,
};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata, ScopeRefEntry};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

mod common;

const PLUGIN_ID: &str = "guarded-wiring";

mod vm_contract {
    include!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../lockgate-policy/tests/fixtures/vm_contract.rs"
    ));
}

use vm_contract::permissions::vm::{self as permissions, InstanceScope, PoolScope};

#[lockgate::capability("admin")]
mod admin_permissions {
    use lockgate::Permission;

    pub const RESTART: Permission = Permission::new("restart");
    pub const STATUS: Permission = Permission::new("status");
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct MockVm {
    id: String,
    pool: String,
    created_by: Option<PluginId>,
}

impl ScopedResource<InstanceScope> for MockVm {
    fn scopes_for(&self, subject: &PluginSubject<'_>) -> Vec<InstanceScope> {
        let mut scopes = vec![InstanceScope::Pool(self.pool.clone())];
        if self.created_by.as_ref() == Some(subject.plugin_id()) {
            scopes.push(InstanceScope::CreatedByCaller);
        }
        scopes
    }
}

#[derive(Clone)]
struct MockPool(String);

impl ScopedResource<PoolScope> for MockPool {
    fn scopes_for(&self, _subject: &PluginSubject<'_>) -> Vec<PoolScope> {
        vec![PoolScope::Named(self.0.clone())]
    }
}

#[derive(Debug)]
struct ResolveError;

#[derive(Default)]
struct BodyCalls {
    create: AtomicUsize,
    exec: AtomicUsize,
    destroy: AtomicUsize,
    list_pools: AtomicUsize,
    protocol_version: AtomicUsize,
    mixed_vm: AtomicUsize,
    mixed_admin: AtomicUsize,
    mixed_version: AtomicUsize,
}

struct VmState {
    pools: BTreeSet<String>,
    vms: Mutex<BTreeMap<String, MockVm>>,
    next_vm: AtomicUsize,
    pool_resolutions: AtomicUsize,
    vm_resolutions: AtomicUsize,
    body_calls: BodyCalls,
}

#[derive(Clone)]
struct Imports {
    state: Arc<VmState>,
}

impl Default for Imports {
    fn default() -> Self {
        let vms = [
            (
                "gpu-a",
                MockVm {
                    id: "gpu-a".to_owned(),
                    pool: "gpu".to_owned(),
                    created_by: Some(PluginId::from("plugin-a")),
                },
            ),
            (
                "gpu-b",
                MockVm {
                    id: "gpu-b".to_owned(),
                    pool: "gpu".to_owned(),
                    created_by: Some(PluginId::from("plugin-b")),
                },
            ),
            (
                "cpu-b",
                MockVm {
                    id: "cpu-b".to_owned(),
                    pool: "cpu".to_owned(),
                    created_by: Some(PluginId::from("plugin-b")),
                },
            ),
        ]
        .into_iter()
        .map(|(id, vm)| (id.to_owned(), vm))
        .collect();
        Self {
            state: Arc::new(VmState {
                pools: ["cpu".to_owned(), "gpu".to_owned()].into_iter().collect(),
                vms: Mutex::new(vms),
                next_vm: AtomicUsize::new(1),
                pool_resolutions: AtomicUsize::new(0),
                vm_resolutions: AtomicUsize::new(0),
                body_calls: BodyCalls::default(),
            }),
        }
    }
}

impl ResolveScopedResource<PoolScope, String> for Imports {
    type Resource = MockPool;
    type Error = ResolveError;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        pool: &'a String,
    ) -> Result<Self::Resource, Self::Error> {
        self.state.pool_resolutions.fetch_add(1, Ordering::SeqCst);
        self.state
            .pools
            .contains(pool)
            .then(|| MockPool(pool.clone()))
            .ok_or(ResolveError)
    }
}

impl ResolveScopedResource<InstanceScope, String> for Imports {
    type Resource = MockVm;
    type Error = ResolveError;

    async fn resolve_scoped_resource<'a>(
        &'a self,
        _subject: &'a PluginSubject<'_>,
        vm: &'a String,
    ) -> Result<Self::Resource, Self::Error> {
        self.state.vm_resolutions.fetch_add(1, Ordering::SeqCst);
        let normalized = vm.trim().to_ascii_lowercase();
        self.state
            .vms
            .lock()
            .unwrap()
            .get(&normalized)
            .cloned()
            .ok_or(ResolveError)
    }
}

lockgate::host_bindings!({
    path: "tests/data/guarded_bindings",
    world: "fixture",
    imports_type: Imports,
    data: (),
});

use guest::HostExt;

impl From<ResolveError> for vm::VmError {
    fn from(_: ResolveError) -> Self {
        Self::NotFound
    }
}

impl From<PermissionDenied> for vm::VmError {
    fn from(_: PermissionDenied) -> Self {
        Self::Denied
    }
}

impl From<PermissionDenied> for admin::AdminError {
    fn from(_: PermissionDenied) -> Self {
        Self::Denied
    }
}

impl From<PermissionDenied> for mixed::MixedError {
    fn from(_: PermissionDenied) -> Self {
        Self::Denied
    }
}

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = permissions::CREATE, target = pool)]
    async fn create(&mut self, cx: HostCtx<'_, ()>, pool: String) -> Result<String, vm::VmError> {
        self.state.body_calls.create.fetch_add(1, Ordering::SeqCst);
        let sequence = self.state.next_vm.fetch_add(1, Ordering::SeqCst);
        let id = format!("{pool}-{sequence}");
        self.state.vms.lock().unwrap().insert(
            id.clone(),
            MockVm {
                id: id.clone(),
                pool,
                created_by: Some(cx.subject().plugin_id().clone()),
            },
        );
        Ok(id)
    }

    #[lockgate::requires(
        permission = permissions::EXEC,
        target = vm,
        wire_type = String
    )]
    async fn exec(
        &mut self,
        _cx: HostCtx<'_, ()>,
        vm: MockVm,
        command: String,
    ) -> Result<String, vm::VmError> {
        self.state.body_calls.exec.fetch_add(1, Ordering::SeqCst);
        Ok(format!("{}:{command}", vm.id))
    }

    #[lockgate::requires(permission = permissions::DESTROY, target = vm)]
    async fn destroy(&mut self, _cx: HostCtx<'_, ()>, vm: String) -> Result<(), vm::VmError> {
        self.state.body_calls.destroy.fetch_add(1, Ordering::SeqCst);
        self.state.vms.lock().unwrap().remove(&vm);
        Ok(())
    }

    #[lockgate::requires(permission = permissions::LIST_POOLS)]
    async fn list_pools(&mut self, _cx: HostCtx<'_, ()>) -> Result<Vec<String>, vm::VmError> {
        self.state
            .body_calls
            .list_pools
            .fetch_add(1, Ordering::SeqCst);
        Ok(self.state.pools.iter().cloned().collect())
    }

    #[lockgate::no_capability_required(reason = "returns only a static protocol version")]
    async fn protocol_version(&mut self, _cx: HostCtx<'_, ()>) -> String {
        self.state
            .body_calls
            .protocol_version
            .fetch_add(1, Ordering::SeqCst);
        "1".to_owned()
    }
}

#[lockgate::guarded]
impl admin::Host for Imports {
    #[lockgate::requires(permission = admin_permissions::RESTART)]
    async fn restart(&mut self, _cx: HostCtx<'_, ()>) -> Result<(), admin::AdminError> {
        Ok(())
    }

    #[lockgate::requires(permission = admin_permissions::STATUS)]
    async fn status(&mut self, _cx: HostCtx<'_, ()>) -> Result<(), admin::AdminError> {
        Ok(())
    }
}

#[lockgate::guarded]
impl mixed::Host for Imports {
    #[lockgate::requires(permission = permissions::LIST_POOLS)]
    async fn vm_action(&mut self, _cx: HostCtx<'_, ()>) -> Result<String, mixed::MixedError> {
        self.state
            .body_calls
            .mixed_vm
            .fetch_add(1, Ordering::SeqCst);
        Ok("vm".to_owned())
    }

    #[lockgate::requires(permission = admin_permissions::STATUS)]
    async fn admin_action(&mut self, _cx: HostCtx<'_, ()>) -> Result<String, mixed::MixedError> {
        self.state
            .body_calls
            .mixed_admin
            .fetch_add(1, Ordering::SeqCst);
        Ok("admin".to_owned())
    }

    #[lockgate::no_capability_required(reason = "returns only a static mixed-fixture version")]
    async fn protocol_version(&mut self, _cx: HostCtx<'_, ()>) -> String {
        self.state
            .body_calls
            .mixed_version
            .fetch_add(1, Ordering::SeqCst);
        "mixed-v1".to_owned()
    }
}

#[test]
fn guarded_impl_fills_complete_typed_method_metadata() {
    let interface = vm::__LockgateBinding::INTERFACE;
    assert_eq!(interface.name(), "test:guarded/vm");
    assert_eq!(interface.version(), Some("1.2.3"));

    let methods = <Imports as vm::Host>::__LOCKGATE_POLICY_METHODS;
    assert_eq!(methods.len(), 5);
    for method in methods {
        assert_eq!(method.interface(), interface);
    }
    assert_eq!(methods[0].method().wit_name(), "create");
    assert_eq!(methods[1].method().wit_name(), "exec");
    assert_eq!(methods[2].method().wit_name(), "destroy");
    assert_eq!(methods[3].method().wit_name(), "list-pools");
    assert_eq!(methods[4].method().wit_name(), "protocol-version");

    let create = methods[0].classification();
    let permission = create.permission().unwrap();
    assert_eq!(
        (permission.capability(), permission.permission()),
        ("vm", "create")
    );
    assert_eq!(create.target(), Some("pool"));

    let exec = methods[1].classification();
    let permission = exec.permission().unwrap();
    assert_eq!(
        (permission.capability(), permission.permission()),
        ("vm", "exec")
    );
    assert_eq!(exec.target(), Some("vm"));

    let destroy = methods[2].classification();
    let permission = destroy.permission().unwrap();
    assert_eq!(
        (permission.capability(), permission.permission()),
        ("vm", "destroy")
    );
    assert_eq!(destroy.target(), Some("vm"));

    let list = methods[3].classification();
    let permission = list.permission().unwrap();
    assert_eq!(
        (permission.capability(), permission.permission()),
        ("vm", "list-pools")
    );
    assert_eq!(list.target(), None);

    assert_eq!(
        methods[4].classification().reason(),
        Some("returns only a static protocol version")
    );

    let admin_methods = <Imports as admin::Host>::__LOCKGATE_POLICY_METHODS;
    assert_eq!(admin_methods.len(), 2);
    assert_eq!(
        admin_methods
            .iter()
            .map(|method| {
                let permission = method.classification().permission().unwrap();
                (
                    method.method().wit_name(),
                    permission.capability(),
                    permission.permission(),
                )
            })
            .collect::<Vec<_>>(),
        [
            ("restart", "admin", "restart"),
            ("status", "admin", "status")
        ]
    );

    let mixed_methods = <Imports as mixed::Host>::__LOCKGATE_POLICY_METHODS;
    assert_eq!(mixed_methods.len(), 3);
    assert_eq!(
        mixed_methods
            .iter()
            .map(|method| (
                method.method().wit_name(),
                method
                    .classification()
                    .permission()
                    .map(|permission| (permission.capability(), permission.permission())),
            ))
            .collect::<Vec<_>>(),
        [
            ("vm-action", Some(("vm", "list-pools"))),
            ("admin-action", Some(("admin", "status"))),
            ("protocol-version", None),
        ]
    );
}

#[test]
fn generated_host_import_construction_rejects_a_mismatched_slot() {
    let error = match lockgate::HostBuilder::new(mismatched_slot::Imports) {
        Ok(_) => panic!("mismatched generated policy slot was accepted"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        HostConstructionError::HostImports(HostImportPolicyError::WrongInterface {
            method,
            ..
        }) if method.wit_name() == "create"
    ));
    assert!(error.to_string().contains("test:wrong/vm"));
    assert!(error.to_string().contains("test:guarded/vm@1.2.3"));
}

#[test]
fn generated_host_import_construction_rejects_an_unguarded_impl() {
    let error = match lockgate::HostBuilder::new(unguarded::Imports) {
        Ok(_) => panic!("unguarded generated host implementation was accepted"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        HostConstructionError::HostImports(
            HostImportPolicyError::MissingGuardedImplementation { interface }
        ) if interface.name() == "test:guarded/vm" && interface.version() == Some("1.2.3")
    ));
    assert!(error.to_string().contains("test:guarded/vm@1.2.3"));
    assert!(error.to_string().contains("#[lockgate::guarded]"));
}

#[test]
fn application_host_import_cannot_collide_with_the_reserved_framework_namespace() {
    let error = match lockgate::HostBuilder::new(reserved_namespace::Imports) {
        Ok(_) => panic!("reserved application host interface was accepted"),
        Err(error) => error,
    };
    assert!(matches!(
        error,
        HostConstructionError::HostImports(
            HostImportPolicyError::ReservedFrameworkInterface { interface }
        ) if interface.name() == "lockgate:config/settings" && interface.version().is_none()
    ));
    let message = error.to_string();
    assert!(message.contains("lockgate:config/settings"));
    assert!(message.contains("reserved `lockgate:` framework namespace"));
    assert!(message.contains("dedicated linker rules"));
}

#[tokio::test]
async fn preparation_rejects_an_unregistered_guard_permission() {
    let mut builder = lockgate::HostBuilder::new(Imports::default()).unwrap();
    let error = builder
        .prepare(
            PluginId::from("unused"),
            b"not inspected",
            PluginConfig::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::UnregisteredGuardPermission {
            ref atom,
            ref interface,
            method: "restart",
        } if atom.to_string() == "admin.restart" && interface == "test:guarded/admin@1.2.3"
    ));
    assert!(error.to_string().contains("test:guarded/admin@1.2.3"));

    let mut registered = lockgate::HostBuilder::new(Imports::default())
        .unwrap()
        .register::<permissions::Contract>()
        .unwrap()
        .register::<admin_permissions::Contract>()
        .unwrap();
    let error = registered
        .prepare(
            PluginId::from("unused"),
            b"not inspected",
            PluginConfig::default(),
        )
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::Inspection(_)));
}

fn component(wit: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let package = resolve.push_str("fixture.wit", wit).unwrap();
    let world = resolve.select_world(&[package], None).unwrap();
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap()
}

fn fixture(wit: &str, needs: &NeedsManifest) -> Vec<u8> {
    let metadata = PluginMetadata::new(PLUGIN_ID, "Guarded wiring fixture", "1.0").unwrap();
    let bytes = common::with_custom_section(
        &component(wit),
        PLUGIN_METADATA_SECTION,
        &metadata.to_section_bytes().unwrap(),
    );
    common::with_custom_section(
        &bytes,
        PLUGIN_NEEDS_SECTION,
        &needs.to_section_bytes().unwrap(),
    )
}

fn host_builder() -> lockgate::HostBuilder<()> {
    lockgate::HostBuilder::with_allocation(
        Imports::default(),
        InstanceAllocation::Pooling(PoolingAllocationConfig::default()),
    )
    .unwrap()
    .register::<permissions::Contract>()
    .unwrap()
    .register::<admin_permissions::Contract>()
    .unwrap()
}

async fn admit(builder: &mut lockgate::HostBuilder<()>, bytes: &[u8]) {
    let prepared = builder
        .prepare(PluginId::from(PLUGIN_ID), bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    builder
        .admit(prepared, acceptance, RuntimeLimits::default())
        .await
        .unwrap();
}

const ADMIN_IMPORT: &str = "package test:guarded@1.2.3; interface admin { enum admin-error { denied } restart: func() -> result<_, admin-error>; status: func() -> result<_, admin-error>; } world fixture { import admin; }";
const MIXED_IMPORT: &str = "package test:guarded@1.2.3; interface mixed { enum mixed-error { denied } vm-action: func() -> result<string, mixed-error>; admin-action: func() -> result<string, mixed-error>; protocol-version: func() -> string; } world fixture { import mixed; }";
const BASELINE_IMPORT: &str = "package lockgate:config; interface settings { enum get-error { not-ready } get-json: func() -> result<string, get-error>; } world fixture { import settings; }";
const NO_IMPORTS: &str = "package test:no-imports; world fixture {}";

#[tokio::test]
async fn all_guarded_import_without_a_mapped_need_is_a_manifest_mismatch() {
    let bytes = fixture(ADMIN_IMPORT, &NeedsManifest::empty());
    let mut builder = host_builder();
    let error = builder
        .prepare(PluginId::from(PLUGIN_ID), &bytes, PluginConfig::default())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AdmissionError::HostImportManifestMismatch {
            ref interface,
            ref permissions,
        } if interface == "test:guarded/admin@1.2.3"
            && permissions.iter().map(ToString::to_string).collect::<Vec<_>>()
                == ["admin.restart", "admin.status"]
    ));
    assert_eq!(
        error.to_string(),
        "plugin imports host interface `test:guarded/admin@1.2.3` but declares none of its permissions"
    );
    assert_eq!(
        error.hint(),
        Some("add at least one as a required or optional need, or remove the interface import")
    );
}

#[tokio::test]
async fn either_required_or_optional_mapped_need_wires_an_all_guarded_import() {
    let atom: AtomKey = "admin.restart".parse().unwrap();
    for needs in [
        NeedsManifest::new(vec![NeedEntry::flag(atom.clone())], vec![]).unwrap(),
        NeedsManifest::new(vec![], vec![NeedEntry::flag(atom.clone())]).unwrap(),
    ] {
        let bytes = fixture(ADMIN_IMPORT, &needs);
        admit(&mut host_builder(), &bytes).await;
    }
}

#[tokio::test]
async fn capability_free_method_wires_a_cross_capability_import_without_declared_needs() {
    let bytes = fixture(MIXED_IMPORT, &NeedsManifest::empty());
    admit(&mut host_builder(), &bytes).await;
}

#[tokio::test]
async fn cross_capability_mixed_methods_deny_independently_after_interface_wiring() {
    for (atom, vm_result, admin_result) in [
        ("vm.list-pools", "ok:vm", "denied"),
        ("admin.status", "denied", "ok:admin"),
    ] {
        let imports = Imports::default();
        let needs =
            NeedsManifest::new(vec![NeedEntry::flag(atom.parse().unwrap())], vec![]).unwrap();
        let (host, plugin) =
            runtime_host(imports.clone(), PluginId::from("mixed-plugin"), &needs).await;
        let guest = host.guest(&plugin).unwrap();

        assert_eq!(guest.mixed_version().await.unwrap(), "mixed-v1");
        assert_eq!(guest.mixed_vm().await.unwrap(), vm_result);
        assert_eq!(guest.mixed_admin().await.unwrap(), admin_result);
        assert_eq!(
            imports
                .state
                .body_calls
                .mixed_version
                .load(Ordering::SeqCst),
            1
        );
        assert_eq!(
            imports.state.body_calls.mixed_vm.load(Ordering::SeqCst),
            usize::from(vm_result != "denied")
        );
        assert_eq!(
            imports.state.body_calls.mixed_admin.load(Ordering::SeqCst),
            usize::from(admin_result != "denied")
        );
        host.shutdown().await;
    }
}

#[tokio::test]
async fn framework_settings_import_stays_on_its_dedicated_linker_path() {
    let bytes = fixture(BASELINE_IMPORT, &NeedsManifest::empty());
    admit(&mut host_builder(), &bytes).await;
}

#[tokio::test]
async fn declared_need_without_a_matching_import_still_admits() {
    let atom: AtomKey = "admin.restart".parse().unwrap();
    let needs = NeedsManifest::new(vec![NeedEntry::flag(atom)], vec![]).unwrap();
    let bytes = fixture(NO_IMPORTS, &needs);
    admit(&mut host_builder(), &bytes).await;
}

fn runtime_fixture(plugin_id: &PluginId, needs: &NeedsManifest) -> Vec<u8> {
    let metadata =
        PluginMetadata::new(plugin_id.as_str(), "Guard enforcement fixture", "1.0").unwrap();
    common::policy_fixture(&common::GUARDED_BINDINGS_FIXTURE, &metadata, needs)
}

fn scoped_need(atom: &str, scope: &str) -> NeedEntry {
    NeedEntry::scoped(
        atom.parse().unwrap(),
        vec![ScopeRefEntry::literal(scope).unwrap()],
    )
    .unwrap()
}

async fn runtime_host(
    imports: Imports,
    plugin_id: PluginId,
    needs: &NeedsManifest,
) -> (lockgate::Host<()>, lockgate::PluginHandle) {
    runtime_host_with_limits(imports, plugin_id, needs, RuntimeLimits::default()).await
}

async fn runtime_host_with_limits(
    imports: Imports,
    plugin_id: PluginId,
    needs: &NeedsManifest,
    limits: RuntimeLimits,
) -> (lockgate::Host<()>, lockgate::PluginHandle) {
    let mut builder = lockgate::HostBuilder::with_allocation(
        imports,
        InstanceAllocation::Pooling(PoolingAllocationConfig::default()),
    )
    .unwrap()
    .register::<permissions::Contract>()
    .unwrap()
    .register::<admin_permissions::Contract>()
    .unwrap();
    let bytes = runtime_fixture(&plugin_id, needs);
    let prepared = builder
        .prepare(plugin_id, &bytes, PluginConfig::default())
        .await
        .unwrap();
    let acceptance = prepared.accept_all();
    let plugin = builder.admit(prepared, acceptance, limits).await.unwrap();
    (builder.finish(), plugin)
}

#[tokio::test]
async fn guarded_capability_imports_still_consume_the_call_limit() {
    const LIMIT: u64 = 3;
    let imports = Imports::default();
    let (host, plugin) = runtime_host_with_limits(
        imports.clone(),
        PluginId::from("call-limit-plugin"),
        &NeedsManifest::empty(),
        RuntimeLimits {
            max_host_import_calls: LIMIT,
            ..RuntimeLimits::default()
        },
    )
    .await;
    let guest = host.guest(&plugin).unwrap();

    let error = guest
        .protocol_version_many(LIMIT as u32 + 1)
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        CallError::HostImportCallLimitExceeded { limit } if limit == LIMIT
    ));
    assert_eq!(
        imports
            .state
            .body_calls
            .protocol_version
            .load(Ordering::SeqCst),
        LIMIT as usize
    );
    host.shutdown().await;
}

#[tokio::test]
async fn generated_pool_guards_resolve_once_and_deny_before_the_body() {
    let imports = Imports::default();
    let needs = NeedsManifest::new(
        vec![
            scoped_need("vm.create", "gpu"),
            scoped_need("vm.exec", "pool:gpu"),
        ],
        vec![],
    )
    .unwrap();
    let (host, plugin) = runtime_host(imports.clone(), PluginId::from("pool-plugin"), &needs).await;
    let guest = host.guest(&plugin).unwrap();

    let vm = guest.create("gpu").await.unwrap();
    assert_eq!(vm, "ok:gpu-1");
    assert_eq!(imports.state.pool_resolutions.load(Ordering::SeqCst), 1);
    assert_eq!(imports.state.body_calls.create.load(Ordering::SeqCst), 1);
    assert_eq!(
        guest.exec("gpu-1", "nvidia-smi").await.unwrap(),
        "ok:gpu-1:nvidia-smi"
    );
    assert_eq!(imports.state.vm_resolutions.load(Ordering::SeqCst), 1);
    assert_eq!(imports.state.body_calls.exec.load(Ordering::SeqCst), 1);

    assert_eq!(guest.create("cpu").await.unwrap(), "denied");
    assert_eq!(guest.create("missing").await.unwrap(), "not-found");
    assert_eq!(imports.state.pool_resolutions.load(Ordering::SeqCst), 3);
    assert_eq!(imports.state.body_calls.create.load(Ordering::SeqCst), 1);
    assert_eq!(guest.exec("cpu-b", "hostname").await.unwrap(), "denied");
    assert_eq!(imports.state.vm_resolutions.load(Ordering::SeqCst), 2);
    assert_eq!(imports.state.body_calls.exec.load(Ordering::SeqCst), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn argument_resource_guard_executes_the_normalized_checked_resource() {
    let imports = Imports::default();
    let needs = NeedsManifest::new(vec![scoped_need("vm.exec", "pool:gpu")], vec![]).unwrap();
    let (host, plugin) =
        runtime_host(imports.clone(), PluginId::from("normalized-plugin"), &needs).await;
    let guest = host.guest(&plugin).unwrap();

    assert_eq!(
        guest.exec("  GPU-A  ", "uptime").await.unwrap(),
        "ok:gpu-a:uptime"
    );
    assert_eq!(guest.exec("  CPU-B  ", "hostname").await.unwrap(), "denied");
    assert_eq!(imports.state.vm_resolutions.load(Ordering::SeqCst), 2);
    assert_eq!(imports.state.body_calls.exec.load(Ordering::SeqCst), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn created_by_caller_guards_cover_only_the_callers_own_vms() {
    let imports = Imports::default();
    let needs = NeedsManifest::new(
        vec![
            scoped_need("vm.exec", "created-by-caller"),
            scoped_need("vm.destroy", "created-by-caller"),
        ],
        vec![],
    )
    .unwrap();
    let (host, plugin) = runtime_host(imports.clone(), PluginId::from("plugin-a"), &needs).await;
    let guest = host.guest(&plugin).unwrap();

    assert_eq!(
        guest.exec("gpu-a", "uptime").await.unwrap(),
        "ok:gpu-a:uptime"
    );
    assert_eq!(guest.exec("gpu-b", "uptime").await.unwrap(), "denied");
    assert_eq!(imports.state.body_calls.exec.load(Ordering::SeqCst), 1);

    assert_eq!(guest.destroy("gpu-a").await.unwrap(), "ok");
    assert_eq!(guest.destroy("gpu-b").await.unwrap(), "denied");
    assert_eq!(imports.state.body_calls.destroy.load(Ordering::SeqCst), 1);
    host.shutdown().await;
}

#[tokio::test]
async fn unscoped_and_capability_free_guards_run_through_the_runtime() {
    let denied_imports = Imports::default();
    let (host, plugin) = runtime_host(
        denied_imports.clone(),
        PluginId::from("no-grants"),
        &NeedsManifest::empty(),
    )
    .await;
    let guest = host.guest(&plugin).unwrap();
    assert_eq!(guest.list_pools().await.unwrap(), "denied");
    assert_eq!(
        denied_imports
            .state
            .body_calls
            .list_pools
            .load(Ordering::SeqCst),
        0
    );
    assert_eq!(guest.protocol_version().await.unwrap(), "1");
    assert_eq!(
        denied_imports
            .state
            .body_calls
            .protocol_version
            .load(Ordering::SeqCst),
        1
    );
    host.shutdown().await;

    let allowed_imports = Imports::default();
    let needs = NeedsManifest::new(
        vec![NeedEntry::flag("vm.list-pools".parse().unwrap())],
        vec![],
    )
    .unwrap();
    let (host, plugin) = runtime_host(
        allowed_imports.clone(),
        PluginId::from("list-plugin"),
        &needs,
    )
    .await;
    let guest = host.guest(&plugin).unwrap();
    assert_eq!(guest.list_pools().await.unwrap(), "cpu,gpu");
    assert_eq!(
        allowed_imports
            .state
            .body_calls
            .list_pools
            .load(Ordering::SeqCst),
        1
    );
    host.shutdown().await;
}

mod mismatched_slot {
    use lockgate::HostCtx;

    #[derive(Clone)]
    pub struct Data;

    #[derive(Clone)]
    pub struct Imports;

    lockgate::host_bindings!({
        path: "tests/data/guarded_bindings",
        world: "vm-only",
        imports_type: Imports,
        data: Data,
    });

    impl vm::Host for Imports {
        const __LOCKGATE_POLICY_METHODS: &'static [lockgate::__private::PolicyMethod] =
            &[lockgate::__private::PolicyMethod::__no_capability_required(
                lockgate::__private::InterfaceIdentity::__new("test:wrong/vm", None),
                vm::__LockgateBinding::__LOCKGATE_METHOD_CREATE,
                "construction validation fixture",
            )];

        async fn create(
            &mut self,
            _cx: HostCtx<'_, Data>,
            pool: String,
        ) -> Result<String, vm::VmError> {
            Ok(pool)
        }

        async fn exec(
            &mut self,
            _cx: HostCtx<'_, Data>,
            vm: String,
            command: String,
        ) -> Result<String, vm::VmError> {
            Ok(format!("{vm}:{command}"))
        }

        async fn destroy(
            &mut self,
            _cx: HostCtx<'_, Data>,
            _vm: String,
        ) -> Result<(), vm::VmError> {
            Ok(())
        }

        async fn list_pools(&mut self, _cx: HostCtx<'_, Data>) -> Result<Vec<String>, vm::VmError> {
            Ok(Vec::new())
        }

        async fn protocol_version(&mut self, _cx: HostCtx<'_, Data>) -> String {
            "1".to_owned()
        }
    }
}

mod unguarded {
    use lockgate::HostCtx;

    #[derive(Clone)]
    pub struct Data;

    #[derive(Clone)]
    pub struct Imports;

    lockgate::host_bindings!({
        path: "tests/data/guarded_bindings",
        world: "vm-only",
        imports_type: Imports,
        data: Data,
    });

    impl vm::Host for Imports {
        async fn create(
            &mut self,
            _cx: HostCtx<'_, Data>,
            pool: String,
        ) -> Result<String, vm::VmError> {
            Ok(pool)
        }

        async fn exec(
            &mut self,
            _cx: HostCtx<'_, Data>,
            vm: String,
            command: String,
        ) -> Result<String, vm::VmError> {
            Ok(format!("{vm}:{command}"))
        }

        async fn destroy(
            &mut self,
            _cx: HostCtx<'_, Data>,
            _vm: String,
        ) -> Result<(), vm::VmError> {
            Ok(())
        }

        async fn list_pools(&mut self, _cx: HostCtx<'_, Data>) -> Result<Vec<String>, vm::VmError> {
            Ok(Vec::new())
        }

        async fn protocol_version(&mut self, _cx: HostCtx<'_, Data>) -> String {
            "1".to_owned()
        }
    }
}

mod reserved_namespace {
    use lockgate::HostCtx;

    #[derive(Clone)]
    pub struct Imports;

    lockgate::host_bindings!({
        path: "tests/data/reserved_host_import",
        world: "fixture",
        imports_type: Imports,
        data: (),
    });

    #[lockgate::guarded]
    impl settings::Host for Imports {
        #[lockgate::no_capability_required(
            reason = "reserved namespace collision regression fixture"
        )]
        async fn get_json(&mut self, _cx: HostCtx<'_, ()>) -> String {
            "{}".to_owned()
        }
    }
}
