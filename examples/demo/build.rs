//! Builds the guest components and stages the narrated demo beneath Cargo's output directory.

use std::{env, fs, path::PathBuf, process::Command};

const IDS: [&str; 3] = ["greeter", "caller", "filereader"];

fn main() {
    let demo = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap());
    let output = PathBuf::from(env::var_os("OUT_DIR").unwrap());
    let guest_target = output.join("guest-target");
    let staged = output.join("demo");
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
    let guest_manifest = demo.join("components/Cargo.toml");

    println!("cargo:rerun-if-env-changed=WASI_SYSROOT");
    println!("cargo:rerun-if-env-changed=CARGO_TARGET_WASM32_WASIP2_LINKER");

    println!(
        "cargo:rerun-if-changed={}",
        demo.join("wit/worlds.wit").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        demo.join("wit/deps/demo-host/package.wit").display()
    );
    println!(
        "cargo:rerun-if-changed={}",
        demo.join("wit/deps/demo-greeter/package.wit").display()
    );
    println!("cargo:rerun-if-changed={}", guest_manifest.display());
    println!(
        "cargo:rerun-if-changed={}",
        demo.join("components/Cargo.lock").display()
    );
    for path in [
        demo.join("../../lockgate-plugin/Cargo.toml"),
        demo.join("../../lockgate-plugin/src/lib.rs"),
        demo.join("../../lockgate-plugin-macros/Cargo.toml"),
        demo.join("../../lockgate-plugin-macros/src/lib.rs"),
    ] {
        println!("cargo:rerun-if-changed={}", path.display());
    }
    for id in IDS {
        let dir = demo.join("components").join(id);
        let manifest = dir.join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("src/lib.rs").display()
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
        .expect("failed to run Cargo for demo guests");
    assert!(status.success(), "failed to build demo guests");

    for id in IDS {
        let destination = staged.join("components").join(id);
        fs::create_dir_all(&destination).unwrap();
        fs::copy(
            guest_target
                .join("wasm32-wasip2/debug")
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
