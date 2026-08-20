use std::{error::Error, fmt};

use crate::text::{classify_disallowed_character, find_disallowed_character};

/// Maximum UTF-8 size of a symbolic root name.
pub const MAX_ROOT_NAME_BYTES: usize = 64;
/// Maximum UTF-8 size of a root subpath.
pub const MAX_ROOT_SUBPATH_BYTES: usize = 1024;
/// Maximum number of slash-delimited segments in a root subpath.
pub const MAX_ROOT_SUBPATH_SEGMENTS: usize = 64;

/// Maximum UTF-8 size of one literal scope or setting pointer value.
pub const MAX_SCOPE_VALUE_BYTES: usize = 2048;

/// A symbolic scope in a plugin's needs declaration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScopeRefEntry {
    /// A complete literal scope value.
    Literal(String),
    /// A JSON Pointer into the plugin's settings.
    Setting(String),
    /// A host-mapped symbolic root, optionally narrowed to a subpath.
    Root {
        name: String,
        subpath: Option<String>,
    },
}

impl ScopeRefEntry {
    /// Creates a literal scope. Reserved symbolic prefixes are not literals.
    pub fn literal(value: impl Into<String>) -> Result<Self, ScopeRefEntryError> {
        let reference = Self::Literal(value.into());
        reference.validate()?;
        Ok(reference)
    }

    /// Creates a settings reference from an RFC 6901 JSON Pointer.
    pub fn setting(pointer: impl Into<String>) -> Result<Self, ScopeRefEntryError> {
        let reference = Self::Setting(pointer.into());
        reference.validate()?;
        Ok(reference)
    }

    /// Creates a symbolic root reference.
    pub fn root(name: impl Into<String>) -> Result<Self, ScopeRefEntryError> {
        let reference = Self::Root {
            name: name.into(),
            subpath: None,
        };
        reference.validate()?;
        Ok(reference)
    }

    /// Narrows a symbolic root to a validated relative subpath.
    pub fn join(self, subpath: impl Into<String>) -> Result<Self, ScopeRefEntryError> {
        let Self::Root {
            name,
            subpath: current,
        } = self
        else {
            return Err(ScopeRefEntryError::JoinRequiresRoot);
        };
        if current.is_some() {
            return Err(ScopeRefEntryError::RootAlreadyJoined);
        }
        let reference = Self::Root {
            name,
            subpath: Some(subpath.into()),
        };
        reference.validate()?;
        Ok(reference)
    }

    /// Parses the canonical symbolic wire spelling.
    pub fn from_wire(value: &str) -> Result<Self, ScopeRefEntryError> {
        if let Some(pointer) = value.strip_prefix("setting:") {
            Self::setting(pointer)
        } else if let Some(root) = value.strip_prefix('$') {
            match root.split_once('/') {
                Some((name, subpath)) => Self::root(name)?.join(subpath),
                None => Self::root(root),
            }
        } else {
            Self::literal(value)
        }
    }

    /// Returns the canonical symbolic wire spelling.
    pub fn to_wire(&self) -> String {
        match self {
            Self::Literal(value) => value.clone(),
            Self::Setting(pointer) => format!("setting:{pointer}"),
            Self::Root { name, subpath } => match subpath {
                Some(subpath) => format!("${name}/{subpath}"),
                None => format!("${name}"),
            },
        }
    }

    pub(crate) fn validate(&self) -> Result<(), ScopeRefEntryError> {
        match self {
            Self::Literal(value) if value.is_empty() => Err(ScopeRefEntryError::EmptyLiteral),
            Self::Literal(value) if value.starts_with("setting:") || value.starts_with('$') => {
                Err(ScopeRefEntryError::ReservedLiteralPrefix)
            }
            Self::Literal(value) => validate_bounded_scope_value(value, ScopeValueKind::Literal),
            Self::Setting(pointer) => validate_json_pointer(pointer),
            Self::Root { name, subpath } => {
                validate_root_name(name)?;
                if let Some(subpath) = subpath {
                    validate_root_subpath(subpath)?;
                }
                Ok(())
            }
        }
    }
}

/// A malformed symbolic scope reference.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum ScopeRefEntryError {
    EmptyLiteral,
    ReservedLiteralPrefix,
    InvalidSettingPointer,
    DisallowedCharacter {
        kind: ScopeValueKind,
        byte_index: usize,
        character: char,
    },
    ScopeValueTooLong {
        kind: ScopeValueKind,
        max_bytes: usize,
    },
    InvalidRootName,
    RootNameTooLong {
        max_bytes: usize,
    },
    JoinRequiresRoot,
    RootAlreadyJoined,
    AbsoluteRootSubpath,
    EmptyRootSubpathSegment {
        index: usize,
    },
    DotRootSubpathSegment {
        index: usize,
    },
    RootSubpathContainsBackslash,
    RootSubpathTooLong {
        max_bytes: usize,
    },
    RootSubpathTooDeep {
        max_segments: usize,
    },
}

