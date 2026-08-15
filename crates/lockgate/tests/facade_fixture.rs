mod common;

use lockgate::inspect;
use lockgate_schema::{NeedsDigest, NeedsManifest, PluginMetadata};

#[test]
fn facade_fixture_sections_match_host_encoding_byte_for_byte() {
    let expected_metadata = expected_metadata();
    let expected_needs = NeedsManifest::empty();
    let (metadata, needs) = common::embedded_lockgate_sections(&common::PUBLIC_FIXTURE);

    assert_eq!(
        metadata.expect("facade fixture must embed plugin metadata"),
        expected_metadata.to_section_bytes().unwrap()
    );
    assert_eq!(
        needs.expect("facade fixture must embed permission needs"),
        expected_needs.to_section_bytes().unwrap()
    );
}

#[test]
fn facade_fixture_inspection_preserves_its_declarations() {
    let expected_metadata = expected_metadata();
    let expected_needs = NeedsManifest::empty();
    let inspection = inspect(&common::PUBLIC_FIXTURE).unwrap();

    assert_eq!(inspection.metadata(), &expected_metadata);
    assert_eq!(inspection.needs(), &expected_needs);
    assert_eq!(
        inspection.needs_digest(),
        NeedsDigest::compute(&expected_needs).unwrap()
    );
}

#[test]
fn facade_fixture_resolves_explicit_and_absent_metadata_sources() {
    let inspection = inspect(&common::PUBLIC_FIXTURE).unwrap();

    assert_eq!(inspection.metadata().name(), "Greeter");
    assert_eq!(inspection.metadata().repository(), None);
}

fn expected_metadata() -> PluginMetadata {
    PluginMetadata::new("greeter", "Greeter", "1.0")
        .unwrap()
        .with_description("Public facade fixture")
        .with_license("MIT OR Apache-2.0")
}
