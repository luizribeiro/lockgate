use std::{error::Error, fmt};

const MAX_ROOT_NAME_BYTES: usize = 64;

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

    /// Parses the canonical symbolic wire spelling.
    pub fn from_wire(value: &str) -> Result<Self, ScopeRefError> {
        if let Some(pointer) = value.strip_prefix("setting:") {
            Self::setting(pointer)
        } else if let Some(name) = value.strip_prefix('$') {
            Self::root(name)
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
                if subpath.is_some() {
                    return Err(ScopeRefError::InvalidRootSubpath);
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
    InvalidRootSubpath,
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
            Self::InvalidRootSubpath => formatter.write_str("root subpath is invalid"),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_references_round_trip_through_wire_strings() {
        for reference in [
            ScopeRef::literal("https://example.com").unwrap(),
            ScopeRef::setting("/endpoint/~0name/~1path").unwrap(),
            ScopeRef::root("workspace").unwrap(),
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
}
