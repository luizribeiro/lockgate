#![no_std]

extern crate alloc;

use alloc::string::String;
use core::ffi::c_void;

lockgate_plugin::generate!({
    path: "../../data/host_export_values",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "host-export-values";
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

lockgate_plugin::export!(Fixture);

#[unsafe(no_mangle)]
unsafe extern "C" fn memcmp(left: *const c_void, right: *const c_void, len: usize) -> i32 {
    let left = left.cast::<u8>();
    let right = right.cast::<u8>();
    for offset in 0..len {
        let left = unsafe { *left.add(offset) };
        let right = unsafe { *right.add(offset) };
        if left != right {
            return i32::from(left) - i32::from(right);
        }
    }
    0
}
