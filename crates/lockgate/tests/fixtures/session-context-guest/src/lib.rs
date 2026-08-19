#![no_std]

extern crate alloc;

use alloc::string::{String, ToString};
use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "../../data/session_context",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "session-context";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::session_context::guest::Guest for Fixture {
    fn read() -> String {
        match test::session_context::sessions::read() {
            Ok(session) => alloc::format!("ok:{session}"),
            Err(test::session_context::sessions::SessionError::Gone) => "gone".to_string(),
            Err(test::session_context::sessions::SessionError::Denied) => "denied".to_string(),
        }
    }
}

lockgate_plugin::export!(Fixture);
