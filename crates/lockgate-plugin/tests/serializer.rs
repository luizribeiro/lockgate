extern crate alloc;
extern crate lockgate_policy as lockgate;

use lockgate_plugin::__private::{Manifest, metadata_bytes, metadata_len, needs_bytes, needs_len};
use lockgate_plugin::{Needs, ScopeRef};
use lockgate_schema::{NeedsManifest, PluginMetadata};

#[lockgate::capability("vm")]
mod vm {
    use lockgate::{Scope, ScopedPermission};

    #[derive(Clone, Copy, PartialEq, Eq, lockgate::ScopeRepr)]
    pub enum VmScope {
        All,
    }

    impl Scope for VmScope {}

    pub const CREATE: ScopedPermission<VmScope> = ScopedPermission::new("create");
}

const MANIFEST: Manifest = Manifest {
    id: "com.example.greeter",
    name: "Greeter \"deluxe\" Δ",
    version: "1.0\\portable",
    description: Some("Greets <friends> & neighbors"),
    license: Some("MIT OR Apache-2.0"),
    repository: Some("https://example.com/a\\b"),
    homepage: Some("https://example.com/?q=\"hello\""),
    needs: Needs::NOTHING,
};
const METADATA_LEN: usize = metadata_len(&MANIFEST);
const METADATA: [u8; METADATA_LEN] = metadata_bytes(&MANIFEST);
const NEEDS_LEN: usize = needs_len(&Needs::NOTHING);
const NEEDS: [u8; NEEDS_LEN] = needs_bytes(&Needs::NOTHING);
const DEDUP_NEEDS: Needs =
    Needs::required(&[vm::CREATE.need(&[ScopeRef::literal("gpu"), ScopeRef::literal("gpu")])]);
const DEDUP_NEEDS_LEN: usize = needs_len(&DEDUP_NEEDS);
const DEDUP_NEEDS_BYTES: [u8; DEDUP_NEEDS_LEN] = needs_bytes(&DEDUP_NEEDS);

#[test]
fn const_serializer_matches_host_wire_encoding() {
    let metadata = PluginMetadata::new(MANIFEST.id, MANIFEST.name, MANIFEST.version)
        .unwrap()
        .with_description(MANIFEST.description.unwrap())
        .with_license(MANIFEST.license.unwrap())
        .with_repository(MANIFEST.repository.unwrap())
        .with_homepage(MANIFEST.homepage.unwrap());

    assert_eq!(METADATA.as_slice(), metadata.to_section_bytes().unwrap());
    assert_eq!(
        NEEDS.as_slice(),
        NeedsManifest::empty().to_section_bytes().unwrap()
    );
}

#[test]
fn nothing_preserves_the_previous_hardcoded_manifest_bytes() {
    assert_eq!(
        NEEDS.as_slice(),
        br#"{"format":1,"optional":{},"reasons":{},"required":{}}"#
    );
}

#[test]
fn duplicate_scope_references_are_emitted_once() {
    assert_eq!(
        DEDUP_NEEDS_BYTES.as_slice(),
        br#"{"format":1,"optional":{},"reasons":{},"required":{"vm.create":["gpu"]}}"#
    );
    NeedsManifest::from_section_bytes(&DEDUP_NEEDS_BYTES).unwrap();
}
