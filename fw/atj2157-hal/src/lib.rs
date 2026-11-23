//! ATJ2157 hardware abstraction: start-up, interrupts, BASEPRI critical
//! section, Embassy time driver, executors, watchdog, GPIO and the
//! persistent area (docs/plan/03-hal-drivers.md).
//!
//! Knows the SoC, not the board: pin assignments live in `yoton-bsp`.

#![no_std]

pub mod clock;
mod cs;
pub mod delay;
pub mod dma;
pub mod executor;
pub mod gpio;
pub mod interrupt;
pub mod lcm;
pub mod log;
pub mod persist;
pub mod pwm;
pub mod rt;
pub mod time;
pub mod wdt;

pub use atj2157_pac as pac;

/// Early hardware set-up shared by every image: interrupt priorities, the
/// clock state the boot ROM leaves in USB recovery (so a cold NOR boot and a
/// RAM load start from the same state), and the timers.
pub fn init() {
    interrupt::init_priorities();
    clock::rom_state();
    time::init();
    // Start-up left BASEPRI masking levels 1-3; open them now.
    // SAFETY: handlers are bound and priorities set; unmasking is the
    // intended state for the rest of the run.
    unsafe { cortex_m::register::basepri::write(0) };
}

/// Default panic behaviour for images: record and reboot (to ADFU when a
/// host test asked for it).
pub fn panic(info: &core::panic::PanicInfo) -> ! {
    let mut buf = log::LineBuf::new();
    if let Some(loc) = info.location() {
        let f = loc.file();
        // Keep the tail of the path so the line number fits.
        let mut parts = f.rsplit('/');
        let file = parts.next().unwrap_or(f);
        if let Some(dir) = parts.next() {
            buf.push_str(dir);
            buf.push_str("/");
        }
        buf.push_str(file);
        buf.push_str(":");
        buf.push_u32(loc.line());
    }
    persist::record_panic(buf.as_bytes(), 0);
    wdt::reboot_after_fault()
}
