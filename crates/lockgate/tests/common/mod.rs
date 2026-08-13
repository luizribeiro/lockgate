use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static EXEC_FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();

pub(crate) fn exec_fixture() -> &'static [u8] {
    EXEC_FIXTURE.get_or_init(build_exec_fixture).as_slice()
}

fn build_exec_fixture() -> Vec<u8> {
    let fixture_dir = fixture_dir();
    let manifest = fixture_dir.join("Cargo.toml");
    let output = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            path_str(&manifest),
            "--target",
            "wasm32-wasip2",
            "--release",
            "--locked",
        ])
        .output()
        .expect("failed to run Cargo for the exec guest fixture");

    assert!(
        output.status.success(),
        "exec guest fixture build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let component = fixture_dir.join("target/wasm32-wasip2/release/lockgate_exec_fixture.wasm");
    std::fs::read(&component).unwrap_or_else(|error| {
        panic!(
            "failed to read exec guest fixture at {}: {error}",
            component.display()
        )
    })
}

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/exec-guest")
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path must be valid UTF-8")
}
