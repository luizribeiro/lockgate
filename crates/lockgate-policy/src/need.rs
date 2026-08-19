//! Const-authorable symbolic plugin needs.

/// Maximum UTF-8 size of a symbolic root name.
pub const MAX_ROOT_NAME_BYTES: usize = 64;
/// Maximum UTF-8 size of a root subpath.
pub const MAX_ROOT_SUBPATH_BYTES: usize = 1024;
/// Maximum number of slash-delimited segments in a root subpath.
pub const MAX_ROOT_SUBPATH_SEGMENTS: usize = 64;
/// Maximum UTF-8 size of one literal scope or setting pointer value.
pub const MAX_SCOPE_VALUE_BYTES: usize = 2048;

/// A const-authorable symbolic scope in a plugin's needs declaration.
///
/// References stay symbolic in the embedded manifest. Preparation later
/// resolves and validates each reference against the permission's scope type.
/// Use these constructors instead of spelling reserved wire prefixes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ScopeRef {
    kind: ScopeRefKind,
    value: &'static str,
    subpath: Option<&'static str>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ScopeRefKind {
    Literal,
    Setting,
    Root,
}

impl ScopeRef {
    /// Declares one complete literal scope value.
    pub const fn literal(value: &'static str) -> Self {
        if value.is_empty() {
            panic!("Lockgate literal scope must not be empty");
        }
        if starts_with(value, "setting:") || starts_with(value, "$") {
            panic!("Lockgate literal scope must not use `setting:` or `$` prefix");
        }
        validate_bounded_scope_value(value, ScopeValueKind::Literal);
        Self {
            kind: ScopeRefKind::Literal,
            value,
            subpath: None,
        }
    }

    /// Declares an RFC 6901 JSON Pointer whose string value supplies a scope.
    pub const fn setting(pointer: &'static str) -> Self {
        validate_bounded_scope_value(pointer, ScopeValueKind::SettingPointer);
        if !pointer.is_empty() && pointer.as_bytes()[0] != b'/' {
            panic!(
                "Lockgate setting reference must be an RFC 6901 JSON Pointer with valid `~0`/`~1` escapes"
            );
        }
        let bytes = pointer.as_bytes();
        let mut index = 0;
        while index < bytes.len() {
            if bytes[index] == b'~' {
                if index + 1 >= bytes.len()
                    || !(bytes[index + 1] == b'0' || bytes[index + 1] == b'1')
                {
                    panic!(
                        "Lockgate setting reference must be an RFC 6901 JSON Pointer with valid `~0`/`~1` escapes"
                    );
                }
                index += 1;
            }
            index += 1;
        }
        Self {
            kind: ScopeRefKind::Setting,
            value: pointer,
            subpath: None,
        }
    }

    /// Declares a host-mapped symbolic root.
    pub const fn root(name: &'static str) -> Self {
        validate_root_name(name);
        Self {
            kind: ScopeRefKind::Root,
            value: name,
            subpath: None,
        }
    }

    /// Narrows a symbolic root to one validated relative subpath.
    pub const fn join(mut self, subpath: &'static str) -> Self {
        if !matches!(self.kind, ScopeRefKind::Root) {
            panic!("Lockgate scope join requires a symbolic root reference");
        }
        if self.subpath.is_some() {
            panic!("Lockgate symbolic root reference is already joined");
        }
        validate_root_subpath(subpath);
        self.subpath = Some(subpath);
        self
    }
}

#[derive(Clone, Copy)]
enum ScopeValueKind {
    Literal,
    SettingPointer,
    RootSubpath,
}

const fn validate_bounded_scope_value(value: &str, kind: ScopeValueKind) {
    validate_scope_characters(value, kind);
    match kind {
        ScopeValueKind::Literal if value.len() > MAX_SCOPE_VALUE_BYTES => {
            panic!("Lockgate literal scope exceeds 2048 UTF-8 bytes")
        }
        ScopeValueKind::SettingPointer if value.len() > MAX_SCOPE_VALUE_BYTES => {
            panic!("Lockgate setting pointer exceeds 2048 UTF-8 bytes")
        }
        ScopeValueKind::RootSubpath if value.len() > MAX_ROOT_SUBPATH_BYTES => {
            panic!("Lockgate root subpath exceeds 1024 bytes")
        }
        _ => {}
    }
}

const fn validate_root_name(name: &str) {
    if name.len() > MAX_ROOT_NAME_BYTES {
        panic!("Lockgate root name exceeds 64 bytes");
    }
    let bytes = name.as_bytes();
    if bytes.is_empty() || !is_ascii_lower(bytes[0]) {
        panic!(
            "Lockgate root name must start with a lowercase letter and contain only lowercase letters, digits, or `-`"
        );
    }
    let mut index = 1;
    while index < bytes.len() {
        let byte = bytes[index];
        if !(is_ascii_lower(byte) || is_ascii_digit(byte) || byte == b'-') {
            panic!(
                "Lockgate root name must start with a lowercase letter and contain only lowercase letters, digits, or `-`"
            );
        }
        index += 1;
    }
}

