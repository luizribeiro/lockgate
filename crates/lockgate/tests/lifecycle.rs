mod common;

use lockgate::{AdmissionError, HostBuilder, LimitSet, PluginConfig, SymbolicRoots, inspect};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{NeedsDigest, NeedsManifest, PluginMetadata};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

const PLUGIN_ID: &str = "com.example.lifecycle";

fn component(wit: &str) -> Vec<u8> {
    let mut resolve = Resolve::new();
    let package = resolve.push_str("fixture.wit", wit).unwrap();
    let world = resolve.select_world(&[package], None).unwrap();
    let mut module = dummy_module(&resolve, world, ManglingAndAbi::Standard32);
    embed_component_metadata(&mut module, &resolve, world, StringEncoding::UTF8).unwrap();
    ComponentEncoder::default()
        .module(&module)
        .unwrap()
        .encode()
        .unwrap()
}

fn value_component() -> Vec<u8> {
    component(
        "package test:lifecycle; interface guest { value: func() -> u32; } world fixture { export guest; }",
    )
}

fn engine_rejected_component() -> Vec<u8> {
    component(
        "package test:engine-rejected; interface guest { type failure = error-context; run: func() -> option<failure>; } world fixture { export guest; }",
    )
}

fn metadata() -> PluginMetadata {
    PluginMetadata::new(PLUGIN_ID, "Lifecycle fixture", "1.0").unwrap()
}

fn well_formed_fixture() -> Vec<u8> {
    common::sectioned_fixture(&value_component(), &metadata())
}

#[tokio::test]
async fn prepares_a_well_formed_sectioned_fixture() {
    let bytes = well_formed_fixture();
    let free = inspect(&bytes).unwrap();
    let mut builder = HostBuilder::<()>::new().unwrap();
    let prepared = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap();

    assert_eq!(prepared.inspection(), &free);
    assert_eq!(free.metadata(), &metadata());
    assert_eq!(free.needs(), &NeedsManifest::empty());
    assert_eq!(
        free.needs_digest(),
        NeedsDigest::compute(&NeedsManifest::empty()).unwrap()
    );
    assert_eq!(free.exported_interfaces(), ["test:lifecycle/guest"]);
}

#[tokio::test]
async fn preparation_reports_each_pre_compilation_failure() {
    let base = value_component();
    let needs = NeedsManifest::empty().to_section_bytes().unwrap();
    let metadata_bytes = metadata().to_section_bytes().unwrap();
    let missing_metadata =
        common::with_custom_section(&base, PLUGIN_NEEDS_SECTION, needs.as_slice());
    let malformed_needs = common::with_custom_section(
        &common::with_custom_section(&base, PLUGIN_METADATA_SECTION, &metadata_bytes),
        PLUGIN_NEEDS_SECTION,
        b"not JSON",
    );
    let well_formed = well_formed_fixture();
    let mut builder = HostBuilder::<()>::new().unwrap();

    let error = builder
        .prepare(PLUGIN_ID, &missing_metadata, PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::MissingMetadata));
    assert!(
        error
            .to_string()
            .contains("missing required `lockgate:plugin`")
    );

    let error = builder
        .prepare("com.example.other", &well_formed, PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::PluginIdMismatch { .. }));
    assert!(error.to_string().contains(PLUGIN_ID));

    let error = builder
        .prepare(PLUGIN_ID, &malformed_needs, PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::InvalidNeeds(_)));
    assert!(error.to_string().contains("needs manifest is invalid"));

    let error = builder
        .prepare(PLUGIN_ID, b"not a component", PluginConfig::default())
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::InvalidComponent { .. }));
    assert!(
        error
            .to_string()
            .contains("not a valid WebAssembly component")
    );
}

#[tokio::test]
async fn forbidden_exports_keep_the_stable_teaching_error() {
    let bytes = common::sectioned_fixture(&engine_rejected_component(), &metadata());
    let mut builder = HostBuilder::<()>::new().unwrap();
    let error = builder
        .prepare(PLUGIN_ID, &bytes, PluginConfig::default())
        .await
        .unwrap_err();

    assert!(matches!(error, AdmissionError::UnsupportedExport(_)));
    assert_eq!(error.code(), Some("admission.unsupported-export"));
    assert!(
        error
            .to_string()
            .starts_with("[admission.unsupported-export] ")
    );
    assert!(error.to_string().contains("offending type `error-context`"));
}

#[tokio::test]
async fn non_default_config_is_rejected_instead_of_ignored() {
    let mut builder = HostBuilder::<()>::new().unwrap();
    let mut roots = SymbolicRoots::default();
    roots.insert("workspace", "/tmp/workspace");
    let cases = [
        (
            PluginConfig {
                settings: Some(serde_json::json!({ "enabled": true })),
                ..PluginConfig::default()
            },
            "settings",
        ),
        (
            PluginConfig {
                roots,
                ..PluginConfig::default()
            },
            "symbolic roots",
        ),
        (
            PluginConfig {
                limits: LimitSet::Constrained,
                ..PluginConfig::default()
            },
            "grant limits",
        ),
    ];

    for (config, field) in cases {
        let error = builder
            .prepare(PLUGIN_ID, &well_formed_fixture(), config)
            .await
            .unwrap_err();
        assert!(matches!(
            error,
            AdmissionError::ConfigFeatureUnavailable { field: found } if found == field
        ));
        assert!(
            error
                .to_string()
                .contains("not yet available in this build")
        );
    }
}

#[tokio::test]
async fn artifact_validation_precedes_temporary_config_rejection() {
    let mut builder = HostBuilder::<()>::new().unwrap();
    let config = PluginConfig {
        settings: Some(serde_json::json!({ "enabled": true })),
        ..PluginConfig::default()
    };

    let error = builder
        .prepare(PLUGIN_ID, &value_component(), config)
        .await
        .unwrap_err();
    assert!(matches!(error, AdmissionError::MissingMetadata));
}
