//! The incremental converter: UTF-8 text in pieces in, [`Token`]s out.
//!
//! ```text
//! feed(bytes) ──► input buffer (INPUT B) ──► chunker (white space, ≤ 48 B,
//! 2 chunks look-ahead) ──► Normaliser ──► item queue (ITEMS B: words,
//! letters, breaks, positions) ──► lexicon / rules / stress ──► phones with
//! durations (one word at a time) ──► next()
//! ```
//!
//! Nothing grows with the text: a book is read by feeding it in pieces
//! while pulling tokens ([`Converter::space`] says how much fits). The
//! RAM of one converter is [`Converter`]'s size (`ram` test: 1,408 B on
//! the host).

use crate::duration::{self, Context, Speed};
use crate::fold;
use crate::lexicon::Lexicon;
use crate::normalise::{MAX_CHUNK, MAX_WORD, Normaliser, Options, Sink};
use crate::phoneme::{Ph, Phone, Stress};
use crate::prosody::{BreakKind, Contour};
use crate::words::{self, MAX_WORD_PHONES};

/// Input buffer size (bytes).
pub const INPUT: usize = 256;
/// Normaliser output queue size (bytes).
pub const ITEMS: usize = 768;

/// A spoken word starts; its phones follow.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Word {
    /// Byte offset in the fed text of the chunk the word came from (for
    /// highlighting and resuming).
    pub pos: u32,
    /// A phrase break follows the word (lengthening and the contour fall on
    /// its last syllable).
    pub phrase_final: bool,
    /// A letter said by name (spelling).
    pub spelled: bool,
}

/// One phone with its duration.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Seg {
    /// The phoneme and its stress.
    pub phone: Phone,
    /// Duration at the converter's speed (ms).
    pub ms: u16,
}

/// A pause.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Pause {
    /// What caused it.
    pub kind: BreakKind,
    /// The pitch movement before it.
    pub contour: Contour,
    /// Its length at the converter's speed (ms).
    pub ms: u16,
}

/// What the converter produces.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Token {
    /// A word begins ([`Converter::word_text`] has its normalised text).
    Word(Word),
    /// A phone of the current word.
    Phone(Seg),
    /// A pause and intonation mark.
    Break(Pause),
}

const TAG_POS: u8 = 0x00;
const TAG_WORD: u8 = 0x20;
const TAG_LETTER: u8 = 0x40;
const TAG_BREAK: u8 = 0x60;

struct Items<'a> {
    buf: &'a mut [u8; ITEMS],
    len: &'a mut usize,
    dropped: &'a mut u32,
}

impl Items<'_> {
    fn put(&mut self, bytes: &[u8]) {
        if *self.len + bytes.len() > ITEMS {
            *self.dropped = self.dropped.saturating_add(1);
            return;
        }
        self.buf[*self.len..*self.len + bytes.len()].copy_from_slice(bytes);
        *self.len += bytes.len();
    }
}

impl Sink for Items<'_> {
    fn word(&mut self, w: &[u8]) {
        let n = w.len().min(MAX_WORD);
        if n == 0 {
            return;
        }
        let mut b = [0u8; MAX_WORD + 1];
        b[0] = TAG_WORD | (n as u8 - 1);
        b[1..=n].copy_from_slice(&w[..n]);
        self.put(&b[..=n]);
    }

    fn letter(&mut self, c: u8) {
        self.put(&[TAG_LETTER, c]);
    }

    fn brk(&mut self, kind: BreakKind, contour: Contour) {
        self.put(&[TAG_BREAK, (kind as u8) | ((contour as u8) << 4)]);
    }
}

/// The text-to-phones converter (see the module documentation).
pub struct Converter {
    norm: Normaliser,
    lex: Lexicon<'static>,
    speed: Speed,
    input: [u8; INPUT],
    in_len: usize,
    /// Stream offset of `input[0]`.
    base: u32,
    eof: bool,
    /// Line ends seen since the last chunk.
    newlines: u8,
    items: [u8; ITEMS],
    items_len: usize,
    items_rd: usize,
    dropped: u32,
    segs: [Seg; MAX_WORD_PHONES],
    seg_len: u8,
    seg_rd: u8,
    word: [u8; MAX_WORD],
    word_len: u8,
    pos: u32,
}

