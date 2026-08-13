use lockgate_schema::{
    NeedEntryError, NeedsManifestDecodeError, NeedsManifestValidationError, ScopeRefError,
    decode_needs_manifest,
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
        ("$workspace/generated\\u0000html", "must not contain NUL"),
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
