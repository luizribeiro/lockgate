#[path = "data/donor_vectors.rs"]
mod donor_vectors;

use lockgate_schema::{
    MAX_SECTION_PAYLOAD_BYTES, PLUGIN_METADATA_SECTION, PluginMetadata, PluginMetadataDecodeError,
    PluginMetadataEncodeError, PluginMetadataValidationError, decode_plugin_metadata,
    encode_plugin_metadata,
};

#[test]
fn uses_the_normative_custom_section_name() {
    assert_eq!(PLUGIN_METADATA_SECTION, "lockgate:plugin");
}

#[test]
fn donor_payloads_decode_and_reencode_byte_identically() {
    let vectors = [
        (
            donor_vectors::SCHEMA_UNIT_TEST,
            PluginMetadata::new("com.example.greeter", "Greeter", "1.2.3")
                .unwrap()
                .with_description("Returns greetings"),
        ),
        (
            donor_vectors::SYNC_EXPORT_COMPONENT,
            PluginMetadata::new("sync-export", "Synchronous export fixture", "0.0.0")
                .unwrap()
                .with_description("Exercises synchronous host invocation"),
        ),
    ];

    for (donor_bytes, expected) in vectors {
        let decoded = decode_plugin_metadata(donor_bytes).unwrap();

        assert_eq!(decoded, expected);
        assert_eq!(encode_plugin_metadata(&expected).unwrap(), donor_bytes);
        assert_eq!(encode_plugin_metadata(&decoded).unwrap(), donor_bytes);
    }
}

#[test]
fn round_trip_is_stable_with_every_optional_field() {
    let metadata = PluginMetadata::new("com.example.complete", "Complete", "2.4.1-beta.2+build.7")
        .unwrap()
        .with_description("Every field is populated")
        .with_license("MIT OR Apache-2.0")
        .with_repository("https://example.com/repository")
        .with_homepage("https://example.com");

    let first_encoding = encode_plugin_metadata(&metadata).unwrap();
    let decoded = decode_plugin_metadata(&first_encoding).unwrap();
    let second_encoding = encode_plugin_metadata(&decoded).unwrap();

    assert_eq!(decoded, metadata);
    assert_eq!(second_encoding, first_encoding);
    assert_eq!(
        first_encoding,
        br#"{"format":1,"id":"com.example.complete","name":"Complete","version":"2.4.1-beta.2+build.7","description":"Every field is populated","license":"MIT OR Apache-2.0","repository":"https://example.com/repository","homepage":"https://example.com"}"#
    );
}

