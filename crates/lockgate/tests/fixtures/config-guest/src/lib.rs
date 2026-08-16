#![no_std]

extern crate alloc;

use alloc::alloc::{Layout, alloc, handle_alloc_error, realloc};
use alloc::string::String;
use core::panic::PanicInfo;

#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

#[unsafe(export_name = "cabi_realloc")]
unsafe extern "C" fn cabi_realloc(
    old_ptr: *mut u8,
    old_len: usize,
    align: usize,
    new_len: usize,
) -> *mut u8 {
    let layout;
    let pointer = unsafe {
        if old_len == 0 {
            if new_len == 0 {
                return core::ptr::without_provenance_mut(align);
            }
            layout = Layout::from_size_align_unchecked(new_len, align);
            alloc(layout)
        } else {
            layout = Layout::from_size_align_unchecked(old_len, align);
            realloc(old_ptr, layout, new_len)
        }
    };
    if pointer.is_null() {
        handle_alloc_error(layout);
    }
    pointer
}

wit_bindgen::generate!({
    path: "wit",
    world: "fixture",
    generate_all,
});

struct Fixture;

impl exports::lockgate::config::schema::Guest for Fixture {
    fn settings_schema() -> String {
        match lockgate::config::settings::get_json() {
            Err(lockgate::config::settings::GetError::NotReady) => {}
            Ok(_) => panic!("settings became ready during the schema probe"),
        }
        r#"{"type":"object","required":["message"],"properties":{"message":{"type":"string"}},"additionalProperties":false}"#.into()
    }
}

impl exports::test::config_fixture::guest::Guest for Fixture {
    fn observed_settings() -> String {
        lockgate::config::settings::get_json().expect("validated settings must be ready")
    }
}

export!(Fixture);

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
