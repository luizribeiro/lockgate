#![no_std]

extern crate alloc;

use alloc::string::String;
use core::panic::PanicInfo;

#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

wit_bindgen::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

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

export!(Fixture);

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
