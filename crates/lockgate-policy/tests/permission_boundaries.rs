use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn declarations_and_qualified_identities_stay_separate_outside_the_crate() {
    check_compile_failure(
        "declaration-need",
        r#"
use lockgate_policy::Permission;

fn declaration_is_inert() {
    Permission::new("read").need();
}
"#,
        "no method named `need` found",
    );
    check_compile_failure(
        "declaration-is-not-qualified",
        r#"
use lockgate_policy::Permission;

fn requires_qualified(_: Permission) {}

fn declaration_is_not_qualified() {
    requires_qualified(Permission::new("read"));
}
"#,
        "expected `Permission`, found `PermissionDecl`",
    );
    check_compile_failure(
        "qualified-identity-is-private",
        r#"
use lockgate_policy::Permission;

fn identity_fields_are_private() {
    let _ = Permission {};
}
"#,
        "cannot construct `Permission` with struct literal syntax due to private fields",
    );
    check_compile_failure(
        "scoped-qualified-identity-is-private",
        r#"
use lockgate_policy::{Scope, ScopedPermission};

#[derive(Clone, PartialEq, Eq, lockgate_policy::ScopeRepr)]
enum Project {
    All,
}

impl Scope for Project {}

fn identity_fields_are_private() {
    let _ = ScopedPermission::<Project> {};
}
"#,
        "cannot construct `ScopedPermission<Project>` with struct literal syntax due to private fields",
    );
}

fn check_compile_failure(case: &str, source: &str, expected: &str) {
    let fixture = fixture_dir(case);
    let target = fixture.join("target");
    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(fixture.join("src")).unwrap();
    fs::write(fixture.join("Cargo.toml"), fixture_manifest()).unwrap();
    fs::write(fixture.join("src/lib.rs"), source).unwrap();

    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--quiet",
            "--manifest-path",
            path_str(&fixture.join("Cargo.toml")),
            "--target-dir",
            path_str(&target),
        ])
        .output()
        .expect("failed to check permission boundary fixture");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{case} unexpectedly compiled");
    assert!(
        stderr.contains(expected),
        "{case} did not emit the expected diagnostic\nstderr:\n{stderr}"
    );

    fs::remove_dir_all(&fixture).unwrap();
}

fn fixture_dir(case: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/permission-boundary-diagnostics")
        .join(case)
}

fn fixture_manifest() -> String {
    format!(
        r#"[package]
name = "permission-boundary-diagnostic"
version = "0.1.0"
edition = "2024"

[dependencies]
lockgate-policy = {{ path = {:?} }}

[workspace]
"#,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    )
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path must be valid UTF-8")
}
