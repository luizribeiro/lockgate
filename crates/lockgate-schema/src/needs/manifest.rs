use std::{collections::HashMap, error::Error, fmt};

use super::{AtomKey, NeedEntry, NeedEntryError};

pub(crate) const NEEDS_FORMAT: u32 = 1;

/// A plugin's symbolic required and optional permission needs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NeedsManifest {
    pub(crate) format: u32,
    pub(crate) required: Vec<NeedEntry>,
    pub(crate) optional: Vec<NeedEntry>,
}

impl NeedsManifest {
    /// Creates a validated manifest in canonical atom order.
    pub fn new(
        required: Vec<NeedEntry>,
        optional: Vec<NeedEntry>,
    ) -> Result<Self, NeedsManifestValidationError> {
        let mut manifest = Self {
            format: NEEDS_FORMAT,
            required,
            optional,
        };
        manifest.validate()?;
        manifest
            .required
            .sort_by(|left, right| left.atom.cmp(&right.atom));
        manifest
            .optional
            .sort_by(|left, right| left.atom.cmp(&right.atom));
        Ok(manifest)
    }

    /// Returns the valid deny-by-default empty manifest.
    pub fn empty() -> Self {
        Self {
            format: NEEDS_FORMAT,
            required: Vec::new(),
            optional: Vec::new(),
        }
    }

    /// Returns required permission needs.
    pub fn required(&self) -> &[NeedEntry] {
        &self.required
    }

    /// Returns optional permission needs.
    pub fn optional(&self) -> &[NeedEntry] {
        &self.optional
    }

    pub(crate) fn validate(&self) -> Result<(), NeedsManifestValidationError> {
        if self.format != NEEDS_FORMAT {
            return Err(NeedsManifestValidationError::UnsupportedFormat { found: self.format });
        }
        let mut atoms = HashMap::new();
        for (requirement, entries) in [
            (Requirement::Required, &self.required),
            (Requirement::Optional, &self.optional),
        ] {
            for (index, entry) in entries.iter().enumerate() {
                let location = EntryLocation { requirement, index };
                entry
                    .validate()
                    .map_err(|source| NeedsManifestValidationError::InvalidEntry {
                        location,
                        atom: entry.atom.clone(),
                        source,
                    })?;
                if let Some(first) = atoms.insert(entry.atom.clone(), location) {
                    return Err(NeedsManifestValidationError::DuplicateAtom {
                        atom: entry.atom.clone(),
                        first,
                        duplicate: location,
                    });
                }
            }
        }
        Ok(())
    }
}

impl Default for NeedsManifest {
    fn default() -> Self {
        Self::empty()
    }
}

/// Whether a manifest entry is required or optional.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Requirement {
    Required,
    Optional,
}

impl fmt::Display for Requirement {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Required => "required",
            Self::Optional => "optional",
        })
    }
}

/// The list and zero-based index of a manifest entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EntryLocation {
    pub requirement: Requirement,
    pub index: usize,
}

impl fmt::Display for EntryLocation {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{} entry {}", self.requirement, self.index)
    }
}

/// A semantic validation failure in a needs manifest.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum NeedsManifestValidationError {
    UnsupportedFormat {
        found: u32,
    },
    InvalidEntry {
        location: EntryLocation,
        atom: AtomKey,
        source: NeedEntryError,
    },
    DuplicateAtom {
        atom: AtomKey,
        first: EntryLocation,
        duplicate: EntryLocation,
    },
}

impl fmt::Display for NeedsManifestValidationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::UnsupportedFormat { found } => {
                write!(formatter, "unsupported needs manifest format {found}")
            }
            Self::InvalidEntry {
                location,
                atom,
                source,
            } => {
                write!(formatter, "{location} (`{atom}`) is invalid: {source}")
            }
            Self::DuplicateAtom {
                atom,
                first,
                duplicate,
            } => write!(
                formatter,
                "{duplicate} duplicates atom `{atom}` first declared at {first}"
            ),
        }
    }
}

impl Error for NeedsManifestValidationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::InvalidEntry { source, .. } => Some(source),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn flag(atom: &str) -> NeedEntry {
        NeedEntry::flag(atom.parse().unwrap())
    }

    #[test]
    fn empty_manifest_is_valid() {
        assert_eq!(
            NeedsManifest::default(),
            NeedsManifest::new(vec![], vec![]).unwrap()
        );
    }

    #[test]
    fn entries_are_sorted_by_atom() {
        let manifest = NeedsManifest::new(
            vec![flag("notify.send"), flag("fs.read")],
            vec![flag("state.write"), flag("state.read")],
        )
        .unwrap();

        assert_eq!(manifest.required()[0].atom().to_string(), "fs.read");
        assert_eq!(manifest.optional()[0].atom().to_string(), "state.read");
    }

    #[test]
    fn duplicate_errors_name_both_entry_locations() {
        let error =
            NeedsManifest::new(vec![flag("notify.send")], vec![flag("notify.send")]).unwrap_err();

        assert!(error.to_string().contains(
            "optional entry 0 duplicates atom `notify.send` first declared at required entry 0"
        ));
    }
}
