use std::path::PathBuf;
use std::process::Command;

#[test]
fn missing_component_reports_the_plugin_build_command() {
    let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("missing-plugin.wasm");
    let output = Command::new(env!("CARGO_BIN_EXE_audit-host"))
        .arg(missing)
        .output()
        .expect("failed to run audit host");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("could not read plugin component"));
    assert!(stderr.contains(
        "nix develop -c cargo build --manifest-path examples/audit/plugin/Cargo.toml \
         --target wasm32-wasip2 --release"
    ));
}
