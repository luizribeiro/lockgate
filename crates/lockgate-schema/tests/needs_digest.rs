use lockgate_schema::NeedsDigest;
use lockgate_schema::sections::needs::{decode_needs_manifest, encode_needs_manifest};

const GOLDEN: &[u8] = br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":true}}"#;

fn digest(bytes: &[u8]) -> NeedsDigest {
    NeedsDigest::compute(&decode_needs_manifest(bytes).unwrap()).unwrap()
}

#[test]
fn representative_manifest_has_stable_bytes_and_digest() {
    let manifest = decode_needs_manifest(GOLDEN).unwrap();
    let digest = NeedsDigest::compute(&manifest).unwrap();

    assert_eq!(encode_needs_manifest(&manifest).unwrap(), GOLDEN);
    assert_eq!(
        digest.to_hex(),
        "d5d165650de64ce4839f097b60d2a681d2e51d828bf72e0db6c02883454ef794"
    );
    assert_eq!(digest.to_string(), format!("sha256:{}", digest.to_hex()));
    assert_eq!(digest.as_bytes().len(), 32);
}

#[test]
fn semantically_equal_manifests_have_equal_digests() {
    let reordered = br#"{"required":{"notify.send":true,"fs.read":["current","$workspace/generated/html","current"]},"reasons":{"http.request":"deliver notifications"},"optional":{"http.request":["setting:/endpoint"]},"format":1}"#;

    assert_eq!(digest(GOLDEN), digest(reordered));
}

#[test]
fn changing_any_manifest_entry_field_changes_the_digest() {
    let changes: &[&[u8]] = &[
        br#"{"format":1,"optional":{"http.fetch":["setting:/endpoint"]},"reasons":{"http.fetch":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":true}}"#,
        br#"{"format":1,"optional":{"http.request":["setting:/other"]},"reasons":{"http.request":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":true}}"#,
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"deliver alerts"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":true}}"#,
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"]},"reasons":{"http.request":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"],"notify.send":["all"]}}"#,
        br#"{"format":1,"optional":{"http.request":["setting:/endpoint"],"notify.send":true},"reasons":{"http.request":"deliver notifications"},"required":{"fs.read":["$workspace/generated/html","current"]}}"#,
    ];
    let original = digest(GOLDEN);

    for changed in changes {
        assert_ne!(
            digest(changed),
            original,
            "unchanged digest for {changed:?}"
        );
    }
}
