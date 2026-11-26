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
/// that may come first, the rest's late end and the code-cache lines the
/// render loads again after it (2026-11-26, the player: `late=` up to
/// 1.8 ms, ≈ 2 ms of reloads; was 2 ms, `ub=synth:rest:5ms`).
pub const MARGIN_US: u32 = 4_000;
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

/// How long after thread mode went to sleep during a playback rest the
/// resting task looks again (µs): still asleep then, thread mode has run
/// everything it had, and the rest ends ([`rest_over`]). Long enough for a
/// wake-up that was already pending (a task woken in the main executor's
/// last poll) to have cleared the mark.
pub const IDLE_PROBE_US: u32 = 50;

/// Whether a playback rest until `until_us` is over at `now_us`: its time
/// is up, or — after its first look (`resting`) — thread mode is asleep
/// (`thread_asleep`: it went to sleep after the rest began and nothing woke
/// it since). A rest is for thread mode; once thread mode sleeps, the rest
/// of it idled the CPU while the PCM ring ran short (2026-11-26, the
/// player: 20 underruns in 107 s at a render load of 25–40 %; a third of
/// the time in rests, the PC profile 20 % asleep in thread mode).
#[must_use]
#[inline(always)]
pub fn rest_over(now_us: u64, until_us: u64, resting: bool, thread_asleep: bool) -> bool {
    now_us >= until_us || (resting && thread_asleep)
}

/// Whether thread mode, going to sleep at `now_us` (low 32 bits) during a
/// rest that ends at `end_us` (low 32 bits), has the resting task look
/// again [`IDLE_PROBE_US`] later: only if the rest lasts longer than that
/// (else it ends by itself; a stale entry is never probed).
#[must_use]
#[inline(always)]
pub fn probe_due(now_us: u32, end_us: u32) -> bool {
    (end_us.wrapping_sub(now_us) as i32) > IDLE_PROBE_US as i32
}

/// How long a playback rest lasts when thread mode has `backlog_us` of
/// work waiting as it starts and its sleep ends the rest ([`rest_over`]):
/// the work and the probe, at most [`REST_US`] (the host model's view;
/// work that arrives during the rest is the caller's).
#[must_use]
pub fn rest_len(backlog_us: u32) -> u32 {
    backlog_us.saturating_add(IDLE_PROBE_US).min(REST_US)
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

    /// A rest ends at its time, or once thread mode sleeps — never on its
    /// first look; thread mode probes only a rest that lasts longer than
    /// the probe (also across the counter's wrap).
    #[test]
    fn a_rest_ends_when_thread_mode_sleeps() {
        assert!(rest_over(4_000, 4_000, false, false), "time up");
        assert!(!rest_over(1_000, 4_000, false, true), "first look: thread mode has not run yet");
        assert!(!rest_over(1_000, 4_000, true, false), "thread mode busy");
        assert!(rest_over(1_000, 4_000, true, true), "thread mode asleep");
        assert!(probe_due(0, IDLE_PROBE_US + 1));
        assert!(!probe_due(0, IDLE_PROBE_US));
        assert!(!probe_due(5_000, 4_000), "stale");
        assert!(probe_due(u32::MAX - 10, 1_000), "across the wrap");
        assert_eq!(rest_len(0), IDLE_PROBE_US);
        assert_eq!(rest_len(1_000), 1_000 + IDLE_PROBE_US);
        assert_eq!(rest_len(10_000), REST_US);
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
