use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn capability_authoring_errors_are_teaching_diagnostics() {
    check_fails(
        "non-inline",
        r#"#[lockgate_policy::capability("vm")] pub mod vm;"#,
        "requires an inline module",
    );
    check_fails(
        "alias-hidden",
        r#"
use lockgate_policy::Permission;
#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::Permission;
    type Alias = Permission;
    pub const READ: Alias = Alias::new("read");
}
"#,
        "hides its permission type behind an alias",
    );
    check_fails(
        "duplicate-permission",
        r#"
use lockgate_policy::Permission;
#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::Permission;
    pub const READ: Permission = Permission::new("read");
    pub const READ_AGAIN: Permission = Permission::new("read");
}
"#,
        "permission ID `read` is declared more than once",
    );
    check_fails(
        "malformed-capability",
        r#"#[lockgate_policy::capability("Virtual_Machines")] pub mod vm {}"#,
        "invalid capability ID `Virtual_Machines`",
    );
    check_fails(
        "malformed-permission",
        r#"
use lockgate_policy::Permission;
#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::Permission;
    pub const READ: Permission = Permission::new("Read_All");
}
"#,
        "invalid permission ID `Read_All`",
    );
    check_fails(
        "conditional-duplicate",
        r#"
use lockgate_policy::Permission;
#[lockgate_policy::capability("vm")]
pub mod vm {
    use super::Permission;
    #[cfg(any())]
    pub const FIRST: Permission = Permission::new("read");
    #[cfg(not(any()))]
    pub const SECOND: Permission = Permission::new("read");
    #[cfg(not(any()))]
    pub const THIRD: Permission = Permission::new("read");
}
"#,
        "permission ID `read` is declared more than once",
    );
}

fn check_fails(case: &str, source: &str, expected: &str) {
    let fixture = fixture_root().join(case);
    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(fixture.join("src")).unwrap();
    fs::write(fixture.join("Cargo.toml"), fixture_manifest(case)).unwrap();
    fs::write(fixture.join("src/lib.rs"), source).unwrap();

    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--quiet",
            "--manifest-path",
            path_str(&fixture.join("Cargo.toml")),
            "--target-dir",
            path_str(&fixture_root().join("target")),
        ])
        .output()
        .expect("failed to check capability diagnostic fixture");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{case} unexpectedly compiled");
    assert!(
        stderr.contains(expected),
        "{case} did not emit the expected diagnostic\nstderr:\n{stderr}"
    );
    fs::remove_dir_all(fixture).unwrap();
}

fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/capability-authoring-diagnostics")
}

fn fixture_manifest(case: &str) -> String {
    format!(
        r#"[package]
name = "capability-{case}"
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