impl core::fmt::Debug for Converter {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Converter").field("speed", &self.speed).field("buffered", &self.in_len).finish()
    }
}

impl Default for Converter {
    fn default() -> Converter {
        Converter::new(Options::default(), Speed::NORMAL)
    }
}

const BLANK: Seg = Seg { phone: Phone::new(Ph::AH, Stress::None), ms: 0 };

impl Converter {
    /// A converter with the built-in lexicon.
    pub fn new(opts: Options, speed: Speed) -> Converter {
        Converter::with_lexicon(opts, speed, Lexicon::builtin())
    }

    /// A converter over another lexicon (tests, a lexicon paged from NOR).
    pub fn with_lexicon(opts: Options, speed: Speed, lex: Lexicon<'static>) -> Converter {
        Converter {
            norm: Normaliser::new(opts),
            lex,
            speed,
            input: [0; INPUT],
            in_len: 0,
            base: 0,
            eof: false,
            newlines: 0,
            items: [0; ITEMS],
            items_len: 0,
            items_rd: 0,
            dropped: 0,
            segs: [BLANK; MAX_WORD_PHONES],
            seg_len: 0,
            seg_rd: 0,
            word: [0; MAX_WORD],
            word_len: 0,
            pos: 0,
        }
    }

    /// Drops all buffered text and output; the next text starts afresh
    /// (offsets from 0).
    pub fn reset(&mut self) {
        let (opts, speed, lex) = (self.norm.options(), self.speed, self.lex);
        *self = Converter::with_lexicon(opts, speed, lex);
    }

    /// The speed for the durations from now on.
    pub fn set_speed(&mut self, speed: Speed) {
        self.speed = speed;
    }

    /// The current speed.
    pub fn speed(&self) -> Speed {
        self.speed
    }

    /// Changes the normaliser options (from the next chunk on).
    pub fn set_options(&mut self, opts: Options) {
        self.norm.set_options(opts);
    }

    /// Free input space in bytes.
    pub fn space(&self) -> usize {
        INPUT - self.in_len
    }

    /// Adds text; returns how many bytes were taken (the rest does not fit
    /// yet: pull tokens and feed again). Pieces may split UTF-8 characters.
    pub fn feed(&mut self, text: &[u8]) -> usize {
        let n = text.len().min(self.space());
        self.input[self.in_len..self.in_len + n].copy_from_slice(&text[..n]);
        self.in_len += n;
        n
    }

    /// No more text follows: what is buffered is read to the end.
    pub fn finish(&mut self) {
        self.eof = true;
    }

    /// Whether everything fed was read (after [`Converter::finish`]).
    pub fn is_done(&self) -> bool {
        self.eof && self.in_len == 0 && self.items_rd >= self.items_len && self.seg_rd >= self.seg_len
    }

    /// Items the normaliser could not queue (0 for any realistic text; the
    /// queue holds the worst case of three chunks of ordinary text).
    pub fn dropped(&self) -> u32 {
        self.dropped
    }

    /// The normalised text of the word the last [`Token::Word`] started
    /// (a letter's own character for a spelled letter).
    pub fn word_text(&self) -> &[u8] {
        &self.word[..usize::from(self.word_len)]
    }

    /// Feeds all of `text`, finishes, and calls `f` for every token.
    pub fn convert(&mut self, text: &[u8], mut f: impl FnMut(&Converter, Token)) {
        let mut rest = text;
        loop {
            let n = self.feed(rest);
            rest = &rest[n..];
            if rest.is_empty() {
                self.finish();
            }
            while let Some(t) = self.next_token() {
                f(self, t);
            }
            if rest.is_empty() {
                break;
            }
        }
    }

    /// The next token, or `None` when more text is needed (or, after
    /// [`Converter::finish`], at the end).
    pub fn next_token(&mut self) -> Option<Token> {
        loop {
            if self.seg_rd < self.seg_len {
                let s = self.segs[usize::from(self.seg_rd)];
                self.seg_rd += 1;
                return Some(Token::Phone(s));
            }
            if self.items_rd < self.items_len {
                if let Some(t) = self.item() {
                    return Some(t);
                }
                continue;
            }
            self.items_rd = 0;
            self.items_len = 0;
            if !self.fill() {
                return None;
            }
        }
    }

