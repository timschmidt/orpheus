//! One word to phones: the exception lexicon, then the lexicon with a
//! regular ending taken off (`-s`, `-'s`, `-es`, `-ies`, `-ed`, `-ing`,
//! `-ly`, `-er`, `-est`), then the letter-to-sound rules with the stress
//! heuristic. Letter names for spelling.

use crate::lexicon::{Entry, Lexicon, MAX_WORD};
use crate::phoneme::{Ph, Phone, Stress};
use crate::rules::{self, RulePhone};
use crate::stress;

/// Longest phone string of one word (longer output is cut).
pub const MAX_WORD_PHONES: usize = 48;

/// Where pronunciations come from: the committed lexicon, or the lexicon
/// tool's set under construction.
pub trait Lookup {
    /// Looks `word` (lower case) up, writing a full entry's phones to `out`.
    fn lookup(&self, word: &[u8], out: &mut [Phone]) -> Option<Entry>;
}

impl Lookup for Lexicon<'_> {
    fn lookup(&self, word: &[u8], out: &mut [Phone]) -> Option<Entry> {
        Lexicon::lookup(self, word, out)
    }
}

/// How a word was pronounced (statistics, tests).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// A full lexicon entry.
    Lexicon,
    /// The rules' phonemes with the lexicon's stress.
    LexiconStress,
    /// A lexicon stem with a regular ending.
    Stem,
    /// The rules and the stress heuristic.
    Rules,
}

/// The rules alone (with the heuristic stress): phones into `out`, count
/// returned. `word` is lower or upper case ASCII; other bytes are dropped.
pub fn by_rules(word: &[u8], out: &mut [Phone]) -> usize {
    let mut up = [0u8; 64];
    let mut n = 0;
    for &c in word {
        if c.is_ascii_alphabetic() && n < up.len() {
            up[n] = c.to_ascii_uppercase();
            n += 1;
        }
    }
    let mut rp = [RulePhone { ph: Ph::AH, reduced: false }; MAX_WORD_PHONES];
    let mut k = 0;
    rules::translate(&up[..n], |p| {
        if k < rp.len() {
            rp[k] = p;
            k += 1;
        }
    });
    stress::assign(&up[..n], &rp[..k], out)
}

/// Moves the primary stress of `ph` to vowel `v` (others unstressed).
fn restress(ph: &mut [Phone], v: u8) {
    let mut k = 0u8;
    for p in ph.iter_mut() {
        if p.ph.is_vowel() {
            p.stress = if k == v { Stress::Primary } else { Stress::Unstressed };
            k += 1;
        }
    }
}

fn full(lex: &impl Lookup, word: &[u8], out: &mut [Phone]) -> Option<(usize, Source)> {
    match lex.lookup(word, out)? {
        Entry::Full(n) => Some((n, Source::Lexicon)),
        Entry::Stress(v) => {
            let n = by_rules(word, out);
            restress(&mut out[..n], v);
            Some((n, Source::LexiconStress))
        }
    }
}

/// Appends phones to `out[n..]`; returns the new length.
fn push(out: &mut [Phone], mut n: usize, ph: &[Phone]) -> usize {
    for &p in ph {
        if n < out.len() {
            out[n] = p;
            n += 1;
        }
    }
    n
}

const fn c(ph: Ph) -> Phone {
    Phone::new(ph, Stress::None)
}

const fn v0(ph: Ph) -> Phone {
    Phone::new(ph, Stress::Unstressed)
}

/// The `-s` ending after the stem's last phone: /ɪz/ after sibilants, /s/
/// after other voiceless sounds, /z/ otherwise.
fn plural(out: &mut [Phone], n: usize) -> usize {
    match out[..n].last().map(|p| p.ph) {
        Some(Ph::S | Ph::Z | Ph::SH | Ph::ZH | Ph::CH | Ph::JH) => push(out, n, &[v0(Ph::IH), c(Ph::Z)]),
        Some(p) if !p.is_voiced() => push(out, n, &[c(Ph::S)]),
        _ => push(out, n, &[c(Ph::Z)]),
    }
}

