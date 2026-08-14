use std::path::PathBuf;
use std::process::Command;

#[test]
fn host_bindings_supports_a_renamed_facade_dependency() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lockgate/tests/fixtures/renamed-host-facade/Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--locked",
        ])
        .output()
        .expect("failed to check renamed host facade fixture");

    assert!(
        output.status.success(),
        "renamed host facade fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

#[test]
fn generated_clients_compile_for_every_admitted_value_shape() {
    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lockgate/tests/fixtures/host-export-values/Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--locked",
        ])
        .output()
        .expect("failed to check host export values fixture");

    assert!(
        output.status.success(),
        "host export values fixture failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}
