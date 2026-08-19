//! Guest authoring facade for Lockgate plugins.
//!
//! This facade is `no_std` on WebAssembly and uses `std` on native targets so
//! dependent guest crates can use the native test harness. The default
//! `runtime` feature supplies the allocator, canonical ABI realloc export, and
//! trap-on-panic handler required by `no_std` WebAssembly guests.
//! Disable it when the final guest supplies its own program-wide runtime floor,
//! such as for a custom allocator or a future standard-library guest.
//!
//! A guest panic traps and fails only its current invocation. Capturing panic
//! messages is intentionally deferred to a future runtime diagnostics feature.
//! Direct `cargo test --target wasm32-wasip2` fails for this crate by construction because the runtime feature's panic handler collides with std's in the test harness; test through the dependent fixtures instead.
#![cfg_attr(target_arch = "wasm32", no_std)]

#[doc(hidden)]
pub extern crate alloc;
extern crate self as lockgate_plugin;

pub use schemars::{self, JsonSchema};
use serde::de::{DeserializeOwned, Error as _, MapAccess, Visitor};
pub use serde::{self, Deserialize};
#[doc(hidden)]
pub use wit_bindgen as __wit_bindgen;

pub use lockgate_policy::{HttpOrigin, Need, Needs, Permission, ScopeRef, ScopedPermission, http};

#[cfg(all(feature = "runtime", target_arch = "wasm32"))]
mod runtime {
    extern crate alloc;

    use alloc::alloc::{Layout, alloc, handle_alloc_error, realloc};
    use core::panic::PanicInfo;

    #[global_allocator]
    static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

    /// Reallocates guest memory for canonical ABI lifting and lowering.
    ///
    /// # Safety
    ///
    /// `align` must be a nonzero power of two, and rounding `new_len` up to
    /// `align` must not overflow `isize::MAX`. Violating these requirements is
    /// immediate undefined behavior because this function uses
    /// [`Layout::from_size_align_unchecked`]. When `old_len` and `new_len` are
    /// both zero, the function returns `align` as the pointer without touching
    /// the allocator. Allocation failure calls [`handle_alloc_error`] in debug
    /// builds and traps via [`core::arch::wasm32::unreachable`] in release
    /// builds.
    ///
    /// A non-null `old_ptr` must denote an allocation made by this allocator
    /// with the supplied `old_len` and `align`. When `old_len` is nonzero,
    /// `new_len` must also be nonzero. The canonical ABI is the only intended
    /// caller.
    #[unsafe(export_name = "cabi_realloc")]
    pub unsafe extern "C" fn cabi_realloc(
        old_ptr: *mut u8,
        old_len: usize,
        align: usize,
        new_len: usize,
    ) -> *mut u8 {
        let layout;
        let pointer = unsafe {
            if old_len == 0 {
                if new_len == 0 {
                    return core::ptr::without_provenance_mut(align);
                }
                layout = Layout::from_size_align_unchecked(new_len, align);
                alloc(layout)
            } else {
                debug_assert_ne!(
                    new_len, 0,
                    "the canonical ABI never shrinks an allocation to zero"
                );
                layout = Layout::from_size_align_unchecked(old_len, align);
                realloc(old_ptr, layout, new_len)
            }
        };
        if pointer.is_null() {
            if cfg!(debug_assertions) {
                handle_alloc_error(layout);
            } else {
                core::arch::wasm32::unreachable()
            }
        }
        pointer
    }

    #[panic_handler]
    fn panic(_info: &PanicInfo<'_>) -> ! {
        core::arch::wasm32::unreachable()
    }

    /// Compares two byte regions for dependencies used by generated schemas.
    ///
    /// # Safety
    ///
    /// Both pointers must be valid to read for `len` bytes.
    #[unsafe(no_mangle)]
    pub unsafe extern "C" fn memcmp(
        left: *const core::ffi::c_void,
        right: *const core::ffi::c_void,
        len: usize,
    ) -> i32 {
        let left = left.cast::<u8>();
        let right = right.cast::<u8>();
        let mut index = 0;
        while index < len {
            let left_byte = unsafe { left.add(index).read() };
            let right_byte = unsafe { right.add(index).read() };
            if left_byte != right_byte {
                return i32::from(left_byte) - i32::from(right_byte);
            }
            index += 1;
        }
        0
    }
}

/// Generates guest bindings for one WIT world and automatically adds the
/// framework-owned `lockgate:config` settings import and schema export.
pub use lockgate_plugin_macros::generate;

