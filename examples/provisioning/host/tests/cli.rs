use std::path::{Path, PathBuf};
use std::process::Command;

fn provisioning_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("provisioning host should have a parent directory")
        .to_owned()
}

#[test]
fn policy_v2_provisioning_flow_allows_and_denies_by_membership() {
    let root = provisioning_root();
    let manifest = root.join("plugin/Cargo.toml");
    let build = Command::new(env!("CARGO"))
        .args([
            "build",
            "--manifest-path",
            manifest.to_str().unwrap(),
            "--target",
            "wasm32-wasip2",
            "--release",
            "--locked",
        ])
        .output()
        .expect("failed to build provisioning plugin");
    assert!(
        build.status.success(),
        "provisioning plugin build failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&build.stdout),
        String::from_utf8_lossy(&build.stderr),
    );

    let component = root.join("plugin/target/wasm32-wasip2/release/provisioning_plugin.wasm");
    let output = Command::new(env!("CARGO_BIN_EXE_provisioning-host"))
        .arg(component)
        .output()
        .expect("failed to run provisioning host");
    assert!(
        output.status.success(),
        "provisioning host failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr),
    );

    let stdout = String::from_utf8(output.stdout).unwrap();
    for expected in [
        "protocol version: 1",
        "allowed: vm.create pool=gpu -> gpu/vm-1",
        "allowed: vm.exec vm=gpu/vm-1 via=created-by-caller",
        "allowed: vm.exec vm=gpu/base via=pool:gpu",
        "denied: vm.exec vm=cpu/base atom=vm.exec",
        "allowed: vm.destroy vm=gpu/vm-1 via=created-by-caller",
        "plugin outcome: explicit create/exec/destroy grants enforced by membership",
    ] {
        assert!(
            stdout.contains(expected),
            "missing `{expected}` in:\n{stdout}"
        );
    }
}

#[test]
fn missing_component_reports_the_plugin_build_command() {
    let missing = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("missing-plugin.wasm");
    let output = Command::new(env!("CARGO_BIN_EXE_provisioning-host"))
        .arg(missing)
        .output()
        .expect("failed to run provisioning host");

    assert!(!output.status.success());
    let stderr = String::from_utf8(output.stderr).unwrap();
    assert!(stderr.contains("could not read plugin component"));
    assert!(stderr.contains(
        "nix develop -c cargo build --manifest-path examples/provisioning/plugin/Cargo.toml \
         --target wasm32-wasip2 --release"
    ));
}
