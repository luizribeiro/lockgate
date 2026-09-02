use alloc::string::String;
use core::fmt;

/// Stable identity for a plugin.
#[derive(Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PluginId(String);

impl PluginId {
    /// Returns the raw plugin identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(self.as_str())
    }
}

impl From<String> for PluginId {
    fn from(value: String) -> Self {
        Self(value)
    }
}

impl From<&str> for PluginId {
    fn from(value: &str) -> Self {
        Self(String::from(value))
    }
}

#[cfg(test)]
mod tests {
    use super::PluginId;
    use alloc::{
        collections::BTreeMap,
        string::{String, ToString},
    };

    #[test]
    fn display_round_trips_raw_id() {
        let plugin_id = PluginId::from("openai");

        assert_eq!(plugin_id.to_string(), "openai");
    }

    #[test]
    fn constructs_from_string_and_str() {
        let owned = PluginId::from(String::from("kagi"));
        let borrowed = PluginId::from("sandbox");

        assert_eq!(owned.as_str(), "kagi");
        assert_eq!(borrowed.as_str(), "sandbox");
    }

    #[test]
    fn works_as_a_btree_map_key() {
        let mut plugins = BTreeMap::new();
        plugins.insert(PluginId::from("sandbox"), 3);
        plugins.insert(PluginId::from("openai"), 1);
        plugins.insert(PluginId::from("kagi"), 2);

        assert_eq!(plugins.get(&PluginId::from("kagi")), Some(&2));
        assert_eq!(
            plugins.into_keys().collect::<alloc::vec::Vec<_>>(),
            [
                PluginId::from("kagi"),
                PluginId::from("openai"),
                PluginId::from("sandbox"),
            ]
        );
    }
}
