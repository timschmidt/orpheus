//! ATJ2157 hardware abstraction: start-up, interrupts, BASEPRI critical
//! section, Embassy time driver, executors, watchdog, GPIO and the
//! persistent area (docs/plan/03-hal-drivers.md).
//!
//! Knows the SoC, not the board: pin assignments live in `yoton-bsp`.

#![no_std]
// `&*(&raw const STATIC)` is how 2024-edition code borrows a `static mut`
// buffer that a DMA engine also writes.
#![allow(clippy::deref_addrof)]

pub mod adc;
pub mod clock;
mod cs;
pub mod cycles;
pub mod dac;
pub mod delay;
pub mod dma;
pub mod executor;
pub mod gpio;
pub mod i2c_soft;
pub mod interrupt;
pub mod lcm;
pub mod log;
pub mod nor;
pub mod pager;
pub mod persist;
pub mod pmu;
#[cfg(feature = "prof")]
pub mod prof;
pub mod pwm;
pub mod rt;
pub mod rtc_clock;
pub mod sd;
pub mod time;
pub mod uart;
pub mod usb;
pub mod wake;
pub mod wdt;

pub use atj2157_pac as pac;

/// Early hardware set-up shared by every image: interrupt priorities, the
/// clock state the boot ROM leaves in USB recovery (so a cold NOR boot and a
/// RAM load start from the same state), and the timers.
pub fn init() {
    interrupt::init_priorities();
    clock::rom_state();
    time::init();
    rt::enable_stack_guard();
    // Start-up left BASEPRI masking levels 1-3; open them now.
    // SAFETY: handlers are bound and priorities set; unmasking is the
    // intended state for the rest of the run.
    unsafe { cortex_m::register::basepri::write(0) };
}

/// Default panic behaviour for images: record and reboot (to ADFU when a
/// host test asked for it).
pub fn panic(info: &core::panic::PanicInfo) -> ! {
    // Straight into the fault record: no line buffer on the stack (it is
    // the bottom of the deepest paths, BOOK-1/2).
    let mut buf = persist::PanicText::start();
    if let Some(loc) = info.location() {
        let f = loc.file();
        // Keep the tail of the path so the line number fits.
        // Keep the last three path components (crate/src/file). Without
        // indexing or string patterns that could panic: a panic in here
        // would recurse (VER-10, `xtask stack`).
        let mut cut = f.len();
        for _ in 0..3 {
            match f.as_bytes().get(..cut).and_then(|b| b.iter().rposition(|&c| c == b'/')) {
                Some(i) => cut = i,
                None => {
                    cut = 0;
                    break;
                }
            }
        }
        let tail = f.get(cut..).unwrap_or("");
        buf.push_str(if tail.as_bytes().first() == Some(&b'/') { tail.get(1..).unwrap_or("") } else { tail });
        buf.push_str(":");
        buf.push_u32(loc.line());
    }
    if let Some(m) = info.message().as_str() {
        buf.push_str(" ");
        buf.push_str(m);
    }
    buf.finish(0);
    wdt::reboot_after_fault()
}
