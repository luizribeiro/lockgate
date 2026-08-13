use std::{error::Error, fmt};

use crate::needs::{
    AtomKey, AtomKeyError, EntryLocation, NeedReasonError, NeedsManifestValidationError,
    ScopeRefError,
};

/// A failure while decoding a needs manifest custom-section payload.
#[derive(Debug)]
#[non_exhaustive]
pub enum NeedsManifestDecodeError {
    PayloadTooLarge {
        actual_bytes: usize,
        max_bytes: usize,
    },
    InvalidJson(serde_json::Error),
    InvalidAtom {
        location: EntryLocation,
        value: String,
        source: AtomKeyError,
    },
    FalseFlag {
        location: EntryLocation,
        atom: AtomKey,
    },
    InvalidScope {
        location: EntryLocation,
        atom: AtomKey,
        scope_index: usize,
        value: String,
        source: ScopeRefError,
    },
    InvalidReasonAtom {
        reason_index: usize,
        value: String,
        source: AtomKeyError,
    },
    DuplicateReason {
        atom: AtomKey,
        first_index: usize,
        duplicate_index: usize,
    },
    UndeclaredReason {
        reason_index: usize,
        atom: AtomKey,
    },
    InvalidReason {
        location: EntryLocation,
        atom: AtomKey,
        reason_index: usize,
        source: NeedReasonError,
    },
    InvalidManifest(NeedsManifestValidationError),
}

impl fmt::Display for NeedsManifestDecodeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::PayloadTooLarge {
                actual_bytes,
                max_bytes,
            } => write!(
                formatter,
                "needs manifest payload is {actual_bytes} bytes; maximum is {max_bytes} bytes"
            ),
            Self::InvalidJson(error) => {
                write!(
                    formatter,
                    "needs manifest is not valid schema JSON: {error}"
                )
            }
            Self::InvalidAtom {
                location,
                value,
                source,
            } => {
                write!(
                    formatter,
                    "{location} has invalid atom key `{value}`: {source}"
                )
            }
            Self::FalseFlag { location, atom } => {
                write!(
                    formatter,
                    "{location} (`{atom}`) is a false flag; declared flags must be true"
                )
            }
            Self::InvalidScope {
                location,
                atom,
                scope_index,
                value,
                source,
            } => write!(
                formatter,
                "{location} (`{atom}`) has invalid scope {scope_index} `{value}`: {source}"
            ),
            Self::InvalidReasonAtom {
                reason_index,
                value,
                source,
            } => write!(
                formatter,
                "reason entry {reason_index} has invalid atom key `{value}`: {source}"
            ),
            Self::DuplicateReason {
                atom,
                first_index,
                duplicate_index,
            } => write!(
                formatter,
                "reason entry {duplicate_index} duplicates atom `{atom}` from reason entry {first_index}"
            ),
            Self::UndeclaredReason { reason_index, atom } => write!(
                formatter,
                "reason entry {reason_index} names undeclared atom `{atom}`"
            ),
            Self::InvalidReason {
                location,
                atom,
                reason_index,
                source,
            } => write!(
                formatter,
                "reason entry {reason_index} for {location} (`{atom}`) is invalid: {source}"
            ),
            Self::InvalidManifest(error) => {
                write!(formatter, "needs manifest failed validation: {error}")
            }
        }
    }
}

impl Error for NeedsManifestDecodeError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::PayloadTooLarge { .. } => None,
            Self::InvalidJson(error) => Some(error),
            Self::InvalidAtom { source, .. } | Self::InvalidReasonAtom { source, .. } => {
                Some(source)
            }
            Self::InvalidScope { source, .. } => Some(source),
            Self::InvalidReason { source, .. } => Some(source),
            Self::InvalidManifest(error) => Some(error),
            Self::FalseFlag { .. }
            | Self::DuplicateReason { .. }
            | Self::UndeclaredReason { .. } => None,
        }
    }
}
