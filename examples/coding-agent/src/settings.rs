//! Opaque provider settings populated from a namespaced environment prefix.

use std::{collections::HashMap, env};

const PREFIX: &str = "CODING_AGENT_PROVIDER_";

pub(crate) struct Settings {
    values: HashMap<String, String>,
}

impl Settings {
    pub(crate) fn from_env() -> Self {
        Self {
            values: env::vars()
                .filter_map(|(name, value)| {
                    name.strip_prefix(PREFIX)
                        .map(|name| (name.to_ascii_lowercase().replace('_', "-"), value))
                })
                .collect(),
        }
    }

    #[cfg(test)]
    pub(crate) fn new(values: impl IntoIterator<Item = (String, String)>) -> Self {
        Self {
            values: values.into_iter().collect(),
        }
    }

    pub(crate) fn get(&self, key: &str) -> Option<String> {
        self.values.get(key).cloned()
    }
}
