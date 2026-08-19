extern crate alloc;
extern crate lockgate_policy as lockgate;

use lockgate_plugin::__private::{Manifest, metadata_bytes, metadata_len, needs_bytes, needs_len};
use lockgate_plugin::{Need, Needs, Permission, ScopeRef};
use lockgate_schema::{NeedKind, NeedsManifest, PluginMetadata};

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

#[lockgate::capability("s")]
mod stress {
    use lockgate::{Scope, ScopedPermission};

    #[derive(Clone, Copy, PartialEq, Eq, lockgate::ScopeRepr)]
    pub enum StressScope {
        All,
    }

    impl Scope for StressScope {}

    pub const ZERO: ScopedPermission<StressScope> = ScopedPermission::new("000");
}

// All 128 scopes fit; 47 atoms crosses rustc's deny-by-default `long_running_const_eval` threshold.
const ATOM_COUNT: usize = 46;
const SCOPE_COUNT: usize = 128;
const INDEXED_NAME_WIDTH: usize = 3;
static PERMISSION_NAMES: [u8; ATOM_COUNT * INDEXED_NAME_WIDTH] =
    indexed_name_bytes::<{ ATOM_COUNT * INDEXED_NAME_WIDTH }>();
static SCOPE_NAMES: [u8; SCOPE_COUNT * INDEXED_NAME_WIDTH] =
    indexed_name_bytes::<{ SCOPE_COUNT * INDEXED_NAME_WIDTH }>();
static STRESS_SCOPES: [ScopeRef; SCOPE_COUNT] = stress_scopes();
static STRESS_ENTRIES: [Need; ATOM_COUNT] = stress_entries();

const fn indexed_name_bytes<const N: usize>() -> [u8; N] {
    assert!(N.is_multiple_of(INDEXED_NAME_WIDTH));
    assert!(N / INDEXED_NAME_WIDTH <= 1_000);
    let mut names = [0; N];
    let mut index = 0;
    while index < N / INDEXED_NAME_WIDTH {
        let offset = index * INDEXED_NAME_WIDTH;
        names[offset] = b'0' + (index / 100) as u8;
        names[offset + 1] = b'0' + ((index / 10) % 10) as u8;
        names[offset + 2] = b'0' + (index % 10) as u8;
        index += 1;
    }
    names
}

const fn indexed_name(names: &'static [u8], index: usize) -> &'static str {
    let offset = index * INDEXED_NAME_WIDTH;
    // SAFETY: Both static name tables contain consecutive three-byte ASCII names.
    let name =
        unsafe { core::slice::from_raw_parts(names.as_ptr().add(offset), INDEXED_NAME_WIDTH) };
    // SAFETY: `indexed_name_bytes` fills every byte with ASCII.
    unsafe { core::str::from_utf8_unchecked(name) }
}

const fn stress_scopes() -> [ScopeRef; SCOPE_COUNT] {
    let mut scopes = [ScopeRef::literal("000"); SCOPE_COUNT];
    let mut index = 0;
    while index < scopes.len() {
        scopes[index] = ScopeRef::literal(indexed_name(&SCOPE_NAMES, index));
        index += 1;
    }
    scopes
}

const fn stress_entries() -> [Need; ATOM_COUNT] {
    let mut entries =
        [lockgate::__private::qualify_permission("s", Permission::new("000")).need(); ATOM_COUNT];
    let mut index = 0;
    while index < entries.len() {
        let permission = indexed_name(&PERMISSION_NAMES, index);
        entries[index] = if index == 0 {
            stress::ZERO.need(&STRESS_SCOPES)
        } else {
            lockgate::__private::qualify_permission("s", Permission::new(permission)).need()
        };
        index += 1;
    }
    entries
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
const STRESS_NEEDS: Needs = Needs::required(&STRESS_ENTRIES);
const STRESS_NEEDS_LEN: usize = needs_len(&STRESS_NEEDS);
const STRESS_NEEDS_BYTES: [u8; STRESS_NEEDS_LEN] = needs_bytes(&STRESS_NEEDS);

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

#[test]
fn const_serializer_and_decoder_handle_frozen_manifest_ceilings() {
    let decoded = NeedsManifest::from_section_bytes(&STRESS_NEEDS_BYTES).unwrap();

    assert_eq!(decoded.required().len(), ATOM_COUNT);
    assert!(decoded.optional().is_empty());
    assert_eq!(decoded.required()[0].atom().to_string(), "s.000");
    let NeedKind::Scoped(scopes) = decoded.required()[0].kind() else {
        panic!("first stress permission should be scoped");
    };
    assert_eq!(scopes.len(), SCOPE_COUNT);
    assert_eq!(scopes[73].to_wire(), "073");
    assert_eq!(decoded.required()[45].atom().to_string(), "s.045");
    assert_eq!(decoded.to_section_bytes().unwrap(), STRESS_NEEDS_BYTES);
}
