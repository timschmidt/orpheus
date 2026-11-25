//! Stage 2's slot decision (docs/plan/02-architecture.md §2.2, §2.5): which
//! firmware slot to start, from the boot counter word and what the slots
//! hold. `fw/boot/stage2` runs [`decide`] itself (it must stay ≤ 1 KiB, so
//! this is small and `#[inline(always)]`); the host tests below cover every
//! case.
//!
//! **Counter word** (`0x137FF8`, [`crate::mailbox::OFF_BOOT_COUNT`]):
//! `"BMY" << 8 | tripped << 4 | tries`. `tries` (bits 0–3) counts starts
//! of the slot being tried now; `tripped` (bits 4–6, bit `4 + i` for
//! [`SLOTS`]`[i]`) marks slots that failed [`BOOT_COUNT_LIMIT`] starts in a
//! row. The firmware writes 0 once a start is healthy (30 s) and before a
//! deliberate reboot, which forgets both. A word without the tag reads as
//! "nothing tried yet" (cold boot); the old stage 2's `"BMY" << 8 | tries`
//! is the same word with `tripped = 0`.
//!
//! **Order of candidates**: the valid (`"YFW1"`) slots in [`SLOTS`] order;
//! once a slot has tripped, also the superseded slots (magic word cleared by
//! an update, [`crate::slot::SUPERSEDED`]; the image behind it is intact
//! and stage 2 checks its resident sum as for a valid one), in the same
//! order — after an update A → B the old image is only superseded, and it
//! is the one to go back to when B keeps crashing. A slot whose bit is set
//! in `tripped` is skipped.
//!
//! Nothing here writes NOR; the state lives in the two RAM words, so a
//! power cycle (RAM lost) starts again from the first valid slot.

use crate::layout::SLOTS;
use crate::mailbox::{
    BOOT_COUNT_LIMIT, BOOT_COUNT_TAG, BootInfo, PATH_ADFU_COUNTER, PATH_ADFU_NOIMAGE, PATH_FIRMWARE,
};
use crate::slot::{MAGIC, SUPERSEDED};

/// Number of slots stage 2 knows.
pub const N_SLOTS: usize = SLOTS.len();

/// The counter word for `tripped` slots and `tries` starts of the current
/// one.
#[must_use]
pub const fn count_word(tripped: u8, tries: u8) -> u32 {
    BOOT_COUNT_TAG << 8 | ((tripped as u32) & 7) << 4 | (tries as u32) & 0xF
}

/// `(tripped, tries)` of a counter word (`(0, 0)` without the tag).
#[must_use]
pub const fn parse_count(w: u32) -> (u8, u8) {
    if w >> 8 == BOOT_COUNT_TAG { (((w >> 4) & 7) as u8, (w & 0xF) as u8) } else { (0, 0) }
}

/// What stage 2 does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Decision {
    /// Index in [`SLOTS`] to start; `None` = USB recovery.
    pub slot: Option<u8>,
    /// New counter word.
    pub count: u32,
    /// New boot-info word ([`BootInfo`]): path 1 with `tripped << 4 |
    /// tries` and the slot index, or path 5 (every candidate tripped) /
    /// 6 (no image) with `tripped << 4`.
    pub info: u32,
}

/// Decide from the counter word `count`. `startable(i, magic)` says
/// whether slot `i` has header magic `magic` ([`MAGIC`] or
/// [`SUPERSEDED`]) and an intact resident image (stage 2: version, bounds,
/// resident sum). It is called in candidate order, and when a slot is
/// chosen the call for it was the last one (stage 2 keeps that slot's
/// header in its scratch buffer and starts from it).
#[inline(always)]
pub fn decide(count: u32, mut startable: impl FnMut(usize, u32) -> bool) -> Decision {
    let (mut tripped, mut tries) = parse_count(count);
    // Two passes (valid, then superseded), each over the slots; nested so
    // that `i < N_SLOTS` is evident to the optimiser (stage 2 indexes
    // `SLOTS[i]` without a bounds-check panic path).
    let mut pass = 0;
    while pass < 2 {
        let magic = if pass == 0 { MAGIC } else { SUPERSEDED };
        let mut i = 0;
        while i < N_SLOTS {
            if tripped & 1 << i == 0 && (pass == 0 || tripped != 0) && startable(i, magic) {
                if u32::from(tries) < BOOT_COUNT_LIMIT {
                    tries += 1;
                    let info = BootInfo { path: PATH_FIRMWARE, attempt: tripped << 4 | tries, keys: i as u8 };
                    return Decision {
                        slot: Some(i as u8),
                        count: count_word(tripped, tries),
                        info: info.to_word(),
                    };
                }
                // This is the slot the last starts tried: it tripped.
                tripped |= 1 << i;
                tries = 0;
            }
            i += 1;
        }
        pass += 1;
    }
    let path = if tripped != 0 { PATH_ADFU_COUNTER } else { PATH_ADFU_NOIMAGE };
    Decision { slot: None, count: 0, info: BootInfo { path, attempt: tripped << 4, keys: 0 }.to_word() }
}

