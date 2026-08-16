#![no_std]

extern crate alloc;

use alloc::alloc::{Layout, alloc, handle_alloc_error, realloc};
use core::panic::PanicInfo;

// Proves that disabling the facade runtime leaves room for a guest-supplied
// allocator, canonical realloc export, and panic policy.
#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

macro_rules! __lockgate_wit_export {
    ($plugin:ident) => {};
}

struct RenamedFacade;

impl lgp::Plugin for RenamedFacade {
    const ID: &'static str = "renamed-facade";
    const DESCRIPTION: lgp::MetadataSource = lgp::MetadataSource::Absent;
    const LICENSE: lgp::MetadataSource = lgp::MetadataSource::Absent;
    const REPOSITORY: lgp::MetadataSource = lgp::MetadataSource::Absent;
    const HOMEPAGE: lgp::MetadataSource = lgp::MetadataSource::Absent;
    const NEEDS: lgp::Needs = lgp::Needs::NOTHING;
    type Settings = lgp::NoSettings;
}

lgp::export!(RenamedFacade);

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
            core::arch::wasm32::unreachable()
        }
    }
    pointer
}

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
