use lockgate_schema::NeedsManifest;
use lockgate_schema::needs::{MAX_ATOMS_PER_MANIFEST, MAX_SCOPE_VALUE_BYTES, MAX_SCOPES_PER_ENTRY};
use lockgate_schema::sections::needs::DecodeError;
use lockgate_schema::{
    NeedEntryError, NeedsManifestValidationError, ScopeRefEntryError, ScopeValueKind,
};

fn decode(required: &str, optional: &str) -> Result<(), DecodeError> {
    let bytes =
        format!(r#"{{"format":1,"optional":{optional},"reasons":{{}},"required":{required}}}"#);
    NeedsManifest::from_section_bytes(bytes.as_bytes()).map(|_| ())
}

#[test]
fn rejects_duplicate_atoms_within_and_across_lists() {
    let cases = [
        (
            r#"{"notify.send":true,"notify.send":true}"#,
            "{}",
            "required entry 1",
        ),
        (
            "{}",
            r#"{"notify.send":true,"notify.send":true}"#,
            "optional entry 1",
        ),
        (
            r#"{"notify.send":true}"#,
            r#"{"notify.send":true}"#,
            "optional entry 0",
        ),
    ];

    for (required, optional, duplicate) in cases {
        let error = decode(required, optional).unwrap_err();
        assert!(matches!(
            error,
            DecodeError::InvalidManifest(NeedsManifestValidationError::DuplicateAtom { .. })
        ));
        assert!(error.to_string().contains(duplicate));
        assert!(error.to_string().contains("first declared at"));
    }
}

#[test]
fn rejects_malformed_atom_keys_and_false_flags_at_their_entry_index() {
    for atom in ["notify", ".send", "notify.", "notify.send.extra"] {
        let error = decode(&format!(r#"{{"{atom}":true}}"#), "{}").unwrap_err();
        assert!(matches!(error, DecodeError::InvalidAtom { .. }));
        assert!(error.to_string().contains("required entry 0"));
    }

    let error = decode(r#"{"notify.send":false}"#, "{}").unwrap_err();
    assert!(matches!(error, DecodeError::FalseFlag { .. }));
    assert!(error.to_string().contains("declared flags must be true"));
}

#[test]
fn rejects_empty_scoped_entries() {
    let error = decode(r#"{"fs.read":[]}"#, "{}").unwrap_err();

    assert!(matches!(
        error,
        DecodeError::InvalidManifest(NeedsManifestValidationError::InvalidEntry {
            source: NeedEntryError::EmptyScopes,
            ..
        })
    ));
    assert!(error.to_string().contains("required entry 0 (`fs.read`)"));
}

#[test]
fn rejects_malformed_setting_and_root_references() {
    for (scope, expected) in [
        ("setting:endpoint", "RFC 6901 JSON Pointer"),
        ("$Workspace", "root name must start with a lowercase letter"),
    ] {
        let error = decode(&format!(r#"{{"fs.read":["{scope}"]}}"#), "{}").unwrap_err();
        assert!(matches!(error, DecodeError::InvalidScope { .. }));
        assert!(error.to_string().contains("required entry 0"));
        assert!(error.to_string().contains(expected));
    }
}

#[test]
fn rejects_empty_literal_scopes() {
    let error = decode(r#"{"fs.read":[""]}"#, "{}").unwrap_err();

    assert!(matches!(
        error,
        DecodeError::InvalidScope {
            source: ScopeRefEntryError::EmptyLiteral,
            ..
        }
    ));
    assert!(error.to_string().contains("required entry 0"));
    assert!(
        error
            .to_string()
            .contains("literal scope must not be empty")
    );
}

#[test]
fn rejects_every_unsafe_root_subpath_form() {
    let cases = [
        ("$workspace//absolute", "must be relative"),
        ("$workspace/", "segment 0 must not be empty"),
        ("$workspace/.", "must not be `.` or `..`"),
        ("$workspace/generated/..", "must not be `.` or `..`"),
        ("$workspace/generated//html", "segment 1 must not be empty"),
        ("$workspace/generated/", "segment 1 must not be empty"),
        (
            "$workspace/generated\\\\html",
            "must not contain backslashes",
        ),
        (
            "$workspace/generated\\u0000html",
            "control character at byte 9",
        ),
    ];

    for (scope, expected) in cases {
        let error = decode(&format!(r#"{{"fs.read":["{scope}"]}}"#), "{}").unwrap_err();
        assert!(matches!(error, DecodeError::InvalidScope { .. }));
        assert!(error.to_string().contains(expected), "{scope}: {error}");
    }
}

#[test]
fn rejects_control_and_format_characters_in_every_scope_value_kind() {
    let cases = [
        ("setting:/a\0b", ScopeValueKind::SettingPointer, '\0', 2),
        ("setting:/a\nb", ScopeValueKind::SettingPointer, '\n', 2),
        ("literal\0value", ScopeValueKind::Literal, '\0', 7),
        (
            "literal\u{2029}value",
            ScopeValueKind::Literal,
            '\u{2029}',
            7,
        ),
        (
            "$workspace/a\u{202e}b",
            ScopeValueKind::RootSubpathSegment,
            '\u{202e}',
            1,
        ),
        (
            "$workspace/a\u{7}b",
            ScopeValueKind::RootSubpathSegment,
            '\u{7}',
            1,
        ),
        (
            "$workspace/a\u{200b}b",
            ScopeValueKind::RootSubpathSegment,
            '\u{200b}',
            1,
        ),
    ];

    for (scope, kind, character, byte_index) in cases {
        let payload = serde_json::json!({
            "format": 1,
            "optional": {},
            "reasons": {},
            "required": { "fs.read": [scope] },
        });
        let error =
            NeedsManifest::from_section_bytes(&serde_json::to_vec(&payload).unwrap()).unwrap_err();
        assert!(matches!(
            error,
            DecodeError::InvalidScope {
                source: ScopeRefEntryError::DisallowedCharacter {
                    kind: found_kind,
                    byte_index: found_byte_index,
                    character: found_character,
                },
                ..
            } if found_kind == kind
                && found_character == character
                && found_byte_index == byte_index
        ));
        if scope.contains('\u{2029}') {
            assert!(error.to_string().contains("line separator at byte 7"));
        }
    }
}

#[test]
fn bounds_literal_and_setting_pointer_utf8_bytes() {
    for (scope, expected) in [
        ("é".repeat(MAX_SCOPE_VALUE_BYTES / 2), Ok(())),
        (
            "é".repeat(MAX_SCOPE_VALUE_BYTES / 2 + 1),
            Err(ScopeValueKind::Literal),
        ),
        (
            format!("setting:/{}", "a".repeat(MAX_SCOPE_VALUE_BYTES - 1)),
            Ok(()),
        ),
        (
            format!("setting:/{}", "a".repeat(MAX_SCOPE_VALUE_BYTES)),
            Err(ScopeValueKind::SettingPointer),
        ),
    ] {
        let payload = serde_json::json!({
            "format": 1,
            "optional": {},
            "reasons": {},
            "required": { "fs.read": [scope] },
        });
        let result =
            NeedsManifest::from_section_bytes(&serde_json::to_vec(&payload).unwrap()).map(|_| ());
        match expected {
            Ok(()) => result.unwrap(),
            Err(kind) => assert!(matches!(
                result.unwrap_err(),
                DecodeError::InvalidScope {
                    source: ScopeRefEntryError::ScopeValueTooLong {
                        kind: found_kind,
                        max_bytes: MAX_SCOPE_VALUE_BYTES,
                    },
                    ..
                } if found_kind == kind
            )),
        }
    }
}

#[test]
fn bounds_scopes_before_deduplication() {
    for (count, expected) in [
        (MAX_SCOPES_PER_ENTRY, Ok(())),
        (MAX_SCOPES_PER_ENTRY + 1, Err(())),
    ] {
        let payload = serde_json::json!({
            "format": 1,
            "optional": {},
            "reasons": {},
            "required": { "fs.read": vec!["same"; count] },
        });
        let result =
            NeedsManifest::from_section_bytes(&serde_json::to_vec(&payload).unwrap()).map(|_| ());
        match expected {
            Ok(()) => result.unwrap(),
            Err(()) => assert!(matches!(
                result.unwrap_err(),
                DecodeError::InvalidManifest(
                    NeedsManifestValidationError::InvalidEntry {
                        source: NeedEntryError::TooManyScopes {
                            found,
                            max: MAX_SCOPES_PER_ENTRY,
                        },
                        ..
                    }
                ) if found == MAX_SCOPES_PER_ENTRY + 1
            )),
        }
    }
}

#[test]
fn bounds_combined_atoms_before_deduplication() {
    let entries = |prefix: &str, count: usize| {
        (0..count)
            .map(|index| format!(r#""{prefix}{index}.op":true"#))
            .collect::<Vec<_>>()
            .join(",")
    };
    let half = MAX_ATOMS_PER_MANIFEST / 2;
    decode(
        &format!("{{{}}}", entries("required", half)),
        &format!("{{{}}}", entries("optional", half)),
    )
    .unwrap();

    let duplicate_entries = vec![r#""same.op":true"#; MAX_ATOMS_PER_MANIFEST + 1].join(",");
    let error = decode(&format!("{{{duplicate_entries}}}"), "{}").unwrap_err();
    assert!(matches!(
        error,
        DecodeError::InvalidManifest(
            NeedsManifestValidationError::TooManyAtoms {
                found,
                max: MAX_ATOMS_PER_MANIFEST,
            }
        ) if found == MAX_ATOMS_PER_MANIFEST + 1
    ));
}

#[test]
fn rejects_overlong_and_overdeep_root_subpaths() {
    for (subpath, expected) in [
        (
            "a".repeat(1025),
            ScopeRefEntryError::RootSubpathTooLong { max_bytes: 1024 },
        ),
        (
            ["a"; 65].join("/"),
            ScopeRefEntryError::RootSubpathTooDeep { max_segments: 64 },
        ),
    ] {
        let error =
            decode(&format!(r#"{{"fs.read":["$workspace/{subpath}"]}}"#), "{}").unwrap_err();
        assert!(matches!(
            error,
            DecodeError::InvalidScope { source, .. } if source == expected
        ));
    }
}
