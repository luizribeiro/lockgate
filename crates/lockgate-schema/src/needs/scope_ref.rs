use std::{error::Error, fmt};

const MAX_ROOT_NAME_BYTES: usize = 64;
const MAX_ROOT_SUBPATH_BYTES: usize = 1024;
const MAX_ROOT_SUBPATH_SEGMENTS: usize = 64;

/// A symbolic scope in a plugin's needs declaration.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ScopeRef {
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

impl ScopeRef {
    /// Creates a literal scope. Reserved symbolic prefixes are not literals.
    pub fn literal(value: impl Into<String>) -> Result<Self, ScopeRefError> {
        let reference = Self::Literal(value.into());
        reference.validate()?;
        Ok(reference)
    }

    /// Creates a settings reference from an RFC 6901 JSON Pointer.
    pub fn setting(pointer: impl Into<String>) -> Result<Self, ScopeRefError> {
        let reference = Self::Setting(pointer.into());
        reference.validate()?;
        Ok(reference)
    }

    /// Creates a symbolic root reference.
    pub fn root(name: impl Into<String>) -> Result<Self, ScopeRefError> {
        let reference = Self::Root {
            name: name.into(),
            subpath: None,
        };
        reference.validate()?;
        Ok(reference)
    }

    /// Narrows a symbolic root to a validated relative subpath.
    pub fn join(self, subpath: impl Into<String>) -> Result<Self, ScopeRefError> {
        let Self::Root {
            name,
            subpath: current,
        } = self
        else {
            return Err(ScopeRefError::JoinRequiresRoot);
        };
        if current.is_some() {
            return Err(ScopeRefError::RootAlreadyJoined);
        }
        let reference = Self::Root {
            name,
            subpath: Some(subpath.into()),
        };
        reference.validate()?;
        Ok(reference)
    }

