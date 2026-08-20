use lockgate_schema::needs::{MAX_SCOPE_VALUE_BYTES, MAX_SCOPES_PER_ENTRY};
use lockgate_schema::sections::needs::{DecodeError, EncodeError};
use lockgate_schema::sections::{MAX_SECTION_PAYLOAD_BYTES, PLUGIN_NEEDS_SECTION};
use lockgate_schema::{AtomKey, NeedEntry, NeedsDigest, NeedsManifest, ScopeRefEntry};

fn atom(value: &str) -> AtomKey {
    value.parse().unwrap()
}

#[test]
fn empty_manifest_has_a_canonical_section_payload() {
    assert_eq!(PLUGIN_NEEDS_SECTION, "lockgate:needs");
    assert_eq!(
        NeedsManifest::empty().to_section_bytes().unwrap(),
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
                    ScopeRefEntry::root("workspace")
                        .unwrap()
                        .join("generated/html")
                        .unwrap(),
                    ScopeRefEntry::literal("current").unwrap(),
                ],
            )
            .unwrap(),
        ],
        vec![
            NeedEntry::scoped(
                atom("http.request"),
                vec![ScopeRefEntry::setting("/endpoint").unwrap()],
            )
            .unwrap()
            .with_reason("deliver notifications")
            .unwrap(),
        ],
    )
    .unwrap();

    let encoded = manifest.to_section_bytes().unwrap();
    assert_eq!(
        encoded,
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":true}}"#
    );
    let decoded = NeedsManifest::from_section_bytes(&encoded).unwrap();
    assert_eq!(decoded, manifest);
    assert_eq!(decoded.to_section_bytes().unwrap(), encoded);
}

#[test]
fn dash_prefixed_capabilities_have_stable_canonical_bytes_and_digest() {
    let entries = || {
        [
            NeedEntry::flag(atom("ab.c-x")),
            NeedEntry::flag(atom("ab-c.x")),
        ]
    };
    let [first, second] = entries();
    let forward = NeedsManifest::new(vec![first, second], vec![]).unwrap();
    let [first, second] = entries();
    let reversed = NeedsManifest::new(vec![second, first], vec![]).unwrap();

    let expected =
        br#"{"format":1,"optional":{},"reasons":{},"required":{"ab-c.x":true,"ab.c-x":true}}"#;
    assert_eq!(forward.to_section_bytes().unwrap(), expected);
    assert_eq!(reversed.to_section_bytes().unwrap(), expected);
    assert_eq!(
        forward
            .required()
            .iter()
            .map(|entry| entry.atom().to_string())
            .collect::<Vec<_>>(),
        ["ab-c.x", "ab.c-x"]
    );
    assert_eq!(
        NeedsDigest::compute(&forward).unwrap(),
        NeedsDigest::compute(&reversed).unwrap()
    );
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
                    ScopeRefEntry::literal(format!("{prefix}{}", "x".repeat(length - prefix.len())))
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
    let canonical = NeedsManifest::empty().to_section_bytes().unwrap();
    let mut exact_decode = canonical.clone();
    exact_decode.resize(MAX_SECTION_PAYLOAD_BYTES, b' ');
    assert_eq!(
        NeedsManifest::from_section_bytes(&exact_decode).unwrap(),
        NeedsManifest::empty()
    );
    exact_decode.push(b' ');
    assert!(matches!(
        NeedsManifest::from_section_bytes(&exact_decode).unwrap_err(),
        DecodeError::PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        } if actual_bytes == MAX_SECTION_PAYLOAD_BYTES + 1
    ));

    let scope_count = 4 * MAX_SCOPES_PER_ENTRY;
    let mut lengths = vec![2000; scope_count];
    let baseline = large_manifest(&lengths).to_section_bytes().unwrap();
    let mut remaining = MAX_SECTION_PAYLOAD_BYTES - baseline.len();
    for length in &mut lengths {
        let increase = remaining.min(MAX_SCOPE_VALUE_BYTES - *length);
        *length += increase;
        remaining -= increase;
    }
    assert_eq!(remaining, 0);
    let exact_encode = large_manifest(&lengths).to_section_bytes().unwrap();
    assert_eq!(exact_encode.len(), MAX_SECTION_PAYLOAD_BYTES);

    let adjustable = lengths
        .iter_mut()
        .find(|length| **length < MAX_SCOPE_VALUE_BYTES)
        .unwrap();
    *adjustable += 1;
    assert!(matches!(
        large_manifest(&lengths).to_section_bytes().unwrap_err(),
        EncodeError::PayloadTooLarge {
            actual_bytes,
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        } if actual_bytes == MAX_SECTION_PAYLOAD_BYTES + 1
    ));
}
