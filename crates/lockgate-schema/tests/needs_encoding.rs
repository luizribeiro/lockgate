use lockgate_schema::{
    AtomKey, MAX_SCOPE_VALUE_BYTES, MAX_SCOPES_PER_ENTRY, MAX_SECTION_PAYLOAD_BYTES, NeedEntry,
    NeedsManifest, NeedsManifestDecodeError, NeedsManifestEncodeError, PLUGIN_NEEDS_SECTION,
    ScopeRef, decode_needs_manifest, encode_needs_manifest,
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

    let encoded = encode_needs_manifest(&manifest).unwrap();
    assert_eq!(
        encoded,
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":true}}"#
    );
    let decoded = decode_needs_manifest(&encoded).unwrap();
    assert_eq!(decoded, manifest);
    assert_eq!(encode_needs_manifest(&decoded).unwrap(), encoded);
}

fn large_manifest(scope_lengths: &[usize]) -> NeedsManifest {
    let entries = scope_lengths
        .chunks(MAX_SCOPES_PER_ENTRY)
        .enumerate()
        .map(|(entry_index, lengths)| {
            let scopes = lengths
                .iter()
                .enumerate()
                .map(|(scope_index, &length)| {
                    let prefix = format!("{entry_index:03}{scope_index:03}");
                    ScopeRef::literal(format!("{prefix}{}", "x".repeat(length - prefix.len())))
                        .unwrap()
                })
                .collect();
            NeedEntry::scoped(atom(&format!("cap{entry_index}.op")), scopes).unwrap()
        })
        .collect();
    NeedsManifest::new(entries, vec![]).unwrap()
}

#[test]
fn enforces_needs_section_payload_ceiling_on_encode_and_decode() {
    let canonical = encode_needs_manifest(&NeedsManifest::empty()).unwrap();
    let mut exact_decode = canonical.clone();
    exact_decode.resize(MAX_SECTION_PAYLOAD_BYTES, b' ');
    assert_eq!(
        decode_needs_manifest(&exact_decode).unwrap(),
        NeedsManifest::empty()
    );
    exact_decode.push(b' ');
    assert!(matches!(
        decode_needs_manifest(&exact_decode).unwrap_err(),
        NeedsManifestDecodeError::PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        } if actual_bytes == MAX_SECTION_PAYLOAD_BYTES + 1
    ));

    let scope_count = 4 * MAX_SCOPES_PER_ENTRY;
    let mut lengths = vec![2000; scope_count];
    let baseline = encode_needs_manifest(&large_manifest(&lengths)).unwrap();
    let mut remaining = MAX_SECTION_PAYLOAD_BYTES - baseline.len();
    for length in &mut lengths {
        let increase = remaining.min(MAX_SCOPE_VALUE_BYTES - *length);
        *length += increase;
        remaining -= increase;
    }
    assert_eq!(remaining, 0);
    let exact_encode = encode_needs_manifest(&large_manifest(&lengths)).unwrap();
    assert_eq!(exact_encode.len(), MAX_SECTION_PAYLOAD_BYTES);

    let adjustable = lengths
        .iter_mut()
        .find(|length| **length < MAX_SCOPE_VALUE_BYTES)
        .unwrap();
    *adjustable += 1;
    assert!(matches!(
        encode_needs_manifest(&large_manifest(&lengths)).unwrap_err(),
        NeedsManifestEncodeError::PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        } if actual_bytes == MAX_SECTION_PAYLOAD_BYTES + 1
    ));
}
