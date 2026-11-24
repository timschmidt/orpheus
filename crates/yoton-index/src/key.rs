//! The byte-comparable sort key of a name (04 §6.1 "sort key prefix").
//!
//! [`natural_cmp`] is a lexicographic comparison over *units*: a folded
//! character, or a run of ASCII digits compared as `(value saturated to
//! u64, digit count)`; a shorter unit sequence sorts first; equal sequences
//! fall back to the raw string. A digit run against a character compares
//! the run's first digit with the character, and since no folded non-digit
//! is an ASCII digit, every run sorts between `'/'` (0x2F) and `':'` (0x3A).
//!
//! The key encodes each unit so that byte order equals unit order and no
//! unit's encoding is a prefix of another's:
//!
//! * character: the UTF-8 bytes of `fold(c)` (UTF-8 byte order is code
//!   point order; never 0x30..=0x39 because digits always form runs);
//! * digit run with value `v` (saturated) and `n` digits: `0x30 + b` where
//!   `b` = bytes needed for `v` (0..=8, so the lead is 0x30..=0x38), then
//!   `v` big-endian in `b` bytes, then `z = n - digits(v)` (leading zeros;
//!   for equal `v` this orders like `n`) as one byte if < 0xFF, else 0xFF
//!   and a big-endian u16.
//!
//! Therefore `natural_cmp(a, b) == key(a).cmp(key(b)).then(a.cmp(b))`, and
//! two keys that differ inside their first [`PREFIX`] bytes are ordered by
//! those bytes alone. Records keep that prefix; full names are read only on
//! a tie ([`prefix_cmp`] returns `None`).

use core::cmp::Ordering;

use yoton_apps::browse::fold;

/// Key bytes kept in each record.
pub const PREFIX: usize = 8;

struct KeyWriter<'a> {
    out: &'a mut [u8],
    len: usize,
}

impl KeyWriter<'_> {
    fn push(&mut self, b: u8) {
        if let Some(s) = self.out.get_mut(self.len) {
            *s = b;
        }
        self.len = self.len.saturating_add(1);
    }
}

fn decimal_digits(mut v: u64) -> u32 {
    let mut n = 0;
    while v != 0 {
        v /= 10;
        n += 1;
    }
    n
}

/// Writes the first `out.len()` bytes of the sort key of `name` into `out`
/// (the rest of `out` is left alone) and returns the key's full length.
pub fn sort_key(name: &str, out: &mut [u8]) -> usize {
    let mut w = KeyWriter { out, len: 0 };
    let mut it = name.chars().peekable();
    while let Some(c) = it.next() {
        if let Some(d) = c.to_digit(10).filter(|_| c.is_ascii_digit()) {
            let mut v = u64::from(d);
            let mut n: u32 = 1;
            while let Some(d) = it.peek().copied().filter(char::is_ascii_digit) {
                v = v.saturating_mul(10).saturating_add(u64::from(d.to_digit(10).unwrap_or(0)));
                n = n.saturating_add(1);
                it.next();
            }
            let bytes = (64 - v.leading_zeros()).div_ceil(8);
            w.push(0x30 + bytes as u8);
            for i in (0..bytes).rev() {
                w.push((v >> (8 * i)) as u8);
            }
            let z = n.saturating_sub(decimal_digits(v));
            if z < 0xFF {
                w.push(z as u8);
            } else {
                let z = u16::try_from(z).unwrap_or(u16::MAX);
                w.push(0xFF);
                w.push((z >> 8) as u8);
                w.push(z as u8);
            }
        } else {
            let mut b = [0u8; 4];
            for &x in fold(c).encode_utf8(&mut b).as_bytes() {
                w.push(x);
            }
        }
    }
    w.len
}

/// Prefix of a key as stored in a record: the first [`PREFIX`] key bytes
/// (zero padded) and the full key length (saturated at 255).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct KeyPrefix {
    pub bytes: [u8; PREFIX],
    pub len: u8,
}

impl KeyPrefix {
    #[must_use]
    pub fn of(name: &str) -> KeyPrefix {
        let mut bytes = [0u8; PREFIX];
        let len = sort_key(name, &mut bytes);
        KeyPrefix { bytes, len: u8::try_from(len).unwrap_or(u8::MAX) }
    }
}

