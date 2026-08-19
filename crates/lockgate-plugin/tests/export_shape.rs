use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

const BASE_GUIDANCE: &str = "cannot cross an invocation boundary; return value data instead, keep durable state behind a host capability, or use a future scoped invocation feature";
const ASYNC_GUIDANCE: &str = "Async WIT functions are supported; this restriction applies to `future`, `stream`, and `error-context` value types and resource handles crossing the invocation boundary, not to the function's async declaration.";

#[test]
fn guest_build_rejects_every_forbidden_export_shape_with_host_guidance() {
    check_compile_failure(
        "resource",
        "package test:shape; interface api { resource file; run: func() -> own<file>; } world plugin { export api; }",
        &[
            "unsupported export `test:shape/api#run`",
            "offending type `own<file>`",
            BASE_GUIDANCE,
        ],
    );
    check_compile_failure(
        "future",
        "package test:shape; interface api { type nested = option<future<string>>; run: func() -> nested; } world plugin { export api; }",
        &[
            "unsupported export `test:shape/api#run`",
            "offending type `future`",
            BASE_GUIDANCE,
            ASYNC_GUIDANCE,
        ],
    );
    check_compile_failure(
        "stream",
        "package test:shape; interface api { type nested = result<string, stream<string>>; run: func() -> nested; } world plugin { export api; }",
        &[
            "unsupported export `test:shape/api#run`",
            "offending type `stream`",
            BASE_GUIDANCE,
            ASYNC_GUIDANCE,
        ],
    );
}

fn check_compile_failure(case: &str, wit: &str, expected: &[&str]) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/export-shape-diagnostics")
        .join(case);
    let target = fixture.join("target");
    let _ = fs::remove_dir_all(&fixture);
    fs::create_dir_all(fixture.join("src")).unwrap();
    fs::create_dir_all(fixture.join("wit")).unwrap();
    fs::write(fixture.join("Cargo.toml"), fixture_manifest()).unwrap();
    fs::write(
        fixture.join("src/lib.rs"),
        "#![no_std]\nlockgate_plugin::generate!({ path: \"wit\", world: \"plugin\" });",
    )
    .unwrap();
    fs::write(fixture.join("wit/world.wit"), wit).unwrap();

    let output = Command::new(env!("CARGO"))
        .args([
            "check",
            "--quiet",
            "--manifest-path",
            path_str(&fixture.join("Cargo.toml")),
            "--target-dir",
            path_str(&target),
            "--target",
            "wasm32-wasip2",
        ])
        .output()
        .expect("failed to check export-shape fixture");
    let stderr = String::from_utf8_lossy(&output.stderr);

    assert!(!output.status.success(), "{case} unexpectedly compiled");
    for message in expected {
        assert!(
            stderr.contains(message),
            "{case} missing `{message}`\nstderr:\n{stderr}"
        );
    }

    fs::remove_dir_all(&fixture).unwrap();
}

fn fixture_manifest() -> String {
    format!(
        r#"[package]
name = "export-shape-diagnostic"
version = "0.1.0"
edition = "2024"

[dependencies]
lockgate-plugin = {{ path = {:?}, default-features = false }}

[workspace]
"#,
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
    )
}

fn path_str(path: &Path) -> &str {
    path.to_str().expect("fixture path must be valid UTF-8")
}
