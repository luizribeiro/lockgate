#[path = "data/donor_vectors.rs"]
mod donor_vectors;

use lockgate_schema::sections::metadata::{DecodeError, EncodeError};
use lockgate_schema::sections::{MAX_SECTION_PAYLOAD_BYTES, PLUGIN_METADATA_SECTION};
use lockgate_schema::{PluginMetadata, PluginMetadataValidationError};

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
        let decoded = PluginMetadata::from_section_bytes(donor_bytes).unwrap();

        assert_eq!(decoded, expected);
        assert_eq!(expected.to_section_bytes().unwrap(), donor_bytes);
        assert_eq!(decoded.to_section_bytes().unwrap(), donor_bytes);
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

    let first_encoding = metadata.to_section_bytes().unwrap();
    let decoded = PluginMetadata::from_section_bytes(&first_encoding).unwrap();
    let second_encoding = decoded.to_section_bytes().unwrap();

    assert_eq!(decoded, metadata);
    assert_eq!(second_encoding, first_encoding);
    assert_eq!(
        first_encoding,
        br#"{"format":1,"id":"com.example.complete","name":"Complete","version":"2.4.1-beta.2+build.7","description":"Every field is populated","license":"MIT OR Apache-2.0","repository":"https://example.com/repository","homepage":"https://example.com"}"#
    );
}

#[test]
fn opaque_display_versions_round_trip() {
    for version in ["2024.08", "a1b2c3d"] {
        let metadata = PluginMetadata::new("plugin", "Plugin", version).unwrap();
        let encoded = metadata.to_section_bytes().unwrap();
        let decoded = PluginMetadata::from_section_bytes(&encoded).unwrap();

        assert_eq!(decoded.version(), version);
        assert_eq!(decoded, metadata);
    }
}

#[test]
fn rejects_disallowed_version_format_characters_at_their_byte_index() {
    let error = PluginMetadata::from_section_bytes(
        br#"{"format":1,"id":"plugin","name":"Plugin","version":"release\u202ecandidate"}"#,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        DecodeError::InvalidMetadata(PluginMetadataValidationError::DisallowedCharacter {
            field: lockgate_schema::PluginMetadataField::Version,
            byte_index: 7,
            character: '\u{202e}',
        })
    ));
    assert!(error.to_string().contains("format character at byte 7"));
}

#[test]
fn bounds_version_display_strings_by_utf8_bytes() {
    let exact = PluginMetadata::new("plugin", "Plugin", "x".repeat(128)).unwrap();
    let encoded = exact.to_section_bytes().unwrap();
    assert_eq!(PluginMetadata::from_section_bytes(&encoded).unwrap(), exact);

    assert_eq!(
        PluginMetadata::new("plugin", "Plugin", "x".repeat(129)).unwrap_err(),
        PluginMetadataValidationError::FieldTooLong {
            field: lockgate_schema::PluginMetadataField::Version,
            max_bytes: 128,
        }
    );
}

#[test]
fn rejects_truncated_payloads_without_panicking() {
    let error = PluginMetadata::from_section_bytes(br#"{"format":1,"id":"plugin""#).unwrap_err();

    assert!(matches!(error, DecodeError::InvalidJson(_)));
    assert!(error.to_string().contains("not valid schema JSON"));
}

#[test]
fn rejects_unknown_format_versions_with_the_version_in_the_error() {
    let error = PluginMetadata::from_section_bytes(
        br#"{"format":7,"id":"plugin","name":"Plugin","version":"1.0.0"}"#,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        DecodeError::InvalidMetadata(PluginMetadataValidationError::UnsupportedFormat { found: 7 })
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
            "plugin id contains whitespace at byte 3",
        ),
        (
            br#"{"format":1,"id":"plugin","name":" ","version":"1.0.0"}"#,
            "plugin name must not be empty",
        ),
        (
            br#"{"format":1,"id":"plugin","name":"Plugin","version":"release\u202ecandidate"}"#,
            "plugin version contains a format character at byte 7",
        ),
        (
            br#"{"format":1,"id":"plugin","name":"Plugin","version":"1.0.0","homepage":" "}"#,
            "plugin homepage must not be empty",
        ),
    ];

    for (payload, expected_message) in invalid_payloads {
        let error = PluginMetadata::from_section_bytes(payload).unwrap_err();

        assert!(
            error.to_string().contains(expected_message),
            "unexpected error for {payload:?}: {error}"
        );
    }
}

#[test]
fn rejects_non_utf8_and_non_json_payloads() {
    for payload in [&[0xff, 0xfe][..], &b"not JSON"[..]] {
        let error = PluginMetadata::from_section_bytes(payload).unwrap_err();

        assert!(matches!(error, DecodeError::InvalidJson(_)));
        assert!(error.to_string().contains("not valid schema JSON"));
    }
}

#[test]
fn rejects_wrong_typed_fields() {
    for payload in [
        &br#"{"format":"1","id":"plugin","name":"Plugin","version":"1.0.0"}"#[..],
        &br#"{"format":1,"id":123,"name":"Plugin","version":"1.0.0"}"#[..],
    ] {
        let error = PluginMetadata::from_section_bytes(payload).unwrap_err();

        assert!(matches!(error, DecodeError::InvalidJson(_)));
        assert!(error.to_string().contains("invalid type"));
    }
}

#[test]
fn rejects_fields_outside_the_wire_schema() {
    let error = PluginMetadata::from_section_bytes(
        br#"{"format":1,"id":"plugin","name":"Plugin","version":"1.0.0","publisher":"Example"}"#,
    )
    .unwrap_err();

    assert!(matches!(error, DecodeError::InvalidJson(_)));
    assert!(error.to_string().contains("unknown field `publisher`"));
}

#[test]
fn rejects_missing_required_fields() {
    let error =
        PluginMetadata::from_section_bytes(br#"{"format":1,"name":"Plugin","version":"1.0.0"}"#)
            .unwrap_err();

    assert!(matches!(error, DecodeError::InvalidJson(_)));
    assert!(error.to_string().contains("missing field `id`"));
}

#[test]
fn encoding_revalidates_builder_fields() {
    let metadata = PluginMetadata::new("plugin", "Plugin", "1.0.0")
        .unwrap()
        .with_description(" ");

    let error = metadata.to_section_bytes().unwrap_err();

    assert!(
        error
            .to_string()
            .contains("plugin description must not be empty")
    );
}

#[test]
fn validates_fields_before_encoding_and_enforces_decode_ceiling() {
    let over = PluginMetadata::new("plugin", "Plugin", "1.0.0")
        .unwrap()
        .with_description("x".repeat(2049));
    assert!(matches!(
        over.to_section_bytes().unwrap_err(),
        EncodeError::InvalidMetadata(PluginMetadataValidationError::FieldTooLong {
            field: lockgate_schema::PluginMetadataField::Description,
            max_bytes: 2048,
        })
    ));

    let oversized_garbage = vec![0; MAX_SECTION_PAYLOAD_BYTES + 1];
    assert!(matches!(
        PluginMetadata::from_section_bytes(&oversized_garbage).unwrap_err(),
        DecodeError::PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        } if actual_bytes == MAX_SECTION_PAYLOAD_BYTES + 1
    ));
}
