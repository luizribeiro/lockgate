use std::{error::Error, fmt};

use super::{AtomKey, ScopeRef, ScopeRefError};

/// The operation kind and symbolic scope references declared by one need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NeedKind {
    /// An unscoped yes/no operation.
    Flag,
    /// An operation over a non-empty union of symbolic scopes.
    Scoped(Vec<ScopeRef>),
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
    pub fn scoped(atom: AtomKey, mut scopes: Vec<ScopeRef>) -> Result<Self, NeedEntryError> {
        if scopes.is_empty() {
            return Err(NeedEntryError::EmptyScopes);
        }
        for (index, reference) in scopes.iter().enumerate() {
            reference
                .validate()
                .map_err(|source| NeedEntryError::InvalidScope { index, source })?;
        }
        scopes.sort_by_key(ScopeRef::to_wire);
        scopes.dedup();
        Ok(Self {
            atom,
            kind: NeedKind::Scoped(scopes),
            reason: None,
        })
    }

    /// Returns the permission atom.
    pub fn atom(&self) -> &AtomKey {
        &self.atom
    }

    /// Returns whether the operation is a flag or scoped operation.
    pub fn kind(&self) -> &NeedKind {
        &self.kind
    }
}

/// A malformed need entry.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NeedEntryError {
    EmptyScopes,
    InvalidScope { index: usize, source: ScopeRefError },
}

impl fmt::Display for NeedEntryError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScopes => formatter.write_str("scoped need must contain at least one scope"),
            Self::InvalidScope { index, source } => {
                write!(formatter, "scope reference {index} is invalid: {source}")
            }
        }
    }
}

impl Error for NeedEntryError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::EmptyScopes => None,
            Self::InvalidScope { source, .. } => Some(source),
        }
    }
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
                ScopeRef::root("workspace").unwrap(),
                ScopeRef::literal("all").unwrap(),
                ScopeRef::root("workspace").unwrap(),
            ],
        )
        .unwrap();

        assert_eq!(
            entry.kind(),
            &NeedKind::Scoped(vec![
                ScopeRef::root("workspace").unwrap(),
                ScopeRef::literal("all").unwrap(),
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
            vec![ScopeRef::Root {
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
}
