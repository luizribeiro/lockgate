#![no_std]

extern crate alloc;

use alloc::string::String;
use core::panic::PanicInfo;

#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

lockgate_plugin::generate!({
    path: "../../data/detached_jobs",
    world: "fixture",
});

struct Fixture;

impl lockgate_plugin::Plugin for Fixture {
    const ID: &'static str = "detached-jobs";
}

impl exports::test::detached_jobs::guest::Guest for Fixture {
    fn start(kind: u8) -> String {
        match test::detached_jobs::application::start(kind) {
            Ok(job_id) => job_id,
            Err(error) => alloc::format!("error:{error}"),
        }
    }

    async fn cancel() {
        if test::detached_jobs::application::start(0).is_err() {
            core::arch::wasm32::unreachable();
        }
        test::detached_jobs::application::suspend().await;
    }

    fn healthy() -> u32 {
        7
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
