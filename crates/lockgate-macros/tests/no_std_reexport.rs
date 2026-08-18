use std::path::PathBuf;
use std::process::Command;

#[test]
fn scope_repr_supports_a_renamed_no_std_policy_reexport() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/no-std-policy-reexport/Cargo.toml");
    let target =
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/macro-diagnostic-fixtures");
    let output = Command::new(env!("CARGO"))
        .args(["check", "--manifest-path"])
        .arg(&manifest)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .expect("failed to check renamed no_std policy re-export fixture");

    assert!(
        output.status.success(),
        "renamed no_std policy re-export fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