impl fmt::Display for ScopeRefEntryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyLiteral => formatter.write_str("literal scope must not be empty"),
            Self::ReservedLiteralPrefix => {
                formatter.write_str("literal scope must not use `setting:` or `$` prefix")
            }
            Self::InvalidSettingPointer => formatter.write_str(
                "setting reference must be an RFC 6901 JSON Pointer with valid `~0`/`~1` escapes",
            ),
            Self::DisallowedCharacter {
                kind,
                byte_index,
                character,
            } => {
                let character_kind = classify_disallowed_character(*character)
                    .expect("stored scope character must be disallowed");
                write!(
                    formatter,
                    "{kind} contains a Unicode {character_kind} at byte {byte_index}"
                )
            }
            Self::ScopeValueTooLong { kind, max_bytes } => {
                write!(formatter, "{kind} exceeds {max_bytes} UTF-8 bytes")
            }
            Self::InvalidRootName => formatter.write_str(
                "root name must start with a lowercase letter and contain only lowercase letters, digits, or `-`",
            ),
            Self::RootNameTooLong { max_bytes } => {
                write!(formatter, "root name exceeds {max_bytes} bytes")
            }
            Self::JoinRequiresRoot => formatter.write_str("only a root reference can be joined"),
            Self::RootAlreadyJoined => formatter.write_str("root reference is already joined"),
            Self::AbsoluteRootSubpath => formatter.write_str("root subpath must be relative"),
            Self::EmptyRootSubpathSegment { index } => {
                write!(formatter, "root subpath segment {index} must not be empty")
            }
            Self::DotRootSubpathSegment { index } => write!(
                formatter,
                "root subpath segment {index} must not be `.` or `..`"
            ),
            Self::RootSubpathContainsBackslash => {
                formatter.write_str("root subpath must not contain backslashes")
            }
            Self::RootSubpathTooLong { max_bytes } => {
                write!(formatter, "root subpath exceeds {max_bytes} bytes")
            }
            Self::RootSubpathTooDeep { max_segments } => {
                write!(formatter, "root subpath exceeds {max_segments} segments")
            }
        }
    }
}

impl Error for ScopeRefEntryError {}

/// The scope value whose characters failed validation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScopeValueKind {
    Literal,
    SettingPointer,
    RootSubpathSegment,
}

impl fmt::Display for ScopeValueKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Literal => "literal scope",
            Self::SettingPointer => "setting pointer",
            Self::RootSubpathSegment => "root subpath segment",
        })
    }
}

fn validate_scope_characters(value: &str, kind: ScopeValueKind) -> Result<(), ScopeRefEntryError> {
    if let Some((byte_index, character, _)) = find_disallowed_character(value) {
        return Err(ScopeRefEntryError::DisallowedCharacter {
            kind,
            byte_index,
            character,
        });
    }
    Ok(())
}

fn validate_bounded_scope_value(
    value: &str,
    kind: ScopeValueKind,
) -> Result<(), ScopeRefEntryError> {
    validate_scope_characters(value, kind)?;
    if value.len() > MAX_SCOPE_VALUE_BYTES {
        return Err(ScopeRefEntryError::ScopeValueTooLong {
            kind,
            max_bytes: MAX_SCOPE_VALUE_BYTES,
        });
    }
    Ok(())
}

fn validate_json_pointer(pointer: &str) -> Result<(), ScopeRefEntryError> {
    validate_bounded_scope_value(pointer, ScopeValueKind::SettingPointer)?;
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(ScopeRefEntryError::InvalidSettingPointer);
    }
    let mut bytes = pointer.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return Err(ScopeRefEntryError::InvalidSettingPointer);
        }
    }
    Ok(())
}

fn validate_root_name(name: &str) -> Result<(), ScopeRefEntryError> {
    if name.len() > MAX_ROOT_NAME_BYTES {
        return Err(ScopeRefEntryError::RootNameTooLong {
            max_bytes: MAX_ROOT_NAME_BYTES,
        });
    }
    let mut bytes = name.bytes();
    if !matches!(bytes.next(), Some(b'a'..=b'z'))
        || !bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
    {
        return Err(ScopeRefEntryError::InvalidRootName);
    }
    Ok(())
}

