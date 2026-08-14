#![allow(
    dead_code,
    reason = "each integration-test binary uses only its own fixture helper"
)]

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::LazyLock;

use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{NeedsManifest, PluginMetadata};
use wasm_encoder::reencode::{Error as ReencodeError, Reencode, ReencodeComponent};
use wasm_encoder::{ComponentSection, CustomSection};
use wasmparser::{Parser, Payload};
use wasmtime::component::Linker;

#[path = "../../src/exec/mod.rs"]
pub(crate) mod exec;
#[path = "../../src/jobs.rs"]
pub(crate) mod jobs;

use exec::{ExecLimits, StoreCtx};

pub(crate) const LIMITS: ExecLimits = ExecLimits {
    instantiation_fuel: 1_000_000,
    max_memory_bytes: 16 * 1024 * 1024,
};
pub(crate) const INVOCATION_FUEL: u64 = 1_000_000;

pub(crate) struct TestState;

pub(crate) static EXEC_FIXTURE: LazyLock<Vec<u8>> =
    LazyLock::new(|| build_fixture("exec-guest", "lockgate_exec_fixture.wasm"));
pub(crate) static DETACHED_JOBS_FIXTURE: LazyLock<Vec<u8>> =
    LazyLock::new(|| build_fixture("detached-jobs-guest", "lockgate_detached_jobs_fixture.wasm"));
pub(crate) static EXEC_CONCURRENT_FIXTURE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    build_fixture(
        "exec-concurrent-guest",
        "lockgate_exec_concurrent_fixture.wasm",
    )
});
pub(crate) static EXEC_FUEL_FIXTURE: LazyLock<Vec<u8>> =
    LazyLock::new(|| build_fixture("exec-fuel-guest", "lockgate_exec_fuel_fixture.wasm"));
pub(crate) static PUBLIC_FIXTURE: LazyLock<Vec<u8>> =
    LazyLock::new(|| build_fixture("public-guest", "lockgate_public_fixture.wasm"));
pub(crate) static HOST_BINDINGS_FIXTURE: LazyLock<Vec<u8>> =
    LazyLock::new(|| build_fixture("host-bindings-guest", "lockgate_host_bindings_fixture.wasm"));
pub(crate) static HOST_EXPORT_VALUES_FIXTURE: LazyLock<Vec<u8>> = LazyLock::new(|| {
    build_fixture(
        "host-export-values-guest",
        "lockgate_host_export_values_guest.wasm",
    )
});

pub(crate) fn wire_ready_wait(linker: &mut Linker<StoreCtx<TestState>>) -> wasmtime::Result<()> {
    linker
        .instance("test:exec/host")?
        .func_wrap_concurrent("wait", |_, (): ()| Box::pin(async { Ok(()) }))?;
    Ok(())
}

pub(crate) fn sectioned_fixture(bytes: &[u8], metadata: &PluginMetadata) -> Vec<u8> {
    let metadata_bytes = metadata.to_section_bytes().unwrap();
    let needs_bytes = NeedsManifest::empty().to_section_bytes().unwrap();

    // The public guest now self-embeds its identity. Keep pre-facade callers
    // byte-for-byte unchanged by accepting a match or replacing both sections.
    let bytes = match embedded_lockgate_sections(bytes) {
        (Some(embedded_metadata), Some(embedded_needs))
            if embedded_metadata == metadata_bytes && embedded_needs == needs_bytes =>
        {
            return bytes.to_vec();
        }
        (Some(_), Some(_)) => without_lockgate_sections(bytes),
        (None, None) => bytes.to_vec(),
        _ => panic!("fixture must embed both Lockgate sections or neither"),
    };
    let bytes = with_custom_section(&bytes, PLUGIN_METADATA_SECTION, &metadata_bytes);
    with_custom_section(&bytes, PLUGIN_NEEDS_SECTION, &needs_bytes)
}

pub(crate) fn embedded_lockgate_sections(bytes: &[u8]) -> (Option<&[u8]>, Option<&[u8]>) {
    #[derive(Clone, Copy, PartialEq, Eq)]
    enum ContainerKind {
        Module,
        Component,
    }

    let mut containers = Vec::new();
    let mut metadata = None;
    let mut needs = None;
    for payload in Parser::new(0).parse_all(bytes) {
        match payload.expect("fixture must be valid WebAssembly") {
            Payload::ModuleSection { .. } => containers.push(ContainerKind::Module),
            Payload::ComponentSection { .. } => containers.push(ContainerKind::Component),
            Payload::End(_) if !containers.is_empty() => {
                containers.pop();
            }
            Payload::CustomSection(section)
                if matches!(containers.as_slice(), [] | [ContainerKind::Module]) =>
            {
                match section.name() {
                    PLUGIN_METADATA_SECTION => {
                        assert!(metadata.replace(section.data()).is_none());
                    }
                    PLUGIN_NEEDS_SECTION => {
                        assert!(needs.replace(section.data()).is_none());
                    }
                    _ => {}
                }
            }
            _ => {}
        }
    }
    (metadata, needs)
}

pub(crate) fn with_custom_section(bytes: &[u8], name: &str, data: &[u8]) -> Vec<u8> {
    let mut output = bytes.to_vec();
    CustomSection {
        name: name.into(),
        data: data.into(),
    }
    .append_to_component(&mut output);
    output
}

struct StripLockgateSections;

impl Reencode for StripLockgateSections {
    type Error = core::convert::Infallible;

    fn parse_custom_section(
        &mut self,
        module: &mut wasm_encoder::Module,
        section: wasmparser::CustomSectionReader<'_>,
    ) -> Result<(), ReencodeError<Self::Error>> {
        if is_lockgate_section(section.name()) {
            return Ok(());
        }
        wasm_encoder::reencode::utils::parse_custom_section(self, module, section)
    }
}

impl ReencodeComponent for StripLockgateSections {
    fn parse_component_custom_section(
        &mut self,
        component: &mut wasm_encoder::Component,
        section: wasmparser::CustomSectionReader<'_>,
    ) -> Result<(), ReencodeError<Self::Error>> {
        if is_lockgate_section(section.name()) {
            return Ok(());
        }
        wasm_encoder::reencode::component_utils::parse_component_custom_section(
            self, component, section,
        )
    }
}

fn without_lockgate_sections(bytes: &[u8]) -> Vec<u8> {
    let mut component = wasm_encoder::Component::new();
    StripLockgateSections
        .parse_component(&mut component, Parser::new(0), bytes)
        .expect("fixture component must re-encode");
    component.finish()
}

fn is_lockgate_section(name: &str) -> bool {
    matches!(name, PLUGIN_METADATA_SECTION | PLUGIN_NEEDS_SECTION)
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
        .parent()
        .expect("fixture crate must belong to the fixture workspace")
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
