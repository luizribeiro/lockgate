#![no_std]

use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "exec-concurrent";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::exec_concurrent::guest::Guest for Fixture {
    async fn run() -> u32 {
        futures::join!(
            test::exec_concurrent::host::first(),
            test::exec_concurrent::host::second(),
        );
        2
    }
}

lockgate_plugin::export!(Fixture);
