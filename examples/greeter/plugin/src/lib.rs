#![no_std]

extern crate alloc;

use alloc::format;
use lockgate_plugin::MetadataSource;

lockgate_plugin::generate!({
    path: "../wit",
    world: "plugin",
});

struct Greeter;

impl lockgate_plugin::Plugin for Greeter {
    const ID: &'static str = "greeter";
    // NAME, VERSION, and DESCRIPTION come from Cargo package metadata.
    const LICENSE: MetadataSource = MetadataSource::Absent;
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
}

impl exports::example::greeter::greeter::Guest for Greeter {
    fn greet(name: alloc::string::String) -> alloc::string::String {
        format!("Hello, {name}!")
    }
}

lockgate_plugin::export!(Greeter);
