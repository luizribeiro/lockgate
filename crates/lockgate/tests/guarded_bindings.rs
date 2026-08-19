use lockgate::{HostCtx, Scope, ScopeRepr};

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
struct CallData {
    vm: String,
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

    #[lockgate::requires(permission = permissions::EXEC, target = cx.data().vm)]
    async fn exec(&mut self, cx: HostCtx<'_, CallData>, vm: String, command: String) -> String {
        let _ = &cx.data().vm;
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
    assert_eq!(exec.target(), Some("cx.data().vm"));

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