    /// Parses the canonical symbolic wire spelling.
    pub fn from_wire(value: &str) -> Result<Self, ScopeRefError> {
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

    pub(crate) fn validate(&self) -> Result<(), ScopeRefError> {
        match self {
            Self::Literal(value) if value.starts_with("setting:") || value.starts_with('$') => {
                Err(ScopeRefError::ReservedLiteralPrefix)
            }
            Self::Literal(_) => Ok(()),
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
pub enum ScopeRefError {
    ReservedLiteralPrefix,
    InvalidSettingPointer,
    InvalidRootName,
    RootNameTooLong { max_bytes: usize },
    JoinRequiresRoot,
    RootAlreadyJoined,
    AbsoluteRootSubpath,
    EmptyRootSubpathSegment { index: usize },
    DotRootSubpathSegment { index: usize },
    RootSubpathContainsBackslash,
    RootSubpathContainsNul,
    RootSubpathTooLong { max_bytes: usize },
    RootSubpathTooDeep { max_segments: usize },
}

impl fmt::Display for ScopeRefError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::ReservedLiteralPrefix => {
                formatter.write_str("literal scope must not use `setting:` or `$` prefix")
            }
            Self::InvalidSettingPointer => formatter.write_str(
                "setting reference must be an RFC 6901 JSON Pointer with valid `~0`/`~1` escapes",
            ),
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
            Self::RootSubpathContainsNul => {
                formatter.write_str("root subpath must not contain NUL")
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

impl Error for ScopeRefError {}

fn validate_json_pointer(pointer: &str) -> Result<(), ScopeRefError> {
    if !pointer.is_empty() && !pointer.starts_with('/') {
        return Err(ScopeRefError::InvalidSettingPointer);
    }
    let mut bytes = pointer.bytes();
    while let Some(byte) = bytes.next() {
        if byte == b'~' && !matches!(bytes.next(), Some(b'0' | b'1')) {
            return Err(ScopeRefError::InvalidSettingPointer);
        }
    }
    Ok(())
}

fn validate_root_name(name: &str) -> Result<(), ScopeRefError> {
    if name.len() > MAX_ROOT_NAME_BYTES {
        return Err(ScopeRefError::RootNameTooLong {
            max_bytes: MAX_ROOT_NAME_BYTES,
        });
    }
    let mut bytes = name.bytes();
    if !matches!(bytes.next(), Some(b'a'..=b'z'))
        || !bytes.all(|byte| matches!(byte, b'a'..=b'z' | b'0'..=b'9' | b'-'))
    {
        return Err(ScopeRefError::InvalidRootName);
    }
    Ok(())
}

fn validate_root_subpath(subpath: &str) -> Result<(), ScopeRefError> {
    if subpath.starts_with('/') {
        return Err(ScopeRefError::AbsoluteRootSubpath);
    }
    if subpath.contains('\\') {
        return Err(ScopeRefError::RootSubpathContainsBackslash);
    }
    if subpath.contains('\0') {
        return Err(ScopeRefError::RootSubpathContainsNul);
    }
    if subpath.len() > MAX_ROOT_SUBPATH_BYTES {
        return Err(ScopeRefError::RootSubpathTooLong {
            max_bytes: MAX_ROOT_SUBPATH_BYTES,
        });
    }
    for (index, segment) in subpath.split('/').enumerate() {
        if index >= MAX_ROOT_SUBPATH_SEGMENTS {
            return Err(ScopeRefError::RootSubpathTooDeep {
                max_segments: MAX_ROOT_SUBPATH_SEGMENTS,
            });
        }
        if segment.is_empty() {
            return Err(ScopeRefError::EmptyRootSubpathSegment { index });
        }
        if matches!(segment, "." | "..") {
            return Err(ScopeRefError::DotRootSubpathSegment { index });
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
            ScopeRef::literal("https://example.com").unwrap(),
            ScopeRef::setting("/endpoint/~0name/~1path").unwrap(),
            ScopeRef::root("workspace").unwrap(),
            ScopeRef::root("workspace")
                .unwrap()
                .join("generated/html")
                .unwrap(),
        ] {
            assert_eq!(
                ScopeRef::from_wire(&reference.to_wire()).unwrap(),
                reference
            );
        }
    }

    #[test]
    fn setting_references_require_valid_json_pointers() {
        assert!(ScopeRef::setting("").is_ok());
        for pointer in ["endpoint", "/bad~", "/bad~2escape"] {
            assert_eq!(
                ScopeRef::setting(pointer).unwrap_err(),
                ScopeRefError::InvalidSettingPointer
            );
        }
    }

    #[test]
    fn roots_use_bounded_lowercase_names() {
        for name in ["", "Workspace", "two_words", "9root"] {
            assert_eq!(
                ScopeRef::root(name).unwrap_err(),
                ScopeRefError::InvalidRootName
            );
        }
        assert_eq!(
            ScopeRef::root("a".repeat(65)).unwrap_err(),
            ScopeRefError::RootNameTooLong { max_bytes: 64 }
        );
    }

    #[test]
    fn literals_cannot_impersonate_symbolic_references() {
        for value in ["setting:/endpoint", "$workspace"] {
            assert_eq!(
                ScopeRef::literal(value).unwrap_err(),
                ScopeRefError::ReservedLiteralPrefix
            );
        }
    }

    #[test]
    fn rejects_unsafe_root_subpaths() {
        let cases = [
            ("/absolute", ScopeRefError::AbsoluteRootSubpath),
            ("", ScopeRefError::EmptyRootSubpathSegment { index: 0 }),
            (".", ScopeRefError::DotRootSubpathSegment { index: 0 }),
            (
                "generated//html",
                ScopeRefError::EmptyRootSubpathSegment { index: 1 },
            ),
            (
                "generated/",
                ScopeRefError::EmptyRootSubpathSegment { index: 1 },
            ),
            (
                "generated/../html",
                ScopeRefError::DotRootSubpathSegment { index: 1 },
            ),
            (
                "generated\\html",
                ScopeRefError::RootSubpathContainsBackslash,
            ),
            ("generated\0html", ScopeRefError::RootSubpathContainsNul),
        ];
        for (subpath, expected) in cases {
            assert_eq!(
                ScopeRef::root("workspace")
                    .unwrap()
                    .join(subpath)
                    .unwrap_err(),
                expected
            );
        }
    }

    #[test]
    fn bounds_root_subpath_length_and_depth() {
        assert_eq!(
            ScopeRef::root("workspace")
                .unwrap()
                .join("a".repeat(1025))
                .unwrap_err(),
            ScopeRefError::RootSubpathTooLong { max_bytes: 1024 }
        );
        assert_eq!(
            ScopeRef::root("workspace")
                .unwrap()
                .join(["a"; 65].join("/"))
                .unwrap_err(),
            ScopeRefError::RootSubpathTooDeep { max_segments: 64 }
        );
    }
}
