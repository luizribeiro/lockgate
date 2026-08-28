//! Semantic drift between approved and current resolved permission requests
//! and exported interfaces.

use std::collections::{BTreeMap, BTreeSet};

use super::GrantReview;

/// Concrete permission and exported-interface changes since the prior approval.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct DriftReport {
    pub changes: Vec<DriftChange>,
    pub export_changes: Vec<ExportDrift>,
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

/// One exported interface name's old and new versioned surface.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExportDrift {
    pub name: String,
    pub kind: ExportDriftKind,
    pub before: Vec<String>,
    pub after: Vec<String>,
}

/// The semantic kind of an exported interface change.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportDriftKind {
    Gained,
    Lost,
    VersionChanged,
}

impl DriftKind {
    fn blocks_admission(self) -> bool {
        matches!(
            self,
            Self::NewGrant | Self::ScopeWidened | Self::BecameRequired
        )
    }
}

impl ExportDriftKind {
    pub(crate) fn blocks_admission(self) -> bool {
        matches!(self, Self::Gained)
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
        export_changes: Vec::new(),
        blocks_admission,
    }
}

pub(crate) fn diff_exports(before: &[String], after: &[String]) -> Vec<ExportDrift> {
    let before = indexed_exports(before);
    let after = indexed_exports(after);
    let keys = before
        .keys()
        .chain(after.keys())
        .cloned()
        .collect::<BTreeSet<_>>();

    keys.into_iter()
        .filter_map(|name| {
            let before = before.get(&name);
            let after = after.get(&name);
            let kind = match (before, after) {
                (None, Some(_)) => ExportDriftKind::Gained,
                (Some(_), None) => ExportDriftKind::Lost,
                (Some(before), Some(after)) if before != after => ExportDriftKind::VersionChanged,
                (Some(_), Some(_)) | (None, None) => return None,
            };
            Some(ExportDrift {
                name,
                kind,
                before: before
                    .into_iter()
                    .flat_map(|versions| versions.iter().cloned())
                    .collect(),
                after: after
                    .into_iter()
                    .flat_map(|versions| versions.iter().cloned())
                    .collect(),
            })
        })
        .collect()
}

fn scope_change(before: &GrantReview, after: &GrantReview) -> Option<DriftKind> {
    // `resolve_needs` rejects declaration kinds that disagree with a capability's
    // const-fixed scopedness, so one atom cannot transition from scoped to flag here.
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

fn indexed_exports(exports: &[String]) -> BTreeMap<String, BTreeSet<String>> {
    let mut indexed = BTreeMap::<String, BTreeSet<String>>::new();
    for exported in exports {
        let name = exported
            .rsplit_once('@')
            .map_or(exported.as_str(), |(name, _)| name);
        indexed
            .entry(name.to_owned())
            .or_default()
            .insert(exported.clone());
    }
    indexed
}

#[cfg(test)]
mod tests {
    use super::*;

    fn exports(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn unversioned_interface_names_are_compared_as_complete_keys() {
        let changes = diff_exports(
            &exports(&["test:drift/before"]),
            &exports(&["test:drift/after"]),
        );

        assert_eq!(
            changes,
            vec![
                ExportDrift {
                    name: "test:drift/after".to_owned(),
                    kind: ExportDriftKind::Gained,
                    before: Vec::new(),
                    after: exports(&["test:drift/after"]),
                },
                ExportDrift {
                    name: "test:drift/before".to_owned(),
                    kind: ExportDriftKind::Lost,
                    before: exports(&["test:drift/before"]),
                    after: Vec::new(),
                },
            ]
        );
    }

    #[test]
    fn simultaneous_versions_form_one_version_change() {
        let changes = diff_exports(
            &exports(&["test:drift/role@2.0.0", "test:drift/role@1.0.0"]),
            &exports(&["test:drift/role@2.0.0"]),
        );

        assert_eq!(
            changes,
            vec![ExportDrift {
                name: "test:drift/role".to_owned(),
                kind: ExportDriftKind::VersionChanged,
                before: exports(&["test:drift/role@1.0.0", "test:drift/role@2.0.0"]),
                after: exports(&["test:drift/role@2.0.0"]),
            }]
        );
    }

    #[test]
    fn versioned_and_unversioned_forms_share_one_name() {
        let changes = diff_exports(
            &exports(&["test:drift/role"]),
            &exports(&["test:drift/role@1.0.0"]),
        );

        assert_eq!(
            changes,
            vec![ExportDrift {
                name: "test:drift/role".to_owned(),
                kind: ExportDriftKind::VersionChanged,
                before: exports(&["test:drift/role"]),
                after: exports(&["test:drift/role@1.0.0"]),
            }]
        );
    }
}
