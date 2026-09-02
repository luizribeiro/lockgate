//! Storage-agnostic consent records over resolved permission declarations
//! and exported interfaces.

use std::{error::Error, fmt};

use lockgate_schema::{GrantSet, GrantValue, NeedEntry};
use serde::{Deserialize, Serialize};

use super::{DriftReport, PreparedNeedsDigest, ResolvedNeeds, diff_exports, diff_grants};
use crate::{Acceptance, PluginId, Prepared};

/// The complete resolved permission and export surface presented for operator approval.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ConsentManifest {
    pub plugin_id: PluginId,
    pub plugin_label: String,
    pub request_digest: PreparedNeedsDigest,
    pub component_digest: String,
    pub exported_interfaces: Vec<String>,
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

/// A host-persisted approval of one resolved permission request and export surface.
///
/// Legacy records deserialize with an empty export list. A component that currently
/// exports any interfaces therefore requires one-time re-approval because the old
/// record could not vouch for those extension points.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConsentRecord {
    #[serde(rename = "instance_id")]
    pub plugin_id: PluginId,
    #[serde(alias = "fingerprint")]
    pub request_digest: PreparedNeedsDigest,
    #[serde(default)]
    pub component_digest: Option<String>,
    #[serde(default)]
    pub exported_interfaces: Vec<String>,
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
                manifest.plugin_id
            ),
            Self::Drift { manifest, .. } => write!(
                formatter,
                "instance `{}` has expanded its requested access and requires approval",
                manifest.plugin_id
            ),
        }
    }
}

impl Error for ConsentRequired {}

/// Reports permission and exported-interface drift from a prior approval.
pub fn consent_drift(prior: &ConsentRecord, manifest: &ConsentManifest) -> DriftReport {
    let mut drift = diff_grants(&prior.grants, &manifest.grants);
    drift.export_changes = diff_exports(&prior.exported_interfaces, &manifest.exported_interfaces);
    drift.blocks_admission |= drift
        .export_changes
        .iter()
        .any(|change| change.kind.blocks_admission());
    drift
}

impl Prepared {
    /// Projects this prepared request into the complete operator review surface.
    pub fn review(&self) -> ConsentManifest {
        let mut exported_interfaces = self.inspection.exported_interfaces().to_vec();
        exported_interfaces.sort();
        exported_interfaces.dedup();
        ConsentManifest {
            plugin_id: self.plugin_id.clone(),
            plugin_label: self.inspection.metadata().name().to_owned(),
            request_digest: self.prepared_digest,
            component_digest: self.component_digest.clone(),
            exported_interfaces,
            grants: grant_reviews(&self.resolved, self.inspection.needs()),
        }
    }

    /// Records host-supplied approval of this complete resolved request.
    pub fn approve(&self, approved_at: String) -> ConsentRecord {
        let manifest = self.review();
        ConsentRecord {
            plugin_id: manifest.plugin_id,
            request_digest: manifest.request_digest,
            component_digest: Some(manifest.component_digest),
            exported_interfaces: manifest.exported_interfaces,
            grants: manifest.grants,
            approved_at,
        }
    }

    /// Returns a provenance-bound acceptance only when the permission request and exported
    /// interfaces are covered by the prior approval.
    #[allow(
        clippy::result_large_err,
        reason = "the public error intentionally carries the complete manifest and drift report for operator review"
    )]
    pub fn accept_reviewed(
        &self,
        prior: Option<&ConsentRecord>,
    ) -> Result<Acceptance, ConsentRequired> {
        let Some(prior) = prior.filter(|prior| prior.plugin_id == self.plugin_id) else {
            return Err(ConsentRequired::FirstRun {
                manifest: self.review(),
            });
        };
        let manifest = self.review();
        if prior.request_digest == manifest.request_digest
            && prior.exported_interfaces == manifest.exported_interfaces
        {
            return Ok(self.accept_all());
        }

        let drift = consent_drift(prior, &manifest);
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