    // ------------------------------------------------------------------

    /// Whether the queued item at `at` is a phrase break.
    fn break_at(&self, at: usize) -> bool {
        at < self.items_len
            && self.items[at] & 0xE0 == TAG_BREAK
            && BreakKind::from_u8(self.items.get(at + 1).copied().unwrap_or(0) & 15)
                .is_some_and(BreakKind::ends_phrase)
    }

    fn item(&mut self) -> Option<Token> {
        let at = self.items_rd;
        let tag = self.items[at];
        match tag & 0xE0 {
            TAG_POS => {
                let b = &self.items[at + 1..at + 5];
                self.pos = u32::from_le_bytes([b[0], b[1], b[2], b[3]]);
                self.items_rd += 5;
                None
            }
            TAG_WORD => {
                let n = usize::from(tag & 0x1F) + 1;
                let mut w = [0u8; MAX_WORD];
                w[..n].copy_from_slice(&self.items[at + 1..at + 1 + n]);
                self.items_rd += 1 + n;
                let phrase_final = self.break_at(self.items_rd);
                let mut ph = [Phone::new(Ph::AH, Stress::None); MAX_WORD_PHONES];
                let (k, _) = words::pronounce(&self.lex, &w[..n], &mut ph);
                self.word[..n].copy_from_slice(&w[..n]);
                self.word_len = n as u8;
                self.set_segs(&ph[..k], phrase_final);
                Some(Token::Word(Word { pos: self.pos, phrase_final, spelled: false }))
            }
            TAG_LETTER => {
                let c = self.items[at + 1];
                self.items_rd += 2;
                let phrase_final = self.break_at(self.items_rd);
                self.word[0] = c;
                self.word_len = 1;
                self.set_segs(words::letter_name(c), phrase_final);
                Some(Token::Word(Word { pos: self.pos, phrase_final, spelled: true }))
            }
            _ => {
                let v = self.items[at + 1];
                self.items_rd += 2;
                let kind = BreakKind::from_u8(v & 15).unwrap_or(BreakKind::Group);
                let contour = Contour::from_u8(v >> 4).unwrap_or(Contour::Level);
                Some(Token::Break(Pause { kind, contour, ms: duration::pause_ms(kind, self.speed) }))
            }
        }
    }

    /// The word's phones with their durations.
    fn set_segs(&mut self, ph: &[Phone], phrase_final: bool) {
        let n = ph.len().min(MAX_WORD_PHONES);
        let vowels = ph[..n].iter().filter(|p| p.ph.is_vowel()).count();
        let last_vowel = ph[..n].iter().rposition(|p| p.ph.is_vowel()).unwrap_or(0);
        for k in 0..n {
            let p = ph[k];
            let consonant = !p.ph.is_vowel();
            let near = |i: Option<usize>| i.and_then(|i| ph.get(i)).is_some_and(|q| !q.ph.is_vowel());
            let ctx = Context {
                word_initial: k == 0,
                word_final_syllable: k >= last_vowel,
                phrase_final_syllable: phrase_final && k >= last_vowel,
                polysyllabic: vowels >= 2,
                next: ph.get(k + 1).map(|q| q.ph),
                cluster: consonant && (near(k.checked_sub(1)) || near(Some(k + 1))),
            };
            let reduced = duration::reduce(p.ph, p.stress, self.speed);
            let phone = Phone::new(reduced, p.stress);
            let ms = duration::duration_ms(reduced, p.stress, &ctx, self.speed);
            self.segs[k] = Seg { phone, ms };
        }
        self.seg_len = n as u8;
        self.seg_rd = 0;
    }

    /// Drops `n` bytes from the front of the input.
    fn drop_input(&mut self, n: usize) {
        let n = n.min(self.in_len);
        self.input.copy_within(n..self.in_len, 0);
        self.in_len -= n;
        self.base = self.base.wrapping_add(n as u32);
    }

