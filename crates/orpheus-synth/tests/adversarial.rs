//! Adversarial sound banks, sample WAVs and event streams (VER-3, 07
//! §3.7) through the fuzz bodies (`fuzz/src/lib.rs`: no panic; decoded
//! patches round-trip; every sample finite and within [−1, 1]; voice
//! counts bounded): every seed swept (cut at every byte of its head,
//! damaged, 16/32-bit fields at their edges); events with every
//! parameter at its float edges (NaN, ±∞, subnormal, ±max) and the
//! rate/voice limits at theirs.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
mod bodies;

use adversarial::{config, seeds, sweep};
use proptest::prelude::*;

#[test]
fn banks_swept() {
    for (name, s) in seeds("bank_decode") {
        eprintln!("{name}");
        sweep(&s, 0, 64, bodies::bank_decode);
    }
}

#[test]
fn sample_wavs_swept() {
    for (name, s) in seeds("wav_parse") {
        eprintln!("{name}");
        sweep(&s, 0, 64, bodies::wav_parse);
    }
}

/// The float values that break DSP code.
const FLOATS: [f32; 12] = [
    f32::NAN,
    f32::INFINITY,
    f32::NEG_INFINITY,
    f32::MAX,
    f32::MIN,
    f32::MIN_POSITIVE,
    1.0e-45,
    -0.0,
    0.0,
    1.0,
    -1.0,
    65_536.0,
];

#[test]
fn every_event_with_float_edges() {
    // Every op, part and a/b byte value class, with x at every float edge
    // and as u16 extremes, followed by notes and a render.
    for rate in 0..6u8 {
        for op in 0..16u8 {
            let mut d = vec![rate, 7, 0xFF, op];
            for x in FLOATS {
                for (part, a, b) in [(0u8, 0u8, 0u8), (15, 127, 127), (255, 255, 255), (9, 36, 1)] {
                    d.extend([op, part, a, b]);
                    d.extend(x.to_le_bytes());
                }
            }
            for x in [[0u8, 0, 0, 0], [0xFF, 0xFF, 0, 0], [0xFF; 4]] {
                d.extend([op, 1, 60, 100]);
                d.extend(x);
            }
            // Notes on every part, a render, notes off, a render.
            for part in 0..16u8 {
                d.extend([0, part, 60, 127, 0, 0, 0, 0]);
            }
            d.extend([8, 0, 0x00, 0x10, 0, 0, 0, 0]);
            for part in 0..16u8 {
                d.extend([1, part, 60, 0, 0, 0, 0, 0]);
            }
            d.extend([9, 0, 0x00, 0x10, 0, 0, 0, 0]);
            bodies::synth_events(&d);
        }
    }
    for (name, s) in seeds("synth_events") {
        eprintln!("{name}");
        sweep(&s, 0, 32, bodies::synth_events);
    }
}

proptest! {
    #![proptest_config(config("synth-event-records", 96))]

    /// Event records in any order with any values (renders included).
    #[test]
    fn event_records(head in prop::array::uniform4(any::<u8>()),
                     recs in prop::collection::vec((0u8..16, any::<u8>(), any::<u8>(), any::<u8>(),
                                                    prop_oneof![prop::sample::select(FLOATS.to_vec()), any::<f32>()]), 0..64)) {
        let mut d = head.to_vec();
        for (op, part, a, b, x) in recs {
            // Renders kept short so a case stays fast.
            let (a, b) = if op >= 8 { (a, b & 0x0F) } else { (a, b) };
            d.extend([op, part, a, b]);
            d.extend(x.to_le_bytes());
        }
        bodies::synth_events(&d);
    }
}
