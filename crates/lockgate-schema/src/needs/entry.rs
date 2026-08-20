use std::{error::Error, fmt};

use super::{AtomKey, ScopeRefEntry, ScopeRefEntryError};
use crate::text::{DisplayStringViolation, classify_disallowed_character, validate_display_string};

/// Maximum number of scope values in one scoped entry before deduplication.
pub const MAX_SCOPES_PER_ENTRY: usize = 128;

/// The operation kind and symbolic scope references declared by one need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NeedKind {
    /// An unscoped yes/no operation.
    Flag,
    /// An operation over a non-empty union of symbolic scopes.
    Scoped(Vec<ScopeRefEntry>),
}

/// One symbolic permission need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedEntry {
    pub(crate) atom: AtomKey,
    pub(crate) kind: NeedKind,
    pub(crate) reason: Option<String>,
}

impl NeedEntry {
    /// Declares an unscoped flag operation.
    pub fn flag(atom: AtomKey) -> Self {
        Self {
            atom,
            kind: NeedKind::Flag,
            reason: None,
        }
    }

    /// Declares a scoped operation and canonicalizes its set of references.
    pub fn scoped(atom: AtomKey, mut scopes: Vec<ScopeRefEntry>) -> Result<Self, NeedEntryError> {
        validate_scopes(&scopes)?;
        scopes.sort_by_key(ScopeRefEntry::to_wire);
        scopes.dedup();
        Ok(Self {
            atom,
            kind: NeedKind::Scoped(scopes),
            reason: None,
        })
    }

    /// Attaches one line of plugin-authored consent prose.
    pub fn with_reason(mut self, reason: impl Into<String>) -> Result<Self, NeedEntryError> {
        let reason = reason.into();
        validate_reason(&reason).map_err(NeedEntryError::InvalidReason)?;
        self.reason = Some(reason);
        Ok(self)
    }

    /// Returns the permission atom.
    pub fn atom(&self) -> &AtomKey {
        &self.atom
    }

    /// Returns whether the operation is a flag or scoped operation.
    pub fn kind(&self) -> &NeedKind {
        &self.kind
    }

    /// Returns the optional plugin-authored reason.
    pub fn reason(&self) -> Option<&str> {
        self.reason.as_deref()
    }

    pub(crate) fn validate(&self) -> Result<(), NeedEntryError> {
        if let NeedKind::Scoped(scopes) = &self.kind {
            validate_scopes(scopes)?;
        }
        if let Some(reason) = &self.reason {
            validate_reason(reason).map_err(NeedEntryError::InvalidReason)?;
        }
        Ok(())
    }
}

fn validate_scopes(scopes: &[ScopeRefEntry]) -> Result<(), NeedEntryError> {
    if scopes.len() > MAX_SCOPES_PER_ENTRY {
        return Err(NeedEntryError::TooManyScopes {
            found: scopes.len(),
            max: MAX_SCOPES_PER_ENTRY,
        });
    }
    if scopes.is_empty() {
        return Err(NeedEntryError::EmptyScopes);
    }
    for (index, reference) in scopes.iter().enumerate() {
        reference
            .validate()
            .map_err(|source| NeedEntryError::InvalidScope { index, source })?;
    }
    Ok(())
}

/// A malformed need entry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NeedEntryError {
    EmptyScopes,
    TooManyScopes {
        found: usize,
        max: usize,
    },
    InvalidScope {
        index: usize,
        source: ScopeRefEntryError,
    },
    InvalidReason(NeedReasonError),
}

impl fmt::Display for NeedEntryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScopes => formatter.write_str("scoped need must contain at least one scope"),
            Self::TooManyScopes { found, max } => {
                write!(
                    formatter,
                    "scoped need contains {found} scopes; maximum is {max}"
                )
            }
            Self::InvalidScope { index, source } => {
                write!(formatter, "scope reference {index} is invalid: {source}")
            }
            Self::InvalidReason(source) => write!(formatter, "reason is invalid: {source}"),
        }
    }
}