fn validate_root_subpath(subpath: &str) -> Result<(), ScopeRefEntryError> {
    validate_scope_characters(subpath, ScopeValueKind::RootSubpathSegment)?;
    if subpath.starts_with('/') {
        return Err(ScopeRefEntryError::AbsoluteRootSubpath);
    }
    if subpath.contains('\\') {
        return Err(ScopeRefEntryError::RootSubpathContainsBackslash);
    }
    if subpath.len() > MAX_ROOT_SUBPATH_BYTES {
        return Err(ScopeRefEntryError::RootSubpathTooLong {
            max_bytes: MAX_ROOT_SUBPATH_BYTES,
        });
    }
    for (index, segment) in subpath.split('/').enumerate() {
        if index >= MAX_ROOT_SUBPATH_SEGMENTS {
            return Err(ScopeRefEntryError::RootSubpathTooDeep {
                max_segments: MAX_ROOT_SUBPATH_SEGMENTS,
            });
        }
        if segment.is_empty() {
            return Err(ScopeRefEntryError::EmptyRootSubpathSegment { index });
        }
        if matches!(segment, "." | "..") {
            return Err(ScopeRefEntryError::DotRootSubpathSegment { index });
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_references_round_trip_through_wire_strings() {
        for reference in [
            ScopeRefEntry::literal("https://example.com").unwrap(),
            ScopeRefEntry::setting("/endpoint/~0name/~1path").unwrap(),
            ScopeRefEntry::root("workspace").unwrap(),
            ScopeRefEntry::root("workspace")
                .unwrap()
                .join("generated/html")
                .unwrap(),
        ] {
            assert_eq!(
                ScopeRefEntry::from_wire(&reference.to_wire()).unwrap(),
                reference
            );
        }
    }

    #[test]
    fn setting_references_require_valid_json_pointers() {
        assert!(ScopeRefEntry::setting("").is_ok());
        for pointer in ["endpoint", "/bad~", "/bad~2escape"] {
            assert_eq!(
                ScopeRefEntry::setting(pointer).unwrap_err(),
                ScopeRefEntryError::InvalidSettingPointer
            );
        }
    }

    #[test]
    fn roots_use_bounded_lowercase_names() {
        assert!(ScopeRefEntry::root("a".repeat(MAX_ROOT_NAME_BYTES)).is_ok());
        for name in ["", "Workspace", "two_words", "9root"] {
            assert_eq!(
                ScopeRefEntry::root(name).unwrap_err(),
                ScopeRefEntryError::InvalidRootName
            );
        }
        assert_eq!(
            ScopeRefEntry::root("a".repeat(65)).unwrap_err(),
            ScopeRefEntryError::RootNameTooLong { max_bytes: 64 }
        );
    }

    #[test]
    fn literals_cannot_impersonate_symbolic_references() {
        for value in ["setting:/endpoint", "$workspace"] {
            assert_eq!(
                ScopeRefEntry::literal(value).unwrap_err(),
                ScopeRefEntryError::ReservedLiteralPrefix
            );
        }
    }

    #[test]
    fn literal_scopes_cannot_be_empty() {
        assert_eq!(
            ScopeRefEntry::literal("").unwrap_err(),
            ScopeRefEntryError::EmptyLiteral
        );
    }

    #[test]
    fn rejects_unsafe_root_subpaths() {
        let cases = [
            ("/absolute", ScopeRefEntryError::AbsoluteRootSubpath),
            ("", ScopeRefEntryError::EmptyRootSubpathSegment { index: 0 }),
            (".", ScopeRefEntryError::DotRootSubpathSegment { index: 0 }),
            (
                "generated//html",
                ScopeRefEntryError::EmptyRootSubpathSegment { index: 1 },
            ),
            (
                "generated/",
                ScopeRefEntryError::EmptyRootSubpathSegment { index: 1 },
            ),
            (
                "generated/../html",
                ScopeRefEntryError::DotRootSubpathSegment { index: 1 },
            ),
            (
                "generated\\html",
                ScopeRefEntryError::RootSubpathContainsBackslash,
            ),
            (
                "generated\0html",
                ScopeRefEntryError::DisallowedCharacter {
                    kind: ScopeValueKind::RootSubpathSegment,
                    byte_index: 9,
                    character: '\0',
                },
            ),
        ];
        for (subpath, expected) in cases {
            assert_eq!(
                ScopeRefEntry::root("workspace")
                    .unwrap()
                    .join(subpath)
                    .unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn bounds_root_subpath_length_and_depth() {
        assert!(
            ScopeRefEntry::root("workspace")
                .unwrap()
                .join("a".repeat(MAX_ROOT_SUBPATH_BYTES))
                .is_ok()
        );
        assert!(
            ScopeRefEntry::root("workspace")
                .unwrap()
                .join(["a"; MAX_ROOT_SUBPATH_SEGMENTS].join("/"))
                .is_ok()
        );
        assert_eq!(
            ScopeRefEntry::root("workspace")
                .unwrap()
                .join("a".repeat(1025))
                .unwrap_err(),
            ScopeRefEntryError::RootSubpathTooLong { max_bytes: 1024 }
        );
        assert_eq!(
            ScopeRefEntry::root("workspace")
                .unwrap()
                .join(["a"; 65].join("/"))
                .unwrap_err(),
            ScopeRefEntryError::RootSubpathTooDeep { max_segments: 64 }
        );
    }
}
