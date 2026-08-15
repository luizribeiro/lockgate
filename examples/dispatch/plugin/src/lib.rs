#![no_std]

extern crate alloc;

use alloc::format;
use lockgate_plugin::{MetadataSource, Needs};

lockgate_plugin::generate!({
    path: "../wit",
    world: "plugin",
});

struct Dispatch;

impl lockgate_plugin::Plugin for Dispatch {
    const ID: &'static str = "dispatch";
    // NAME, VERSION, and DESCRIPTION come from Cargo package metadata.
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    // Explicit authority claim: this plugin requests no host capabilities.
    const NEEDS: Needs = Needs::NOTHING;
}

impl exports::example::dispatch::tasks::Guest for Dispatch {
    fn run(task: alloc::string::String) -> alloc::string::String {
        example::dispatch::dispatch::send(&format!("{task} report"));
        if task == "backup" {
            example::dispatch::dispatch::send("undeliverable report");
        }
        format!("queued {task}")
    }
}

lockgate_plugin::export!(Dispatch);
