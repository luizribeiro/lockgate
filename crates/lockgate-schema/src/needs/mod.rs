use std::{error::Error, fmt, str::FromStr};

mod digest;
mod entry;
mod manifest;
mod scope_ref;
mod wire;

pub use digest::NeedsDigest;
pub use entry::{MAX_SCOPES_PER_ENTRY, NeedEntry, NeedEntryError, NeedKind, NeedReasonError};
pub use manifest::{
    EntryLocation, MAX_ATOMS_PER_MANIFEST, NeedsManifest, NeedsManifestValidationError, Requirement,
};
pub use scope_ref::{
    MAX_ROOT_NAME_BYTES, MAX_ROOT_SUBPATH_BYTES, MAX_ROOT_SUBPATH_SEGMENTS, MAX_SCOPE_VALUE_BYTES,
    ScopeCharacterKind, ScopeRef, ScopeRefError, ScopeValueKind,
};
pub use wire::{
    NeedsManifestDecodeError, NeedsManifestEncodeError, decode_needs_manifest,
    encode_needs_manifest,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DisallowedCharacterKind {
    Control,
    Format,
}

pub(crate) fn find_disallowed_character(value: &str) -> Option<(usize, DisallowedCharacterKind)> {
    value.char_indices().find_map(|(byte_index, character)| {
        if character.is_control() {
            Some((byte_index, DisallowedCharacterKind::Control))
        } else if is_format_character(character) {
            Some((byte_index, DisallowedCharacterKind::Format))
        } else {
            None
        }
    })
}

// Rust exposes Unicode Cc through `is_control`, but not Cf. Keep this explicit
// table synchronized with Unicode's format-character assignments.
fn is_format_character(character: char) -> bool {
    matches!(
        character,
        '\u{00ad}'
            | '\u{0600}'..='\u{0605}'
            | '\u{061c}'
            | '\u{06dd}'
            | '\u{070f}'
            | '\u{0890}'..='\u{0891}'
            | '\u{08e2}'
            | '\u{180e}'
            | '\u{200b}'..='\u{200f}'
            | '\u{202a}'..='\u{202e}'
            | '\u{2060}'..='\u{2064}'
            | '\u{2066}'..='\u{206f}'
            | '\u{feff}'
            | '\u{fff9}'..='\u{fffb}'
            | '\u{110bd}'
            | '\u{110cd}'
            | '\u{13430}'..='\u{1343f}'
            | '\u{1bca0}'..='\u{1bca3}'
            | '\u{1d173}'..='\u{1d17a}'
            | '\u{e0001}'
            | '\u{e0020}'..='\u{e007f}'
    )
}

/// The identity of one permission operation, written `capability.operation`.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct AtomKey {
    capability: String,
    operation: String,
}

impl AtomKey {
    /// Creates an atom key from its two non-empty segments.
    pub fn new(
        capability: impl Into<String>,
        operation: impl Into<String>,
    ) -> Result<Self, AtomKeyError> {
        let capability = capability.into();
        let operation = operation.into();
        if capability.is_empty() {
            return Err(AtomKeyError::EmptyCapability);
        }
        if operation.is_empty() {
            return Err(AtomKeyError::EmptyOperation);
        }
        if capability.contains('.') || operation.contains('.') {
            return Err(AtomKeyError::WrongSegmentCount {
                value: format!("{capability}.{operation}"),
            });
        }
        Ok(Self {
            capability,
            operation,
        })
    }

    /// Returns the capability segment.
    pub fn capability(&self) -> &str {
        &self.capability
    }

    /// Returns the operation segment.
    pub fn operation(&self) -> &str {
        &self.operation
    }
}

impl FromStr for AtomKey {
    type Err = AtomKeyError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let Some((capability, operation)) = value.split_once('.') else {
            return Err(AtomKeyError::WrongSegmentCount {
                value: value.into(),
            });
        };
        Self::new(capability, operation)
    }
}

impl fmt::Display for AtomKey {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}.{}", self.capability, self.operation)
    }
}

/// A malformed permission atom key.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum AtomKeyError {
    /// The key does not contain exactly two dot-separated segments.
    WrongSegmentCount { value: String },
    /// The capability segment is empty.
    EmptyCapability,
    /// The operation segment is empty.
    EmptyOperation,
}

impl fmt::Display for AtomKeyError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::WrongSegmentCount { value } => write!(
                formatter,
                "atom key `{value}` must have exactly two segments: capability.operation"
            ),
            Self::EmptyCapability => formatter.write_str("atom key capability must not be empty"),
            Self::EmptyOperation => formatter.write_str("atom key operation must not be empty"),
        }
    }
}

impl Error for AtomKeyError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn atom_key_round_trips_through_its_symbolic_form() {
        let atom: AtomKey = "sessions.edit-history".parse().unwrap();

        assert_eq!(atom.capability(), "sessions");
        assert_eq!(atom.operation(), "edit-history");
        assert_eq!(atom.to_string(), "sessions.edit-history");
    }

    #[test]
    fn atom_key_requires_exactly_two_segments() {
        for value in ["sessions", "sessions.read.extra"] {
            assert!(matches!(
                value.parse::<AtomKey>(),
                Err(AtomKeyError::WrongSegmentCount { .. })
            ));
        }
    }

    #[test]
    fn atom_key_requires_both_segments() {
        assert_eq!(
            ".read".parse::<AtomKey>().unwrap_err(),
            AtomKeyError::EmptyCapability
        );
        assert_eq!(
            "sessions.".parse::<AtomKey>().unwrap_err(),
            AtomKeyError::EmptyOperation
        );
    }
}
