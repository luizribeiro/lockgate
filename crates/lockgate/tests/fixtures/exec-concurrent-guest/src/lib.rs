#![no_std]

use core::panic::PanicInfo;

#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

wit_bindgen::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl exports::test::exec_concurrent::guest::Guest for Fixture {
    async fn run() -> u32 {
        futures::join!(
            test::exec_concurrent::host::first(),
            test::exec_concurrent::host::second(),
        );
        2
    }
}

export!(Fixture);

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
