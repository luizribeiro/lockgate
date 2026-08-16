#![no_std]

extern crate alloc;

use alloc::string::String;
use lockgate_plugin::{MetadataSource, Needs, NoSettings};

lockgate_plugin::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "greeter";
    const DISPLAY_NAME: MetadataSource = MetadataSource::Explicit("Greeter");
    const VERSION: MetadataSource = MetadataSource::Explicit("1.0");
    const REPOSITORY: MetadataSource = MetadataSource::Absent;
    const HOMEPAGE: MetadataSource = MetadataSource::Absent;
    const NEEDS: Needs = Needs::NOTHING;
    type Settings = NoSettings;
}

static mut PIN_COUNT: u32 = 0;

impl exports::test::public::greeter::Guest for Fixture {
    fn greet(name: String) -> String {
        let mut greeting = String::from("Hello, ");
        greeting.push_str(&name);
        greeting.push('!');
        greeting
    }
}

impl exports::test::public::diagnostics::Guest for Fixture {
    fn value() -> u32 {
        42
    }

    fn pin() -> u32 {
        unsafe {
            let pin = &raw mut PIN_COUNT;
            let next = pin.read().wrapping_add(1);
            pin.write(next);
            next
        }
    }

    fn trap() {
        core::arch::wasm32::unreachable()
    }

    fn work(iterations: u32) -> u32 {
        let mut checksum = 0x9e37_79b9_u32;
        for iteration in 1..=iterations {
            for round in 0..32_u32 {
                checksum = checksum
                    .rotate_left(5)
                    .wrapping_add(iteration)
                    .wrapping_mul(0x045d_9f3b)
                    ^ round;
            }
        }
        checksum
    }
}

lockgate_plugin::export!(Fixture);
