use std::path::{Path, PathBuf};
use std::process::Command;

#[test]
fn export_type_and_module_diagnostics_are_stable() {
    check_fails(
        "unexported-type",
        "host role code generation cannot reference type `payload` from interface \
         `test:unexported-type/source` because that interface is not exported by the selected world",
    );
    check_fails(
        "module-collision",
        "two WIT interfaces map to module `values`",
    );
}

fn check_fails(fixture: &str, expected: &str) {
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    let manifest = manifest_dir
        .join("tests/fixtures")
        .join(fixture)
        .join("Cargo.toml");
    let target = manifest_dir.join("../../target/macro-diagnostic-fixtures");
    let output = Command::new(env!("CARGO"))
        .args(["check", "--manifest-path"])
        .arg(&manifest)
        .env("CARGO_TARGET_DIR", target)
        .output()
        .unwrap_or_else(|error| panic!("failed to check {}: {error}", display(&manifest)));
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(
        !output.status.success(),
        "{} unexpectedly compiled",
        display(&manifest)
    );
    assert!(
        stderr.contains(expected),
        "{} did not contain the expected diagnostic\nexpected:\n{expected}\nstderr:\n{stderr}",
        display(&manifest),
    );
}

fn display(path: &Path) -> String {
    path.display().to_string()
}