/// Exports a generated guest implementation and embeds its plugin manifests.
/// Call `export!` exactly once per plugin. A second invocation in the same
/// module deliberately fails at compile time with duplicate-definition errors
/// for the generated statics: one plugin has one manifest.
///
/// The plugin identity is a required trait item, so omitting it is diagnosed
/// by Rust as a missing trait item:
///
/// ```compile_fail,E0046
/// use lockgate_plugin::{MetadataSource, Needs, NoSettings, Plugin, export};
///
/// macro_rules! __lockgate_wit_export {
///     ($plugin:ident) => {};
/// }
///
/// struct MissingIdentity;
/// impl Plugin for MissingIdentity {
///     const DESCRIPTION: MetadataSource = MetadataSource::Absent;
///     const LICENSE: MetadataSource = MetadataSource::Absent;
///     const REPOSITORY: MetadataSource = MetadataSource::Absent;
///     const HOMEPAGE: MetadataSource = MetadataSource::Absent;
///     const NEEDS: Needs = Needs::NOTHING;
///     type Settings = NoSettings;
/// }
/// export!(MissingIdentity);
/// ```
///
/// An explicitly empty identity is rejected during const evaluation:
///
/// ```compile_fail,E0080
/// use lockgate_plugin::{MetadataSource, Needs, NoSettings, Plugin, export};
///
/// macro_rules! __lockgate_wit_export {
///     ($plugin:ident) => {};
/// }
///
/// struct EmptyIdentity;
/// impl Plugin for EmptyIdentity {
///     const ID: &'static str = "";
///     const DESCRIPTION: MetadataSource = MetadataSource::Absent;
///     const LICENSE: MetadataSource = MetadataSource::Absent;
///     const REPOSITORY: MetadataSource = MetadataSource::Absent;
///     const HOMEPAGE: MetadataSource = MetadataSource::Absent;
///     const NEEDS: Needs = Needs::NOTHING;
///     type Settings = NoSettings;
/// }
/// export!(EmptyIdentity);
/// ```
///
/// Permission needs are also a required trait item, so every manifest states
/// its authority posture:
///
/// ```compile_fail,E0046
/// use lockgate_plugin::{MetadataSource, NoSettings, Plugin};
///
/// struct MissingNeeds;
/// impl Plugin for MissingNeeds {
///     const ID: &'static str = "missing-needs";
///     const DESCRIPTION: MetadataSource = MetadataSource::Absent;
///     const LICENSE: MetadataSource = MetadataSource::Absent;
///     const REPOSITORY: MetadataSource = MetadataSource::Absent;
///     const HOMEPAGE: MetadataSource = MetadataSource::Absent;
///     type Settings = NoSettings;
/// }
/// ```
pub use lockgate_plugin_macros::export;

/// Selects the source of one display-metadata field.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MetadataSource {
    /// Read the corresponding Cargo package field at the [`export!`] call site.
    Cargo,
    /// Embed the supplied value.
    Explicit(&'static str),
    /// Deliberately omit an optional wire field.
    Absent,
}

/// Static declarations embedded into a plugin component by [`export!`](macro@export).
///
/// Identity is always explicit. Every display-metadata const identifies its
/// source, defaulting to the corresponding Cargo package field at the
/// `export!` call site. Missing or empty Cargo fields are compile errors unless
/// the source is explicitly changed; display name and version cannot be
/// absent.
/// Permission needs have no default so every manifest explicitly states its
/// maximum authority, including [`Needs::NOTHING`].
///
/// Application SDKs re-export their shared permission constants beside this
/// facade. A guest then requests typed permissions without reproducing stable
/// atom strings:
///
/// ```ignore
/// use sage_plugin::{permissions::vm, Need, Needs, NoSettings, Plugin, ScopeRef};
///
/// const REQUIRED: &[Need] = &[
///     vm::CREATE.need(&[
///         ScopeRef::literal("gpu"),
///         ScopeRef::setting("/fallback-pool"),
///         ScopeRef::root("workspace").join("generated/images"),
///     ]),
///     vm::EXEC.need(&[
///         ScopeRef::literal("pool:gpu"),
///         ScopeRef::literal("created-by-caller"),
///     ]),
///     vm::DESTROY.need(&[ScopeRef::literal("created-by-caller")]),
/// ];
/// const OPTIONAL: &[Need] = &[vm::LIST_POOLS.need()];
///
/// struct Provisioner;
/// impl Plugin for Provisioner {
///     const ID: &'static str = "provisioning";
///     const NEEDS: Needs = Needs::required(REQUIRED).optional(OPTIONAL);
///     type Settings = NoSettings;
/// }
/// ```
///
/// Scoped needs require at least one reference. References remain symbolic in
/// the component and are resolved and checked against the permission's scope
/// type during host preparation. Typed need and scope-reference invariants are
/// checked during const authoring and again by the host's defensive decoder;
/// display-metadata controls remain enforced at host admission.
pub trait Plugin {
    const ID: &'static str;
    /// The human-facing label used in consent screens and listings.
    ///
    /// This defaults to Cargo's package name at the [`export!`] call site and
    /// maps to the frozen `name` wire field. [`Plugin::ID`] alone is stable
    /// plugin identity.
    const DISPLAY_NAME: MetadataSource = MetadataSource::Cargo;
    const VERSION: MetadataSource = MetadataSource::Cargo;
    const DESCRIPTION: MetadataSource = MetadataSource::Cargo;
    const LICENSE: MetadataSource = MetadataSource::Cargo;
    const REPOSITORY: MetadataSource = MetadataSource::Cargo;
    const HOMEPAGE: MetadataSource = MetadataSource::Cargo;
    const NEEDS: Needs;
    const SETTINGS_POLICY: SettingsPolicy = SettingsPolicy::Closed;
    type Settings: DeserializeOwned + JsonSchema;

