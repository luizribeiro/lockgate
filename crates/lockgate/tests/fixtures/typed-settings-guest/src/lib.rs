#![no_std]

extern crate alloc;

use lockgate_plugin::{Deserialize, JsonSchema, MetadataSource, Needs, Plugin};

lockgate_plugin::generate!({ path: "wit", world: "plugin" });

#[derive(Deserialize, JsonSchema)]
#[schemars(crate = "lockgate_plugin::schemars")]
#[serde(
    crate = "lockgate_plugin::serde",
    rename_all = "kebab-case",
    deny_unknown_fields
)]
struct Settings {
    required: alloc::string::String,
    #[serde(default = "default_suffix")]
    suffix: alloc::string::String,
}

fn default_suffix() -> alloc::string::String {
    "guest-default".into()
}

struct Fixture;

impl Plugin for Fixture {
    const ID: &'static str = "typed-settings";
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = Settings;
}

impl exports::test::typed_settings::guest::Guest for Fixture {
    fn observed_settings() -> alloc::string::String {
        let settings = Self::settings();
        alloc::format!("{}:{}", settings.required, settings.suffix)
    }
}

lockgate_plugin::export!(Fixture);
