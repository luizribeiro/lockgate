#![no_std]

use core::panic::PanicInfo;

#[global_allocator]
static ALLOCATOR: dlmalloc::GlobalDlmalloc = dlmalloc::GlobalDlmalloc;

wit_bindgen::generate!({
    path: "wit",
    world: "fixture",
});

struct Fixture;

impl exports::test::exec_fuel::guest::Guest for Fixture {
    fn run(iterations: u32) -> u32 {
        let mut checksum = 0x9e37_79b9_u32;
        for iteration in 1..=iterations {
            for round in 0..32_u32 {
                checksum = checksum
                    .rotate_left(5)
                    .wrapping_add(iteration)
                    .wrapping_mul(0x045d_9f3b)
                    ^ round;
            }
            test::exec_fuel::host::report(iteration, checksum);
        }
        checksum
    }
}

export!(Fixture);

#[panic_handler]
fn panic(_info: &PanicInfo<'_>) -> ! {
    core::arch::wasm32::unreachable()
}
