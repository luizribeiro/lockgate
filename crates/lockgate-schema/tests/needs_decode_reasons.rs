use lockgate_schema::sections::needs::NeedsDecodeError;
use lockgate_schema::{NeedReasonError, NeedsManifest};

fn decode_reason(reason: &str) -> Result<(), NeedsDecodeError> {
    let bytes = serde_json::json!({
        "format": 1,
        "optional": {},
        "reasons": { "notify.send": reason },
        "required": { "notify.send": true },
    });
    NeedsManifest::from_section_bytes(&serde_json::to_vec(&bytes).unwrap()).map(|_| ())
}

#[test]
fn rejects_every_invalid_reason_shape() {
    let cases = [
        (" ".to_owned(), NeedReasonError::Empty),
        (
            "first\nsecond".to_owned(),
            NeedReasonError::DisallowedCharacter {
                byte_index: 5,
                character: '\n',
            },
        ),
        (
            "before\u{7}after".to_owned(),
            NeedReasonError::DisallowedCharacter {
                byte_index: 6,
                character: '\u{7}',
            },
        ),
        (
            "before\u{202e}after".to_owned(),
            NeedReasonError::DisallowedCharacter {
                byte_index: 6,
                character: '\u{202e}',
            },
        ),
        (
            "before\u{2028}after".to_owned(),
            NeedReasonError::DisallowedCharacter {
                byte_index: 6,
                character: '\u{2028}',
            },
        ),
        ("é".repeat(257), NeedReasonError::TooLong { max_bytes: 512 }),
    ];

    for (reason, expected) in cases {
        let error = decode_reason(&reason).unwrap_err();
        assert!(matches!(
            &error,
            NeedsDecodeError::InvalidReason {
                reason_index: 0,
                source,
                ..
            } if *source == expected
        ));
        assert!(error.to_string().contains("required entry 0"));
        if reason.contains('\u{2028}') {
            assert!(error.to_string().contains("line separator at byte 6"));
        }
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
    let manifest = NeedsManifest::from_section_bytes(&serde_json::to_vec(&bytes).unwrap()).unwrap();

    assert_eq!(manifest.required()[0].reason(), Some(reason.as_str()));
}

#[test]
fn rejects_duplicate_reason_keys() {
    let error = NeedsManifest::from_section_bytes(
        br#"{"format":1,"optional":{},"reasons":{"notify.send":"first","notify.send":"second"},"required":{"notify.send":true}}"#,
    )
    .unwrap_err();

    assert!(matches!(
        error,
        NeedsDecodeError::DuplicateReason {
            first_index: 0,
            duplicate_index: 1,
            ..
        }
    ));
}

#[test]
fn rejects_malformed_and_undeclared_reason_atoms() {
    let malformed = NeedsManifest::from_section_bytes(
        br#"{"format":1,"optional":{},"reasons":{"notify":"why"},"required":{"notify.send":true}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        malformed,
        NeedsDecodeError::InvalidReasonAtom {
            reason_index: 0,
            ..
        }
    ));

    let undeclared = NeedsManifest::from_section_bytes(
        br#"{"format":1,"optional":{},"reasons":{"http.request":"why"},"required":{"notify.send":true}}"#,
    )
    .unwrap_err();
    assert!(matches!(
        undeclared,
        NeedsDecodeError::UndeclaredReason {
            reason_index: 0,
            ..
        }
    ));
}

#[test]
fn valid_reasons_round_trip_in_canonical_atom_order() {
    let bytes = br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"notify.send":"send alerts","http.request":"reach endpoint"},"required":{"notify.send":true}}"#;
    let manifest = NeedsManifest::from_section_bytes(bytes).unwrap();

    assert_eq!(
        manifest.to_section_bytes().unwrap(),
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"reach endpoint","notify.send":"send alerts"},"required":{"notify.send":true}}"#
    );
}
