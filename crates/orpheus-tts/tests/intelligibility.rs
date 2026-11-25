//! Intelligibility (04 §8): the word error rate of an offline speech
//! recogniser on a fixed sentence set, synthesised at 1×, 2× and 4×
//! (gated by `budgets.toml [tts]`) and at 6× and 10× (measured, not gated).
//!
//! The recogniser is not part of the repository. The test runs when one is
//! configured and reports "not run" otherwise:
//!
//! * `ORPHEUS_ASR_URL=http://127.0.0.1:8080` — a whisper.cpp
//!   `whisper-server` (its `/inference` endpoint; the WAVs are posted with
//!   `curl`), e.g. `whisper-server -m ggml-base.en.bin --port 8080`;
//! * or `ORPHEUS_ASR_CMD='whisper-cli -m ggml-base.en.bin -nt -np -f {wav}'`
//!   — any command that prints the transcript of `{wav}` on stdout.
//!
//! `ORPHEUS_ASR_VOICES=all` adds the Low and Bright voices at 1× and 2×
//! (measured). WAVs: 16 kHz mono, in Cargo's per-test temporary directory.
//!
//! Measured (TTS-2, 2026-11-25, whisper.cpp 1.9.4, `ggml-base.en`, greedy):
//! see 04 §8 "As built (TTS-2)".

use std::path::{Path, PathBuf};
use std::process::Command;

use orpheus_tts::{Config, Options, Speaker, Speed, Voice};

/// The sentence set: written for this test (no numbers, so the reference
/// needs no normalisation); every English phoneme, clusters, questions,
/// commas, an exclamation and an aside.
pub const SENTENCES: [&str; 24] = [
    "The quick brown fox jumps over the lazy dog.",
    "Please turn the volume down a little.",
    "She showed us the shells she found.",
    "Would you like to resume where you stopped?",
    "The battery is almost empty, so plug in the charger.",
    "Open the folder and choose another song.",
    "We will meet at the station after lunch.",
    "Where did you put the green umbrella?",
    "This old radio still plays music every morning.",
    "Thank you for listening to this chapter.",
    "The weather was cold, but the children played outside.",
    "Press the middle button to pause the track.",
    "A large ship sailed slowly into the harbour.",
    "Do you want to hear the next album?",
    "He jumped over the fence and ran away!",
    "The library closes early on public holidays.",
    "Bring your headphones when you go running.",
    "My brother bought a new bicycle yesterday.",
    "The garden smells of fresh bread and roses.",
    "Nobody knew why the lights went out.",
    "Choose a voice that sounds pleasant to you.",
    "The bridge (built long ago) is very narrow.",
    "Keep the player away from water and heat.",
    "Is this the right way to the museum?",
];

/// Words of `s`, lower case, letters and apostrophes only.
pub fn words(s: &str) -> Vec<String> {
    s.to_lowercase()
        .split(|c: char| !(c.is_alphanumeric() || c == '\''))
        .filter(|w| !w.is_empty())
        .map(|w| w.replace("harbor", "harbour").trim_matches('\'').to_string())
        .filter(|w| !w.is_empty())
        .collect()
}

/// Word-level edit distance.
pub fn edits(a: &[String], b: &[String]) -> usize {
    let mut row: Vec<usize> = (0..=b.len()).collect();
    for (i, x) in a.iter().enumerate() {
        let mut prev = row[0];
        row[0] = i + 1;
        for (j, y) in b.iter().enumerate() {
            let cur = row[j + 1];
            row[j + 1] = if x == y { prev } else { 1 + prev.min(row[j]).min(row[j + 1]) };
            prev = cur;
        }
    }
    row[b.len()]
}

fn render(text: &str, voice: Voice, speed: Speed) -> Vec<i16> {
    let cfg = Config { rate: 16_000, channels: 1, voice, speed, seed: 0x5EED };
    let mut sp = Speaker::new(cfg, Options::default());
    let mut pcm = Vec::new();
    let mut block = [0i16; 1024];
    let mut rest = text.as_bytes();
    // Half a second of silence around the sentence (recognisers like it).
    pcm.extend(std::iter::repeat_n(0i16, 8000));
    loop {
        let n = sp.feed(rest);
        rest = &rest[n..];
        if rest.is_empty() {
            sp.finish();
        }
        let got = sp.render(&mut block, |_, _| {});
        pcm.extend_from_slice(&block[..got]);
        if sp.is_idle() {
            break;
        }
    }
    pcm.extend(std::iter::repeat_n(0i16, 8000));
    pcm
}

pub fn write_wav(path: &Path, rate: u32, pcm: &[i16]) {
    let data = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2).to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b).unwrap();
}

enum Asr {
    Url(String),
    Cmd(String),
}

impl Asr {
    fn from_env() -> Option<Asr> {
        if let Ok(u) = std::env::var("ORPHEUS_ASR_URL") {
            return Some(Asr::Url(u.trim_end_matches('/').to_string()));
        }
        std::env::var("ORPHEUS_ASR_CMD").ok().map(Asr::Cmd)
    }

