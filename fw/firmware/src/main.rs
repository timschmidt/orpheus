//! yotonrs firmware. Placeholder entry until WP-HAL-1 / WP-PG-1 wire the
//! real start-up; proves the device workspace builds.
#![no_std]
#![no_main]

core::arch::global_asm!(
    ".section .init, \"ax\"",
    ".global _start",
    ".thumb_func",
    "_start:",
    "    cpsid i",
    "    ldr r0, =__stack_top",
    "    mov sp, r0",
    "    b {main}",
    ".ltorg",
    main = sym main,
);

extern "C" fn main() -> ! {
    loop {
        core::hint::spin_loop();
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {}
}
