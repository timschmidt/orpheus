//! Pacing of long card work on the real-time executor (owner's soak,
//! 2026-11-25).
//!
//! The card task runs on the interrupt-level executor and the SD driver
//! spins for its microsecond transfers, so a loop of card reads never gives
//! the CPU back on its own: thread mode (the UI, USB, the watchdog's task)
//! starves, and a long loop ends in a watchdog reset (Shuffle all over the
//! library, the album-cover search in a large folder, the card walk). The
//! card's read and write ask [`due`] before each transfer and, when the
//! task has worked [`RUN_US`] since its last rest, sleep [`REST_US`] first.

/// Longest stretch of card work before a rest (µs).
pub const RUN_US: u32 = 8_000;
/// Length of a rest (µs): thread mode gets at least REST / (RUN + REST),
/// a third (1.5 ms left a screenshot over USB timing out behind a long
/// cover search).
pub const REST_US: u32 = 4_000;

/// Whether a rest is due at `now_us`, the last one having ended at
/// `last_us` (both from a free-running µs counter; wrapping is fine).
#[must_use]
pub fn due(now_us: u32, last_us: u32) -> bool {
    now_us.wrapping_sub(last_us) >= RUN_US
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression (2026-11-25): card loops on the RT executor never
    /// yielded and reset the player through the watchdog. A rest is due
    /// after RUN_US of work, also across the counter's wrap.
    #[test]
    fn regress_card_work_rests_every_8_ms() {
        assert!(!due(1_000, 0));
        assert!(!due(RUN_US - 1, 0));
        assert!(due(RUN_US, 0));
        assert!(due(5_000, u32::MAX - 4_000));
        assert!(!due(2_000, u32::MAX - 4_000));
    }
}