    /// Skips white space from `at`; returns the position after it and the
    /// line ends seen. Stops before an incomplete UTF-8 character.
    fn skip_space(&self, mut at: usize) -> (usize, u8) {
        let mut nl = 0u8;
        while at < self.in_len {
            let (cp, len) = fold::decode(&self.input[at..self.in_len]);
            if cp.is_none() && len == 1 && !self.eof && self.in_len - at < 4 && self.input[at] >= 0xC0 {
                break; // maybe the start of a character still to come
            }
            match cp {
                Some(c) if fold::is_space(c) => {
                    if c == u32::from(b'\n') {
                        nl = nl.saturating_add(1);
                    }
                    at += len;
                }
                _ => break,
            }
        }
        (at, nl)
    }

    /// The end of the chunk starting at `at` and whether it is complete
    /// (white space after it, the end of the text, or the length limit).
    fn chunk_end(&self, at: usize) -> (usize, bool) {
        let mut i = at;
        while i < self.in_len {
            if i - at >= MAX_CHUNK {
                // Cut on a character boundary.
                let mut cut = i;
                while cut > at + 1 && self.input[cut] & 0xC0 == 0x80 {
                    cut -= 1;
                }
                return (cut, true);
            }
            let (cp, len) = fold::decode(&self.input[i..self.in_len]);
            if cp.is_some_and(fold::is_space) {
                return (i, true);
            }
            if cp.is_none() && !self.eof && self.in_len - i < 4 && self.input[i] >= 0xC0 {
                return (i, false);
            }
            i += len;
        }
        (self.in_len, self.eof)
    }

    /// Runs the normaliser over the next chunk; `false` if it needs more
    /// input (or the text is done).
    fn fill(&mut self) -> bool {
        loop {
            let (start, nl) = self.skip_space(0);
            self.newlines = self.newlines.saturating_add(nl);
            self.drop_input(start);
            if self.in_len == 0 {
                return false;
            }
            let full = self.in_len == INPUT;
            let (e0, done0) = self.chunk_end(0);
            if !done0 && !full {
                return false;
            }
            // Look-ahead: up to two more complete chunks within the
            // paragraph.
            let mut ahead: [Option<(usize, usize)>; 2] = [None, None];
            let mut at = e0;
            let mut wait = false;
            for slot in ahead.iter_mut() {
                let (s, nl) = self.skip_space(at);
                if nl >= 2 || (nl >= 1 && self.norm.options().lines) {
                    break;
                }
                if s >= self.in_len {
                    wait = !self.eof && !full;
                    break;
                }
                let (e, done) = self.chunk_end(s);
                if !done {
                    wait = !full;
                    break;
                }
                *slot = Some((s, e));
                at = e;
            }
            if wait {
                return false;
            }
            // Line ends before this chunk.
            let mut items =
                Items { buf: &mut self.items, len: &mut self.items_len, dropped: &mut self.dropped };
            if self.newlines >= 2 {
                self.norm.paragraph(&mut items);
            } else if self.newlines == 1 {
                self.norm.line(&mut items);
            }
            self.newlines = 0;
            let pos = self.base;
            let mut b = [TAG_POS, 0, 0, 0, 0];
            b[1..].copy_from_slice(&pos.to_le_bytes());
            items.put(&b);
            let input = &self.input;
            let la = [ahead[0].map(|(s, e)| &input[s..e]), ahead[1].map(|(s, e)| &input[s..e])];
            let used = self.norm.chunk(&input[..e0], la, &mut items);
            let end = match used {
                1 => e0,
                2 => ahead[0].map_or(e0, |x| x.1),
                _ => ahead[1].or(ahead[0]).map_or(e0, |x| x.1),
            };
            self.drop_input(end);
            if self.items_len > 5 {
                return true;
            }
            // Only a position: nothing said for this chunk; go on.
            self.items_len = 0;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram() {
        // The converter's whole state (stated in the module docs and 04 §8).
        let n = core::mem::size_of::<Converter>();
        assert!(n <= 1536, "Converter is {n} B");
    }
}