/// The `-ed` ending: /ɪd/ after T and D, /t/ after voiceless sounds, /d/.
fn past(out: &mut [Phone], n: usize) -> usize {
    match out[..n].last().map(|p| p.ph) {
        Some(Ph::T | Ph::D) => push(out, n, &[v0(Ph::IH), c(Ph::D)]),
        Some(p) if !p.is_voiced() => push(out, n, &[c(Ph::T)]),
        _ => push(out, n, &[c(Ph::D)]),
    }
}

#[derive(Clone, Copy)]
enum Ending {
    S,
    Ed,
    Ing,
    Ly,
    Er,
    Est,
}

/// (written ending, ending, stem repairs: "" as is, "e" add e, "y" i→y, "2" undouble)
type EndingRule = (&'static [u8], Ending, &'static [&'static [u8]]);

fn stemmed(lex: &impl Lookup, w: &[u8], out: &mut [Phone]) -> Option<usize> {
    const ENDINGS: [EndingRule; 11] = [
        (b"'s", Ending::S, &[b""]),
        (b"ies", Ending::S, &[b"y"]),
        (b"es", Ending::S, &[b"", b"e"]),
        (b"s", Ending::S, &[b""]),
        (b"ied", Ending::Ed, &[b"y"]),
        (b"ed", Ending::Ed, &[b"", b"e", b"2"]),
        (b"ing", Ending::Ing, &[b"", b"e", b"2"]),
        (b"ily", Ending::Ly, &[b"y"]),
        (b"ly", Ending::Ly, &[b""]),
        (b"er", Ending::Er, &[b"", b"e", b"2"]),
        (b"est", Ending::Est, &[b"", b"e", b"2"]),
    ];
    for (suf, ending, repairs) in ENDINGS {
        if w.len() < suf.len() + 3 || !w.ends_with(suf) {
            continue;
        }
        let base = &w[..w.len() - suf.len()];
        if matches!(ending, Ending::S) && suf == b"s" && (base.ends_with(b"s") || base.ends_with(b"u")) {
            continue;
        }
        for &rep in repairs {
            let mut stem = [0u8; MAX_WORD];
            let mut m = base.len().min(MAX_WORD);
            stem[..m].copy_from_slice(&base[..m]);
            match rep {
                b"e" if m < MAX_WORD => {
                    stem[m] = b'e';
                    m += 1;
                }
                b"y" if m < MAX_WORD => {
                    stem[m] = b'y';
                    m += 1;
                }
                b"2" => {
                    if m < 2 || stem[m - 1] != stem[m - 2] {
                        continue;
                    }
                    m -= 1;
                }
                b"" => {}
                _ => continue,
            }
            let Some((n, Source::Lexicon)) = full(lex, &stem[..m], out) else { continue };
            // A stem found: the ending in its regular form.
            let n = match ending {
                Ending::S => plural(out, n),
                Ending::Ed => past(out, n),
                Ending::Ing => push(out, n, &[v0(Ph::IH), c(Ph::NG)]),
                Ending::Ly => push(out, n, &[c(Ph::L), v0(Ph::IY)]),
                Ending::Er => push(out, n, &[v0(Ph::ER)]),
                Ending::Est => push(out, n, &[v0(Ph::AH), c(Ph::S), c(Ph::T)]),
            };
            return Some(n);
        }
    }
    None
}

/// Pronounces one word (`a`–`z`, `'`; upper case is folded) into `out`
/// through `lex`; returns the phone count and the source.
pub fn pronounce(lex: &impl Lookup, word: &[u8], out: &mut [Phone]) -> (usize, Source) {
    let mut w = [0u8; MAX_WORD];
    let mut n = 0;
    for &ch in word {
        let ch = ch.to_ascii_lowercase();
        if (ch.is_ascii_lowercase() || ch == b'\'') && n < MAX_WORD {
            w[n] = ch;
            n += 1;
        }
    }
    let w = &w[..n];
    if word.len() <= MAX_WORD {
        if let Some(r) = full(lex, w, out) {
            return r;
        }
        if let Some(k) = stemmed(lex, w, out) {
            return (k, Source::Stem);
        }
        (by_rules(w, out), Source::Rules)
    } else {
        (by_rules(word, out), Source::Rules)
    }
}

/// The name of a letter (`a`–`z`, either case) as phones.
pub fn letter_name(ch: u8) -> &'static [Phone] {
    use Ph::*;
    const PRI: Stress = Stress::Primary;
    const fn s(ph: Ph) -> Phone {
        Phone::new(ph, Stress::Primary)
    }
    const fn k(ph: Ph) -> Phone {
        Phone::new(ph, Stress::None)
    }
    const fn u(ph: Ph) -> Phone {
        Phone::new(ph, Stress::Unstressed)
    }
    const NAMES: [&[Phone]; 26] = [
        &[s(EY)],
        &[k(B), s(IY)],
        &[k(S), s(IY)],
        &[k(D), s(IY)],
        &[s(IY)],
        &[s(EH), k(F)],
        &[k(JH), s(IY)],
        &[s(EY), k(CH)],
        &[s(AY)],
        &[k(JH), s(EY)],
        &[k(K), s(EY)],
        &[s(EH), k(L)],
        &[s(EH), k(M)],
        &[s(EH), k(N)],
        &[s(OW)],
        &[k(P), s(IY)],
        &[k(K), k(Y), s(UW)],
        &[s(AA), k(R)],
        &[s(EH), k(S)],
        &[k(T), s(IY)],
        &[k(Y), s(UW)],
        &[k(V), s(IY)],
        &[k(D), Phone::new(AH, PRI), k(B), u(AH), k(L), k(Y), u(UW)],
        &[s(EH), k(K), k(S)],
        &[k(W), s(AY)],
        &[k(Z), s(IY)],
    ];
    let l = ch.to_ascii_lowercase();
    if l.is_ascii_lowercase() { NAMES[usize::from(l - b'a')] } else { &[] }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::phoneme::write_arpabet;
    use std::string::String;

    const BLANK: Phone = Phone::new(Ph::AH, Stress::None);

    fn say(w: &str) -> (String, Source) {
        let mut out = [BLANK; MAX_WORD_PHONES];
        let (n, src) = pronounce(&Lexicon::builtin(), w.as_bytes(), &mut out);
        let mut b = [0u8; 256];
        let k = write_arpabet(&out[..n], &mut b);
        (String::from_utf8_lossy(&b[..k]).into_owned(), src)
    }

    #[test]
    fn letters() {
        assert_eq!(letter_name(b'w').len(), 7);
        assert_eq!(letter_name(b'A'), letter_name(b'a'));
        assert!(letter_name(b'1').is_empty());
    }

    #[test]
    fn endings() {
        let mut out = [BLANK; 8];
        out[0] = c(Ph::K);
        out[1] = Phone::new(Ph::AE, Stress::Primary);
        out[2] = c(Ph::T);
        assert_eq!(out[plural(&mut out, 3) - 1].ph, Ph::S);
        out[2] = c(Ph::D);
        assert_eq!(out[plural(&mut out, 3) - 1].ph, Ph::Z);
        out[2] = c(Ph::S);
        assert_eq!(plural(&mut out, 3), 5);
        out[2] = c(Ph::T);
        assert_eq!(past(&mut out, 3), 5);
        out[2] = c(Ph::P);
        assert_eq!(out[past(&mut out, 3) - 1].ph, Ph::T);
    }

    #[test]
    fn sources() {
        assert_eq!(say("women").1, Source::Lexicon);
        assert_eq!(say("the").1, Source::Rules);
        let (_, src) = say("zzyzx");
        assert_eq!(src, Source::Rules);
        // Overlong words go to the rules without panicking.
        let long = "a".repeat(200);
        let _ = say(&long);
    }
}
