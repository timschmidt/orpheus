//! Unicode helpers: simple case folding, the exFAT up-case table, code page
//! 437 and UTF-16 to UTF-8 conversion.
//!
//! Case folding covers the Basic Multilingual Plane (every script with
//! case in the BMP: Latin, Greek, Cyrillic, Armenian, Georgian, Cherokee,
//! Glagolitic, Coptic, ...). Supplementary-plane letters (Deseret, Osage, ...)
//! compare case-sensitively. The same function is meant for the collation of
//! 04-audio §6.1.

use crate::tables::{CP437_HIGH, FOLD, Run, UPCASE};

fn lookup(table: &[Run], u: u16) -> u16 {
    // last run whose start <= u
    let idx = table.partition_point(|r| r.start <= u);
    if idx == 0 {
        return u;
    }
    let Some(r) = table.get(idx - 1) else {
        return u;
    };
    let off = u - r.start;
    let step = u16::from(r.step.max(1));
    if !off.is_multiple_of(step) || off / step >= r.count {
        return u;
    }
    u.wrapping_add(r.delta)
}

/// Simple case folding of one UTF-16 code unit (surrogates map to themselves).
#[must_use]
pub fn fold16(u: u16) -> u16 {
    if u < 0x80 {
        return if (0x41..=0x5A).contains(&u) { u + 32 } else { u };
    }
    lookup(&FOLD, u)
}

/// Simple case folding of a character (BMP; other planes unchanged).
#[must_use]
pub fn fold(c: char) -> char {
    let v = u32::from(c);
    if v > 0xFFFF {
        return c;
    }
    #[allow(clippy::cast_possible_truncation)]
    let f = fold16(v as u16);
    char::from_u32(u32::from(f)).unwrap_or(c)
}

/// exFAT recommended up-case mapping of one code unit.
#[must_use]
pub fn upcase16(u: u16) -> u16 {
    if u < 0x80 {
        return if (0x61..=0x7A).contains(&u) { u - 32 } else { u };
    }
    lookup(&UPCASE, u)
}

/// Case-insensitive equality of two strings (simple case folding).
#[must_use]
pub fn eq_ignore_case(a: &str, b: &str) -> bool {
    let mut x = a.encode_utf16();
    let mut y = b.encode_utf16();
    loop {
        match (x.next(), y.next()) {
            (None, None) => return true,
            (Some(p), Some(q)) if fold16(p) == fold16(q) => {}
            _ => return false,
        }
    }
}

/// Decode one OEM byte of a FAT short name (code page 437).
#[must_use]
pub fn cp437(b: u8) -> u16 {
    if b < 0x80 { u16::from(b) } else { CP437_HIGH[usize::from(b - 0x80)] }
}

/// Convert UTF-16 to UTF-8 inside one buffer.
///
/// The UTF-16 code units are stored little-endian in `buf[src..src + 2 * n]`;
/// the UTF-8 result is written to the start of `buf`. Works in place as long
/// as `src >= n + 3` (UTF-8 needs at most 3 bytes per unit, so the writer
/// never overtakes the reader). Unpaired surrogates become U+FFFD. Returns the
/// UTF-8 length.
pub(crate) fn utf16_to_utf8_in_place(buf: &mut [u8], src: usize, n: usize) -> usize {
    let mut w = 0usize;
    let mut i = 0usize;
    let unit = |buf: &[u8], i: usize| -> u16 {
        let p = src + 2 * i;
        match (buf.get(p), buf.get(p + 1)) {
            (Some(&lo), Some(&hi)) => u16::from_le_bytes([lo, hi]),
            _ => 0xFFFD,
        }
    };
    while i < n {
        let u = unit(buf, i);
        i += 1;
        let c: u32 = if (0xD800..0xDC00).contains(&u) {
            if i < n {
                let l = unit(buf, i);
                if (0xDC00..0xE000).contains(&l) {
                    i += 1;
                    0x10000 + ((u32::from(u) - 0xD800) << 10) + (u32::from(l) - 0xDC00)
                } else {
                    0xFFFD
                }
            } else {
                0xFFFD
            }
        } else if (0xDC00..0xE000).contains(&u) {
            0xFFFD
        } else {
            u32::from(u)
        };
        let ch = char::from_u32(c).unwrap_or('\u{FFFD}');
        let mut tmp = [0u8; 4];
        let s = ch.encode_utf8(&mut tmp);
        for &b in s.as_bytes() {
            if let Some(slot) = buf.get_mut(w) {
                *slot = b;
            }
            w += 1;
        }
    }
    w.min(buf.len())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folding_basics() {
        assert_eq!(fold('A'), 'a');
        assert_eq!(fold('Ä'), 'ä');
        assert_eq!(fold('Ω'), 'ω');
        assert_eq!(fold('ς'), 'σ');
        assert_eq!(fold('Ж'), 'ж');
        assert_eq!(fold('Ё'), 'ё');
        assert_eq!(fold('Ā'), 'ā');
        assert_eq!(fold('ſ'), 's');
        assert_eq!(fold('ẞ'), 'ß');
        assert_eq!(fold('1'), '1');
        assert_eq!(fold('日'), '日');
        assert!(eq_ignore_case("Пётр Ильич", "ПЁТР ИЛЬИЧ"));
        assert!(eq_ignore_case("ΣΊΣΥΦΟΣ", "σίσυφος"));
        assert!(!eq_ignore_case("abc", "abd"));
        assert!(!eq_ignore_case("abc", "abcd"));
    }

    #[test]
    fn upcase_basics() {
        assert_eq!(upcase16(u16::from(b'a')), u16::from(b'A'));
        assert_eq!(upcase16(0x00E4), 0x00C4);
        assert_eq!(upcase16(0x0436), 0x0416);
        assert_eq!(upcase16(0x03C9), 0x03A9);
    }

    /// Cross-check the generated table against CaseFolding.txt when present.
    #[test]
    fn folding_matches_ucd() {
        let Ok(text) = std::fs::read_to_string("/usr/share/unicode/ucd/CaseFolding.txt") else {
            return;
        };
        let mut n = 0;
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("");
            let f: std::vec::Vec<&str> = line.split(';').map(str::trim).collect();
            if f.len() < 3 || !(f[1] == "C" || f[1] == "S") {
                continue;
            }
            let c = u32::from_str_radix(f[0], 16).unwrap();
            let t = u32::from_str_radix(f[2], 16).unwrap();
            if c > 0xFFFF || t > 0xFFFF {
                continue;
            }
            assert_eq!(u32::from(fold16(c as u16)), t, "U+{c:04X}");
            n += 1;
        }
        assert!(n > 1000);
    }

    #[test]
    fn in_place_conversion() {
        let s = "Ünïcødé 日本語 😀 x";
        let units: std::vec::Vec<u16> = s.encode_utf16().collect();
        let n = units.len();
        let mut buf = [0u8; 128];
        let src = n + 3;
        for (i, u) in units.iter().enumerate() {
            buf[src + 2 * i..src + 2 * i + 2].copy_from_slice(&u.to_le_bytes());
        }
        let len = utf16_to_utf8_in_place(&mut buf, src, n);
        assert_eq!(core::str::from_utf8(&buf[..len]).unwrap(), s);
    }

    #[test]
    fn cp437_table() {
        assert_eq!(cp437(0x41), 0x41);
        assert_eq!(cp437(0x80), 0x00C7);
        assert_eq!(cp437(0xE1), 0x00DF);
    }
}
