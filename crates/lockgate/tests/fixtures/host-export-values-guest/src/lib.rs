#![no_std]

extern crate alloc;

use alloc::string::String;
use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "../../data/host_export_values",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "host-export-values";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::host_export_values::values::Guest for Fixture {
    fn round_trip(
        payload: exports::test::host_export_values::values::Payload,
    ) -> exports::test::host_export_values::values::Payload {
        payload
    }

    fn probe(ok: bool) -> Result<u32, String> {
        if ok { Ok(42) } else { Err("rejected".into()) }
    }
}

impl exports::test::host_export_values::mirror::Guest for Fixture {
    fn round_trip(
        payload: exports::test::host_export_values::values::Payload,
    ) -> exports::test::host_export_values::values::Payload {
        payload
    }
}

impl exports::test::host_export_values::type_::Guest for Fixture {
    fn ping() -> u32 {
        13
    }
}

impl exports::test::host_export_values::union::Guest for Fixture {
    fn ping() -> u32 {
        17
    }
}

lockgate_plugin::export!(Fixture);
