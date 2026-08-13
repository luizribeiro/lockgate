use lockgate::{InspectError, inspect};
use lockgate_schema::sections::{PLUGIN_METADATA_SECTION, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{NeedsDigest, NeedsManifest, PluginMetadata};
use wasm_encoder::{ComponentSection, CustomSection};
use wit_component::{ComponentEncoder, StringEncoding, dummy_module, embed_component_metadata};
use wit_parser::{ManglingAndAbi, Resolve};

const PLUGIN_ID: &str = "com.example.inspection";

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

fn with_custom_section(bytes: &[u8], name: &str, data: &[u8]) -> Vec<u8> {
    let mut output = bytes.to_vec();
    CustomSection {
        name: name.into(),
        data: data.into(),
    }
    .append_to_component(&mut output);
    output
}

fn sectioned_component(wit: &str) -> Vec<u8> {
    let metadata = PluginMetadata::new(PLUGIN_ID, "Inspection fixture", "1.0").unwrap();
    let bytes = with_custom_section(
        &component(wit),
        PLUGIN_METADATA_SECTION,
        &metadata.to_section_bytes().unwrap(),
    );
    with_custom_section(
        &bytes,
        PLUGIN_NEEDS_SECTION,
        &NeedsManifest::empty().to_section_bytes().unwrap(),
    )
}

#[test]
fn inspects_metadata_needs_digest_and_exports() {
    let bytes = sectioned_component(
        "package test:inspection; interface guest { value: func() -> u32; } world fixture { export guest; }",
    );

    let inspection = inspect(&bytes).unwrap();
    assert_eq!(inspection.metadata().id(), PLUGIN_ID);
    assert_eq!(inspection.needs(), &NeedsManifest::empty());
    assert_eq!(
        inspection.needs_digest(),
        NeedsDigest::compute(&NeedsManifest::empty()).unwrap()
    );
    assert_eq!(inspection.exported_interfaces(), ["test:inspection/guest"]);
}

#[test]
fn inspection_does_not_apply_wasmtime_admission_rules() {
    let bytes = sectioned_component(
        "package test:engine-rejected; interface guest { type failure = error-context; run: func() -> option<failure>; } world fixture { export guest; }",
    );

    let inspection = inspect(&bytes).unwrap();
    assert_eq!(
        inspection.exported_interfaces(),
        ["test:engine-rejected/guest"]
    );

    let mut config = wasmtime::Config::new();
    config
        .wasm_component_model(true)
        .wasm_component_model_async(true);
    let engine = wasmtime::Engine::new(&config).unwrap();
    assert!(wasmtime::component::Component::new(&engine, &bytes).is_err());
}

#[test]
fn inspection_reports_missing_sections_without_an_engine() {
    let bytes = component(
        "package test:missing; interface guest { value: func() -> u32; } world fixture { export guest; }",
    );
    let error = inspect(&bytes).unwrap_err();

    assert!(matches!(error, InspectError::MissingMetadata));
    assert!(
        error
            .to_string()
            .contains("missing required `lockgate:plugin`")
    );
}

#[test]
fn inspection_distinguishes_duplicate_metadata_from_missing_needs() {
    let base = component(
        "package test:section-errors; interface guest { value: func() -> u32; } world fixture { export guest; }",
    );
    let metadata = PluginMetadata::new(PLUGIN_ID, "Inspection fixture", "1.0").unwrap();
    let metadata_bytes = metadata.to_section_bytes().unwrap();
    let metadata_only = with_custom_section(&base, PLUGIN_METADATA_SECTION, &metadata_bytes);

    let error = inspect(&metadata_only).unwrap_err();
    assert!(matches!(error, InspectError::MissingNeeds));
    assert!(
        error
            .to_string()
            .contains("missing required `lockgate:needs`")
    );

    let duplicate = with_custom_section(&metadata_only, PLUGIN_METADATA_SECTION, &metadata_bytes);
    let error = inspect(&duplicate).unwrap_err();
    assert!(matches!(
        error,
        InspectError::DuplicateSection {
            name: PLUGIN_METADATA_SECTION
        }
    ));
    assert!(error.to_string().contains("duplicate `lockgate:plugin`"));
}
