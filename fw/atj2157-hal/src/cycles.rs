//! The Cortex-M4's cycle counter (`DWT_CYCCNT`, ARMv7-M architecture
//! reference manual C1.8), for measurements (`synth bench`, SYN-3).
//!
//! Whether the ATJ2157 implements the DWT cycle counter is not documented
//! (U-DWT-1, 01-hardware §9): [`start`] enables it (`DEMCR.TRCENA`,
//! `DWT_CTRL.CYCCNTENA`) unless `DWT_CTRL.NOCYCCNT` says it is absent, and
//! [`ok`] reports whether it counts. Callers also time with
//! [`crate::time::now_us`] and the clock level, so a missing counter costs
//! nothing but precision. Reading it is one load (no side effects); the
//! counter wraps every 2³² cycles (30 s at 144 MHz).

use core::ptr::{read_volatile, write_volatile};

const DEMCR: *mut u32 = 0xE000_EDFC as *mut u32;
const DWT_CTRL: *mut u32 = 0xE000_1000 as *mut u32;
const DWT_CYCCNT: *mut u32 = 0xE000_1004 as *mut u32;
const TRCENA: u32 = 1 << 24;
const CYCCNTENA: u32 = 1;
const NOCYCCNT: u32 = 1 << 25;

/// Enable the counter (idempotent). False: the core has no cycle counter.
pub fn start() -> bool {
    // SAFETY: architectural debug registers of the core (System Control
    // Space); setting TRCENA and CYCCNTENA only enables the DWT's counter,
    // which nothing else in the firmware uses.
    unsafe {
        write_volatile(DEMCR, read_volatile(DEMCR) | TRCENA);
        let c = read_volatile(DWT_CTRL);
        if c & NOCYCCNT != 0 {
            return false;
        }
        write_volatile(DWT_CTRL, c | CYCCNTENA);
    }
    ok()
}

/// The counter now (0 when it is absent or off).
#[inline(always)]
#[must_use]
pub fn now() -> u32 {
    // SAFETY: a read of an architectural register.
    unsafe { read_volatile(DWT_CYCCNT) }
}

/// Whether the counter counts (two reads differ).
#[must_use]
pub fn ok() -> bool {
    let a = now();
    core::hint::spin_loop();
    core::hint::spin_loop();
    now() != a
}
