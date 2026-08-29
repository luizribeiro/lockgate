//! Const-authorable symbolic plugin needs.

use crate::atom::QualifiedAtom;

/// Maximum UTF-8 size of a symbolic root name.
pub const MAX_ROOT_NAME_BYTES: usize = 64;
/// Maximum UTF-8 size of a root subpath.
pub const MAX_ROOT_SUBPATH_BYTES: usize = 1024;
/// Maximum number of slash-delimited segments in a root subpath.
pub const MAX_ROOT_SUBPATH_SEGMENTS: usize = 64;
/// Maximum UTF-8 size of one literal scope or setting pointer value.
pub const MAX_SCOPE_VALUE_BYTES: usize = 2048;
/// Maximum number of scope references on one scoped need.
pub const MAX_SCOPES_PER_NEED: usize = 128;
/// Maximum number of atoms across a needs declaration.
pub const MAX_ATOMS_PER_NEEDS: usize = 256;

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

    /// Declares an RFC 6901 JSON Pointer whose string or string-array value supplies scopes.
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

/// One typed requested permission atom.
///
/// Construct needs from qualified permission constants with
/// [`Permission::need`](crate::Permission::need) or
/// [`ScopedPermission::need`](crate::ScopedPermission::need). This keeps the
/// stable atom identity in the shared capability contract instead of repeating
/// strings in each plugin.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Need {
    atom: QualifiedAtom,
    kind: NeedKind,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NeedKind {
    Flag,
    Scoped(&'static [ScopeRef]),
}

impl Need {
    pub(crate) const fn flag(atom: QualifiedAtom) -> Self {
        Self {
            atom,
            kind: NeedKind::Flag,
        }
    }

    pub(crate) const fn scoped(atom: QualifiedAtom, scopes: &'static [ScopeRef]) -> Self {
        if scopes.is_empty() {
            panic!(
                "Lockgate scoped need must request at least one scope; pass a non-empty `&[ScopeRef]`"
            );
        }
        if scopes.len() > MAX_SCOPES_PER_NEED {
            panic!("Lockgate scoped need exceeds 128 scope references");
        }
        Self {
            atom,
            kind: NeedKind::Scoped(scopes),
        }
    }
}

/// A plugin's const-authorable required and optional permission needs.
///
/// Required needs must be accepted for admission. Optional needs remain a
/// distinct wire group, although the first policy slice accepts both groups
/// together. An atom may appear only once across both groups.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Needs {
    format: u32,
    required: &'static [Need],
    optional: Option<&'static [Need]>,
}

impl Needs {
    /// Declares that a plugin requests no host capabilities.
    pub const NOTHING: Self = Self {
        format: 1,
        required: &[],
        optional: None,
    };

    /// Declares the permission needs without which the plugin cannot operate.
    pub const fn required(required: &'static [Need]) -> Self {
        if required.len() > MAX_ATOMS_PER_NEEDS {
            panic!("Lockgate needs manifest exceeds 256 atoms");
        }
        validate_unique_entries(required, Requirement::Required);
        Self {
            format: 1,
            required,
            optional: None,
        }
    }

    /// Adds permission needs whose denial leaves the plugin operational.
    pub const fn optional(mut self, optional: &'static [Need]) -> Self {
        if self.optional.is_some() {
            panic!("Lockgate Needs::optional may be called only once");
        }
        if self.required.len() + optional.len() > MAX_ATOMS_PER_NEEDS {
            panic!("Lockgate needs manifest exceeds 256 atoms");
        }
        validate_unique_entries(optional, Requirement::Optional);

        let mut optional_index = 0;
        while optional_index < optional.len() {
            let mut required_index = 0;
            while required_index < self.required.len() {
                if same_atom(&optional[optional_index], &self.required[required_index]) {
                    panic!(
                        "Lockgate duplicate atom: an optional need repeats an atom already declared as required; declare each permission atom only once"
                    );
                }
                required_index += 1;
            }
            optional_index += 1;
        }
        self.optional = Some(optional);
        self
    }
}

