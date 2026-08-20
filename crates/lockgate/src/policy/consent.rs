//! Storage-agnostic consent records over resolved permission declarations.

use std::{error::Error, fmt};

use lockgate_schema::{GrantSet, GrantValue, NeedEntry};
use serde::{Deserialize, Serialize};

use super::{DriftReport, PreparedNeedsDigest, ResolvedNeeds, diff_grants};
use crate::{Acceptance, Prepared};

/// The complete resolved permission request presented for operator approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentManifest {
    pub instance_id: String,
    pub plugin_label: String,
    pub request_digest: PreparedNeedsDigest,
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
    #[serde(alias = "fingerprint")]
    pub request_digest: PreparedNeedsDigest,
    pub grants: Vec<GrantReview>,
    pub approved_at: String,
}

/// The operator approval needed before this prepared instance may be admitted.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ConsentRequired {
    FirstRun {
        manifest: ConsentManifest,
    },
    Drift {
        manifest: ConsentManifest,
        drift: DriftReport,
    },
}

impl fmt::Display for ConsentRequired {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::FirstRun { manifest } => write!(
                formatter,
                "instance `{}` requires approval before its first admission",
                manifest.instance_id
            ),
            Self::Drift { manifest, .. } => write!(
                formatter,
                "instance `{}` has expanded its permission request and requires approval",
                manifest.instance_id
            ),
        }
    }
}

impl Error for ConsentRequired {}

impl Prepared {
    /// Projects this prepared request into the complete operator review surface.
    pub fn review(&self) -> ConsentManifest {
        ConsentManifest {
            instance_id: self.instance_id.clone(),
            plugin_label: self.inspection.metadata().name().to_owned(),
            request_digest: self.prepared_digest,
            grants: grant_reviews(&self.resolved, self.inspection.needs()),
        }
    }

    /// Records host-supplied approval of this complete resolved request.
    pub fn approve(&self, approved_at: String) -> ConsentRecord {
        let manifest = self.review();
        ConsentRecord {
            instance_id: manifest.instance_id,
            request_digest: manifest.request_digest,
            grants: manifest.grants,
            approved_at,
        }
    }

    /// Returns a provenance-bound acceptance only when this exact request was approved.
    #[allow(
        clippy::result_large_err,
        reason = "the public error intentionally carries the complete manifest and drift report for operator review"
    )]
    pub fn accept_reviewed(
        &self,
        prior: Option<&ConsentRecord>,
    ) -> Result<Acceptance, ConsentRequired> {
        let Some(prior) = prior.filter(|prior| prior.instance_id == self.instance_id) else {
            return Err(ConsentRequired::FirstRun {
                manifest: self.review(),
            });
        };
        if prior.request_digest == self.prepared_digest {
            return Ok(self.accept_all());
        }

        let manifest = self.review();
        let drift = diff_grants(&prior.grants, &manifest.grants);
        if drift.blocks_admission {
            Err(ConsentRequired::Drift { manifest, drift })
        } else {
            Ok(self.accept_all())
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
