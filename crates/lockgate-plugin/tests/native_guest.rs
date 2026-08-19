use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn generated_guest_runs_unit_tests_on_the_native_target() {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../lockgate/tests/fixtures/typed-settings-guest/Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "test",
            "--quiet",
            "--manifest-path",
            path_str(&fixture),
            "-p",
            "lockgate-typed-settings-fixture",
        ])
        .output()
        .expect("failed to run native guest unit tests");

    assert!(
        output.status.success(),
        "native guest unit tests failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path must be valid UTF-8")
}
