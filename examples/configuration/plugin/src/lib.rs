#![no_std]

extern crate alloc;

use lockgate_plugin::{Deserialize, JsonSchema, MetadataSource, Needs, Plugin};

lockgate_plugin::generate!({ path: "../wit", world: "plugin" });

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "lockgate_plugin::schemars")]
#[serde(crate = "lockgate_plugin::serde", deny_unknown_fields)]
struct Settings {
    /// Text placed before each formatted value.
    prefix: alloc::string::String,
    /// Ending applied when the host leaves it unspecified.
    #[serde(default = "default_ending")]
    ending: alloc::string::String,
}

fn default_ending() -> alloc::string::String {
    "!".into()
}

struct Formatter;

impl Plugin for Formatter {
    const ID: &'static str = "configuration";
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = Settings;
}

impl exports::example::configuration::formatter::Guest for Formatter {
    fn format(value: alloc::string::String) -> alloc::string::String {
        let settings = Self::settings();
        alloc::format!("{}: {}{}", settings.prefix, value, settings.ending)
    }
}

lockgate_plugin::export!(Formatter);
