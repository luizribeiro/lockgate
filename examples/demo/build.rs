//! Builds the guest components and stages the narrated demo beneath Cargo's output directory.

use std::{env, fs, path::PathBuf, process::Command};

const IDS: [&str; 5] = ["greeter", "caller", "filereader", "naughty", "dynamic"];

fn main() {
    let demo = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let repository = demo.join("../..").canonicalize().unwrap();
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let component_target = output.join("component-target");
    let staged = output.join("demo");
    let cargo = env::var_os("CARGO").unwrap();

    println!(
        "cargo:rerun-if-changed={}",
        repository.join("wit/core.wit").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        demo.join("packages/demo-greeter-0.1.0.wasm").display()
    );
    for id in IDS {
        let dir = demo.join("components").join(id);
        let manifest = dir.join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("Cargo.lock").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("plugin.toml").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("src/lib.rs").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("wit/world.wit").display()
        );
        let status = Command::new(&cargo)
            .args([
                "component",
                "build",
                "--quiet",
                "--locked",
                "--manifest-path",
            ])
            .arg(&manifest)
            .arg("--target-dir")
            .arg(&component_target)
            .status()
            .expect("cargo-component must be installed (enter the Nix dev shell)");
        assert!(status.success(), "failed to build plugin {id}");

        let destination = staged.join("components").join(id);
        fs::create_dir_all(&destination).unwrap();
        fs::copy(dir.join("plugin.toml"), destination.join("plugin.toml")).unwrap();
        fs::copy(
            component_target
                .join("wasm32-wasip1/debug")
                .join(format!("{id}.wasm")),
            destination.join(format!("{id}.wasm")),
        )
        .unwrap();
    }

    let sandbox = staged.join("sandbox/shared");
    fs::create_dir_all(&sandbox).unwrap();
    let allowed = demo.join("sandbox/shared/allowed.txt");
    println!("cargo:rerun-if-changed={}", allowed.display());
    fs::copy(allowed, sandbox.join("allowed.txt")).unwrap();
    println!("cargo:rustc-env=LOCKGATE_DEMO_ROOT={}", staged.display());
}
