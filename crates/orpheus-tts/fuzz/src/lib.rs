//! Fuzz-target bodies of `orpheus-tts` (VER-4), shared by the cargo-fuzz
//! targets in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! * [`tts_text`] — the converter (normaliser, rules, lexicon, durations)
//!   over arbitrary bytes. Input: byte 0 options (bit 0 spell, bit 1
//!   lines), byte 1 speed in quarters, byte 2 the feed piece size; the rest
//!   is the text, fed in pieces while tokens are pulled. Oracles: no panic;
//!   at most 24 tokens per input byte (+16); word offsets within the text
//!   and never going back; consonants unstressed and vowels stressed; phone
//!   durations 1–2500 ms, pauses ≤ 3 s; the converter ends done.
//! * [`tts_lexicon`] — the lexicon decoder over arbitrary bytes: parse,
//!   look words up, walk all entries. Oracle: no panic; a walk visits at
//!   most the stated entry count.
//! * [`tts_synth`] — the speech renderer over fuzzed token streams (or, with
//!   bit 7 of byte 0, over fuzzed text through the converter). Input:
//!   byte 0 the voice (bits 0–1), mono/stereo (bit 2), the output rate
//!   (bits 3–5: 8, 11.025, 16, 22.05, 24, 32, 44.1, 48 kHz), a stop
//!   (bit 6); byte 1 the speed in quarters; byte 2 the block size; then
//!   3-byte tokens (kind and phoneme, stress or break kind and contour,
//!   duration in 10 ms) or text. Oracles: no panic; every sample finite and
//!   within ±1 (the `f32` path) and the `i16` path the same samples; the
//!   output ends (`Event::End`) within the duration of what was pushed
//!   (+ 1 frame); without asides or a stop, exactly that duration. Inputs
//!   are cut at 20 s of audio (fuzzing speed).

#![forbid(unsafe_code)]

use orpheus_tts::lexicon::Lexicon;
use orpheus_tts::phoneme::{ALL, Phone};
use orpheus_tts::prosody::KINDS;
use orpheus_tts::resample::frames_for;
use orpheus_tts::{
    Config, Contour, Converter, Event, Options, Pause, Seg, Speaker, Speed, Stop, Stress, Synth, Token,
    Voice, Word,
};

const RATES: [u32; 8] = [8000, 11_025, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000];

/// The renderer over fuzzed tokens or text.
pub fn tts_synth(data: &[u8]) {
    let (head, body) = data.split_at(data.len().min(3));
    let b0 = head.first().copied().unwrap_or(0);
    let voice = Voice::ALL[usize::from(b0 & 3) % 3];
    let channels = if b0 & 4 != 0 { 2 } else { 1 };
    let rate = RATES[usize::from((b0 >> 3) & 7)];
    let stop = b0 & 64 != 0;
    let speed = Speed::from_quarters(head.get(1).copied().unwrap_or(4));
    let block = usize::from(head.get(2).copied().unwrap_or(200)).max(1);
    let cfg = Config { rate, channels, voice, speed, seed: u32::from(b0) + 1 };
    if b0 & 128 != 0 {
        text(cfg, &body[..body.len().min(512)], block, stop);
        return;
    }
    // Decode the token stream.
    let mut toks = Vec::new();
    let mut total_ms = 0u64;
    let mut aside = false;
    for c in body.chunks_exact(3).take(400) {
        let ms = u16::from(c[2]) * 10;
        if total_ms + u64::from(ms) > 20_000 {
            break;
        }
        let t = match c[0] % 4 {
            0 => Token::Word(Word {
                pos: u32::from(c[1]),
                phrase_final: c[1] & 1 != 0,
                spelled: c[1] & 2 != 0,
            }),
            3 => {
                let kind = KINDS[usize::from(c[1] & 15) % KINDS.len()];
                aside |= kind == orpheus_tts::BreakKind::ParenOpen;
                let contour = Contour::from_u8((c[1] >> 4) % 5).unwrap_or(Contour::Level);
                total_ms += u64::from(ms);
                Token::Break(Pause { kind, contour, ms })
            }
            _ => {
                let ph = ALL[usize::from(c[0] >> 2) % ALL.len()];
                let stress = [Stress::Unstressed, Stress::Primary, Stress::Secondary][usize::from(c[1] % 3)];
                total_ms += u64::from(ms);
                Token::Phone(Seg { phone: Phone::new(ph, stress), ms })
            }
        };
        toks.push(t);
    }
    let want = frames_for(16 * total_ms, rate);
    let a = run_tokens(cfg, &toks, block, stop, false, want);
    assert!(a.len() as u64 <= want + 1, "{} frames for {total_ms} ms", a.len());
    if stop {
        return;
    }
    if !aside {
        assert_eq!(a.len() as u64, want, "{total_ms} ms at {rate} Hz");
    }
    // The i16 path (another block size) is the f32 path rounded.
    let b = run_tokens(cfg, &toks, block.max(64), false, true, want);
    assert_eq!(a.len(), b.len());
    for (x, y) in a.iter().zip(&b) {
        let r = (x * 32767.0).round();
        assert!((r - y).abs() <= 1.0, "{x} vs {y}");
    }
}

