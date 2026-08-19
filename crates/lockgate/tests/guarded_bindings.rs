use lockgate::{
    AdmissionError, HostConstructionError, HostCtx, HostImportPolicyError, PluginConfig, Scope,
    ScopeRepr,
};

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
            interface: "test:guarded/vm",
            method: "create",
        } if atom.to_string() == "vm.create"
    ));

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

mod mismatched_slot {
    use lockgate::HostCtx;

    #[derive(Clone)]
    pub struct Data;

    #[derive(Clone)]
    pub struct Imports;

    lockgate::host_bindings!({
        path: "tests/data/guarded_bindings",
        world: "fixture",
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
        world: "fixture",
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
