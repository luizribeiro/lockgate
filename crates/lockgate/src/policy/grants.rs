//! Provenance-bound prepared digests and immutable admitted grants.

use std::fmt;

use lockgate_schema::{AtomKey, GrantSet, GrantValue, NeedsDigest};
use sha2::{Digest, Sha256};

use super::ResolvedNeeds;

/// The concrete permissions frozen for one admitted plugin.
///
/// Only Lockgate admission constructs this type. It intentionally has no
/// public constructor, mutable accessor, insertion method, or deserialization
/// path; guard expansion consumes its crate-internal read-only queries.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct EffectiveGrants {
    grants: GrantSet,
}

impl EffectiveGrants {
    pub(crate) fn from_resolved(resolved: ResolvedNeeds) -> Self {
        let mut grants = resolved.required;
        for (atom, value) in resolved.optional.iter() {
            let previous = match value {
                GrantValue::Flag => grants.insert_flag(atom.clone()),
                GrantValue::Scopes(scopes) => grants
                    .insert_scopes(atom.clone(), scopes.iter().cloned())
                    .expect("resolved scoped grants are non-empty"),
            };
            debug_assert!(
                previous.is_none(),
                "validated needs cannot repeat an atom across requirement groups"
            );
        }
        Self { grants }
    }

    #[allow(
        dead_code,
        reason = "guard expansion consumes this immutable query surface in the next policy chunk"
    )]
    pub(crate) fn has_unscoped(&self, atom: &AtomKey) -> bool {
        matches!(self.grants.get(atom), Some(GrantValue::Flag))
    }

    #[allow(
        dead_code,
        reason = "guard expansion consumes this immutable query surface in the next policy chunk"
    )]
    pub(crate) fn scoped_values(&self, atom: &AtomKey) -> Option<&[String]> {
        match self.grants.get(atom) {
            Some(GrantValue::Scopes(scopes)) => Some(scopes),
            Some(GrantValue::Flag) | None => None,
        }
    }
}

/// Digest of the exact concrete request represented by one `Prepared` value.
///
/// This domain-separated hash includes the frozen symbolic manifest digest and
/// both resolved requirement groups. It therefore changes when a setting or
/// symbolic root changes a concrete canonical scope, even if the component's
/// static manifest bytes are unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PreparedNeedsDigest([u8; 32]);

impl PreparedNeedsDigest {
    pub(crate) fn compute(symbolic: NeedsDigest, resolved: &ResolvedNeeds) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"lockgate:prepared-needs:v1\0");
        digest.update(symbolic.as_bytes());
        hash_grant_set(&mut digest, b"required", &resolved.required);
        hash_grant_set(&mut digest, b"optional", &resolved.optional);
        Self(digest.finalize().into())
    }

    fn to_hex(self) -> String {
        const HEX: &[u8; 16] = b"0123456789abcdef";
        let mut output = String::with_capacity(64);
        for byte in self.0 {
            output.push(HEX[(byte >> 4) as usize] as char);
            output.push(HEX[(byte & 0x0f) as usize] as char);
        }
        output
    }
}

impl fmt::Display for PreparedNeedsDigest {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "sha256:{}", self.to_hex())
    }
}

fn hash_grant_set(digest: &mut Sha256, group: &[u8], grants: &GrantSet) {
    hash_bytes(digest, group);
    hash_len(digest, grants.iter().count());
    for (atom, value) in grants.iter() {
        hash_bytes(digest, atom.to_string().as_bytes());
        match value {
            GrantValue::Flag => digest.update([0]),
            GrantValue::Scopes(scopes) => {
                digest.update([1]);
                hash_len(digest, scopes.len());
                for scope in scopes {
                    hash_bytes(digest, scope.as_bytes());
                }
            }
        }
    }
}

fn hash_bytes(digest: &mut Sha256, bytes: &[u8]) {
    hash_len(digest, bytes.len());
    digest.update(bytes);
}

fn hash_len(digest: &mut Sha256, length: usize) {
    digest.update((length as u64).to_be_bytes());
}

#[cfg(test)]
mod tests {
    use lockgate_schema::{NeedEntry, NeedsManifest, ScopeRef};

    use super::*;

    fn atom(value: &str) -> AtomKey {
        value.parse().unwrap()
    }

    #[test]
    fn effective_grants_expose_only_frozen_read_queries() {
        let mut required = GrantSet::new();
        required.insert_flag(atom("sessions.send"));
        required
            .insert_scopes(
                atom("sessions.read"),
                ["current".to_owned(), "all".to_owned(), "all".to_owned()],
            )
            .unwrap();
        let grants = EffectiveGrants::from_resolved(ResolvedNeeds {
            required,
            optional: GrantSet::new(),
        });

        assert!(grants.has_unscoped(&atom("sessions.send")));
        assert!(!grants.has_unscoped(&atom("sessions.missing")));
        assert_eq!(
            grants.scoped_values(&atom("sessions.read")),
            Some(["all".to_owned(), "current".to_owned()].as_slice())
        );
        assert_eq!(grants.scoped_values(&atom("sessions.send")), None);
    }

    #[test]
    fn prepared_digest_covers_resolved_values_and_wire_order() {
        let manifest = NeedsManifest::new(
            vec![
                NeedEntry::flag(atom("ab.c-x")),
                NeedEntry::scoped(atom("ab-c.x"), vec![ScopeRef::setting("/scope").unwrap()])
                    .unwrap(),
            ],
            vec![],
        )
        .unwrap();
        let symbolic = NeedsDigest::compute(&manifest).unwrap();
        let resolved = |scope: &str, reverse: bool| {
            let mut required = GrantSet::new();
            if reverse {
                required.insert_flag(atom("ab.c-x"));
                required
                    .insert_scopes(atom("ab-c.x"), [scope.to_owned()])
                    .unwrap();
            } else {
                required
                    .insert_scopes(atom("ab-c.x"), [scope.to_owned()])
                    .unwrap();
                required.insert_flag(atom("ab.c-x"));
            }
            ResolvedNeeds {
                required,
                optional: GrantSet::new(),
            }
        };

        assert_eq!(
            PreparedNeedsDigest::compute(symbolic, &resolved("all", false)),
            PreparedNeedsDigest::compute(symbolic, &resolved("all", true))
        );
        assert_ne!(
            PreparedNeedsDigest::compute(symbolic, &resolved("all", false)),
            PreparedNeedsDigest::compute(symbolic, &resolved("current", false))
        );
    }
}
