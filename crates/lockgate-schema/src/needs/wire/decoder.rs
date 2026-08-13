use std::{collections::HashMap, fmt, marker::PhantomData, str::FromStr};

use serde::{Deserialize, Deserializer, de::MapAccess, de::Visitor};

use crate::MAX_SECTION_PAYLOAD_BYTES;

use crate::needs::{
    AtomKey, EntryLocation, NeedEntry, NeedsManifest, NeedsManifestValidationError, Requirement,
    ScopeRef, entry::validate_reason, manifest::NEEDS_FORMAT,
};

use super::NeedsManifestDecodeError;

/// Decodes and validates a `lockgate:needs` custom-section payload.
pub fn decode_needs_manifest(bytes: &[u8]) -> Result<NeedsManifest, NeedsManifestDecodeError> {
    if bytes.len() > MAX_SECTION_PAYLOAD_BYTES {
        return Err(NeedsManifestDecodeError::PayloadTooLarge {
            actual_bytes: bytes.len(),
            max_bytes: MAX_SECTION_PAYLOAD_BYTES,
        });
    }
    let raw: RawManifest =
        serde_json::from_slice(bytes).map_err(NeedsManifestDecodeError::InvalidJson)?;
    if raw.format != NEEDS_FORMAT {
        return Err(NeedsManifestDecodeError::InvalidManifest(
            NeedsManifestValidationError::UnsupportedFormat { found: raw.format },
        ));
    }
    let required = decode_entries(raw.required.0, Requirement::Required)?;
    let optional = decode_entries(raw.optional.0, Requirement::Optional)?;
    let locations = required
        .iter()
        .enumerate()
        .map(|(index, entry)| {
            (
                entry.atom.clone(),
                EntryLocation {
                    requirement: Requirement::Required,
                    index,
                },
            )
        })
        .chain(optional.iter().enumerate().map(|(index, entry)| {
            (
                entry.atom.clone(),
                EntryLocation {
                    requirement: Requirement::Optional,
                    index,
                },
            )
        }))
        .collect::<HashMap<_, _>>();
    let mut manifest = NeedsManifest::new(required, optional)
        .map_err(NeedsManifestDecodeError::InvalidManifest)?;
    attach_reasons(&mut manifest, raw.reasons.0, &locations)?;
    manifest
        .validate()
        .map_err(NeedsManifestDecodeError::InvalidManifest)?;
    Ok(manifest)
}

fn decode_entries(
    raw: Vec<(String, RawNeed)>,
    requirement: Requirement,
) -> Result<Vec<NeedEntry>, NeedsManifestDecodeError> {
    raw.into_iter()
        .enumerate()
        .map(|(index, (value, need))| {
            let location = EntryLocation { requirement, index };
            let atom = AtomKey::from_str(&value).map_err(|source| {
                NeedsManifestDecodeError::InvalidAtom {
                    location,
                    value,
                    source,
                }
            })?;
            match need {
                RawNeed::Flag(true) => Ok(NeedEntry::flag(atom)),
                RawNeed::Flag(false) => Err(NeedsManifestDecodeError::FalseFlag { location, atom }),
                RawNeed::Scoped(values) => {
                    let scopes = values
                        .into_iter()
                        .enumerate()
                        .map(|(scope_index, value)| {
                            ScopeRef::from_wire(&value).map_err(|source| {
                                NeedsManifestDecodeError::InvalidScope {
                                    location,
                                    atom: atom.clone(),
                                    scope_index,
                                    value,
                                    source,
                                }
                            })
                        })
                        .collect::<Result<Vec<_>, _>>()?;
                    NeedEntry::scoped(atom.clone(), scopes).map_err(|source| {
                        NeedsManifestDecodeError::InvalidManifest(
                            NeedsManifestValidationError::InvalidEntry {
                                location,
                                atom,
                                source,
                            },
                        )
                    })
                }
            }
        })
        .collect()
}

fn attach_reasons(
    manifest: &mut NeedsManifest,
    reasons: Vec<(String, String)>,
    locations: &HashMap<AtomKey, EntryLocation>,
) -> Result<(), NeedsManifestDecodeError> {
    let mut seen = HashMap::new();
    for (reason_index, (value, reason)) in reasons.into_iter().enumerate() {
        let atom = AtomKey::from_str(&value).map_err(|source| {
            NeedsManifestDecodeError::InvalidReasonAtom {
                reason_index,
                value,
                source,
            }
        })?;
        if let Some(first_index) = seen.insert(atom.clone(), reason_index) {
            return Err(NeedsManifestDecodeError::DuplicateReason {
                atom,
                first_index,
                duplicate_index: reason_index,
            });
        }
        let Some(&location) = locations.get(&atom) else {
            return Err(NeedsManifestDecodeError::UndeclaredReason { reason_index, atom });
        };
        validate_reason(&reason).map_err(|source| NeedsManifestDecodeError::InvalidReason {
            location,
            atom: atom.clone(),
            reason_index,
            source,
        })?;
        let entries = match location.requirement {
            Requirement::Required => &mut manifest.required,
            Requirement::Optional => &mut manifest.optional,
        };
        let entry = entries
            .iter_mut()
            .find(|entry| entry.atom == atom)
            .ok_or_else(|| NeedsManifestDecodeError::UndeclaredReason {
                reason_index,
                atom: atom.clone(),
            })?;
        entry.reason = Some(reason);
    }
    Ok(())
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawManifest {
    format: u32,
    optional: PairMap<RawNeed>,
    reasons: PairMap<String>,
    required: PairMap<RawNeed>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum RawNeed {
    Flag(bool),
    Scoped(Vec<String>),
}

struct PairMap<T>(Vec<(String, T)>);

impl<'de, T: Deserialize<'de>> Deserialize<'de> for PairMap<T> {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct PairMapVisitor<T>(PhantomData<T>);

        impl<'de, T: Deserialize<'de>> Visitor<'de> for PairMapVisitor<T> {
            type Value = PairMap<T>;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("an object keyed by permission atom")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut pairs = Vec::new();
                while let Some(pair) = map.next_entry()? {
                    pairs.push(pair);
                }
                Ok(PairMap(pairs))
            }
        }

        deserializer.deserialize_map(PairMapVisitor(PhantomData))
    }
}