impl Error for NeedEntryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EmptyScopes | Self::TooManyScopes { .. } => None,
            Self::InvalidScope { source, .. } => Some(source),
            Self::InvalidReason(source) => Some(source),
        }
    }
}

/// A malformed need reason.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NeedReasonError {
    Empty,
    DisallowedCharacter { byte_index: usize, character: char },
    TooLong { max_bytes: usize },
}

impl fmt::Display for NeedReasonError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Empty => formatter.write_str("reason must not be empty"),
            Self::DisallowedCharacter {
                byte_index,
                character,
            } => write!(
                formatter,
                "reason contains a {} at byte {byte_index}",
                classify_disallowed_character(*character)
                    .expect("stored reason character must be disallowed")
            ),
            Self::TooLong { max_bytes } => {
                write!(formatter, "reason exceeds {max_bytes} UTF-8 bytes")
            }
        }
    }
}

impl Error for NeedReasonError {}

pub(crate) fn validate_reason(reason: &str) -> Result<(), NeedReasonError> {
    validate_display_string(reason, 512, |_| false).map_err(|violation| match violation {
        DisplayStringViolation::Empty => NeedReasonError::Empty,
        DisplayStringViolation::DisallowedCharacter {
            byte_index,
            character,
        } => NeedReasonError::DisallowedCharacter {
            byte_index,
            character,
        },
        DisplayStringViolation::TooLong { max_bytes } => NeedReasonError::TooLong { max_bytes },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn atom() -> AtomKey {
        AtomKey::new("fs", "read").unwrap()
    }

    #[test]
    fn scoped_entries_are_non_empty_sets_in_wire_order() {
        let entry = NeedEntry::scoped(
            atom(),
            vec![
                ScopeRefEntry::root("workspace").unwrap(),
                ScopeRefEntry::literal("all").unwrap(),
                ScopeRefEntry::root("workspace").unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(
            entry.kind(),
            &NeedKind::Scoped(vec![
                ScopeRefEntry::root("workspace").unwrap(),
                ScopeRefEntry::literal("all").unwrap(),
            ])
        );
    }

    #[test]
    fn scoped_entries_reject_empty_lists() {
        assert_eq!(
            NeedEntry::scoped(atom(), Vec::new()).unwrap_err(),
            NeedEntryError::EmptyScopes
        );
    }

    #[test]
    fn validation_catches_publicly_constructed_invalid_references() {
        let error = NeedEntry::scoped(
            atom(),
            vec![ScopeRefEntry::Root {
                name: "Workspace".into(),
                subpath: None,
            }],
        )
        .unwrap_err();

        assert!(matches!(
            error,
            NeedEntryError::InvalidScope { index: 0, .. }
        ));
    }

    #[test]
    fn reasons_are_single_non_empty_lines_without_controls() {
        for (reason, expected) in [
            (" ", NeedReasonError::Empty),
            (
                "first\nsecond",
                NeedReasonError::DisallowedCharacter {
                    byte_index: 5,
                    character: '\n',
                },
            ),
            (
                "before\u{7}after",
                NeedReasonError::DisallowedCharacter {
                    byte_index: 6,
                    character: '\u{7}',
                },
            ),
            (
                "before\u{202e}after",
                NeedReasonError::DisallowedCharacter {
                    byte_index: 6,
                    character: '\u{202e}',
                },
            ),
            (
                "before\u{2028}after",
                NeedReasonError::DisallowedCharacter {
                    byte_index: 6,
                    character: '\u{2028}',
                },
            ),
        ] {
            assert_eq!(
                NeedEntry::flag(atom()).with_reason(reason).unwrap_err(),
                NeedEntryError::InvalidReason(expected)
            );
        }
    }

    #[test]
    fn reason_limit_counts_utf8_bytes() {
        assert!(NeedEntry::flag(atom()).with_reason("é".repeat(256)).is_ok());
        assert_eq!(
            NeedEntry::flag(atom())
                .with_reason("é".repeat(257))
                .unwrap_err(),
            NeedEntryError::InvalidReason(NeedReasonError::TooLong { max_bytes: 512 })
        );
    }
}
