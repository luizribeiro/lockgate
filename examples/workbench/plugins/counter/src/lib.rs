#![no_std]

extern crate alloc;

use lockgate_plugin::{MetadataSource, Needs};

lockgate_plugin::generate!({
    path: "wit",
    world: "counter",
});

struct Counter;

impl lockgate_plugin::Plugin for Counter {
    const ID: &'static str = "counter";
    // DISPLAY_NAME, VERSION, and DESCRIPTION come from Cargo package metadata.
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    // Explicit authority claim: this plugin requests no host capabilities.
    const NEEDS: Needs = Needs::NOTHING;
}

impl exports::example::workbench::stats::Guest for Counter {
    fn measure(text: alloc::string::String) -> u32 {
        text.split_whitespace().count() as u32
    }
}

lockgate_plugin::export!(Counter);
