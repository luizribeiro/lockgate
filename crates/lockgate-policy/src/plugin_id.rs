use alloc::string::String;
use core::{convert::Infallible, fmt, str::FromStr};

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

impl FromStr for PluginId {
    type Err = Infallible;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Ok(Self::from(value))
    }
}

#[cfg(feature = "serde")]
impl serde::Serialize for PluginId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(self.as_str())
    }
}

#[cfg(feature = "serde")]
impl<'de> serde::Deserialize<'de> for PluginId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        <String as serde::Deserialize>::deserialize(deserializer).map(Self)
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
    fn parses_from_str() {
        let plugin_id: PluginId = "kagi".parse().unwrap();

        assert_eq!(plugin_id, PluginId::from("kagi"));
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

    #[cfg(feature = "serde")]
    #[test]
    fn serde_json_round_trip() {
        let plugin_id = PluginId::from("openai");

        let json = serde_json::to_string(&plugin_id).unwrap();

        assert_eq!(json, r#""openai""#);
        assert_eq!(serde_json::from_str::<PluginId>(&json).unwrap(), plugin_id);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_json_btree_map_round_trip_uses_string_keys() {
        let plugins = BTreeMap::from([
            (PluginId::from("openai"), 1_u32),
            (PluginId::from("kagi"), 2_u32),
        ]);

        let json = serde_json::to_string(&plugins).unwrap();

        assert_eq!(json, r#"{"kagi":2,"openai":1}"#);
        assert_eq!(
            serde_json::from_str::<BTreeMap<PluginId, u32>>(&json).unwrap(),
            plugins
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_json_string_deserializes_into_plugin_id() {
        assert_eq!(
            serde_json::from_str::<PluginId>(r#""kagi""#).unwrap(),
            PluginId::from("kagi")
        );
    }
}
