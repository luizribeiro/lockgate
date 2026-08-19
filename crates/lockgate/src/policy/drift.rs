//! Semantic drift between approved and current resolved permission requests.

use std::collections::{BTreeMap, BTreeSet};

use super::GrantReview;

/// Concrete permission changes since the prior approval.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DriftReport {
    pub changes: Vec<DriftChange>,
    pub blocks_admission: bool,
}

/// One permission's old and new concrete scope state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DriftChange {
    pub capability: String,
    pub permission: String,
    pub kind: DriftKind,
    pub before: Option<Vec<String>>,
    pub after: Option<Vec<String>>,
}

/// The semantic kind of a concrete permission change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriftKind {
    NewGrant,
    ScopeWidened,
    BecameRequired,
    RemovedGrant,
    ScopeNarrowed,
    BecameOptional,
}

impl DriftKind {
    fn blocks_admission(self) -> bool {
        matches!(
            self,
            Self::NewGrant | Self::ScopeWidened | Self::BecameRequired
        )
    }
}

pub(crate) fn diff_grants(before: &[GrantReview], after: &[GrantReview]) -> DriftReport {
    let before = indexed(before);
    let after = indexed(after);
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    let changes = keys
        .into_iter()
        .filter_map(|(capability, permission)| {
            let before = before
                .get(&(capability.clone(), permission.clone()))
                .copied();
            let after = after
                .get(&(capability.clone(), permission.clone()))
                .copied();
            let kind = match (before, after) {
                (None, Some(_)) => DriftKind::NewGrant,
                (Some(_), None) => DriftKind::RemovedGrant,
                (Some(before), Some(after)) => {
                    if before.optional && !after.optional {
                        DriftKind::BecameRequired
                    } else {
                        match scope_change(before, after) {
                            Some(DriftKind::ScopeWidened) => DriftKind::ScopeWidened,
                            _ if before.optional != after.optional => DriftKind::BecameOptional,
                            Some(DriftKind::ScopeNarrowed) => DriftKind::ScopeNarrowed,
                            None => return None,
                            Some(_) => unreachable!("scope changes only have scope drift kinds"),
                        }
                    }
                }
                (None, None) => return None,
            };
            Some(DriftChange {
                capability,
                permission,
                kind,
                before: before.map(|grant| grant.scopes.clone()),
                after: after.map(|grant| grant.scopes.clone()),
            })
        })
        .collect::<Vec<_>>();
    let blocks_admission = changes.iter().any(|change| change.kind.blocks_admission());

    DriftReport {
        changes,
        blocks_admission,
    }
}

fn scope_change(before: &GrantReview, after: &GrantReview) -> Option<DriftKind> {
    let before = before.scopes.iter().collect::<BTreeSet<_>>();
    let after = after.scopes.iter().collect::<BTreeSet<_>>();
    if before == after {
        None
    } else if after.difference(&before).next().is_some() {
        Some(DriftKind::ScopeWidened)
    } else {
        Some(DriftKind::ScopeNarrowed)
    }
}

fn indexed(grants: &[GrantReview]) -> BTreeMap<(String, String), &GrantReview> {
    grants
        .iter()
        .map(|grant| ((grant.capability.clone(), grant.permission.clone()), grant))
        .collect()
}
