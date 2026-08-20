#![no_std]

use sage_plugin::{MetadataSource, Needs, NoSettings};

sage_plugin::generate!({
    inline: r#"
        package test:reexported-plugin;

        interface guest {
            value: func() -> u32;
        }

        world fixture {
            export guest;
        }
    "#,
    world: "fixture",
    facade: ::sage_plugin,
});

struct Fixture;

impl sage_plugin::Plugin for Fixture {
    const ID: &'static str = "reexported-plugin";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::reexported_plugin::guest::Guest for Fixture {
    fn value() -> u32 {
        73
    }
}

sage_plugin::export!(Fixture; facade = ::sage_plugin);
