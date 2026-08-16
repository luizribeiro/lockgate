//! Guest authoring facade for Lockgate plugins.
//!
//! The default `runtime` feature supplies the allocator, canonical ABI realloc
//! export, and trap-on-panic handler required by `no_std` WebAssembly guests.
//! Disable it when the final guest supplies its own program-wide runtime floor,
//! such as for a custom allocator or a future standard-library guest.
//!
//! A guest panic traps and fails only its current invocation. Capturing panic
//! messages is intentionally deferred to a future runtime diagnostics feature.
//! Direct `cargo test --target wasm32-wasip2` fails for this crate by construction because the runtime feature's panic handler collides with std's in the test harness; test through the dependent fixtures instead.
#![no_std]

extern crate self as lockgate_plugin;

#[doc(hidden)]
pub use wit_bindgen as __wit_bindgen;

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
}

/// Generates guest bindings for one WIT world.
///
/// Automatic `lockgate:config` wiring and compile-time export-shape checks
/// extend this wrapper in later facade steps.
#[macro_export]
macro_rules! generate {
    ({ path: $path:literal, world: $world:literal $(,)? }) => {
        $crate::__wit_bindgen::generate!({
            path: $path,
            world: $world,
            export_macro_name: "__lockgate_wit_export",
            runtime_path: "::lockgate_plugin::__wit_bindgen::rt",
        });
    };
}

/// Exports a generated guest implementation and embeds its plugin manifests.
/// Call `export!` exactly once per plugin. A second invocation in the same
/// module deliberately fails at compile time with duplicate-definition errors
/// for the generated statics: one plugin has one manifest.
///
/// The plugin identity is a required trait item, so omitting it is diagnosed
/// by Rust as a missing trait item:
///
/// ```compile_fail,E0046
/// use lockgate_plugin::{MetadataSource, Needs, Plugin, export};
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
/// }
/// export!(MissingIdentity);
/// ```
///
/// An explicitly empty identity is rejected during const evaluation:
///
/// ```compile_fail,E0080
/// use lockgate_plugin::{MetadataSource, Needs, Plugin, export};
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
/// }
/// export!(EmptyIdentity);
/// ```
///
/// Permission needs are also a required trait item, so every manifest states
/// its authority posture:
///
/// ```compile_fail,E0046
/// use lockgate_plugin::{MetadataSource, Plugin};
///
/// struct MissingNeeds;
/// impl Plugin for MissingNeeds {
///     const ID: &'static str = "missing-needs";
///     const DESCRIPTION: MetadataSource = MetadataSource::Absent;
///     const LICENSE: MetadataSource = MetadataSource::Absent;
///     const REPOSITORY: MetadataSource = MetadataSource::Absent;
///     const HOMEPAGE: MetadataSource = MetadataSource::Absent;
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
/// Display-string controls, format characters, and length limits are enforced
/// at host admission in this version; compile-time checks arrive with the later
/// const-needs validation work.
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
}

/// A const-constructible guest permission declaration.
///
/// This first facade version intentionally exposes only [`Needs::NOTHING`].
/// Required and optional need constructors arrive with the grant declaration
/// surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    format: u32,
}

impl Needs {
    /// Declares that a plugin requests no host capabilities.
    pub const NOTHING: Self = Self { format: 1 };
}

/// Implementation details shared with the facade's generated code.
/// This module exists solely for code generated by [`export!`] and is exempt
/// from semantic-versioning guarantees. It must never be used directly.
#[doc(hidden)]
pub mod __private {
    use super::Needs;

    const NEEDS_BYTES: &[u8] = br#"{"format":1,"optional":{},"reasons":{},"required":{}}"#;

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
        NEEDS_BYTES.len()
    }

    pub const fn needs_bytes<const N: usize>(needs: &Needs) -> [u8; N] {
        if N != needs_len(needs) {
            panic!("Lockgate needs serializer length mismatch");
        }
        let mut output = [0; N];
        let mut index = 0;
        while index < N {
            output[index] = NEEDS_BYTES[index];
            index += 1;
        }
        output
    }

    const fn validate_manifest(manifest: &Manifest) {
        if manifest.id.is_empty() {
            panic!("Lockgate plugin ID must not be empty");
        }
        validate_needs(&manifest.needs);
    }

    const fn validate_needs(needs: &Needs) {
        if needs.format != 1 {
            panic!("unsupported Lockgate needs format");
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
            len += match bytes[index] {
                b'\"' | b'\\' | 0x08 | b'\t' | b'\n' | 0x0c | b'\r' => 2,
                0x00..=0x1f => 6,
                _ => 1,
            };
            index += 1;
        }
        len
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
            let byte = bytes[index];
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
            index += 1;
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
