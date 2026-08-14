use std::path::PathBuf;
use std::process::Command;

#[test]
fn export_supports_a_renamed_facade_dependency() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lockgate/tests/fixtures/renamed-facade/Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--target",
            "wasm32-wasip2",
            "--locked",
        ])
        .output()
        .expect("failed to check renamed facade fixture");

    assert!(
        output.status.success(),
        "renamed facade fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
