use lockgate::{
    Acceptance, AdmissionError, HostConstructionError, HostCtx, HostImportPolicyError,
    InvocationCtx, PluginConfig, RuntimeLimits, Scope, ScopeRepr,
};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{AtomKey, NeedEntry, NeedsManifest, PluginMetadata};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

mod common;

const PLUGIN_ID: &str = "guarded-wiring";

#[derive(Clone, PartialEq, Eq, ScopeRepr)]
enum VmScope {
    #[scope(rename = "all")]
    All,
}

impl Scope for VmScope {}

#[lockgate::capability("vm")]
mod permissions {
    use super::VmScope;
    use lockgate::{Permission, ScopedPermission};

    pub const CREATE: ScopedPermission<VmScope> = ScopedPermission::new("create");
    pub const EXEC: ScopedPermission<VmScope> = ScopedPermission::new("exec");
    pub const LIST_POOLS: Permission = Permission::new("list-pools");
    pub const RESTART: Permission = Permission::new("restart");
    pub const STATUS: Permission = Permission::new("status");
}

#[derive(Clone)]
struct Session {
    owner: String,
}

#[derive(Clone)]
struct CallData {
    vm: String,
    session: Session,
}

#[derive(Clone)]
struct Imports;

lockgate::host_bindings!({
    path: "tests/data/guarded_bindings",
    world: "fixture",
    imports: Imports,
    data: CallData,
});

#[lockgate::guarded]
impl vm::Host for Imports {
    #[lockgate::requires(permission = permissions::CREATE, target = pool)]
    async fn create(&mut self, _cx: HostCtx<'_, CallData>, pool: String) -> String {
        pool
    }

    #[lockgate::requires(permission = permissions::EXEC, target = cx.data().session.owner)]
    async fn exec(&mut self, cx: HostCtx<'_, CallData>, vm: String, command: String) -> String {
        let _ = (&cx.data().vm, &cx.data().session.owner);
        format!("{vm}:{command}")
    }

    #[lockgate::requires(permission = permissions::LIST_POOLS)]
    async fn list_pools(&mut self, _cx: HostCtx<'_, CallData>) -> Vec<String> {
        Vec::new()
    }

    #[lockgate::no_capability_required(reason = "returns only a static protocol version")]
    async fn protocol_version(&mut self, _cx: HostCtx<'_, CallData>) -> String {
        "1".to_owned()
    }
}

