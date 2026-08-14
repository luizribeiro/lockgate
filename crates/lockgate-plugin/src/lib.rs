//! Guest authoring facade for Lockgate plugins.
#![no_std]

/// Static declarations embedded into a plugin component by [`export!`](macro@export).
///
/// Identity is always explicit. Optional metadata defaults to the corresponding
/// Cargo package field at the `export!` call site, and permission needs default
/// to deny-by-default emptiness.
pub trait Plugin {
    const ID: &'static str;
    const NAME: Option<&'static str> = None;
    const VERSION: Option<&'static str> = None;
    const DESCRIPTION: Option<&'static str> = None;
    const LICENSE: Option<&'static str> = None;
    const REPOSITORY: Option<&'static str> = None;
    const HOMEPAGE: Option<&'static str> = None;
    const NEEDS: Needs = Needs::EMPTY;
}

/// A const-constructible guest permission declaration.
///
/// This first facade version intentionally exposes only [`Needs::EMPTY`].
/// Required and optional need constructors arrive with the grant declaration
/// surface.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    format: u32,
}

impl Needs {
    pub const EMPTY: Self = Self { format: 1 };
}

/// Implementation details shared with the facade's generated code.
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

    pub const fn cargo_optional(value: Option<&'static str>) -> Option<&'static str> {
        match value {
            Some(value) if !value.as_bytes().is_empty() => Some(value),
            Some(_) | None => None,
        }
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
        if manifest.id.as_bytes().is_empty() {
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
