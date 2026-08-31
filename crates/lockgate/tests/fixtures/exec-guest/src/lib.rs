#![no_std]

use core::panic::PanicInfo;

// This one low-level execution fixture deliberately stays facade-independent:
// it proves Lockgate can host a raw wit-bindgen component, so it retains the
// single hand-written allocator and panic floor used by the raw fixture set.
#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

wit_bindgen::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

static mut PIN_COUNT: u32 = 0;

impl exports::test::exec::guest::Guest for Fixture {
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

    fn spin() -> u32 {
        loop {
            core::hint::spin_loop();
        }
    }

    async fn suspend() -> u32 {
        test::exec::host::wait().await;
        7
    }

    fn trap() {
        core::arch::wasm32::unreachable()
    }

    async fn import_then_trap() {
        test::exec::host::wait().await;
        core::arch::wasm32::unreachable()
    }
}

export!(Fixture);

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
