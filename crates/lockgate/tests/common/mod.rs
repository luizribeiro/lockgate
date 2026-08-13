#![allow(
    dead_code,
    reason = "each integration-test binary uses only its own fixture helper"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;

static EXEC_FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();
static EXEC_CONCURRENT_FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();
static EXEC_FUEL_FIXTURE: OnceLock<Vec<u8>> = OnceLock::new();

pub(crate) fn exec_fixture() -> &'static [u8] {
    EXEC_FIXTURE
        .get_or_init(|| build_fixture("exec-guest", "lockgate_exec_fixture.wasm"))
        .as_slice()
}

pub(crate) fn exec_concurrent_fixture() -> &'static [u8] {
    EXEC_CONCURRENT_FIXTURE
        .get_or_init(|| {
            build_fixture(
                "exec-concurrent-guest",
                "lockgate_exec_concurrent_fixture.wasm",
            )
        })
        .as_slice()
}

pub(crate) fn exec_fuel_fixture() -> &'static [u8] {
    EXEC_FUEL_FIXTURE
        .get_or_init(|| build_fixture("exec-fuel-guest", "lockgate_exec_fuel_fixture.wasm"))
        .as_slice()
}

fn build_fixture(directory: &str, artifact: &str) -> Vec<u8> {
    let fixture_dir = fixture_dir(directory);
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
        .expect("failed to run Cargo for an exec guest fixture");

    assert!(
        output.status.success(),
        "exec guest fixture {directory} build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let component = fixture_dir
        .join("target/wasm32-wasip2/release")
        .join(artifact);
    std::fs::read(&component).unwrap_or_else(|error| {
        panic!(
            "failed to read exec guest fixture at {}: {error}",
            component.display()
        )
    })
}

fn fixture_dir(directory: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(directory)
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path must be valid UTF-8")
}
