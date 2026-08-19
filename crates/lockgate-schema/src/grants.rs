use std::{collections::BTreeMap, error::Error, fmt};

use serde::{Deserialize, Serialize};

use crate::AtomKey;

/// A concrete accepted value for one capability atom.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum GrantValue {
    Flag,
    Scopes(Vec<String>),
}

/// Concrete grants accepted by an application consent flow.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(transparent)]
pub struct GrantSet(BTreeMap<AtomKey, GrantValue>);

impl GrantSet {
    /// Creates an empty grant set.
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds or replaces an unscoped flag grant.
    pub fn insert_flag(&mut self, atom: AtomKey) -> Option<GrantValue> {
        self.0.insert(atom, GrantValue::Flag)
    }

    /// Adds or replaces a scoped grant in canonical set order.
    pub fn insert_scopes(
        &mut self,
        atom: AtomKey,
        scopes: impl IntoIterator<Item = String>,
    ) -> Result<Option<GrantValue>, GrantValueError> {
        let mut scopes: Vec<_> = scopes.into_iter().collect();
        if scopes.is_empty() {
            return Err(GrantValueError::EmptyScopes);
        }
        scopes.sort();
        scopes.dedup();
        Ok(self.0.insert(atom, GrantValue::Scopes(scopes)))
    }

    /// Returns the concrete value stored for `atom`.
    pub fn get(&self, atom: &AtomKey) -> Option<&GrantValue> {
        self.0.get(atom)
    }

    /// Iterates over concrete grants in canonical atom wire order.
    pub fn iter(&self) -> impl Iterator<Item = (&AtomKey, &GrantValue)> {
        self.0.iter()
    }
}

/// A malformed concrete grant value.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum GrantValueError {
    EmptyScopes,
}

impl fmt::Display for GrantValueError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyScopes => {
                formatter.write_str("scoped grant must contain at least one scope")
            }
        }
    }
}

impl Error for GrantValueError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constructors_store_canonical_grant_values() {
        let mut grants = GrantSet::new();
        let flag: AtomKey = "notify.send".parse().unwrap();
        let scoped: AtomKey = "fs.read".parse().unwrap();

        assert_eq!(grants.insert_flag(flag.clone()), None);
        assert_eq!(grants.0.get(&flag), Some(&GrantValue::Flag));
        assert_eq!(
            grants
                .insert_scopes(
                    scoped.clone(),
                    [
                        "workspace".to_string(),
                        "data".to_string(),
                        "data".to_string()
                    ]
                )
                .unwrap(),
            None
        );
        assert_eq!(
            grants.0.get(&scoped),
            Some(&GrantValue::Scopes(vec![
                "data".to_string(),
                "workspace".to_string()
            ]))
        );
    }

    #[test]
    fn scoped_grants_reject_an_empty_scope_list() {
        let mut grants = GrantSet::new();
        let atom: AtomKey = "fs.read".parse().unwrap();

        assert_eq!(
            grants.insert_scopes(atom, Vec::new()),
            Err(GrantValueError::EmptyScopes)
        );
        assert!(grants.0.is_empty());
    }

    #[test]
    fn reinsertion_replaces_and_returns_the_previous_value() {
        let mut grants = GrantSet::new();
        let atom: AtomKey = "fs.read".parse().unwrap();
        assert_eq!(grants.insert_flag(atom.clone()), None);

        assert_eq!(
            grants
                .insert_scopes(atom.clone(), ["workspace".to_string()])
                .unwrap(),
            Some(GrantValue::Flag)
        );
        assert_eq!(grants.0.len(), 1);
        assert_eq!(
            grants.0.get(&atom),
            Some(&GrantValue::Scopes(vec!["workspace".to_string()]))
        );
    }

    #[test]
    fn grant_sets_round_trip_through_serde() {
        let mut grants = GrantSet::new();
        grants.insert_flag("notify.send".parse().unwrap());
        grants
            .insert_scopes(
                "fs.read".parse().unwrap(),
                ["workspace".to_owned(), "data".to_owned()],
            )
            .unwrap();

        let encoded = serde_json::to_value(&grants).unwrap();
        let decoded: GrantSet = serde_json::from_value(encoded.clone()).unwrap();

        assert_eq!(decoded, grants);
        assert_eq!(
            encoded,
            serde_json::json!({
                "fs.read": ["data", "workspace"],
                "notify.send": null,
            })
        );
    }
}
