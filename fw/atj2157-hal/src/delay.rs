//! Busy-wait delays for start-up code that runs before the timers.

use core::sync::atomic::{AtomicU32, Ordering};

/// Core clock in Hz, updated by the clock governor.
static CORE_HZ: AtomicU32 = AtomicU32::new(48_000_000);

pub fn set_core_hz(hz: u32) {
    CORE_HZ.store(hz, Ordering::Relaxed);
}

#[must_use]
pub fn core_hz() -> u32 {
    CORE_HZ.load(Ordering::Relaxed)
}

/// Spin for at least `us` microseconds (loop of ~4 cycles per iteration;
/// no flash wait states, code in SRAM).
pub fn spin_us(us: u32) {
    let iters = (core_hz() / 4_000_000).max(1) * us;
    cortex_m::asm::delay(iters * 4);
}
