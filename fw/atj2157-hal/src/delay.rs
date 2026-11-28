//! Busy-wait delays: [`spin_us`] for start-up code that runs before the
//! timers, [`wait_us`] on the time base once it runs.

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

/// Spin for at least `us` microseconds, counting loop iterations.
///
/// **It spins about three times as long** (AU-PERF, 2026-11-25, from the
/// code; not measured on the player): `cortex_m::asm::delay(n)` (0.7.10)
/// runs `n` iterations of `subs`/`bne`, 3–4 cycles each on the Cortex-M4
/// (01 §9), and is given `core_hz / 1 MHz × us` iterations. Left so: every
/// caller's wait (PLL, DAC, card, USB, NOR, LCD start-up) was proven on the
/// player with the longer time. New code with a time base uses
/// [`wait_us`].
pub fn spin_us(us: u32) {
    let iters = (core_hz() / 4_000_000).max(1) * us;
    cortex_m::asm::delay(iters * 4);
}

/// About 1 µs of CPU-only waiting (no bus access: a `subs`/`bne` loop of
/// `core_hz / 4 MHz` iterations, 3–4 cycles each) between two reads of a
/// peripheral register in a polling loop.
///
/// PERF-SD round 2 (2026-11-27): the card reader's waits spin, and a
/// tight loop reading `SD_CTL`, the T0 counter or the USB `DMA_STAT` at
/// 288 MHz starved the DMAs on the peripheral bus: with the USB OUT DMA
/// beside the SD write DMA at 48 MHz, ≈ 6 % of the card writes never
/// finished (the SD DMA still busy 500 ms after the controller's end;
/// the host saw I/O errors). At a pinned 48 MHz core the same writes
/// worked.
#[inline(always)]
pub fn pause() {
    cortex_m::asm::delay((core_hz() / 4_000_000).max(1));
}

/// As [`wait_us`], reading the time base about once a µs ([`pause`]
/// between reads): a long busy wait that leaves the bus to the DMAs.
#[inline(never)]
pub fn wait_us_gently(us: u32) {
    let t0 = crate::time::now_us_raw();
    for _ in 0..(us + 2).saturating_mul(4) {
        if crate::time::now_us_raw().wrapping_sub(t0) > us {
            return;
        }
        pause();
    }
}

/// Busy-wait at least `us` µs on the time base (T0 at 1 MHz, running from
/// `hal::init`): `us` to `us + 1` µs, whatever the core clock, and time
/// spent in interrupts counts towards it. Bounded by an iteration count
/// (256 per µs, far more than the loop can do at 288 MHz) so that a stopped
/// counter cannot hang the caller.
#[inline]
pub fn wait_us(us: u32) {
    let t0 = crate::time::now_us_raw();
    for _ in 0..(us + 2).saturating_mul(256) {
        if crate::time::now_us_raw().wrapping_sub(t0) > us {
            return;
        }
    }
}