/// Renders tokens; returns mono samples in [−1, 1] (from the `f32` path, or
/// the `i16` path scaled).
fn run_tokens(cfg: Config, toks: &[Token], block: usize, stop: bool, int: bool, want: u64) -> Vec<f32> {
    let mut s = Synth::new(cfg);
    let ch = usize::from(cfg.channels);
    let mut out = Vec::new();
    let mut it = toks.iter().peekable();
    let mut ended = false;
    let mut stopped = false;
    let mut f = vec![0.0f32; block];
    let mut i = vec![0i16; block * ch];
    // Each call renders a full block unless it ends or needs tokens.
    let rounds = want as usize / block + 2 * toks.len() + 100;
    for round in 0..rounds {
        while let Some(t) = it.peek() {
            if !s.push(**t) {
                break;
            }
            it.next();
        }
        if it.peek().is_none() {
            s.finish();
        }
        if stop && !stopped && round == 3 {
            s.stop(if toks.len() % 2 == 0 { Stop::Now } else { Stop::AtWord });
            stopped = true;
        }
        let mut end = |_: usize, e: Event| ended |= e == Event::End;
        let n = if int {
            let n = s.render(&mut i, &mut end);
            for k in 0..n {
                if ch == 2 {
                    assert_eq!(i[2 * k], i[2 * k + 1]);
                }
                out.push(f32::from(i[k * ch]));
            }
            n
        } else {
            let n = s.render_f32(&mut f, &mut end);
            for v in &f[..n] {
                assert!(v.is_finite() && v.abs() <= 1.0, "sample {v}");
            }
            out.extend_from_slice(&f[..n]);
            n
        };
        if ended || (n == 0 && s.is_idle()) {
            return out;
        }
        assert!(n > 0 || s.space() > 0 || it.peek().is_none(), "stuck with a full queue");
    }
    panic!("renderer did not end");
}

fn text(cfg: Config, body: &[u8], block: usize, stop: bool) {
    let mut sp = Speaker::new(cfg, Options::default());
    let ch = usize::from(cfg.channels);
    let mut buf = vec![0i16; block * ch];
    let mut rest = body;
    let mut frames = 0usize;
    // ≤ 24 tokens per byte, ≤ 3 s each: a generous bound on the audio.
    let bound = (body.len() as u64 + 1) * 24 * 3 * u64::from(cfg.rate);
    for round in 0..(bound as usize / block + body.len() + 100) {
        let n = sp.feed(rest);
        rest = &rest[n..];
        if rest.is_empty() {
            sp.finish();
        }
        if stop && round == 2 {
            sp.stop(Stop::Now);
        }
        frames += sp.render(&mut buf, |_, _| {});
        if sp.is_idle() && rest.is_empty() {
            assert!(frames as u64 <= bound);
            return;
        }
    }
    panic!("speaker did not end");
}

/// The converter over arbitrary text.
pub fn tts_text(data: &[u8]) {
    let (head, text) = data.split_at(data.len().min(3));
    let opt = head.first().copied().unwrap_or(0);
    let speed = Speed::from_quarters(head.get(1).copied().unwrap_or(4));
    let piece = usize::from(head.get(2).copied().unwrap_or(255)).max(1);
    let mut conv = Converter::new(Options { spell: opt & 1 != 0, lines: opt & 2 != 0 }, speed);
    let mut rest = text;
    let mut tokens = 0usize;
    let mut last_pos = 0u32;
    loop {
        let n = conv.feed(&rest[..rest.len().min(piece)]);
        rest = &rest[n..];
        if rest.is_empty() {
            conv.finish();
        }
        while let Some(t) = conv.next_token() {
            tokens += 1;
            assert!(tokens <= 24 * text.len() + 16, "{tokens} tokens for {} bytes", text.len());
            match t {
                Token::Word(w) => {
                    assert!(w.pos as usize <= text.len());
                    assert!(w.pos >= last_pos);
                    last_pos = w.pos;
                    assert!(!conv.word_text().is_empty());
                }
                Token::Phone(s) => {
                    assert_eq!(s.phone.ph.is_vowel(), s.phone.stress != Stress::None);
                    assert!((1..=2500).contains(&s.ms));
                }
                Token::Break(p) => assert!(p.ms <= 3000),
            }
        }
        if rest.is_empty() {
            break;
        }
    }
    assert!(conv.is_done());
}

/// The lexicon decoder over arbitrary bytes.
pub fn tts_lexicon(data: &[u8]) {
    let Some(lex) = Lexicon::new(data) else { return };
    let mut out = [orpheus_tts::Phone::new(orpheus_tts::Ph::AH, Stress::None); 48];
    for w in [&b"the"[..], b"a", b"zebra", b"music", b"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa", b"m"] {
        let _ = lex.lookup(w, &mut out);
    }
    let mut n = 0;
    lex.for_each(|w, _, ph| {
        n += 1;
        assert!(w.len() <= 32 && ph.len() <= 48);
    });
    assert!(n <= lex.len());
}