    /// Reads and deserializes this invocation's retained validated settings.
    ///
    /// Validation and retention happen during host preparation. A mismatch
    /// between the generated schema and a custom `Deserialize` implementation
    /// is therefore a plugin construction bug and traps with the settings type
    /// and serde error rather than entering every call site as a `Result`.
    fn settings() -> Self::Settings
    where
        Self: Sized,
    {
        __private::deserialize_settings::<Self>()
    }
}

/// Controls whether a plugin's top-level settings object accepts unknown keys.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SettingsPolicy {
    /// Reject unknown top-level keys.
    #[default]
    Closed,
    /// Permit unknown top-level keys.
    Open,
}

/// The explicit settings type for a plugin that accepts no configuration.
///
/// Unlike a unit struct's incidental generated schema, `NoSettings` always
/// means a closed empty JSON object. It accepts `{}` and rejects every key.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct NoSettings;

impl JsonSchema for NoSettings {
    fn inline_schema() -> bool {
        true
    }

    fn schema_name() -> alloc::borrow::Cow<'static, str> {
        "NoSettings".into()
    }

    fn json_schema(_generator: &mut schemars::SchemaGenerator) -> schemars::Schema {
        schemars::json_schema!({
            "type": "object",
            "properties": {},
            "unevaluatedProperties": false
        })
    }
}

impl<'de> serde::Deserialize<'de> for NoSettings {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct NoSettingsVisitor;

        impl<'de> Visitor<'de> for NoSettingsVisitor {
            type Value = NoSettings;

            fn expecting(&self, formatter: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
                formatter.write_str("an empty settings object")
            }

            fn visit_map<A>(self, mut map: A) -> Result<Self::Value, A::Error>
            where
                A: MapAccess<'de>,
            {
                if let Some(key) = map.next_key::<alloc::string::String>()? {
                    return Err(A::Error::unknown_field(&key, &[]));
                }
                Ok(NoSettings)
            }
        }

        deserializer.deserialize_map(NoSettingsVisitor)
    }
}

/// Implementation details shared with the facade's generated code.
/// This module exists solely for code generated by [`export!`] and is exempt
/// from semantic-versioning guarantees. It must never be used directly.
#[doc(hidden)]
pub mod __private {
    use super::{Need, Needs, Plugin, ScopeRef, SettingsPolicy};
    use alloc::string::String;
    use lockgate_policy::__private::{
        need_capability, need_permission, need_scopes, needs_format, needs_optional,
        needs_required, scope_ref_wire_byte, scope_ref_wire_len,
    };

    const MAX_SECTION_PAYLOAD_BYTES: usize = 1024 * 1024;

    #[derive(Clone, Copy)]
    pub struct Manifest {
        pub id: &'static str,
        pub name: &'static str,
        pub version: &'static str,
        pub description: Option<&'static str>,
        pub license: Option<&'static str>,
        pub repository: Option<&'static str>,
        pub homepage: Option<&'static str>,
        pub needs: Needs,
    }

