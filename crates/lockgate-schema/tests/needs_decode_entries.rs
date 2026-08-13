use lockgate_schema::{
    MAX_ATOMS_PER_MANIFEST, MAX_SCOPE_VALUE_BYTES, MAX_SCOPES_PER_ENTRY, NeedEntryError,
    NeedsManifestDecodeError, NeedsManifestValidationError, ScopeCharacterKind, ScopeRefError,
    ScopeValueKind, decode_needs_manifest,
};

fn decode(required: &str, optional: &str) -> Result<(), NeedsManifestDecodeError> {
    let bytes =
        format!(r#"{{"format":1,"optional":{optional},"reasons":{{}},"required":{required}}}"#);
    decode_needs_manifest(bytes.as_bytes()).map(|_| ())
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
            NeedsManifestDecodeError::InvalidManifest(
                NeedsManifestValidationError::DuplicateAtom { .. }
            )
        ));
        assert!(error.to_string().contains(duplicate));
        assert!(error.to_string().contains("first declared at"));
    }
}

#[test]
fn rejects_malformed_atom_keys_and_false_flags_at_their_entry_index() {
    for atom in ["notify", ".send", "notify.", "notify.send.extra"] {
        let error = decode(&format!(r#"{{"{atom}":true}}"#), "{}").unwrap_err();
        assert!(matches!(
            error,
            NeedsManifestDecodeError::InvalidAtom { .. }
        ));
        assert!(error.to_string().contains("required entry 0"));
    }

    let error = decode(r#"{"notify.send":false}"#, "{}").unwrap_err();
    assert!(matches!(error, NeedsManifestDecodeError::FalseFlag { .. }));
    assert!(error.to_string().contains("declared flags must be true"));
}

#[test]
fn rejects_empty_scoped_entries() {
    let error = decode(r#"{"fs.read":[]}"#, "{}").unwrap_err();

    assert!(matches!(
        error,
        NeedsManifestDecodeError::InvalidManifest(NeedsManifestValidationError::InvalidEntry {
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
        assert!(matches!(
            error,
            NeedsManifestDecodeError::InvalidScope { .. }
        ));
        assert!(error.to_string().contains("required entry 0"));
        assert!(error.to_string().contains(expected));
    }
}

#[test]
fn rejects_empty_literal_scopes() {
    let error = decode(r#"{"fs.read":[""]}"#, "{}").unwrap_err();

    assert!(matches!(
        error,
        NeedsManifestDecodeError::InvalidScope {
            source: ScopeRefError::EmptyLiteral,
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
            "control (Cc) character at byte 9",
        ),
    ];

    for (scope, expected) in cases {
        let error = decode(&format!(r#"{{"fs.read":["{scope}"]}}"#), "{}").unwrap_err();
        assert!(matches!(
            error,
            NeedsManifestDecodeError::InvalidScope { .. }
        ));
        assert!(error.to_string().contains(expected), "{scope}: {error}");
    }
}

#[test]
fn rejects_control_and_format_characters_in_every_scope_value_kind() {
    let cases = [
        (
            "setting:/a\0b",
            ScopeValueKind::SettingPointer,
            ScopeCharacterKind::Control,
            2,
        ),
        (
            "setting:/a\nb",
            ScopeValueKind::SettingPointer,
            ScopeCharacterKind::Control,
            2,
        ),
        (
            "literal\0value",
            ScopeValueKind::Literal,
            ScopeCharacterKind::Control,
            7,
        ),
        (
            "$workspace/a\u{202e}b",
            ScopeValueKind::RootSubpathSegment,
            ScopeCharacterKind::Format,
            1,
        ),
        (
            "$workspace/a\u{7}b",
            ScopeValueKind::RootSubpathSegment,
            ScopeCharacterKind::Control,
            1,
        ),
        (
            "$workspace/a\u{200b}b",
            ScopeValueKind::RootSubpathSegment,
            ScopeCharacterKind::Format,
            1,
        ),
    ];

    for (scope, kind, character_kind, byte_index) in cases {
        let payload = serde_json::json!({
            "format": 1,
            "optional": {},
            "reasons": {},
            "required": { "fs.read": [scope] },
        });
        let error = decode_needs_manifest(&serde_json::to_vec(&payload).unwrap()).unwrap_err();
        assert!(matches!(
            error,
            NeedsManifestDecodeError::InvalidScope {
                source: ScopeRefError::DisallowedCharacter {
                    kind: found_kind,
                    character_kind: found_character_kind,
                    byte_index: found_byte_index,
                },
                ..
            } if found_kind == kind
                && found_character_kind == character_kind
                && found_byte_index == byte_index
        ));
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
        let result = decode_needs_manifest(&serde_json::to_vec(&payload).unwrap()).map(|_| ());
        match expected {
            Ok(()) => result.unwrap(),
            Err(kind) => assert!(matches!(
                result.unwrap_err(),
                NeedsManifestDecodeError::InvalidScope {
                    source: ScopeRefError::ScopeValueTooLong {
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
        let result = decode_needs_manifest(&serde_json::to_vec(&payload).unwrap()).map(|_| ());
        match expected {
            Ok(()) => result.unwrap(),
            Err(()) => assert!(matches!(
                result.unwrap_err(),
                NeedsManifestDecodeError::InvalidManifest(
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
        NeedsManifestDecodeError::InvalidManifest(
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
            ScopeRefError::RootSubpathTooLong { max_bytes: 1024 },
        ),
        (
            ["a"; 65].join("/"),
            ScopeRefError::RootSubpathTooDeep { max_segments: 64 },
        ),
    ] {
        let error =
            decode(&format!(r#"{{"fs.read":["$workspace/{subpath}"]}}"#), "{}").unwrap_err();
        assert!(matches!(
            error,
            NeedsManifestDecodeError::InvalidScope { source, .. } if source == expected
        ));
    }
}
