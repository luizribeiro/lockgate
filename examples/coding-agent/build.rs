//! Builds the example provider and tool components and stages their artifacts.

use std::{env, fs, path::PathBuf, process::Command};

const PLUGINS: [&str; 3] = ["inkling-provider", "list-files", "read-file"];

fn main() {
    let example = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let guest_target = output.join("guest-target");
    let staged = output.join("coding-agent");
    let cargo = env::var_os("CARGO").unwrap();
    let wasi_sysroot = PathBuf::from(
        env::var_os("WASI_SYSROOT")
            .expect("WASI_SYSROOT must point to a sysroot containing wasm32-wasip2 libc"),
    );
    let wasi_libdir = wasi_sysroot.join("lib/wasm32-wasip2");
    assert!(
        wasi_libdir.join("libc.a").is_file(),
        "WASI_SYSROOT must contain wasm32-wasip2 libc at {}",
        wasi_libdir.display()
    );
    let guest_manifest = example.join("components/Cargo.toml");

    println!("cargo:rerun-if-env-changed=WASI_SYSROOT");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_WASM32_WASIP2_LINKER");
    for path in [
        example.join("wit/worlds.wit"),
        example.join("wit/deps/coding-agent/package.wit"),
        guest_manifest.clone(),
        example.join("components/Cargo.lock"),
        example.join("../../lockgate-plugin/Cargo.toml"),
        example.join("../../lockgate-plugin/src/lib.rs"),
        example.join("../../lockgate-plugin-macros/Cargo.toml"),
        example.join("../../lockgate-plugin-macros/src/lib.rs"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for plugin in PLUGINS {
        println!(
            "cargo:rerun-if-changed={}",
            example
                .join("components")
                .join(plugin)
                .join("src/lib.rs")
                .display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            example
                .join("components")
                .join(plugin)
                .join("Cargo.toml")
                .display()
        );
    }

    let status = Command::new(&cargo)
        .env_remove("CARGO_ENCODED_RUSTFLAGS")
        .env("RUSTFLAGS", format!("-Lnative={}", wasi_libdir.display()))
        .args([
            "build",
            "--quiet",
            "--locked",
            "-Zbuild-std=std,panic_abort",
            "--workspace",
            "--manifest-path",
        ])
        .arg(&guest_manifest)
        .args(["--target", "wasm32-wasip2"])
        .arg("--target-dir")
        .arg(&guest_target)
        .status()
        .expect("failed to build coding-agent plugins");
    assert!(status.success(), "failed to build coding-agent plugins");

    for plugin in PLUGINS {
        let crate_name = plugin.replace('-', "_");
        let destination = staged.join("plugins").join(plugin);
        fs::create_dir_all(&destination).unwrap();
        fs::copy(
            guest_target
                .join("wasm32-wasip2/debug")
                .join(format!("{crate_name}.wasm")),
            destination.join(format!("{plugin}.wasm")),
        )
        .unwrap();
    }
    println!(
        "cargo:rustc-env=LOCKGATE_CODING_AGENT_ROOT={}",
        staged.display()
    );
}
