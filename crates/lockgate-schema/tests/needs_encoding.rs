use lockgate_schema::{
    AtomKey, NeedEntry, NeedsManifest, PLUGIN_NEEDS_SECTION, ScopeRef, encode_needs_manifest,
};

fn atom(value: &str) -> AtomKey {
    value.parse().unwrap()
}

#[test]
fn empty_manifest_has_a_canonical_section_payload() {
    assert_eq!(PLUGIN_NEEDS_SECTION, "lockgate:needs");
    assert_eq!(
        encode_needs_manifest(&NeedsManifest::empty()).unwrap(),
        br#"{"format":1,"optional":{},"reasons":{},"required":{}}"#
    );
}

#[test]
fn encoding_sorts_maps_and_uses_symbolic_scope_strings() {
    let manifest = NeedsManifest::new(
        vec![
            NeedEntry::flag(atom("notify.send")),
            NeedEntry::scoped(
                atom("fs.read"),
                vec![
                    ScopeRef::root("workspace")
                        .unwrap()
                        .join("generated/html")
                        .unwrap(),
                    ScopeRef::literal("current").unwrap(),
                ],
            )
            .unwrap(),
        ],
        vec![
            NeedEntry::scoped(
                atom("http.request"),
                vec![ScopeRef::setting("/endpoint").unwrap()],
            )
            .unwrap()
            .with_reason("deliver notifications")
            .unwrap(),
        ],
    )
    .unwrap();

    assert_eq!(
        encode_needs_manifest(&manifest).unwrap(),
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":true}}"#
    );
}
