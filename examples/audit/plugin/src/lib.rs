#![no_std]

extern crate alloc;

use alloc::format;
use lockgate_plugin::{MetadataSource, Needs};

lockgate_plugin::generate!({
    path: "../wit",
    world: "plugin",
});

struct Audit;

impl lockgate_plugin::Plugin for Audit {
    const ID: &'static str = "audit";
    // DISPLAY_NAME, VERSION, and DESCRIPTION come from Cargo package metadata.
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    // Explicit authority claim: this plugin requests no host capabilities.
    const NEEDS: Needs = Needs::NOTHING;
}

impl exports::example::audit::tasks::Guest for Audit {
    fn run(task: alloc::string::String) -> alloc::string::String {
        example::audit::audit::log(&format!("started {task}"));
        example::audit::audit::log(&format!("finished {task}"));
        format!("completed {task}")
    }
}

lockgate_plugin::export!(Audit);
