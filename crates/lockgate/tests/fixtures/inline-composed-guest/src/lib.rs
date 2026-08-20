#![no_std]

use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    inline: r#"
        package test:inline-composed;

        interface shared {
            type answer = u32;
        }

        interface guest {
            use shared.{answer};
            run: func() -> answer;
        }

        world fixture {
            export guest;
        }
    "#,
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "inline-composed";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::inline_composed::guest::Guest for Fixture {
    fn run() -> u32 {
        42
    }
}

lockgate_plugin::export!(Fixture);