#[derive(Clone, Copy)]
enum Requirement {
    Required,
    Optional,
}

const fn validate_unique_entries(entries: &[Need], requirement: Requirement) {
    let mut duplicate_index = 1;
    while duplicate_index < entries.len() {
        let mut first_index = 0;
        while first_index < duplicate_index {
            if same_atom(&entries[duplicate_index], &entries[first_index]) {
                match requirement {
                    Requirement::Required => panic!(
                        "Lockgate duplicate atom: a required need repeats an earlier required atom; declare each permission atom only once"
                    ),
                    Requirement::Optional => panic!(
                        "Lockgate duplicate atom: an optional need repeats an earlier optional atom; declare each permission atom only once"
                    ),
                }
            }
            first_index += 1;
        }
        duplicate_index += 1;
    }
}

const fn same_atom(left: &Need, right: &Need) -> bool {
    equal_str(left.atom.capability(), right.atom.capability())
        && equal_str(left.atom.permission(), right.atom.permission())
}

const fn equal_str(left: &str, right: &str) -> bool {
    let left = left.as_bytes();
    let right = right.as_bytes();
    if left.len() != right.len() {
        return false;
    }
    let mut index = 0;
    while index < left.len() {
        if left[index] != right[index] {
            return false;
        }
        index += 1;
    }
    true
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
pub const fn need_capability(need: &Need) -> &'static str {
    need.atom.capability()
}

#[doc(hidden)]
pub const fn need_permission(need: &Need) -> &'static str {
    need.atom.permission()
}

#[doc(hidden)]
pub const fn need_scopes(need: &Need) -> Option<&'static [ScopeRef]> {
    match need.kind {
        NeedKind::Flag => None,
        NeedKind::Scoped(scopes) => Some(scopes),
    }
}

#[doc(hidden)]
pub const fn needs_format(needs: &Needs) -> u32 {
    needs.format
}

#[doc(hidden)]
pub const fn needs_required(needs: &Needs) -> &'static [Need] {
    needs.required
}

#[doc(hidden)]
pub const fn needs_optional(needs: &Needs) -> &'static [Need] {
    match needs.optional {
        Some(optional) => optional,
        None => &[],
    }
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

#[cfg(test)]
mod tests {
    extern crate std;

    use super::*;

    const LIST_ATOM: QualifiedAtom = match QualifiedAtom::new("documents", "list") {
        Ok(atom) => atom,
        Err(_) => panic!("test atom must be valid"),
    };
    const READ_ATOM: QualifiedAtom = match QualifiedAtom::new("documents", "read") {
        Ok(atom) => atom,
        Err(_) => panic!("test atom must be valid"),
    };
    const LIST: Need = Need::flag(LIST_ATOM);
    const READ: Need = Need::flag(READ_ATOM);
    static TOO_MANY_NEEDS: [Need; 257] = [LIST; 257];
    static TOO_MANY_SCOPES: [ScopeRef; 129] = [ScopeRef::literal("project"); 129];

    #[test]
    fn nothing_is_the_empty_format_one_manifest() {
        assert_eq!(needs_format(&Needs::NOTHING), 1);
        assert!(needs_required(&Needs::NOTHING).is_empty());
        assert!(needs_optional(&Needs::NOTHING).is_empty());
    }

    #[test]
    fn collection_ceilings_are_enforced() {
        assert!(std::panic::catch_unwind(|| Need::scoped(READ_ATOM, &TOO_MANY_SCOPES)).is_err());
        assert!(std::panic::catch_unwind(|| Needs::required(&TOO_MANY_NEEDS)).is_err());
    }

    #[test]
    fn duplicate_atoms_are_rejected_within_and_across_groups() {
        assert!(std::panic::catch_unwind(|| Needs::required(&[LIST, LIST])).is_err());
        assert!(std::panic::catch_unwind(|| Needs::required(&[]).optional(&[READ, READ])).is_err());
        assert!(std::panic::catch_unwind(|| Needs::required(&[LIST]).optional(&[LIST])).is_err());
    }
}
