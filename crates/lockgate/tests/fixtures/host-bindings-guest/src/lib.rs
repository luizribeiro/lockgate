#![no_std]

extern crate alloc;

use alloc::string::String;
use core::panic::PanicInfo;

#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

lockgate_plugin::generate!({
    path: "../../data/host_bindings",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "host-caller";
}

impl exports::test::host_bindings::guest::Guest for Fixture {
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

#[unsafe(export_name = "cabi_realloc")]
unsafe extern "C" fn cabi_realloc(
    old_ptr: *mut u8,
    old_len: usize,
    align: usize,
    new_len: usize,
) -> *mut u8 {
    use alloc::alloc::{Layout, alloc, handle_alloc_error, realloc};

    let layout;
    let pointer = unsafe {
        if old_len == 0 {
            if new_len == 0 {
                return align as *mut u8;
            }
            layout = Layout::from_size_align_unchecked(new_len, align);
            alloc(layout)
        } else {
            layout = Layout::from_size_align_unchecked(old_len, align);
            realloc(old_ptr, layout, new_len)
        }
    };
    if pointer.is_null() {
        if cfg!(debug_assertions) {
            handle_alloc_error(layout);
        } else {
            core::arch::wasm32::unreachable();
        }
    }
    pointer
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
