use lockgate_schema::{
    NeedReasonError, NeedsManifestDecodeError, decode_needs_manifest, encode_needs_manifest,
};

fn decode_reason(reason: &str) -> Result<(), NeedsManifestDecodeError> {
    let bytes = serde_json::json!({
        "format": 1,
        "optional": {},
        "reasons": { "notify.send": reason },
        "required": { "notify.send": true },
    });
    decode_needs_manifest(&serde_json::to_vec(&bytes).unwrap()).map(|_| ())
}

#[test]
fn rejects_every_invalid_reason_shape() {
    let cases = [
        (" ".to_owned(), NeedReasonError::Empty),
        ("first\nsecond".to_owned(), NeedReasonError::MultipleLines),
        (
            "before\u{7}after".to_owned(),
            NeedReasonError::ControlCharacter { byte_index: 6 },
        ),
        ("é".repeat(257), NeedReasonError::TooLong { max_bytes: 512 }),
    ];

    for (reason, expected) in cases {
        let error = decode_reason(&reason).unwrap_err();
        assert!(matches!(
            &error,
            NeedsManifestDecodeError::InvalidReason {
                reason_index: 0,
                source,
                ..
            } if *source == expected
        ));
        assert!(error.to_string().contains("required entry 0"));
    }
}

#[test]
fn accepts_a_reason_at_the_exact_utf8_byte_limit() {
    let reason = "é".repeat(256);
    let bytes = serde_json::json!({
        "format": 1,
        "optional": {},
        "reasons": { "notify.send": reason },
        "required": { "notify.send": true },
    });
    let manifest = decode_needs_manifest(&serde_json::to_vec(&bytes).unwrap()).unwrap();

    assert_eq!(manifest.required()[0].reason(), Some(reason.as_str()));
}

#[test]
fn rejects_duplicate_reason_keys() {
    let error = decode_needs_manifest(
        br#"{"format":1,"optional":{},"reasons":{"notify.send":"first","notify.send":"second"},"required":{"notify.send":true}}"#,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        NeedsManifestDecodeError::DuplicateReason {
            first_index: 0,
            duplicate_index: 1,
            ..
        }
    ));
}

#[test]
fn rejects_malformed_and_undeclared_reason_atoms() {
    let malformed = decode_needs_manifest(
        br#"{"format":1,"optional":{},"reasons":{"notify":"why"},"required":{"notify.send":true}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        malformed,
        NeedsManifestDecodeError::InvalidReasonAtom {
            reason_index: 0,
            ..
        }
    ));

    let undeclared = decode_needs_manifest(
        br#"{"format":1,"optional":{},"reasons":{"http.request":"why"},"required":{"notify.send":true}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        undeclared,
        NeedsManifestDecodeError::UndeclaredReason {
            reason_index: 0,
            ..
        }
    ));
}

#[test]
fn valid_reasons_round_trip_in_canonical_atom_order() {
    let bytes = br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"notify.send":"send alerts","http.request":"reach endpoint"},"required":{"notify.send":true}}"#;
    let manifest = decode_needs_manifest(bytes).unwrap();

    assert_eq!(
        encode_needs_manifest(&manifest).unwrap(),
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"reach endpoint","notify.send":"send alerts"},"required":{"notify.send":true}}"#
    );
}
