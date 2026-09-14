use alloc::string::String;
use core::{error::Error, fmt, str::FromStr};

/// A malformed plugin identifier.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InvalidPluginId {
    value: String,
    reason: InvalidPluginIdReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum InvalidPluginIdReason {
    Empty,
    DisallowedCharacter,
}

impl fmt::Display for InvalidPluginId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let value = self.value.escape_debug();
        match self.reason {
            InvalidPluginIdReason::Empty => {
                write!(formatter, "plugin ID `{value}` must not be empty")
            }
            InvalidPluginIdReason::DisallowedCharacter => write!(
                formatter,
                "plugin ID `{value}` may only contain lowercase ASCII letters, digits and `-`"
            ),
        }
    }
}

impl Error for InvalidPluginId {}

/// Stable identity for a plugin.
/// Plugin IDs contain one or more lowercase ASCII letters, digits, or `-` characters.
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

impl TryFrom<String> for PluginId {
    type Error = InvalidPluginId;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        let reason = if value.is_empty() {
            Some(InvalidPluginIdReason::Empty)
        } else if !value.chars().all(|character| {
            character.is_ascii_lowercase() || character.is_ascii_digit() || character == '-'
        }) {
            Some(InvalidPluginIdReason::DisallowedCharacter)
        } else {
            None
        };

        match reason {
            Some(reason) => Err(InvalidPluginId { value, reason }),
            None => Ok(Self(value)),
        }
    }
}

impl TryFrom<&str> for PluginId {
    type Error = InvalidPluginId;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        Self::try_from(String::from(value))
    }
}

impl FromStr for PluginId {
    type Err = InvalidPluginId;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        Self::try_from(value)
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
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::try_from(value).map_err(serde::de::Error::custom)
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
        let plugin_id: PluginId = "openai".parse().unwrap();

        assert_eq!(plugin_id.to_string(), "openai");
    }

    #[test]
    fn checked_constructors_accept_realistic_ids() {
        for accepted in ["openai", "kagi", "stateful-tools", "plugin-2"] {
            let parsed: PluginId = accepted.parse().unwrap();
            let borrowed = PluginId::try_from(accepted).unwrap();
            let owned = PluginId::try_from(String::from(accepted)).unwrap();

            assert_eq!(parsed.as_str(), accepted);
            assert_eq!(borrowed, parsed);
            assert_eq!(owned, parsed);
        }
    }

    #[test]
    fn checked_constructors_reject_malformed_ids() {
        for rejected in [
            "",
            "Kagi",
            "kagi.v2",
            "my_plugin",
            "build-plugin@grant-a",
            "a b",
            "a/b",
            "a\nb",
            "k\u{0430}gi",
            "kagi\u{200b}",
        ] {
            assert!(
                rejected.parse::<PluginId>().is_err(),
                "accepted {rejected:?}"
            );
            assert!(
                PluginId::try_from(rejected).is_err(),
                "accepted {rejected:?}"
            );
            assert!(
                PluginId::try_from(String::from(rejected)).is_err(),
                "accepted {rejected:?}"
            );
        }
    }

    #[test]
    fn invalid_plugin_id_display_names_and_quotes_the_problem() {
        let error = PluginId::try_from("a\nb").unwrap_err();

        assert_eq!(
            error.to_string(),
            "plugin ID `a\\nb` may only contain lowercase ASCII letters, digits and `-`"
        );
    }

    #[test]
    fn works_as_a_btree_map_key() {
        let mut plugins = BTreeMap::new();
        plugins.insert(PluginId::try_from("sandbox").unwrap(), 3);
        plugins.insert(PluginId::try_from("openai").unwrap(), 1);
        plugins.insert(PluginId::try_from("kagi").unwrap(), 2);

        assert_eq!(plugins.get(&PluginId::try_from("kagi").unwrap()), Some(&2));
        assert_eq!(
            plugins.into_keys().collect::<alloc::vec::Vec<_>>(),
            [
                PluginId::try_from("kagi").unwrap(),
                PluginId::try_from("openai").unwrap(),
                PluginId::try_from("sandbox").unwrap(),
            ]
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_json_round_trip() {
        let plugin_id = PluginId::try_from("openai").unwrap();

        let json = serde_json::to_string(&plugin_id).unwrap();

        assert_eq!(json, r#""openai""#);
        assert_eq!(serde_json::from_str::<PluginId>(&json).unwrap(), plugin_id);
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_json_btree_map_round_trip_uses_string_keys() {
        let plugins = BTreeMap::from([
            (PluginId::try_from("openai").unwrap(), 1_u32),
            (PluginId::try_from("kagi").unwrap(), 2_u32),
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
            PluginId::try_from("kagi").unwrap()
        );
    }

    #[cfg(feature = "serde")]
    #[test]
    fn serde_json_rejects_malformed_plugin_id() {
        let error = serde_json::from_str::<PluginId>(r#""a/b""#).unwrap_err();

        assert!(
            error.to_string().contains(
                "plugin ID `a/b` may only contain lowercase ASCII letters, digits and `-`"
            )
        );
    }
}
