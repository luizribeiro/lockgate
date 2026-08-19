//! Storage-agnostic consent records over resolved permission declarations.

use lockgate_schema::{GrantSet, GrantValue, NeedEntry};
use serde::{Deserialize, Serialize};

use super::{PreparedNeedsDigest, ResolvedNeeds};
use crate::Prepared;

/// The complete resolved permission request presented for operator approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentManifest {
    pub instance_id: String,
    pub plugin_label: String,
    pub fingerprint: PreparedNeedsDigest,
    pub grants: Vec<GrantReview>,
}

/// One reviewable resolved permission grant.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GrantReview {
    pub capability: String,
    pub permission: String,
    pub scopes: Vec<String>,
    pub optional: bool,
    pub reason: Option<String>,
}

/// A host-persisted approval of one complete resolved permission request.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRecord {
    pub instance_id: String,
    pub fingerprint: PreparedNeedsDigest,
    pub grants: Vec<GrantReview>,
    pub approved_at: String,
}

impl Prepared {
    /// Projects this prepared request into the complete operator review surface.
    pub fn review(&self) -> ConsentManifest {
        ConsentManifest {
            instance_id: self.instance_id.clone(),
            plugin_label: self.inspection.metadata().name().to_owned(),
            fingerprint: self.prepared_digest,
            grants: grant_reviews(&self.resolved, self.inspection.needs()),
        }
    }

    /// Records host-supplied approval of this complete resolved request.
    pub fn approve(&self, approved_at: String) -> ConsentRecord {
        let manifest = self.review();
        ConsentRecord {
            instance_id: manifest.instance_id,
            fingerprint: manifest.fingerprint,
            grants: manifest.grants,
            approved_at,
        }
    }
}

fn grant_reviews(
    resolved: &ResolvedNeeds,
    declared: &lockgate_schema::NeedsManifest,
) -> Vec<GrantReview> {
    let mut reviews = reviews_for_group(&resolved.required, declared.required(), false)
        .chain(reviews_for_group(
            &resolved.optional,
            declared.optional(),
            true,
        ))
        .collect::<Vec<_>>();
    reviews.sort_by(|left, right| {
        (&left.capability, &left.permission).cmp(&(&right.capability, &right.permission))
    });
    reviews
}

fn reviews_for_group<'a>(
    resolved: &'a GrantSet,
    declared: &'a [NeedEntry],
    optional: bool,
) -> impl Iterator<Item = GrantReview> + 'a {
    resolved.iter().map(move |(atom, value)| {
        let reason = declared
            .iter()
            .find(|entry| entry.atom() == atom)
            .expect("resolved grants must retain their symbolic declaration")
            .reason()
            .map(str::to_owned);
        GrantReview {
            capability: atom.capability().to_owned(),
            permission: atom.operation().to_owned(),
            scopes: match value {
                GrantValue::Flag => Vec::new(),
                GrantValue::Scopes(scopes) => scopes.clone(),
            },
            optional,
            reason,
        }
    })
}