    pub const fn metadata_len(manifest: &Manifest) -> usize {
        validate_manifest(manifest);
        let mut len = b"{\"format\":1,\"id\":\"".len()
            + escaped_len(manifest.id)
            + b"\",\"name\":\"".len()
            + escaped_len(manifest.name)
            + b"\",\"version\":\"".len()
            + escaped_len(manifest.version)
            + 1;
        len += optional_field_len("description", manifest.description);
        len += optional_field_len("license", manifest.license);
        len += optional_field_len("repository", manifest.repository);
        len += optional_field_len("homepage", manifest.homepage);
        len + 1
    }

    pub const fn metadata_bytes<const N: usize>(manifest: &Manifest) -> [u8; N] {
        if N != metadata_len(manifest) {
            panic!("Lockgate metadata serializer length mismatch");
        }
        let mut output = [0; N];
        let mut cursor = 0;
        cursor = write_bytes(&mut output, cursor, b"{\"format\":1,\"id\":\"");
        cursor = write_escaped(&mut output, cursor, manifest.id);
        cursor = write_bytes(&mut output, cursor, b"\",\"name\":\"");
        cursor = write_escaped(&mut output, cursor, manifest.name);
        cursor = write_bytes(&mut output, cursor, b"\",\"version\":\"");
        cursor = write_escaped(&mut output, cursor, manifest.version);
        cursor = write_byte(&mut output, cursor, b'\"');
        cursor = write_optional_field(&mut output, cursor, "description", manifest.description);
        cursor = write_optional_field(&mut output, cursor, "license", manifest.license);
        cursor = write_optional_field(&mut output, cursor, "repository", manifest.repository);
        cursor = write_optional_field(&mut output, cursor, "homepage", manifest.homepage);
        cursor = write_byte(&mut output, cursor, b'}');
        if cursor != N {
            panic!("Lockgate metadata serializer fill mismatch");
        }
        output
    }

    pub const fn needs_len(needs: &Needs) -> usize {
        validate_needs(needs);
        let len = b"{\"format\":1,\"optional\":".len()
            + need_map_len(needs_optional(needs))
            + b",\"reasons\":{},\"required\":".len()
            + need_map_len(needs_required(needs))
            + 1;
        if len > MAX_SECTION_PAYLOAD_BYTES {
            panic!("Lockgate needs section exceeds 1 MiB");
        }
        len
    }

    pub const fn needs_bytes<const N: usize>(needs: &Needs) -> [u8; N] {
        if N != needs_len(needs) {
            panic!("Lockgate needs serializer length mismatch");
        }
        let mut output = [0; N];
        let mut cursor = 0;
        cursor = write_bytes(&mut output, cursor, b"{\"format\":1,\"optional\":");
        cursor = write_need_map(&mut output, cursor, needs_optional(needs));
        cursor = write_bytes(&mut output, cursor, b",\"reasons\":{},\"required\":");
        cursor = write_need_map(&mut output, cursor, needs_required(needs));
        cursor = write_byte(&mut output, cursor, b'}');
        if cursor != N {
            panic!("Lockgate needs serializer fill mismatch");
        }
        output
    }

    pub const fn validate_settings_policy<P: Plugin>() {
        if matches!(P::SETTINGS_POLICY, SettingsPolicy::Open)
            && core::mem::size_of::<P::Settings>() == 0
        {
            panic!(
                "Lockgate NoSettings cannot use SettingsPolicy::Open; remove the policy override or declare a settings struct"
            );
        }
    }

    pub fn settings_schema<P: Plugin>() -> String {
        let generator = schemars::generate::SchemaSettings::draft2020_12()
            .for_deserialize()
            .into_generator();
        let mut schema = generator.into_root_schema_for::<P::Settings>();
        if matches!(P::SETTINGS_POLICY, SettingsPolicy::Closed) {
            schema.insert("unevaluatedProperties".into(), false.into());
        }
        serde_json::to_string(&schema).unwrap_or_else(|error| {
            panic!(
                "failed to serialize settings schema for {}: {error}",
                core::any::type_name::<P::Settings>()
            )
        })
    }

    pub fn deserialize_settings<P: Plugin>() -> P::Settings {
        unsafe extern "Rust" {
            fn __lockgate_settings_json() -> String;
        }

        // `generate!` defines this bridge exactly once in the final guest and
        // forwards it to the facade-added configuration import.
        let json = unsafe { __lockgate_settings_json() };
        serde_json::from_str(&json).unwrap_or_else(|error| {
            panic!(
                "failed to deserialize validated Lockgate settings as {}: {error}",
                core::any::type_name::<P::Settings>()
            )
        })
    }

