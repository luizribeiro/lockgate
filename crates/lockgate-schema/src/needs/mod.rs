use std::{cmp::Ordering, error::Error, fmt, iter, str::FromStr};

use serde::{Deserialize, Deserializer, Serialize, Serializer, de};

mod digest;
mod entry;
mod manifest;
mod scope_ref;

pub use digest::{NeedsDigest, hex_encode};
pub(crate) use entry::validate_reason;
pub use entry::{MAX_SCOPES_PER_ENTRY, NeedEntry, NeedEntryError, NeedKind, NeedReasonError};
pub(crate) use manifest::NEEDS_FORMAT;
pub use manifest::{
    EntryLocation, MAX_ATOMS_PER_MANIFEST, NeedsManifest, NeedsManifestValidationError, Requirement,
};
pub use scope_ref::{
    MAX_ROOT_NAME_BYTES, MAX_ROOT_SUBPATH_BYTES, MAX_ROOT_SUBPATH_SEGMENTS, MAX_SCOPE_VALUE_BYTES,
    ScopeRefEntry, ScopeRefEntryError, ScopeValueKind,
};

/// The identity of one permission operation, written `capability.operation`.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AtomKey {
    capability: String,
    operation: String,
}

impl Ord for AtomKey {
    fn cmp(&self, other: &Self) -> Ordering {
        // Atom keys are serialized as one `capability.operation` string. Keep
        // every ordered schema collection aligned with that byte order,
        // including the `ab-c.x`/`ab.c-x` prefix case where derived struct
        // ordering would compare the capability fields differently.
        self.capability
            .bytes()
            .chain(iter::once(b'.'))
            .chain(self.operation.bytes())
            .cmp(
                other
                    .capability
                    .bytes()
                    .chain(iter::once(b'.'))
                    .chain(other.operation.bytes()),
            )
    }
}

impl PartialOrd for AtomKey {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
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

impl Serialize for AtomKey {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_str(self)
    }
}

impl<'de> Deserialize<'de> for AtomKey {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = String::deserialize(deserializer)?;
        value.parse().map_err(de::Error::custom)
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

    #[test]
    fn atom_key_order_matches_its_wire_string_order() {
        let prefixed: AtomKey = "ab-c.x".parse().unwrap();
        let shorter: AtomKey = "ab.c-x".parse().unwrap();

        assert!(prefixed < shorter);
        assert!(prefixed.to_string() < shorter.to_string());
    }
}