/// Order of two names judged by their key prefixes alone, or `None` when
/// the prefixes cannot decide (equal prefixes of longer keys, or equal
/// keys, which `natural_cmp` breaks by the raw string).
#[must_use]
pub fn prefix_cmp(a: &KeyPrefix, b: &KeyPrefix) -> Option<Ordering> {
    let la = usize::from(a.len).min(PREFIX);
    let lb = usize::from(b.len).min(PREFIX);
    let n = la.min(lb);
    match a.bytes[..n].cmp(&b.bytes[..n]) {
        Ordering::Equal => {}
        o => return Some(o),
    }
    // Common bytes equal. A key that ended inside the prefix is a proper
    // prefix of the other key (unit boundaries: the encoding is prefix
    // free), so it sorts first.
    let (ca, cb) = (usize::from(a.len) <= PREFIX, usize::from(b.len) <= PREFIX);
    match (ca, cb) {
        (true, true) if la == lb => None,
        (true, _) if la < lb || !cb => Some(Ordering::Less),
        (_, true) => Some(Ordering::Greater),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use yoton_apps::browse::natural_cmp;

    fn full_key(s: &str) -> ([u8; 1024], usize) {
        let mut k = [0u8; 1024];
        let n = sort_key(s, &mut k);
        (k, n)
    }

    fn key_cmp(a: &str, b: &str) -> Ordering {
        let (ka, na) = full_key(a);
        let (kb, nb) = full_key(b);
        ka[..na].cmp(&kb[..nb]).then(a.cmp(b))
    }

    fn check(a: &str, b: &str) {
        assert_eq!(key_cmp(a, b), natural_cmp(a, b), "{a:?} vs {b:?}");
        let (pa, pb) = (KeyPrefix::of(a), KeyPrefix::of(b));
        if let Some(o) = prefix_cmp(&pa, &pb) {
            assert_eq!(o, natural_cmp(a, b), "prefix {a:?} vs {b:?}");
        }
    }

    const SAMPLES: &[&str] = &[
        "",
        "a",
        "A",
        "a1",
        "a01",
        "a001",
        "a1b",
        "a10",
        "a9",
        "a/",
        "a:",
        "a0",
        "a00",
        "Track 2",
        "Track 10",
        "track 2",
        "x02",
        "x2",
        "Ёлка",
        "елка",
        "Ежи",
        "ς",
        "σ",
        "Σ",
        "日本",
        "日本語",
        "18446744073709551615",
        "18446744073709551616",
        "99999999999999999999",
        "999999999999999999999",
        "0",
        "00",
        "000000000000000000000000000000",
        "1.mp3",
        "1-2",
        "12345678",
        "abcdefgh",
        "abcdefghi",
        "abcdefgh1",
        "abcdefg",
        "abcdefg1",
        "abcdef12",
        "abcdef120",
        "İstanbul",
        "istanbul",
        "ß",
        "ss",
        "émile",
        "Emile",
        "\u{10FFFF}",
        "a\u{7F}",
    ];

    #[test]
    fn matches_natural_cmp() {
        for a in SAMPLES {
            for b in SAMPLES {
                check(a, b);
            }
        }
    }

    #[test]
    fn long_digit_runs() {
        let mut a = [b'0'; 300];
        a[299] = b'1';
        let mut b = [b'0'; 301];
        b[300] = b'1';
        let (a, b) = (core::str::from_utf8(&a).unwrap(), core::str::from_utf8(&b).unwrap());
        check(a, b);
        check(b, a);
    }

    #[test]
    fn random_pairs() {
        let alphabet: &[char] =
            &['a', 'B', '0', '1', '9', ' ', '.', '_', 'я', 'Я', '日', '-', 'z', 'Z', '/', ':'];
        let mut s = 0x1234_5678_9abc_def0u64;
        let mut next = || {
            s ^= s << 13;
            s ^= s >> 7;
            s ^= s << 17;
            s
        };
        let mut a = [0u8; 64];
        let mut b = [0u8; 64];
        for _ in 0..200_000 {
            let mk = |buf: &mut [u8; 64], r: &mut dyn FnMut() -> u64| {
                let n = (r() % 12) as usize;
                let mut len = 0;
                for _ in 0..n {
                    let c = alphabet[(r() % alphabet.len() as u64) as usize];
                    len += c.encode_utf8(&mut buf[len..]).len();
                }
                len
            };
            let na = mk(&mut a, &mut next);
            let nb = if next() % 3 == 0 {
                b[..na].copy_from_slice(&a[..na]);
                let extra = (next() % 4) as usize;
                let mut len = na;
                for _ in 0..extra {
                    let c = alphabet[(next() % alphabet.len() as u64) as usize];
                    len += c.encode_utf8(&mut b[len..]).len();
                }
                len
            } else {
                mk(&mut b, &mut next)
            };
            check(core::str::from_utf8(&a[..na]).unwrap(), core::str::from_utf8(&b[..nb]).unwrap());
        }
    }
}