const fn validate_root_subpath(subpath: &str) {
    validate_bounded_scope_value(subpath, ScopeValueKind::RootSubpath);
    let bytes = subpath.as_bytes();
    if !bytes.is_empty() && bytes[0] == b'/' {
        panic!("Lockgate root subpath must be relative");
    }
    if contains_byte(subpath, b'\\') {
        panic!("Lockgate root subpath must not contain backslashes");
    }
    let mut segment = 0;
    let mut segment_start = 0;
    let mut index = 0;
    while index <= bytes.len() {
        if index == bytes.len() || bytes[index] == b'/' {
            if segment >= MAX_ROOT_SUBPATH_SEGMENTS {
                panic!("Lockgate root subpath exceeds 64 segments");
            }
            if segment_start == index {
                panic!("Lockgate root subpath contains an empty segment");
            }
            let segment_len = index - segment_start;
            if bytes[segment_start] == b'.'
                && (segment_len == 1 || (segment_len == 2 && bytes[segment_start + 1] == b'.'))
            {
                panic!("Lockgate root subpath contains a `.` or `..` segment");
            }
            segment += 1;
            segment_start = index + 1;
        }
        index += 1;
    }
}

const fn validate_scope_characters(value: &str, kind: ScopeValueKind) {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        let (character, width) = decode_utf8(bytes, index);
        if is_disallowed_character(character) {
            match kind {
                ScopeValueKind::Literal => panic!(
                    "Lockgate literal scope contains a disallowed Unicode character; remove control, format, or line-separator characters"
                ),
                ScopeValueKind::SettingPointer => panic!(
                    "Lockgate setting pointer contains a disallowed Unicode character; remove control, format, or line-separator characters"
                ),
                ScopeValueKind::RootSubpath => panic!(
                    "Lockgate root subpath contains a disallowed Unicode character; remove control, format, or line-separator characters"
                ),
            }
        }
        index += width;
    }
}

const fn starts_with(value: &str, prefix: &str) -> bool {
    let value = value.as_bytes();
    let prefix = prefix.as_bytes();
    if value.len() < prefix.len() {
        return false;
    }
    let mut index = 0;
    while index < prefix.len() {
        if value[index] != prefix[index] {
            return false;
        }
        index += 1;
    }
    true
}

const fn contains_byte(value: &str, needle: u8) -> bool {
    let bytes = value.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == needle {
            return true;
        }
        index += 1;
    }
    false
}

const fn is_ascii_lower(byte: u8) -> bool {
    byte >= b'a' && byte <= b'z'
}

const fn is_ascii_digit(byte: u8) -> bool {
    byte >= b'0' && byte <= b'9'
}

const fn decode_utf8(bytes: &[u8], index: usize) -> (u32, usize) {
    let first = bytes[index];
    if first < 0x80 {
        (first as u32, 1)
    } else if first < 0xe0 {
        (
            (((first & 0x1f) as u32) << 6) | ((bytes[index + 1] & 0x3f) as u32),
            2,
        )
    } else if first < 0xf0 {
        (
            (((first & 0x0f) as u32) << 12)
                | (((bytes[index + 1] & 0x3f) as u32) << 6)
                | ((bytes[index + 2] & 0x3f) as u32),
            3,
        )
    } else {
        (
            (((first & 0x07) as u32) << 18)
                | (((bytes[index + 1] & 0x3f) as u32) << 12)
                | (((bytes[index + 2] & 0x3f) as u32) << 6)
                | ((bytes[index + 3] & 0x3f) as u32),
            4,
        )
    }
}

// WHY: Rust has no const Unicode category query. This mirrors the schema's
// Cc/Cf and line-separator rejection table so const authoring and host decoding
// accept the same strings.
const fn is_disallowed_character(character: u32) -> bool {
    character <= 0x1f
        || (character >= 0x7f && character <= 0x9f)
        || character == 0x00ad
        || (character >= 0x0600 && character <= 0x0605)
        || character == 0x061c
        || character == 0x06dd
        || character == 0x070f
        || (character >= 0x0890 && character <= 0x0891)
        || character == 0x08e2
        || character == 0x180e
        || (character >= 0x200b && character <= 0x200f)
        || (character >= 0x2028 && character <= 0x202e)
        || (character >= 0x2060 && character <= 0x2064)
        || (character >= 0x2066 && character <= 0x206f)
        || character == 0xfeff
        || (character >= 0xfff9 && character <= 0xfffb)
        || character == 0x110bd
        || character == 0x110cd
        || (character >= 0x13430 && character <= 0x1343f)
        || (character >= 0x1bca0 && character <= 0x1bca3)
        || (character >= 0x1d173 && character <= 0x1d17a)
        || character == 0xe0001
        || (character >= 0xe0020 && character <= 0xe007f)
}

#[doc(hidden)]
pub const fn scope_ref_wire_len(reference: &ScopeRef) -> usize {
    match reference.kind {
        ScopeRefKind::Literal => reference.value.len(),
        ScopeRefKind::Setting => b"setting:".len() + reference.value.len(),
        ScopeRefKind::Root => {
            1 + reference.value.len()
                + match reference.subpath {
                    Some(subpath) => 1 + subpath.len(),
                    None => 0,
                }
        }
    }
}

#[doc(hidden)]
pub const fn scope_ref_wire_byte(reference: &ScopeRef, index: usize) -> u8 {
    match reference.kind {
        ScopeRefKind::Literal => reference.value.as_bytes()[index],
        ScopeRefKind::Setting => {
            if index < b"setting:".len() {
                b"setting:"[index]
            } else {
                reference.value.as_bytes()[index - b"setting:".len()]
            }
        }
        ScopeRefKind::Root => {
            if index == 0 {
                b'$'
            } else if index <= reference.value.len() {
                reference.value.as_bytes()[index - 1]
            } else {
                match reference.subpath {
                    Some(_) if index == reference.value.len() + 1 => b'/',
                    Some(subpath) => subpath.as_bytes()[index - reference.value.len() - 2],
                    None => panic!("Lockgate root scope wire index is out of bounds"),
                }
            }
        }
    }
}