    const fn validate_manifest(manifest: &Manifest) {
        if manifest.id.is_empty() {
            panic!("Lockgate plugin ID must not be empty");
        }
        validate_needs(&manifest.needs);
    }

    const fn validate_needs(needs: &Needs) {
        if needs_format(needs) != 1 {
            panic!("unsupported Lockgate needs format");
        }
    }

    const fn need_map_len(entries: &[Need]) -> usize {
        let mut len = 2;
        let mut previous = None;
        let mut written = 0;
        while let Some(index) = next_entry(entries, previous) {
            if written != 0 {
                len += 1;
            }
            len += 3 + atom_escaped_len(&entries[index]) + need_value_len(&entries[index]);
            written += 1;
            previous = Some(index);
        }
        len
    }

    const fn need_value_len(need: &Need) -> usize {
        match need_scopes(need) {
            None => 4,
            Some(scopes) => scoped_value_len(scopes),
        }
    }

    const fn scoped_value_len(scopes: &[ScopeRef]) -> usize {
        let mut len = 2;
        let mut previous = None;
        let mut written = 0;
        while let Some(index) = next_scope(scopes, previous) {
            if written != 0 {
                len += 1;
            }
            len += 2 + scope_escaped_len(&scopes[index]);
            written += 1;
            previous = Some(index);
        }
        len
    }

    const fn atom_escaped_len(need: &Need) -> usize {
        escaped_len(need_capability(need)) + 1 + escaped_len(need_permission(need))
    }

    const fn scope_escaped_len(scope: &ScopeRef) -> usize {
        let mut len = 0;
        let mut index = 0;
        while index < scope_ref_wire_len(scope) {
            len += escaped_byte_len(scope_ref_wire_byte(scope, index));
            index += 1;
        }
        len
    }

    macro_rules! define_wire_order {
        (
            next = $next:ident,
            compare = $compare:ident,
            item = $item:ty,
            wire_len = $wire_len:ident,
            wire_byte = $wire_byte:ident
        ) => {
            const fn $next(entries: &[$item], previous: Option<usize>) -> Option<usize> {
                let mut candidate = None;
                let mut index = 0;
                while index < entries.len() {
                    let after_previous = match previous {
                        Some(previous) => $compare(&entries[index], &entries[previous]) > 0,
                        None => true,
                    };
                    if after_previous {
                        let before_candidate = match candidate {
                            Some(candidate) => $compare(&entries[index], &entries[candidate]) < 0,
                            None => true,
                        };
                        if before_candidate {
                            candidate = Some(index);
                        }
                    }
                    index += 1;
                }
                candidate
            }

            const fn $compare(left: &$item, right: &$item) -> i8 {
                let left_len = $wire_len(left);
                let right_len = $wire_len(right);
                let common = if left_len < right_len {
                    left_len
                } else {
                    right_len
                };
                let mut index = 0;
                while index < common {
                    let left_byte = $wire_byte(left, index);
                    let right_byte = $wire_byte(right, index);
                    if left_byte < right_byte {
                        return -1;
                    }
                    if left_byte > right_byte {
                        return 1;
                    }
                    index += 1;
                }
                if left_len < right_len {
                    -1
                } else if left_len > right_len {
                    1
                } else {
                    0
                }
            }
        };
    }

    define_wire_order!(
        next = next_entry,
        compare = compare_atom,
        item = Need,
        wire_len = atom_wire_len,
        wire_byte = atom_wire_byte
    );
    define_wire_order!(
        next = next_scope,
        compare = compare_scope,
        item = ScopeRef,
        wire_len = scope_ref_wire_len,
        wire_byte = scope_ref_wire_byte
    );

    const fn atom_wire_len(need: &Need) -> usize {
        need_capability(need).len() + 1 + need_permission(need).len()
    }

    const fn atom_wire_byte(need: &Need, index: usize) -> u8 {
        let capability = need_capability(need);
        if index < capability.len() {
            capability.as_bytes()[index]
        } else if index == capability.len() {
            b'.'
        } else {
            need_permission(need).as_bytes()[index - capability.len() - 1]
        }
    }

    const fn optional_field_len(name: &str, value: Option<&str>) -> usize {
        match value {
            Some(value) => 6 + name.len() + escaped_len(value),
            None => 0,
        }
    }

    const fn escaped_len(value: &str) -> usize {
        let bytes = value.as_bytes();
        let mut len = 0;
        let mut index = 0;
        while index < bytes.len() {
            len += escaped_byte_len(bytes[index]);
            index += 1;
        }
        len
    }

