use lockgate_schema::{
    NeedsManifest, NeedsManifestDecodeError, NeedsManifestValidationError, decode_needs_manifest,
    encode_needs_manifest,
};

#[test]
fn canonical_empty_manifest_round_trips() {
    let bytes = encode_needs_manifest(&NeedsManifest::empty()).unwrap();
    let decoded = decode_needs_manifest(&bytes).unwrap();

    assert_eq!(decoded, NeedsManifest::empty());
    assert_eq!(encode_needs_manifest(&decoded).unwrap(), bytes);
}

#[test]
fn decoding_normalizes_map_and_scope_set_order() {
    let decoded = decode_needs_manifest(
        br#"{"required":{"notify.send":true,"fs.read":["current","$workspace","current"]},"reasons":{},"optional":{},"format":1}"#,
    )
    .unwrap();

    assert_eq!(
        encode_needs_manifest(&decoded).unwrap(),
        br#"{"format":1,"optional":{},"reasons":{},"required":{"fs.read":["$workspace","current"],"notify.send":true}}"#
    );
}

#[test]
fn decoding_rejects_unknown_format_versions() {
    let error = decode_needs_manifest(br#"{"format":2,"optional":{},"reasons":{},"required":{}}"#)
        .unwrap_err();

    assert!(matches!(
        error,
        NeedsManifestDecodeError::InvalidManifest(
            NeedsManifestValidationError::UnsupportedFormat { found: 2 }
        )
    ));
    assert!(
        error
            .to_string()
            .contains("unsupported needs manifest format 2")
    );
}
