//! Fuzz-target body of `orpheus-input` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`input_samples`] — arbitrary sequences of raw key samples (two ADC
//! ladders and the power line), as noise, bounce and multi-key presses
//! produce them. Oracles: no panic; a sample decodes to at most one key per
//! ladder plus Power; the earphone band never decodes as a ladder-1 key;
//! the debouncer reports a key change only after two equal samples, each
//! edge flips exactly that key, edges come in key order, and the held set
//! always equals the edges applied so far.

#![forbid(unsafe_code)]

use orpheus_input::{Debouncer, EARPHONE_BAND, Key, Keys, Sample, decode, earphones_present};

const LADDER1: [Key; 4] = [Key::Back, Key::Next, Key::VolUp, Key::VolDown];
const LADDER2: [Key; 5] = [Key::Menu, Key::Prev, Key::Down, Key::Ok, Key::Up];

/// Input: 5-byte samples (ladder 1 and 2 as little-endian 16-bit words,
/// power in bit 0 of the fifth byte); bit 1 of the fifth byte repeats the
/// sample once more (a stable reading).
pub fn input_samples(data: &[u8]) {
    for i in 0..=10u8 {
        if let Some(k) = Key::from_index(i) {
            assert_eq!(Key::ALL[usize::from(i)], k);
            assert_eq!(k.bit(), 1 << i);
            assert!(!k.name().is_empty());
        } else {
            assert_eq!(i, 10);
        }
    }
    let mut d = Debouncer::new();
    let mut held = Keys::NONE;
    let mut last: Option<Keys> = None;
    for s in data.chunks_exact(5).take(4096) {
        let sample = Sample {
            ladder1: u16::from_le_bytes([s[0], s[1]]),
            ladder2: u16::from_le_bytes([s[2], s[3]]),
            power: s[4] & 1 != 0,
        };
        let reps = if s[4] & 2 != 0 { 2 } else { 1 };
        let keys = decode(sample);
        assert!(keys.iter().filter(|k| LADDER1.contains(k)).count() <= 1);
        assert!(keys.iter().filter(|k| LADDER2.contains(k)).count() <= 1);
        assert_eq!(keys.contains(Key::Power), sample.power);
        assert!(keys.0 >> 10 == 0);
        if earphones_present(sample) {
            assert!(EARPHONE_BAND.contains(&(sample.ladder1 & 0x3FF)));
            assert!(!LADDER1.iter().any(|&k| keys.contains(k)), "earphone band decoded as a key");
        }
        for _ in 0..reps {
            let before = d.held();
            let (edges, n) = d.feed(keys);
            assert!(n <= 10);
            let stable = last == Some(keys);
            if n > 0 {
                assert!(stable, "a change reported after one sample");
            }
            let mut prev: Option<Key> = None;
            for e in &edges[..n] {
                assert_ne!(held.contains(e.key), e.down, "edge {e:?} does not change the key");
                held = Keys(held.0 ^ e.key.bit());
                if let Some(p) = prev {
                    assert!(p < e.key, "edges out of key order");
                }
                prev = Some(e.key);
            }
            assert_eq!(d.held(), held, "held keys differ from the edges");
            if stable {
                assert_eq!(d.held(), keys, "two equal samples are the held set");
            } else {
                assert_eq!(d.held(), before);
            }
            last = Some(keys);
        }
    }
}
