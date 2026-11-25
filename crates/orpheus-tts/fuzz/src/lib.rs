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

#![forbid(unsafe_code)]

use orpheus_tts::lexicon::Lexicon;
use orpheus_tts::{Converter, Options, Speed, Stress, Token};

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
