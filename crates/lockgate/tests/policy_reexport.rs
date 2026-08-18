use std::path::PathBuf;
use std::process::Command;

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
