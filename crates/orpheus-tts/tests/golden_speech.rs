//! Golden renders of the synthesiser: the FNV-1a hash of the `i16` output
//! of a sentence set at 1×, 0.25×, 4× and 10× in the Neutral and Bright
//! voices (48 kHz stereo), plus one 44.1 kHz and one 16 kHz render, against
//! `tests/goldens/speech.txt`. Everything is `f32` with IEEE basic
//! operations only (orpheus-synth's rule), so the hashes are the same in
//! every build and, with `FPSCR.FZ` = 0, on the player.
//!
//! After an intended change of the sound:
//! `ORPHEUS_WRITE_GOLDENS=1 cargo test -p orpheus-tts --test golden_speech`,
//! and listen to `cargo run -p orpheus-tts --example say -- "…"`.

use orpheus_tts::{Config, Options, Speaker, Speed, Voice};

const FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens/speech.txt");

const SENTENCES: [&str; 4] = [
    "The quick brown fox jumps over the lazy dog.",
    "Would you like to resume where you stopped?",
    "Track 3 of 12 (live), 4:05; battery at 40%!",
    "She sells shells, he jumps... Where is it?",
];

fn fnv(pcm: &[i16]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for s in pcm {
        for b in s.to_le_bytes() {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
    h
}

fn render(text: &str, cfg: Config) -> Vec<i16> {
    let mut sp = Speaker::new(cfg, Options::default());
    let ch = usize::from(cfg.channels);
    let mut out = Vec::new();
    let mut buf = vec![0i16; 777 * ch];
    sp.feed(text.as_bytes());
    sp.finish();
    while !sp.is_idle() {
        let n = sp.render(&mut buf, |_, _| {});
        out.extend_from_slice(&buf[..n * ch]);
    }
    out
}

#[test]
fn golden_renders() {
    let mut table = String::new();
    let mut cases: Vec<(String, Config, &str)> = Vec::new();
    for voice in [Voice::NEUTRAL, Voice::BRIGHT] {
        for q in [4u8, 1, 16, 40] {
            for (i, s) in SENTENCES.iter().enumerate() {
                let cfg =
                    Config { rate: 48_000, channels: 2, voice, speed: Speed::from_quarters(q), seed: 0x5EED };
                cases.push((format!("{}/{:.2}x/s{i}", voice.name, f32::from(q) / 4.0), cfg, s));
            }
        }
    }
    let low = Config { rate: 44_100, channels: 2, voice: Voice::LOW, speed: Speed::NORMAL, seed: 7 };
    cases.push(("Low/1.00x/44k1".into(), low, SENTENCES[0]));
    let mono =
        Config { rate: 16_000, channels: 1, voice: Voice::NEUTRAL, speed: Speed::from_quarters(8), seed: 1 };
    cases.push(("Neutral/2.00x/16k-mono".into(), mono, SENTENCES[2]));
    for (name, cfg, text) in &cases {
        let pcm = render(text, *cfg);
        let peak = pcm.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak > 2000, "{name}: silent (peak {peak})");
        table.push_str(&format!("{name} {:016x} {}\n", fnv(&pcm), pcm.len() / usize::from(cfg.channels)));
    }
    if std::env::var_os("ORPHEUS_WRITE_GOLDENS").is_some() {
        std::fs::write(FILE, &table).unwrap();
        return;
    }
    let want = std::fs::read_to_string(FILE)
        .expect("tests/goldens/speech.txt missing: run with ORPHEUS_WRITE_GOLDENS=1");
    let bad: Vec<String> = table
        .lines()
        .zip(want.lines())
        .filter(|(g, w)| g != w)
        .map(|(g, w)| format!("  got  {g}\n  want {w}"))
        .collect();
    assert_eq!(table.lines().count(), want.lines().count(), "case list changed");
    assert!(bad.is_empty(), "golden renders differ:\n{}", bad.join("\n"));
}