    fn transcribe(&self, wav: &Path) -> String {
        let out = match self {
            Asr::Url(u) => Command::new("curl")
                .args(["-sS", "--max-time", "120"])
                .arg(format!("{u}/inference"))
                .arg("-F")
                .arg(format!("file=@{}", wav.display()))
                .args(["--form-string", "response_format=json", "--form-string", "temperature=0"])
                .output()
                .expect("curl"),
            Asr::Cmd(c) => Command::new("sh")
                .arg("-c")
                .arg(c.replace("{wav}", &wav.display().to_string()))
                .output()
                .expect("ASR command"),
        };
        assert!(out.status.success(), "recogniser failed: {}", String::from_utf8_lossy(&out.stderr));
        let s = String::from_utf8_lossy(&out.stdout).to_string();
        match self {
            // {"text":"..."}: take the string value.
            Asr::Url(_) => s
                .split_once("\"text\":\"")
                .and_then(|(_, r)| {
                    r.rsplit_once('"').map(|(t, _)| t.replace("\\n", " ").replace("\\\"", "\""))
                })
                .unwrap_or(s),
            Asr::Cmd(_) => s,
        }
    }
}

/// Budgets from `budgets.toml [tts]` (`wer_max_pct_1x` …).
fn budget(key: &str) -> Option<f64> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../budgets.toml");
    let text = std::fs::read_to_string(root).ok()?;
    let mut in_tts = false;
    for line in text.lines() {
        let l = line.trim();
        if l.starts_with('[') {
            in_tts = l == "[tts]";
            continue;
        }
        if in_tts
            && let Some((k, v)) = l.split_once('=')
            && k.trim() == key
        {
            return v.split('#').next()?.trim().parse().ok();
        }
    }
    None
}

fn wer(asr: &Asr, dir: &Path, voice: Voice, speed: Speed, show: bool) -> f64 {
    let (mut errs, mut total) = (0, 0);
    for (i, s) in SENTENCES.iter().enumerate() {
        let pcm = render(s, voice, speed);
        let wav: PathBuf = dir.join(format!("{}-{}-{i:02}.wav", voice.name, speed.quarters()));
        write_wav(&wav, 16_000, &pcm);
        let hyp = asr.transcribe(&wav);
        let (r, h) = (words(s), words(&hyp));
        let e = edits(&r, &h);
        if show && e > 0 {
            eprintln!("    {:>5.2}x {e}: {s:?} -> {:?}", speed.as_f32(), hyp.trim());
        }
        errs += e;
        total += r.len();
    }
    100.0 * errs as f64 / total as f64
}

#[test]
fn word_error_rate() {
    let Some(asr) = Asr::from_env() else {
        eprintln!(
            "intelligibility: NOT RUN (no recogniser; set ORPHEUS_ASR_URL to a whisper.cpp server or \
             ORPHEUS_ASR_CMD)"
        );
        return;
    };
    let dir = PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("tts-wer");
    std::fs::create_dir_all(&dir).unwrap();
    let show = std::env::var_os("ORPHEUS_ASR_SHOW").is_some();
    let mut fails = Vec::new();
    eprintln!("intelligibility (word error rate, {} sentences):", SENTENCES.len());
    for (q, key) in [
        (4u8, Some("wer_max_pct_1x")),
        (8, Some("wer_max_pct_2x")),
        (16, Some("wer_max_pct_4x")),
        (24, None),
        (40, None),
    ] {
        let speed = Speed::from_quarters(q);
        let w = wer(&asr, &dir, Voice::NEUTRAL, speed, show);
        let b = key.and_then(budget);
        eprintln!(
            "  Neutral {:>5.2}x: WER {w:5.1} %{}",
            speed.as_f32(),
            b.map_or(" (measured, not gated)".into(), |b| format!(" (budget {b} %)"))
        );
        if let Some(b) = b
            && w > b
        {
            fails.push(format!("{:.2}x: {w:.1} % > {b} %", speed.as_f32()));
        }
    }
    if std::env::var("ORPHEUS_ASR_VOICES").is_ok_and(|v| v == "all") {
        for v in [Voice::LOW, Voice::BRIGHT] {
            for q in [4u8, 8] {
                let w = wer(&asr, &dir, v, Speed::from_quarters(q), show);
                eprintln!("  {:<7} {:>5.2}x: WER {w:5.1} % (measured)", v.name, f32::from(q) / 4.0);
            }
        }
    }
    assert!(fails.is_empty(), "word error rate over budget: {fails:?}");
}

#[test]
fn wer_arithmetic() {
    let a = words("The bridge (built long ago) is very narrow.");
    assert_eq!(a.len(), 8);
    assert_eq!(edits(&a, &a), 0);
    let b = words("the bridge built long ago is narrow");
    assert_eq!(edits(&a, &b), 1);
    assert_eq!(edits(&a, &[]), 8);
}
