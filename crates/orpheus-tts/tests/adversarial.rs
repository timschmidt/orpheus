//! Adversarial inputs (VER-3, 07 §3.7) for text to speak (book text, file names, tags) and the
//! lexicon: every
//! committed seed of the crate's fuzz targets swept (`sweep` in
//! `tests/adversarial.rs`: cut at every byte of its head and at spread
//! points beyond, every head byte damaged, 16/32-bit fields at
//! 0/1/max−1/max at every head offset in both byte orders) through the
//! fuzz bodies, whose oracles are in `fuzz/src/lib.rs` and
//! `docs/verification/baseline-2026-11.md` §8.1.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
#[allow(dead_code)]
mod bodies;

#[allow(unused_imports)]
use adversarial::{config, seeds, sweep};

/// Head bytes swept for `tts_text`.
const HEAD_TTS_TEXT: usize = 64;
/// Head bytes swept for `tts_lexicon`.
const HEAD_TTS_LEXICON: usize = 64;
/// Head bytes swept for `tts_synth`.
const HEAD_TTS_SYNTH: usize = 12;

#[test]
fn tts_text_seeds_swept() {
    for (name, s) in seeds("tts_text") {
        eprintln!("{name}");
        sweep(&s, 0, HEAD_TTS_TEXT, bodies::tts_text);
    }
}

#[test]
fn tts_lexicon_seeds_swept() {
    for (name, s) in seeds("tts_lexicon") {
        eprintln!("{name}");
        sweep(&s, 0, HEAD_TTS_LEXICON, bodies::tts_lexicon);
    }
}

#[test]
fn tts_synth_seeds_swept() {
    for (name, s) in seeds("tts_synth") {
        eprintln!("{name}");
        sweep(&s, 0, HEAD_TTS_SYNTH, bodies::tts_synth);
    }
}
