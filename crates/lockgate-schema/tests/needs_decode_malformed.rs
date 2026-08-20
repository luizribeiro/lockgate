use lockgate_schema::NeedsManifest;
use lockgate_schema::sections::needs::NeedsDecodeError;

#[test]
fn malformed_payloads_return_errors_without_panicking() {
    let payloads: &[&[u8]] = &[br#"{"format":1,"optional":{}"#, b"not JSON", &[0xff, 0xfe]];

    for payload in payloads {
        let result = std::panic::catch_unwind(|| NeedsManifest::from_section_bytes(payload));

        assert!(result.is_ok(), "decoder panicked for {payload:?}");
        let error = result.unwrap().unwrap_err();
        assert!(matches!(error, NeedsDecodeError::InvalidJson(_)));
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
        let error = NeedsManifest::from_section_bytes(payload).unwrap_err();

        assert!(matches!(error, NeedsDecodeError::InvalidJson(_)));
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
        let error = NeedsManifest::from_section_bytes(payload).unwrap_err();

        assert!(matches!(error, NeedsDecodeError::InvalidJson(_)));
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
        let error = NeedsManifest::from_section_bytes(payload).unwrap_err();

        assert!(matches!(error, NeedsDecodeError::InvalidJson(_)));
    }
}
