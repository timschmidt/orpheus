//! The fuzz targets' committed corpus and regression inputs, replayed on
//! stable through the bodies the cargo-fuzz targets run (VER-4); and the
//! seed inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-tts --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/tts_*/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

#[test]
fn tts_text_corpus() {
    let n = replay::replay("tts_text", bodies::tts_text);
    assert!(n >= 6, "{n} inputs");
}

#[test]
fn tts_synth_corpus() {
    let n = replay::replay("tts_synth", bodies::tts_synth);
    assert!(n >= 6, "{n} inputs");
}

/// A `tts_synth` token: Word (`kind` 0), Phone (1, phoneme `ph`), Break
/// (3, break kind and contour in `b`); `ms10` in tens of ms.
fn tok(kind: u8, ph: u8, b: u8, ms10: u8) -> [u8; 3] {
    [kind | (ph << 2), b, ms10]
}

#[test]
fn tts_lexicon_corpus() {
    let n = replay::replay("tts_lexicon", bodies::tts_lexicon);
    assert!(n >= 2, "{n} inputs");
}

fn text_seed(opt: u8, quarters: u8, piece: u8, text: &str) -> Vec<u8> {
    let mut v = vec![opt, quarters, piece];
    v.extend_from_slice(text.as_bytes());
    v
}

#[test]
fn seeds() {
    let text: Vec<(String, Vec<u8>)> = vec![
        (
            "prose".into(),
            text_seed(
                0,
                4,
                200,
                "It was the best of times, it was the worst of times. \"Where are you going?\" she asked; \
                 he said nothing... (Nothing at all!)\n\nChapter IV\n\nMr. Smith met Dr. Jones on St. Paul's \
                 Street at 7:30pm on 25 Nov 2026, etc.",
            ),
        ),
        (
            "numbers".into(),
            text_seed(
                0,
                16,
                7,
                "0 7 42 1,234,567 3.14 -5 +3 ~20 #1 1st 22nd 103rd 1984 2026 1990s '80s 007 01 \
                 1234567890123456789 555-1234 1-800-555-0199 10-20 3/4 24/7 1920x1080 1.2.3 192.168.0.1",
            ),
        ),
        (
            "units-money".into(),
            text_seed(
                0,
                40,
                33,
                "$3.50 $0.99 $1.5M €20 20€ £5.01 ¥100 50% 3.5 % 44.1 kHz 320kbps 5 MB 10ms -3 dB 20°C \
                 4000mAh 2x 4K 12:30 7:05 7pm 10 a.m. 2026-11-25 11/25/2026 November 25th, 2026",
            ),
        ),
        (
            "files-urls".into(),
            text_seed(
                2,
                4,
                1,
                "01 - Artist - Title.mp3\n02_Some_Song.flac\ntrack01.ogg\nmyFileName_v2.txt\n\
                 https://example.com/a?b=1\nwww.orpheus.dev\ntim@example.com\nXMLHttpRequest C# x86 MP3 USB",
            ),
        ),
        ("spell".into(), text_seed(1, 1, 3, "Ab1-c?!@#$%^&*()_+=~`'\"<>,./\\|{}[]; Ω½©")),
        (
            "unicode".into(),
            text_seed(
                0,
                4,
                5,
                "Beyoncé Motörhead Œuvre straße αβγ 日本語 Привет 😀 “quoted” — dash … end ½ ¼ ¾ −3 ×",
            ),
        ),
        (
            "invalid".into(),
            text_seed(0, 4, 2, "")
                .into_iter()
                .chain([0xFF, 0xC3, b'a', 0xE2, 0x82, b' ', 0xF0, 0x9F])
                .collect(),
        ),
    ];
    let lex = orpheus_tts::lexicon::DATA;
    let lexicon: Vec<(String, Vec<u8>)> = vec![
        ("head".into(), lex[..lex.len().min(4096)].to_vec()),
        (
            "tiny".into(),
            b"OTL1\x02\x00\x01\x00\x02\x00\x02a\x01\xff\x01\x02\x05\x01\xff\x01\x00\x00\x40".to_vec(),
        ),
    ];
    // tts_synth: a sentence of tokens; every phoneme; extreme durations and
    // asides; a stop; text through the speaker.
    let mut sentence = vec![0b0011_1000, 4, 64];
    for t in [
        tok(0, 0, 0, 0),
        tok(1, 17, 0, 6),  // D
        tok(1, 10, 1, 12), // IY1
        tok(0, 0, 1, 0),
        tok(1, 28, 0, 8),    // P
        tok(1, 29, 0, 6),    // R
        tok(1, 1, 1, 15),    // AE1
        tok(1, 32, 0, 7),    // T
        tok(3, 0, 0x2A, 45), // Period, Rise
    ] {
        sentence.extend_from_slice(&t);
    }
    let mut every = vec![0b0001_0101, 16, 7];
    for p in 0..39u8 {
        every.extend_from_slice(&tok(if p % 7 == 0 { 0 } else { 1 }, p, p % 3, 3 + p % 9));
    }
    every.extend_from_slice(&tok(3, 0, 0x4B, 20));
    let mut extreme = vec![0b0010_0110, 1, 255];
    for t in [
        tok(3, 0, 2, 255), // ParenOpen, 2.55 s
        tok(0, 0, 3, 0),
        tok(1, 5, 1, 255), // AY1 2.55 s
        tok(1, 21, 0, 1),  // HH 10 ms
        tok(1, 37, 0, 0),  // Z 0 ms
        tok(3, 0, 3, 0),   // ParenClose
        tok(1, 15, 0, 1),  // B
        tok(1, 15, 0, 1),
        tok(1, 23, 0, 1), // K
        tok(3, 0, 0x4B, 1),
    ] {
        extreme.extend_from_slice(&t);
    }
    let mut stopped = sentence.clone();
    stopped[0] |= 64;
    stopped[2] = 3;
    let mut fast = sentence.clone();
    fast[1] = 40;
    fast[0] = 0b0000_0001;
    let mut synth: Vec<(String, Vec<u8>)> =
        vec![("sentence".into(), sentence), ("every".into(), every), ("extreme".into(), extreme)];
    synth.push(("stopped".into(), stopped));
    synth.push(("fast".into(), fast));
    let mut speak = vec![0b1011_1110, 6, 100];
    speak.extend_from_slice(b"Hello (world)! Is it 4:05? Track 3, \"quoted\"... done.");
    synth.push(("text".into(), speak));
    let a = replay::write_seeds("tts_text", &text);
    let b = replay::write_seeds("tts_lexicon", &lexicon);
    let c = replay::write_seeds("tts_synth", &synth);
    for (_, d) in &synth {
        bodies::tts_synth(d);
    }
    if !a && !b && !c {
        for (_, d) in &text {
            bodies::tts_text(d);
        }
        for (_, d) in &lexicon {
            bodies::tts_lexicon(d);
        }
    }
}
