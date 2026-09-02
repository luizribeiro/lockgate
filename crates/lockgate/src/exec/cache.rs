use std::{
    fs::{self, File, OpenOptions},
    hash::{Hash, Hasher},
    io::Write,
    path::PathBuf,
    process,
    sync::atomic::{AtomicU64, Ordering},
};

use lockgate_schema::hex_encode;
use sha2::{Digest, Sha256};
use wasmtime::{Engine, Precompiled, component::Component};

static NEXT_TEMP_FILE: AtomicU64 = AtomicU64::new(1);

pub(crate) struct CompiledComponentCache {
    directory: PathBuf,
}

impl CompiledComponentCache {
    pub(crate) fn new(directory: PathBuf) -> Self {
        Self { directory }
    }

    pub(crate) fn load(&self, engine: &Engine, component_digest: &[u8; 32]) -> Option<Component> {
        let path = self.entry_path(engine, component_digest);
        if !path.is_file()
            || Engine::detect_precompiled_file(&path).ok()? != Some(Precompiled::Component)
        {
            return None;
        }

        // SAFETY: `HostBuilder::compiled_cache` requires an embedder-owned directory
        // that untrusted plugins cannot write. Detection above establishes that the
        // file is a Wasmtime component artifact; deserialization performs the full
        // engine/version compatibility check. Any error is treated as a cache miss.
        unsafe { Component::deserialize_file(engine, path) }.ok()
    }

    pub(crate) fn store(
        &self,
        engine: &Engine,
        component_digest: &[u8; 32],
        component: &Component,
    ) -> bool {
        let Ok(serialized) = component.serialize() else {
            return false;
        };
        if fs::create_dir_all(&self.directory).is_err() {
            return false;
        }

        let path = self.entry_path(engine, component_digest);
        let Some((temp_path, mut temp)) = self.create_temp() else {
            return false;
        };
        let result = temp.write_all(&serialized).and_then(|()| temp.sync_all());
        drop(temp);
        let result = result.and_then(|()| fs::rename(&temp_path, &path));
        if result.is_err() {
            let _ = fs::remove_file(temp_path);
            return false;
        }
        true
    }

    fn entry_path(&self, engine: &Engine, component_digest: &[u8; 32]) -> PathBuf {
        self.directory
            .join(format!("{}.cwasm", cache_key(engine, component_digest)))
    }

    fn create_temp(&self) -> Option<(PathBuf, File)> {
        for _ in 0..16 {
            let sequence = NEXT_TEMP_FILE.fetch_add(1, Ordering::Relaxed);
            let path = self
                .directory
                .join(format!(".{}.{}.tmp", process::id(), sequence));
            match OpenOptions::new().write(true).create_new(true).open(&path) {
                Ok(file) => return Some((path, file)),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => return None,
            }
        }
        None
    }
}

pub(crate) fn cache_key(engine: &Engine, component_digest: &[u8; 32]) -> String {
    let mut hasher = Sha256Hasher::default();
    hasher.write(b"lockgate compiled component cache v1\0");
    // Wasmtime includes its module-version strategy in this fingerprint. The
    // pinned default is `WasmtimeVersion`, so the crate version is covered too.
    engine.precompile_compatibility_hash().hash(&mut hasher);
    hasher.write(component_digest);
    hex_encode(&hasher.finalize())
}

#[derive(Clone, Default)]
struct Sha256Hasher(Sha256);

impl Sha256Hasher {
    fn finalize(self) -> [u8; 32] {
        self.0.finalize().into()
    }
}

impl Hasher for Sha256Hasher {
    fn finish(&self) -> u64 {
        let digest: [u8; 32] = self.0.clone().finalize().into();
        u64::from_le_bytes(digest[..8].try_into().expect("SHA-256 has eight bytes"))
    }

    fn write(&mut self, bytes: &[u8]) {
        self.0.update(bytes);
    }
}
