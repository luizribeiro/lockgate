#![no_std]

extern crate alloc;

use alloc::string::String;

lockgate_plugin::generate!({
    path: "../../data/host_bindings",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "host-caller";
}

impl exports::test::host_bindings::guest::Guest for Fixture {
    fn round_trip(
        payload: exports::test::host_bindings::guest::Payload,
    ) -> exports::test::host_bindings::guest::Payload {
        payload
    }

    fn data() -> String {
        test::host_bindings::application::read_data()
    }

    fn caller() -> String {
        test::host_bindings::application::caller()
    }

    fn rich(value: u32, fail: bool) -> String {
        let request = test::host_bindings::application::Request { value, fail };
        match test::host_bindings::application::transform(request) {
            Ok(request) => alloc::format!("ok:{}", request.value),
            Err(error) => error,
        }
    }

    async fn overlap() -> u32 {
        futures::join!(
            test::host_bindings::application::first(),
            test::host_bindings::application::second(),
        );
        2
    }
}

lockgate_plugin::export!(Fixture);
