use lockgate_schema::sections::needs::NeedsDecodeError;
use lockgate_schema::{NeedsManifest, NeedsManifestValidationError};

#[test]
fn canonical_empty_manifest_round_trips() {
    let bytes = NeedsManifest::empty().to_section_bytes().unwrap();
    let decoded = NeedsManifest::from_section_bytes(&bytes).unwrap();

    assert_eq!(decoded, NeedsManifest::empty());
    assert_eq!(decoded.to_section_bytes().unwrap(), bytes);
}

#[test]
fn decoding_normalizes_map_and_scope_set_order() {
    let decoded = NeedsManifest::from_section_bytes(
        br#"{"required":{"notify.send":true,"fs.read":["current","$workspace","current"]},"reasons":{},"optional":{},"format":1}"#,
    )
    .unwrap();

    assert_eq!(
        decoded.to_section_bytes().unwrap(),
        br#"{"format":1,"optional":{},"reasons":{},"required":{"fs.read":["$workspace","current"],"notify.send":true}}"#
    );
}

#[test]
fn decoding_rejects_unknown_format_versions() {
    let error = NeedsManifest::from_section_bytes(
        br#"{"format":2,"optional":{},"reasons":{},"required":{}}"#,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        NeedsDecodeError::InvalidManifest(NeedsManifestValidationError::UnsupportedFormat {
            found: 2,
        })
    ));
    assert!(
        error
            .to_string()
            .contains("unsupported needs manifest format 2")
    );
}
