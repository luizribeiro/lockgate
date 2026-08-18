use std::path::PathBuf;
use std::process::Command;

use lockgate::{CapabilityContract, Permission, Scope, ScopeRepr, ScopedPermission};

#[derive(Clone, Copy, Debug, PartialEq, Eq, ScopeRepr)]
enum FacadeScope {
    Project,
}

impl Scope for FacadeScope {}

#[test]
fn host_facade_exposes_permission_contract_types() {
    fn accepts_contract<T: CapabilityContract>() {}
    fn accepts_unscoped(_: Option<Permission>) {}
    fn accepts_scoped(_: Option<ScopedPermission<FacadeScope>>) {}

    accepts_unscoped(None);
    accepts_scoped(None);
    accepts_contract::<FixtureContract>();
}

struct FixtureContract;

impl CapabilityContract for FixtureContract {
    const ID: &'static str = "fixture";

    fn permissions() -> &'static [lockgate_policy::__private::ErasedPermission] {
        &[]
    }
}

#[test]
fn scope_repr_derive_supports_a_renamed_host_facade_only_dependency() {
    let manifest =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/scope-reexport/Cargo.toml");
    let target =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/macro-diagnostic-fixtures");
    let output = Command::new(env!("CARGO"))
        .args(["check", "--manifest-path"])
        .arg(&manifest)
        .arg("--locked")
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("failed to check host scope re-export fixture");

    assert!(
        output.status.success(),
        "host scope re-export fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
