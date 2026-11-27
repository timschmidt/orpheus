//! Adversarial inputs (VER-3, 07 §3.7) for the step counter:
//! `/.orpheus/steps.bin` from the card (cut at every byte, every byte
//! damaged, and any days and counts with the CRC made right, then used:
//! rolled to any day, added to, merged), accelerometer samples at their
//! extremes (the part is unidentified, U-ACC-1: any 16-bit values), and
//! hostile trace lines.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;

use adversarial::{config, damaged, prefixes};
use orpheus_proto::hash::crc32;
use orpheus_steps::history::FILE_LEN;
use orpheus_steps::trace::{header_value, parse_line};
use orpheus_steps::{Detector, History, Sensitivity};
use proptest::prelude::*;

fn reseal(b: &mut [u8; FILE_LEN]) {
    let c = crc32(&b[..FILE_LEN - 4]);
    b[FILE_LEN - 4..].copy_from_slice(&c.to_le_bytes());
}

/// A loaded history in use: every operation the card task and the Steps
/// app make, with the invariants the screen relies on.
fn use_history(mut h: History, today: u32, extra: u32) {
    let mut live = History::new();
    live.add(extra);
    live.roll(today);
    live.merge_loaded(h);
    for d in [today, today.wrapping_add(1), today.wrapping_sub(1), 0, u32::MAX - 1, u32::MAX] {
        h.roll(d);
        h.add(extra);
        for ago in 0..9 {
            let _ = h.steps_ago(ago);
        }
        let _ = h.today();
        let _ = h.current_day();
        let b = h.encode();
        assert_eq!(History::decode(&b).map(|x| x.encode()), Some(b), "what is written reads back");
    }
    h.reset_today();
    assert_eq!(h.today(), 0);
}

#[test]
fn history_cut_and_damaged() {
    let mut h = History::new();
    h.roll(20_000);
    h.add(1234);
    h.roll(20_003);
    h.add(99);
    let good = h.encode();
    prefixes(&good, |p| assert!(History::decode(p).is_none()));
    damaged(&good, |i, d| {
        let mut b: [u8; FILE_LEN] = d.try_into().unwrap();
        assert!(History::decode(&b).is_none(), "byte {i} damaged, CRC wrong: loaded");
        reseal(&mut b);
        if let Some(h) = History::decode(&b) {
            use_history(h, 20_004, 5);
        }
    });
    let mut long = good.to_vec();
    long.extend_from_slice(&[0xFF; 100]);
    assert!(History::decode(&long).is_some(), "trailing bytes are ignored");
}

#[test]
fn detector_samples_at_their_extremes() {
    for (s, rate) in [(Sensitivity::Low, 25), (Sensitivity::Medium, 0), (Sensitivity::High, u16::MAX)] {
        let mut d = Detector::new(rate, s);
        d.set_sensitivity(s);
        let ext = [i16::MIN, i16::MIN + 1, -1, 0, 1, i16::MAX - 1, i16::MAX];
        let mut total = 0u32;
        for &x in &ext {
            for &y in &ext {
                for &z in &ext {
                    total = total.saturating_add(d.feed(x, y, z));
                }
            }
        }
        // Full-scale square waves on every axis for a minute at 25 Hz.
        for i in 0..1500 {
            let v = if i % 2 == 0 { i16::MAX } else { i16::MIN };
            total = total.saturating_add(d.feed(v, v, v));
        }
        for m in [i32::MIN, i32::MIN + 1, -1, 0, 1, i32::MAX - 1, i32::MAX] {
            for _ in 0..30 {
                total = total.saturating_add(d.feed_magnitude(m));
            }
        }
        d.restart();
        // A step per two samples at most.
        assert!(total <= (7 * 7 * 7 + 1500 + 7 * 30) / 2 + 8, "{total} steps from {s:?} extremes");
    }
}

#[test]
fn hostile_trace_lines() {
    for l in [
        "",
        "#",
        "1,2,3",
        "1,2,3,4,5",
        "18446744073709551616,0,0,0",
        "-1,0,0,0",
        "0,32768,0,0",
        "0,-32769,0,0",
        "0, 1 , 2 ,3",
        ",,,",
        "\u{FEFF}0,0,0,0",
        "0,0,0,0\u{0}",
        "nan,inf,0,0",
    ] {
        let _ = parse_line(l);
    }
    let _ = header_value("# truth= rate==25 tolerance=\n#truth=x", "truth");
    let _ = header_value(&"# ".repeat(10_000), "rate");
}

proptest! {
    #![proptest_config(config("steps-history-any-days", 256))]

    /// Any days and counts with a right CRC, used.
    #[test]
    fn any_days_and_counts(days in prop::array::uniform7((any::<u32>(), any::<u32>())),
                           today in any::<u32>(), extra in any::<u32>()) {
        let mut b = History::new().encode();
        for (i, (d, s)) in days.iter().enumerate() {
            b[8 + i * 8..12 + i * 8].copy_from_slice(&d.to_le_bytes());
            b[12 + i * 8..16 + i * 8].copy_from_slice(&s.to_le_bytes());
        }
        reseal(&mut b);
        let h = History::decode(&b).expect("a well-formed file loads");
        use_history(h, today, extra);
    }

    /// Any sample stream: bounded step counts.
    #[test]
    fn any_samples(rate in any::<u16>(), samples in prop::collection::vec((any::<i16>(), any::<i16>(), any::<i16>()), 0..600)) {
        let mut d = Detector::new(rate, Sensitivity::High);
        let mut total = 0u32;
        for (x, y, z) in &samples {
            total += d.feed(*x, *y, *z);
        }
        prop_assert!(total as usize <= samples.len() / 2 + 2, "{} steps from {} samples", total, samples.len());
    }
}
