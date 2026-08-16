#![no_std]

use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "exec-fuel";
    const DESCRIPTION: MetadataSource = MetadataSource::Absent;
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

impl exports::test::exec_fuel::guest::Guest for Fixture {
    fn run(iterations: u32) -> u32 {
        let mut checksum = 0x9e37_79b9_u32;
        for iteration in 1..=iterations {
            for round in 0..32_u32 {
                checksum = checksum
                    .rotate_left(5)
                    .wrapping_add(iteration)
                    .wrapping_mul(0x045d_9f3b)
                    ^ round;
            }
            test::exec_fuel::host::report(iteration, checksum);
        }
        checksum
    }
}

lockgate_plugin::export!(Fixture);
