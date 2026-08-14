#![no_std]

extern crate alloc;

mod runtime;

use alloc::format;

lockgate_plugin::generate!({
    path: "wit",
    world: "plugin",
});

struct Greeter;

impl lockgate_plugin::Plugin for Greeter {
    const ID: &'static str = "example.greeter";
}

impl exports::example::greeter::greeter::Guest for Greeter {
    fn greet(name: alloc::string::String) -> alloc::string::String {
        format!("Hello, {name}!")
    }
}

lockgate_plugin::export!(Greeter);