#[lockgate::guarded]
impl admin::Host for Imports {
    #[lockgate::requires(permission = permissions::RESTART)]
    async fn restart(&mut self, _cx: HostCtx<'_, CallData>) {}

    #[lockgate::requires(permission = permissions::STATUS)]
    async fn status(&mut self, _cx: HostCtx<'_, CallData>) {}
}

#[test]
fn guarded_impl_fills_complete_typed_method_metadata() {
    let interface = vm::__LockgateBinding::INTERFACE;
    assert_eq!(interface.name(), "test:guarded/vm");
    assert_eq!(interface.version(), Some("1.2.3"));

    let methods = <Imports as vm::Host>::__LOCKGATE_POLICY_METHODS;
    assert_eq!(methods.len(), 4);
    for method in methods {
        assert_eq!(method.interface(), interface);
    }
    assert_eq!(methods[0].method().wit_name(), "create");
    assert_eq!(methods[1].method().wit_name(), "exec");
    assert_eq!(methods[2].method().wit_name(), "list-pools");
    assert_eq!(methods[3].method().wit_name(), "protocol-version");

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
    assert_eq!(exec.target(), Some("cx.data().session.owner"));

    let list = methods[2].classification();
    let permission = list.permission().unwrap();
    assert_eq!(
        (permission.capability(), permission.permission()),
        ("vm", "list-pools")
    );
    assert_eq!(list.target(), None);

    assert_eq!(
        methods[3].classification().reason(),
        Some("returns only a static protocol version")
    );

    let admin_methods = <Imports as admin::Host>::__LOCKGATE_POLICY_METHODS;
    assert_eq!(admin_methods.len(), 2);
    assert_eq!(
        admin_methods
            .iter()
            .map(|method| {
                let permission = method.classification().permission().unwrap();
                (method.method().wit_name(), permission.permission())
            })
            .collect::<Vec<_>>(),
        [("restart", "restart"), ("status", "status")]
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

#[tokio::test]
async fn preparation_rejects_an_unregistered_guard_permission() {
    let mut builder = lockgate::HostBuilder::new(Imports).unwrap();
    let error = builder
        .prepare("unused", b"not inspected", PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(
        error,
        AdmissionError::UnregisteredGuardPermission {
            ref atom,
            ref interface,
            method: "restart",
        } if atom.to_string() == "vm.restart" && interface == "test:guarded/admin@1.2.3"
    ));
    assert!(error.to_string().contains("test:guarded/admin@1.2.3"));

    let mut registered = lockgate::HostBuilder::new(Imports)
        .unwrap()
        .register::<permissions::Contract>()
        .unwrap();
    let error = registered
        .prepare("unused", b"not inspected", PluginConfig::default())
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

fn host_builder() -> lockgate::HostBuilder<CallData> {
    lockgate::HostBuilder::new(Imports)
        .unwrap()
        .register::<permissions::Contract>()
        .unwrap()
}

fn startup_context() -> InvocationCtx<CallData> {
    InvocationCtx::new(
        CallData {
            vm: "startup-vm".to_owned(),
            session: Session {
                owner: "startup-owner".to_owned(),
            },
        },
        lockgate::BudgetClass::Bounded { fuel: 1_000_000 },
    )
}

async fn admit(builder: &mut lockgate::HostBuilder<CallData>, bytes: &[u8]) {
    let prepared = builder
        .prepare(PLUGIN_ID, bytes, PluginConfig::default())
        .await
        .unwrap();
    builder
        .admit(
            prepared,
            Acceptance::all_declared(),
            RuntimeLimits::default(),
            startup_context(),
        )
        .await
        .unwrap();
}

const ADMIN_IMPORT: &str = "package test:guarded@1.2.3; interface admin { restart: func(); status: func(); } world fixture { import admin; }";
const MIXED_IMPORT: &str = "package test:guarded@1.2.3; interface vm { create: func(pool: string) -> string; exec: func(vm: string, command: string) -> string; list-pools: func() -> list<string>; protocol-version: func() -> string; } world fixture { import vm; }";
const BASELINE_IMPORT: &str = "package lockgate:config; interface settings { enum get-error { not-ready } get-json: func() -> result<string, get-error>; } world fixture { import settings; }";
const NO_IMPORTS: &str = "package test:no-imports; world fixture {}";

#[tokio::test]
async fn all_guarded_import_without_a_mapped_need_is_a_manifest_mismatch() {
    let bytes = fixture(ADMIN_IMPORT, &NeedsManifest::empty());
    let mut builder = host_builder();
    let error = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap_err();

    assert!(matches!(
        error,
        AdmissionError::HostImportManifestMismatch {
            ref interface,
            ref permissions,
        } if interface == "test:guarded/admin@1.2.3"
            && permissions.iter().map(ToString::to_string).collect::<Vec<_>>()
                == ["vm.restart", "vm.status"]
    ));
    let message = error.to_string();
    assert!(message.contains("test:guarded/admin@1.2.3"));
    assert!(message.contains("vm.restart"));
    assert!(message.contains("vm.status"));
    assert!(message.contains("required or optional need"));
}

#[tokio::test]
async fn either_required_or_optional_mapped_need_wires_an_all_guarded_import() {
    let atom: AtomKey = "vm.restart".parse().unwrap();
    for needs in [
        NeedsManifest::new(vec![NeedEntry::flag(atom.clone())], vec![]).unwrap(),
        NeedsManifest::new(vec![], vec![NeedEntry::flag(atom.clone())]).unwrap(),
    ] {
        let bytes = fixture(ADMIN_IMPORT, &needs);
        admit(&mut host_builder(), &bytes).await;
    }
}

#[tokio::test]
async fn capability_free_method_wires_a_mixed_import_without_declared_needs() {
    let bytes = fixture(MIXED_IMPORT, &NeedsManifest::empty());
    admit(&mut host_builder(), &bytes).await;
}

#[tokio::test]
async fn framework_settings_import_stays_on_its_dedicated_linker_path() {
    let bytes = fixture(BASELINE_IMPORT, &NeedsManifest::empty());
    admit(&mut host_builder(), &bytes).await;
}

#[tokio::test]
async fn declared_need_without_a_matching_import_still_admits() {
    let atom: AtomKey = "vm.restart".parse().unwrap();
    let needs = NeedsManifest::new(vec![NeedEntry::flag(atom)], vec![]).unwrap();
    let bytes = fixture(NO_IMPORTS, &needs);
    admit(&mut host_builder(), &bytes).await;
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
        imports: Imports,
        data: Data,
    });

    impl vm::Host for Imports {
        const __LOCKGATE_POLICY_METHODS: &'static [lockgate::__private::PolicyMethod] =
            &[lockgate::__private::PolicyMethod::__no_capability_required(
                lockgate::__private::InterfaceIdentity::__new("test:wrong/vm", None),
                vm::__LockgateBinding::__LOCKGATE_METHOD_CREATE,
                "construction validation fixture",
            )];

        async fn create(&mut self, _cx: HostCtx<'_, Data>, pool: String) -> String {
            pool
        }

        async fn exec(&mut self, _cx: HostCtx<'_, Data>, vm: String, command: String) -> String {
            format!("{vm}:{command}")
        }

        async fn list_pools(&mut self, _cx: HostCtx<'_, Data>) -> Vec<String> {
            Vec::new()
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
        imports: Imports,
        data: Data,
    });

    impl vm::Host for Imports {
        async fn create(&mut self, _cx: HostCtx<'_, Data>, pool: String) -> String {
            pool
        }

        async fn exec(&mut self, _cx: HostCtx<'_, Data>, vm: String, command: String) -> String {
            format!("{vm}:{command}")
        }

        async fn list_pools(&mut self, _cx: HostCtx<'_, Data>) -> Vec<String> {
            Vec::new()
        }

        async fn protocol_version(&mut self, _cx: HostCtx<'_, Data>) -> String {
            "1".to_owned()
        }
    }
}