    const fn escaped_byte_len(byte: u8) -> usize {
        match byte {
            b'\"' | b'\\' | 0x08 | b'\t' | b'\n' | 0x0c | b'\r' => 2,
            0x00..=0x1f => 6,
            _ => 1,
        }
    }

    const fn write_need_map<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        entries: &[Need],
    ) -> usize {
        cursor = write_byte(output, cursor, b'{');
        let mut previous = None;
        let mut written = 0;
        while let Some(index) = next_entry(entries, previous) {
            if written != 0 {
                cursor = write_byte(output, cursor, b',');
            }
            cursor = write_byte(output, cursor, b'\"');
            cursor = write_atom_escaped(output, cursor, &entries[index]);
            cursor = write_bytes(output, cursor, b"\":");
            cursor = write_need_value(output, cursor, &entries[index]);
            written += 1;
            previous = Some(index);
        }
        write_byte(output, cursor, b'}')
    }

    const fn write_atom_escaped<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        need: &Need,
    ) -> usize {
        cursor = write_escaped(output, cursor, need_capability(need));
        cursor = write_byte(output, cursor, b'.');
        write_escaped(output, cursor, need_permission(need))
    }

    const fn write_need_value<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        need: &Need,
    ) -> usize {
        match need_scopes(need) {
            None => write_bytes(output, cursor, b"true"),
            Some(scopes) => {
                cursor = write_byte(output, cursor, b'[');
                let mut previous = None;
                let mut written = 0;
                while let Some(index) = next_scope(scopes, previous) {
                    if written != 0 {
                        cursor = write_byte(output, cursor, b',');
                    }
                    cursor = write_byte(output, cursor, b'\"');
                    cursor = write_scope_escaped(output, cursor, &scopes[index]);
                    cursor = write_byte(output, cursor, b'\"');
                    written += 1;
                    previous = Some(index);
                }
                write_byte(output, cursor, b']')
            }
        }
    }

    const fn write_scope_escaped<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        scope: &ScopeRef,
    ) -> usize {
        let mut index = 0;
        while index < scope_ref_wire_len(scope) {
            cursor = write_escaped_byte(output, cursor, scope_ref_wire_byte(scope, index));
            index += 1;
        }
        cursor
    }

    const fn write_optional_field<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        name: &str,
        value: Option<&str>,
    ) -> usize {
        if let Some(value) = value {
            cursor = write_bytes(output, cursor, b",\"");
            cursor = write_bytes(output, cursor, name.as_bytes());
            cursor = write_bytes(output, cursor, b"\":\"");
            cursor = write_escaped(output, cursor, value);
            cursor = write_byte(output, cursor, b'\"');
        }
        cursor
    }

    const fn write_escaped<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        value: &str,
    ) -> usize {
        let bytes = value.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            cursor = write_escaped_byte(output, cursor, bytes[index]);
            index += 1;
        }
        cursor
    }

    const fn write_escaped_byte<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        byte: u8,
    ) -> usize {
        match byte {
            b'\"' => cursor = write_bytes(output, cursor, b"\\\""),
            b'\\' => cursor = write_bytes(output, cursor, b"\\\\"),
            0x08 => cursor = write_bytes(output, cursor, b"\\b"),
            b'\t' => cursor = write_bytes(output, cursor, b"\\t"),
            b'\n' => cursor = write_bytes(output, cursor, b"\\n"),
            0x0c => cursor = write_bytes(output, cursor, b"\\f"),
            b'\r' => cursor = write_bytes(output, cursor, b"\\r"),
            0x00..=0x1f => {
                cursor = write_bytes(output, cursor, b"\\u00");
                cursor = write_byte(output, cursor, hex(byte >> 4));
                cursor = write_byte(output, cursor, hex(byte & 0x0f));
            }
            _ => cursor = write_byte(output, cursor, byte),
        }
        cursor
    }

    const fn write_bytes<const N: usize>(
        output: &mut [u8; N],
        mut cursor: usize,
        bytes: &[u8],
    ) -> usize {
        let mut index = 0;
        while index < bytes.len() {
            output[cursor] = bytes[index];
            cursor += 1;
            index += 1;
        }
        cursor
    }

    const fn write_byte<const N: usize>(output: &mut [u8; N], cursor: usize, byte: u8) -> usize {
        output[cursor] = byte;
        cursor + 1
    }

    const fn hex(value: u8) -> u8 {
        match value {
            0..=9 => b'0' + value,
            _ => b'a' + value - 10,
        }
    }
}