#[test]
fn rejects_truncated_payloads_without_panicking() {
    let error = decode_plugin_metadata(br#"{"format":1,"id":"plugin""#).unwrap_err();

    assert!(matches!(error, PluginMetadataDecodeError::InvalidJson(_)));
    assert!(error.to_string().contains("not valid schema JSON"));
}

#[test]
fn rejects_unknown_format_versions_with_the_version_in_the_error() {
    let error =
        decode_plugin_metadata(br#"{"format":7,"id":"plugin","name":"Plugin","version":"1.0.0"}"#)
            .unwrap_err();

    assert!(matches!(
        error,
        PluginMetadataDecodeError::InvalidMetadata(
            PluginMetadataValidationError::UnsupportedFormat { found: 7 }
        )
    ));
    assert_eq!(
        error.to_string(),
        "plugin metadata failed validation: unsupported plugin metadata format 7"
    );
}

#[test]
fn rejects_invalid_field_values_with_field_specific_errors() {
    let invalid_payloads: &[(&[u8], &str)] = &[
        (
            br#"{"format":1,"id":"","name":"Plugin","version":"1.0.0"}"#,
            "plugin id must not be empty",
        ),
        (
            br#"{"format":1,"id":"bad id","name":"Plugin","version":"1.0.0"}"#,
            "plugin id must not contain whitespace",
        ),
        (
            br#"{"format":1,"id":"plugin","name":" ","version":"1.0.0"}"#,
            "plugin name must not be empty",
        ),
        (
            br#"{"format":1,"id":"plugin","name":"Plugin","version":"latest"}"#,
            "plugin version is not valid SemVer",
        ),
        (
            br#"{"format":1,"id":"plugin","name":"Plugin","version":"1.0.0","homepage":" "}"#,
            "plugin homepage must not be empty when present",
        ),
    ];

    for (payload, expected_message) in invalid_payloads {
        let error = decode_plugin_metadata(payload).unwrap_err();

        assert!(
            error.to_string().contains(expected_message),
            "unexpected error for {payload:?}: {error}"
        );
    }
}

#[test]
fn rejects_non_utf8_and_non_json_payloads() {
    for payload in [&[0xff, 0xfe][..], &b"not JSON"[..]] {
        let error = decode_plugin_metadata(payload).unwrap_err();

        assert!(matches!(error, PluginMetadataDecodeError::InvalidJson(_)));
        assert!(error.to_string().contains("not valid schema JSON"));
    }
}

#[test]
fn rejects_wrong_typed_fields() {
    for payload in [
        &br#"{"format":"1","id":"plugin","name":"Plugin","version":"1.0.0"}"#[..],
        &br#"{"format":1,"id":123,"name":"Plugin","version":"1.0.0"}"#[..],
    ] {
        let error = decode_plugin_metadata(payload).unwrap_err();

        assert!(matches!(error, PluginMetadataDecodeError::InvalidJson(_)));
        assert!(error.to_string().contains("invalid type"));
    }
}

#[test]
fn rejects_fields_outside_the_wire_schema() {
    let error = decode_plugin_metadata(
        br#"{"format":1,"id":"plugin","name":"Plugin","version":"1.0.0","publisher":"Example"}"#,
    )
    .unwrap_err();

    assert!(matches!(error, PluginMetadataDecodeError::InvalidJson(_)));
    assert!(error.to_string().contains("unknown field `publisher`"));
}

#[test]
fn rejects_missing_required_fields() {
    let error =
        decode_plugin_metadata(br#"{"format":1,"name":"Plugin","version":"1.0.0"}"#).unwrap_err();

    assert!(matches!(error, PluginMetadataDecodeError::InvalidJson(_)));
    assert!(error.to_string().contains("missing field `id`"));
}

#[test]
fn encoding_revalidates_builder_fields() {
    let metadata = PluginMetadata::new("plugin", "Plugin", "1.0.0")
        .unwrap()
        .with_description(" ");

    let error = encode_plugin_metadata(&metadata).unwrap_err();

    assert!(
        error
            .to_string()
            .contains("plugin description must not be empty when present")
    );
}

#[test]
fn enforces_plugin_section_payload_ceiling_on_encode_and_decode() {
    let seed = PluginMetadata::new("plugin", "Plugin", "1.0.0")
        .unwrap()
        .with_description("x");
    let seed_size = encode_plugin_metadata(&seed).unwrap().len();
    let exact_description_size = 1 + MAX_SECTION_PAYLOAD_BYTES - seed_size;
    let exact = PluginMetadata::new("plugin", "Plugin", "1.0.0")
        .unwrap()
        .with_description("x".repeat(exact_description_size));
    let exact_payload = encode_plugin_metadata(&exact).unwrap();

    assert_eq!(exact_payload.len(), MAX_SECTION_PAYLOAD_BYTES);
    assert_eq!(decode_plugin_metadata(&exact_payload).unwrap(), exact);

    let over = PluginMetadata::new("plugin", "Plugin", "1.0.0")
        .unwrap()
        .with_description("x".repeat(exact_description_size + 1));
    assert!(matches!(
        encode_plugin_metadata(&over).unwrap_err(),
        PluginMetadataEncodeError::PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        } if actual_bytes == MAX_SECTION_PAYLOAD_BYTES + 1
    ));

    let oversized_garbage = vec![0; MAX_SECTION_PAYLOAD_BYTES + 1];
    assert!(matches!(
        decode_plugin_metadata(&oversized_garbage).unwrap_err(),
        PluginMetadataDecodeError::PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        } if actual_bytes == MAX_SECTION_PAYLOAD_BYTES + 1
    ));
}
