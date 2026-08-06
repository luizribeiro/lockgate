use std::{env, path::PathBuf, process::Command};

fn main() {
    let root = PathBuf::from(env::var_os("CARGO_MANIFEST_DIR").unwrap()).join("..");
    println!("cargo:rerun-if-changed={}", root.join("wit").display());
    for id in ["greeter", "caller", "filereader", "naughty", "dynamic"] {
        let dir = root.join("plugins").join(id);
        let manifest = dir.join("Cargo.toml");
        println!("cargo:rerun-if-changed={}", manifest.display());
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("src/lib.rs").display()
        );
        println!(
            "cargo:rerun-if-changed={}",
            dir.join("wit/world.wit").display()
        );
        let status = Command::new("cargo")
            .args(["component", "build", "--quiet", "--manifest-path"])
            .arg(&manifest)
            .status()
            .expect("cargo-component must be installed (enter the Nix dev shell)");
        assert!(status.success(), "failed to build plugin {id}");
        std::fs::copy(
            dir.join("target/wasm32-wasip1/debug")
                .join(format!("{id}.wasm")),
            dir.join(format!("{id}.wasm")),
        )
        .expect("failed to place component beside its manifest");
    }
}
