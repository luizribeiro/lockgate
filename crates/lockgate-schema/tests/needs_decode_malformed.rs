use lockgate_schema::sections::needs::{NeedsManifestDecodeError, decode_needs_manifest};

#[test]
fn malformed_payloads_return_errors_without_panicking() {
    let payloads: &[&[u8]] = &[br#"{"format":1,"optional":{}"#, b"not JSON", &[0xff, 0xfe]];

    for payload in payloads {
        let result = std::panic::catch_unwind(|| decode_needs_manifest(payload));

        assert!(result.is_ok(), "decoder panicked for {payload:?}");
        let error = result.unwrap().unwrap_err();
        assert!(matches!(error, NeedsManifestDecodeError::InvalidJson(_)));
        assert!(error.to_string().contains("not valid schema JSON"));
    }
}

#[test]
fn rejects_wrong_typed_manifest_fields() {
    let payloads: &[&[u8]] = &[
        br#"{"format":"1","optional":{},"reasons":{},"required":{}}"#,
        br#"{"format":1,"optional":[],"reasons":{},"required":{}}"#,
        br#"{"format":1,"optional":{},"reasons":[],"required":{}}"#,
        br#"{"format":1,"optional":{},"reasons":{},"required":[]}"#,
        br#"{"format":1,"optional":{},"reasons":{},"required":{"notify.send":1}}"#,
        br#"{"format":1,"optional":{},"reasons":{},"required":{"fs.read":[1]}}"#,
        br#"{"format":1,"optional":{},"reasons":{"notify.send":1},"required":{"notify.send":true}}"#,
    ];

    for payload in payloads {
        let error = decode_needs_manifest(payload).unwrap_err();

        assert!(matches!(error, NeedsManifestDecodeError::InvalidJson(_)));
    }
}

#[test]
fn rejects_missing_top_level_fields() {
    let payloads: &[&[u8]] = &[
        br#"{"optional":{},"reasons":{},"required":{}}"#,
        br#"{"format":1,"reasons":{},"required":{}}"#,
        br#"{"format":1,"optional":{},"required":{}}"#,
        br#"{"format":1,"optional":{},"reasons":{}}"#,
    ];

    for payload in payloads {
        let error = decode_needs_manifest(payload).unwrap_err();

        assert!(matches!(error, NeedsManifestDecodeError::InvalidJson(_)));
        assert!(error.to_string().contains("missing field"));
    }
}

#[test]
fn rejects_unknown_and_duplicate_top_level_fields() {
    let payloads: &[&[u8]] = &[
        br#"{"format":1,"optional":{},"reasons":{},"required":{},"limits":{}}"#,
        br#"{"format":1,"format":1,"optional":{},"reasons":{},"required":{}}"#,
    ];

    for payload in payloads {
        let error = decode_needs_manifest(payload).unwrap_err();

        assert!(matches!(error, NeedsManifestDecodeError::InvalidJson(_)));
    }
}
