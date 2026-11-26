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

/// What the PCM ring must hold, beyond the rest and the piece after it,
/// for a playback rest to start (µs): the outer loop's part or a card read
/// that may come first.
pub const MARGIN_US: u32 = 2_000;
/// How much render a playback rest may be put off for while the ring is
/// short (µs): one render piece (`orpheus_synth::load::PIECE_US`).
pub const DEFER_US: u32 = 4_000;

/// The pace rule for a render that feeds the PCM ring (the synth's
/// pieces, SYN-3): before a piece predicted to take `piece_us` for
/// `len_us` of audio, `busy_us` after the last rest, with `head_us` of
/// audio queued ahead of the DAC (`u32::MAX`: the DAC is not running),
/// whether to rest [`REST_US`] first. A rest is due as by [`due`] when the
/// piece would end [`RUN_US`] or more after the last rest; it starts only
/// if the ring covers it, the piece after it and [`MARGIN_US`] —
/// otherwise the piece renders first and the rest follows. Put off only
/// while the render keeps up with its rests (the piece takes at most two
/// thirds of its audio's time: work and rests then outpace the DAC, so
/// the ring recovers and the next rests are on time) and for at most
/// [`DEFER_US`] of render (`deferred_us`, kept by the caller: what was put
/// off since the last rest); else it rests whatever the ring holds, so
/// thread mode keeps its share when the render cannot keep up (2026-11-26,
/// the player: underruns labelled `synth:rest:4ms`, rests started with
/// less than 4 ms of audio queued).
#[must_use]
pub fn rest_first(busy_us: u32, piece_us: u32, len_us: u32, head_us: u32, deferred_us: &mut u32) -> bool {
    if busy_us.saturating_add(piece_us) < RUN_US {
        return false;
    }
    let short = head_us < REST_US.saturating_add(piece_us).saturating_add(MARGIN_US);
    let keeps_up = piece_us.saturating_mul(3) <= len_us.saturating_mul(2);
    if short && keeps_up && deferred_us.saturating_add(piece_us) <= DEFER_US {
        *deferred_us += piece_us;
        return false;
    }
    *deferred_us = 0;
    true
}

/// Whether the card task should rest before a transfer: only on the
/// real-time executor (`handler`), never inside a synchronous poller
/// (`sync`: a timer would not fire there) or during work with a deadline
/// (`urgent`: the playback ring's refill), and only when [`due`].
#[must_use]
pub fn should_rest(handler: bool, sync: bool, urgent: bool, now_us: u32, last_us: u32) -> bool {
    handler && !sync && !urgent && due(now_us, last_us)
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

    /// A due rest waits for the ring (rest + piece + margin), but for at
    /// most [`DEFER_US`] of render and only while the render keeps up;
    /// not due: no rest.
    #[test]
    fn rest_first_waits_for_the_ring_within_a_bound() {
        let (full, len, ok) = (u32::MAX, 5_805, REST_US + 2_000 + MARGIN_US);
        let mut d = 0;
        assert!(!rest_first(1_000, 2_000, len, 0, &mut d), "not due");
        assert!(rest_first(6_000, 2_000, len, full, &mut d), "due, DAC not running");
        assert!(rest_first(6_000, 2_000, len, ok, &mut d));
        assert!(rest_first(6_000, 2_000, 2_999, 0, &mut d), "a render that cannot keep up: rest");
        assert_eq!(d, 0);
        assert!(!rest_first(6_000, 2_000, 3_000, ok - 1, &mut d), "short ring: render first");
        assert!(!rest_first(8_000, 2_000, len, 0, &mut d), "within the bound");
        assert_eq!(d, DEFER_US);
        assert!(rest_first(10_000, 1, len, 0, &mut d), "past the bound: rest");
        assert_eq!(d, 0, "a rest clears it");
        assert!(!rest_first(60_000, 2_000, len, 0, &mut d), "after a long stall too");
        assert!(rest_first(u32::MAX, u32::MAX, len, 0, &mut d));
    }

    /// Regression (2026-11-25): rests inside the playback refill made a
    /// refill take 25–29 ms; with the decoding it no longer fitted a 32 ms
    /// DAC half (27 underruns while browsing Pictures). Urgent work and
    /// synchronous pollers never rest; thread mode never rests.
    #[test]
    fn regress_playback_refill_is_never_paced() {
        assert!(should_rest(true, false, false, RUN_US, 0));
        assert!(!should_rest(true, false, true, RUN_US, 0), "urgent refill");
        assert!(!should_rest(true, true, false, RUN_US, 0), "synchronous poller");
        assert!(!should_rest(false, false, false, RUN_US, 0), "thread mode");
        assert!(!should_rest(true, false, false, RUN_US - 1, 0), "not due");
    }
}