/// Short name of slot `i` for the UI and the host tools.
#[must_use]
pub const fn slot_name(i: u8) -> &'static str {
    match i {
        0 => "A",
        1 => "B",
        2 => "dual",
        _ => "?",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// What a slot holds in a test.
    #[derive(Clone, Copy, PartialEq, Eq, Debug)]
    enum S {
        Empty,
        Valid,
        Superseded,
    }
    use S::{Empty, Superseded, Valid};

    fn magic_of(s: S) -> Option<u32> {
        match s {
            Empty => None,
            Valid => Some(MAGIC),
            Superseded => Some(SUPERSEDED),
        }
    }

    /// Run `decide` over `slots`, recording the calls.
    fn run(count: u32, slots: [S; 3]) -> (Decision, Vec<(usize, u32)>) {
        let mut calls = Vec::new();
        let d = decide(count, |i, m| {
            calls.push((i, m));
            magic_of(slots[i]) == Some(m)
        });
        (d, calls)
    }

    /// Boot `n` times without a healthy start (the counter word carried
    /// over each time), returning the slot each boot started.
    fn crashes(slots: [S; 3], n: usize) -> (Vec<Option<u8>>, u32) {
        let mut w = 0xDEAD_BEEF; // cold boot: no tag
        let mut out = Vec::new();
        for _ in 0..n {
            let (d, _) = run(w, slots);
            out.push(d.slot);
            w = d.count;
        }
        (out, w)
    }

    fn info(d: &Decision) -> BootInfo {
        BootInfo::from_word(d.info).unwrap()
    }

    #[test]
    fn count_word_layout() {
        assert_eq!(count_word(0, 3), BOOT_COUNT_TAG << 8 | 3);
        assert_eq!(parse_count(count_word(0b101, 2)), (0b101, 2));
        assert_eq!(parse_count(0x1234_5678), (0, 0));
        assert_eq!(parse_count(0), (0, 0));
        // The old stage 2's word: tries only.
        assert_eq!(parse_count(BOOT_COUNT_TAG << 8 | 2), (0, 2));
    }

    #[test]
    fn a_valid_b_empty() {
        let (d, calls) = run(0, [Valid, Empty, Empty]);
        assert_eq!(d.slot, Some(0));
        assert_eq!(d.count, count_word(0, 1));
        assert_eq!(info(&d), BootInfo { path: PATH_FIRMWARE, attempt: 1, keys: 0 });
        assert_eq!(calls, [(0, MAGIC)]);
        // A crashes three times, then nothing is left: USB recovery, the
        // counter cleared, A recorded as tripped.
        let (s, w) = crashes([Valid, Empty, Empty], 4);
        assert_eq!(s, [Some(0), Some(0), Some(0), None]);
        assert_eq!(w, 0);
        let (d, _) = run(count_word(0, 3), [Valid, Empty, Empty]);
        assert_eq!(info(&d), BootInfo { path: PATH_ADFU_COUNTER, attempt: 1 << 4, keys: 0 });
    }

    #[test]
    fn a_tripped_falls_back_to_b() {
        let slots = [Valid, Valid, Empty];
        let (s, _) = crashes(slots, 3);
        assert_eq!(s, [Some(0); 3]);
        let (d, calls) = run(count_word(0, 3), slots);
        assert_eq!(d.slot, Some(1));
        assert_eq!(d.count, count_word(1, 1));
        let i = info(&d);
        assert_eq!((i.path, i.slot(), i.tries(), i.tripped()), (PATH_FIRMWARE, Some(1), 1, 1));
        assert!(i.fell_back());
        // The call for the chosen slot was the last one.
        assert_eq!(calls.last(), Some(&(1, MAGIC)));
        // A stays skipped (not even read) while B is tried.
        let (d, calls) = run(d.count, slots);
        assert_eq!(d.slot, Some(1));
        assert_eq!(calls, [(1, MAGIC)]);
    }

    #[test]
    fn b_tripped_falls_back_to_a() {
        // B alone (A erased): B trips, nothing to fall back to.
        let (d, _) = run(count_word(0, 3), [Empty, Valid, Empty]);
        assert_eq!(d.slot, None);
        // A tripped earlier, B crashing too: nothing left.
        let (d, _) = run(count_word(1, 3), [Valid, Valid, Empty]);
        assert_eq!(d.slot, None);
        assert_eq!(info(&d).tripped(), 0b011);
        // The real B → A case: running B, A superseded (update A → B);
        // B crashes three times, the superseded A starts.
        let slots = [Superseded, Valid, Empty];
        let (s, w) = crashes(slots, 6);
        assert_eq!(s, [Some(1), Some(1), Some(1), Some(0), Some(0), Some(0)]);
        let (d, calls) = run(count_word(0b10, 1), slots);
        assert_eq!(d.slot, Some(0));
        assert_eq!(calls, [(0, MAGIC), (2, MAGIC), (0, SUPERSEDED)]);
        let i = info(&d);
        assert_eq!((i.slot(), i.tripped(), i.tries()), (Some(0), 0b10, 2));
        // ... and when the superseded A crashes as well: USB recovery.
        let (d, _) = run(w, slots);
        assert_eq!(d.slot, None);
        assert_eq!(info(&d).path, PATH_ADFU_COUNTER);
        assert_eq!(info(&d).tripped(), 0b11);
    }

    #[test]
    fn both_tripped_adfu() {
        let slots = [Valid, Valid, Empty];
        let (s, w) = crashes(slots, 7);
        assert_eq!(s, [Some(0), Some(0), Some(0), Some(1), Some(1), Some(1), None]);
        assert_eq!(w, 0, "the counter starts afresh after USB recovery");
        let (d, _) = run(count_word(0b01, 3), slots);
        assert_eq!(
            d,
            Decision {
                slot: None,
                count: 0,
                info: BootInfo { path: PATH_ADFU_COUNTER, attempt: 0b11 << 4, keys: 0 }.to_word()
            }
        );
        // The next reset (from USB recovery) tries A again.
        assert_eq!(run(0, slots).0.slot, Some(0));
    }

    #[test]
    fn invalid_slots_skipped() {
        assert_eq!(run(0, [Empty, Valid, Empty]).0.slot, Some(1));
        assert_eq!(run(0, [Empty, Empty, Empty]).0.slot, None);
        let (d, calls) = run(0, [Empty, Empty, Empty]);
        assert_eq!(info(&d), BootInfo { path: PATH_ADFU_NOIMAGE, attempt: 0, keys: 0 });
        // Superseded slots are not candidates before something tripped.
        assert_eq!(calls, [(0, MAGIC), (1, MAGIC), (2, MAGIC)]);
        assert_eq!(run(0, [Superseded, Empty, Empty]).0.slot, None);
        assert_eq!(run(0, [Superseded, Valid, Empty]).0.slot, Some(1));
        // A superseded slot that tripped is not retried.
        let (d, _) = run(count_word(0b011, 3), [Superseded, Valid, Valid]);
        assert_eq!(d.slot, None);
    }

    #[test]
    fn dual_slot() {
        // The old dual-boot slot alone (dual layout).
        let (d, _) = run(0, [Empty, Empty, Valid]);
        assert_eq!((d.slot, info(&d).slot()), (Some(2), Some(2)));
        // Full A tripped, B empty: the dual slot is next.
        let (s, _) = crashes([Valid, Empty, Valid], 6);
        assert_eq!(s, [Some(0), Some(0), Some(0), Some(2), Some(2), Some(2)]);
        // A and B tripped: the dual slot.
        let (d, _) = run(count_word(0b01, 3), [Valid, Valid, Valid]);
        assert_eq!(d.slot, Some(2));
        assert_eq!(info(&d).tripped(), 0b011);
        // All three tripped.
        let (s, _) = crashes([Valid, Valid, Valid], 10);
        assert_eq!(s[9], None);
    }

    #[test]
    fn counter_after_a_good_boot() {
        // B started after A tripped; the firmware's healthy start (30 s)
        // and every deliberate reboot write 0: the next start tries A again
        // with a fresh count (RAM is all stage 2 has; it never writes NOR).
        let slots = [Valid, Valid, Empty];
        let (d, _) = run(count_word(0, 3), slots);
        assert_eq!(d.slot, Some(1));
        let healthy = 0;
        let (d, _) = run(healthy, slots);
        assert_eq!(d.slot, Some(0));
        assert_eq!(d.count, count_word(0, 1));
        assert!(!info(&d).fell_back());
        // A good boot in the middle of A's tries resets them.
        let (s, _) = crashes(slots, 2);
        assert_eq!(s, [Some(0), Some(0)]);
        let (d, _) = run(0, slots);
        assert_eq!(parse_count(d.count), (0, 1));
    }

    #[test]
    fn garbage_counter_words() {
        // Tag with out-of-range tries: the current slot trips at once.
        let (d, _) = run(BOOT_COUNT_TAG << 8 | 0x0F, [Valid, Valid, Empty]);
        assert_eq!(d.slot, Some(1));
        // Bit 7 is ignored; every word decides without panicking.
        for w in [0u32, u32::MAX, BOOT_COUNT_TAG << 8 | 0xFF, BOOT_COUNT_TAG << 8 | 0x80] {
            for a in [Empty, Valid, Superseded] {
                for b in [Empty, Valid, Superseded] {
                    let (d, _) = run(w, [a, b, Valid]);
                    assert!(d.slot.is_none_or(|s| s < 3));
                    assert!(BootInfo::from_word(d.info).is_some());
                }
            }
        }
    }

    #[test]
    fn names() {
        assert_eq!([slot_name(0), slot_name(1), slot_name(2), slot_name(9)], ["A", "B", "dual", "?"]);
    }
}
