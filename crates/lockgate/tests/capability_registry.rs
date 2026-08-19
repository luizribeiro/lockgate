use lockgate::{CapabilityContract, HostBuilder, Permission, Scope, ScopedPermission};
use lockgate_policy::__private::{
    ErasedPermission, erase_permission, erase_scoped_permission, qualify_permission,
    qualify_scoped_permission,
};

#[lockgate::capability("sessions")]
mod sessions {
    use lockgate::Permission;

    pub const READ: Permission = Permission::new("read");
}

const READ: Permission = qualify_permission("sessions", Permission::new("read"));
const ERASED_READ: ErasedPermission = erase_permission(READ);
static DUPLICATE_PERMISSIONS: [ErasedPermission; 2] = [ERASED_READ, ERASED_READ];

struct DuplicatePermission;
impl CapabilityContract for DuplicatePermission {
    const ID: &'static str = "sessions";

    fn permissions() -> &'static [ErasedPermission] {
        &DUPLICATE_PERMISSIONS
    }
}

struct MalformedCapability;
impl CapabilityContract for MalformedCapability {
    const ID: &'static str = "Virtual_Machines";

    fn permissions() -> &'static [ErasedPermission] {
        &[]
    }
}

const MALFORMED_PERMISSION: Permission =
    qualify_permission("sessions", Permission::new("Read_All"));
static MALFORMED_PERMISSIONS: [ErasedPermission; 1] = [erase_permission(MALFORMED_PERMISSION)];

struct MalformedPermission;
impl CapabilityContract for MalformedPermission {
    const ID: &'static str = "sessions";

    fn permissions() -> &'static [ErasedPermission] {
        &MALFORMED_PERMISSIONS
    }
}

const FOREIGN_PERMISSION: Permission = qualify_permission("other", Permission::new("read"));
static INCONSISTENT_PERMISSIONS: [ErasedPermission; 1] = [erase_permission(FOREIGN_PERMISSION)];

struct InconsistentDescriptor;
impl CapabilityContract for InconsistentDescriptor {
    const ID: &'static str = "sessions";

    fn permissions() -> &'static [ErasedPermission] {
        &INCONSISTENT_PERMISSIONS
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, lockgate::ScopeRepr)]
enum WideningScope {
    All,
    Current,
}

impl Scope for WideningScope {
    fn contains(&self, inner: &Self) -> bool {
        self == inner || matches!(self, Self::All)
    }

    fn intersect(&self, other: &Self) -> Option<Self> {
        match (self, other) {
            (Self::All, Self::Current) | (Self::Current, Self::All) => Some(Self::All),
            _ if self == other => Some(*self),
            _ => None,
        }
    }
}

const BROKEN_READ: ScopedPermission<WideningScope> =
    qualify_scoped_permission("sessions", ScopedPermission::<WideningScope>::new("read"));
static BROKEN_PERMISSIONS: [ErasedPermission; 1] = [erase_scoped_permission(BROKEN_READ)];

struct BrokenScopeContract;
impl CapabilityContract for BrokenScopeContract {
    const ID: &'static str = "sessions";

    fn permissions() -> &'static [ErasedPermission] {
        &BROKEN_PERMISSIONS
    }
}

#[test]
fn generated_contract_registers_and_duplicate_capabilities_are_rejected() {
    let builder = HostBuilder::new(())
        .unwrap()
        .register::<sessions::Contract>()
        .unwrap();
    let error = builder.register::<sessions::Contract>().err().unwrap();
    assert_eq!(
        error.to_string(),
        "capability `sessions` is registered more than once; register each capability contract once"
    );
}

#[test]
fn hand_built_contract_rejections_name_the_offender() {
    let cases = [
        (
            HostBuilder::new(())
                .unwrap()
                .register::<DuplicatePermission>()
                .err()
                .unwrap()
                .to_string(),
            "capability `sessions` declares permission `read` more than once",
        ),
        (
            HostBuilder::new(())
                .unwrap()
                .register::<MalformedCapability>()
                .err()
                .unwrap()
                .to_string(),
            "capability ID `Virtual_Machines` is malformed",
        ),
        (
            HostBuilder::new(())
                .unwrap()
                .register::<MalformedPermission>()
                .err()
                .unwrap()
                .to_string(),
            "capability `sessions` declares malformed permission ID `Read_All`",
        ),
        (
            HostBuilder::new(())
                .unwrap()
                .register::<InconsistentDescriptor>()
                .err()
                .unwrap()
                .to_string(),
            "contract `sessions` returned permission descriptor `other.read`",
        ),
    ];

    for (message, expected) in cases {
        assert!(message.contains(expected), "unexpected error: {message}");
    }
}

#[test]
fn registration_runs_exhaustive_scope_laws() {
    let error = HostBuilder::new(())
        .unwrap()
        .register::<BrokenScopeContract>()
        .err()
        .unwrap();
    let message = error.to_string();
    assert!(message.contains("capability `sessions` permission `read`"));
    assert!(message.contains(core::any::type_name::<WideningScope>()));
    assert!(message.contains("containment/intersection agreement law violated"));
}
