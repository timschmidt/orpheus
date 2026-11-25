//! UTF-8 to the normaliser's byte alphabet: ASCII as is, accented Latin
//! letters folded to ASCII, the symbols the normaliser knows as codes
//! 0x80–0xB7, everything else (other scripts, emoji, invalid bytes) as
//! [`SKIP`].

/// A character that is not read (other scripts, emoji, invalid UTF-8).
pub const SKIP: u8 = 0x7F;
/// `€`
pub const EURO: u8 = 0x80;
/// `£`
pub const POUND: u8 = 0x81;
/// `¥`
pub const YEN: u8 = 0x82;
/// `¢`
pub const CENT: u8 = 0x83;
/// `°`
pub const DEG: u8 = 0x84;
/// `©`
pub const COPY: u8 = 0x85;
/// `®`
pub const REG: u8 = 0x86;
/// `™`
pub const TM: u8 = 0x87;
/// `§`
pub const SECT: u8 = 0x88;
/// `¶`
pub const PARA: u8 = 0x89;
/// `±`
pub const PLUSMINUS: u8 = 0x8A;
/// `×`
pub const TIMES: u8 = 0x8B;
/// `÷`
pub const DIVIDE: u8 = 0x8C;
/// `µ` (and `μ` in units)
pub const MICRO: u8 = 0x8D;
/// `…`
pub const ELLIPSIS: u8 = 0x8E;
/// `–`
pub const ENDASH: u8 = 0x8F;
/// `—`, `―`
pub const EMDASH: u8 = 0x90;
/// `“`, `‘`, `„`, `«`
pub const LQUOTE: u8 = 0x91;
/// `”`, `»`
pub const RQUOTE: u8 = 0x92;
/// `½`
pub const HALF: u8 = 0x93;
/// `¼`
pub const QUARTER: u8 = 0x94;
/// `¾`
pub const THREEQ: u8 = 0x95;
/// `•`, `·`
pub const BULLET: u8 = 0x96;
/// `−` (U+2212)
pub const MINUS: u8 = 0x97;
/// `¿`
pub const INVQ: u8 = 0x98;
/// `¡`
pub const INVEXCL: u8 = 0x99;
/// `Ω` (ohm)
pub const OHM: u8 = 0x9A;
/// First Greek letter code (alpha); 24 letters follow in order.
pub const GREEK: u8 = 0xA0;

/// The Greek letters' names, in alphabet order.
pub const GREEK_NAMES: [&str; 24] = [
    "alpha", "beta", "gamma", "delta", "epsilon", "zeta", "eta", "theta", "iota", "kappa", "lambda", "mu",
    "nu", "xi", "omicron", "pi", "rho", "sigma", "tau", "upsilon", "phi", "chi", "psi", "omega",
];

/// U+00C0–U+00FF folded ('*' marks the special cases).
const LATIN1: &[u8; 64] = b"AAAAAA*CEEEEIIIIDNOOOOO*OUUUUY**aaaaaa*ceeeeiiiidnooooo*ouuuuy*y";
/// U+0100–U+017F folded.
const LATIN_A: &[u8; 128] = b"AaAaAaCcCcCcCcDdDdEeEeEeEeEeGgGgGgGgHhHhIiIiIiIiIiIiJjKkkLlLlLlLlLlNnNnNnnNnOoOoOoOoRrRrRrSsSsSsSsTtTtTtUuUuUuUuUuUuWwYyYZzZzZzs";

/// Decodes one UTF-8 character at the start of `s`: (code point or
/// `None` if invalid, bytes used ≥ 1).
pub fn decode(s: &[u8]) -> (Option<u32>, usize) {
    let b0 = s[0];
    let (len, init) = match b0 {
        0x00..=0x7F => return (Some(u32::from(b0)), 1),
        0xC2..=0xDF => (2, u32::from(b0 & 0x1F)),
        0xE0..=0xEF => (3, u32::from(b0 & 0x0F)),
        0xF0..=0xF4 => (4, u32::from(b0 & 0x07)),
        _ => return (None, 1),
    };
    if s.len() < len {
        return (None, 1);
    }
    let mut cp = init;
    for &b in &s[1..len] {
        if b & 0xC0 != 0x80 {
            return (None, 1);
        }
        cp = (cp << 6) | u32::from(b & 0x3F);
    }
    let min = [0, 0, 0x80, 0x800, 0x10000][len];
    if cp < min || (0xD800..=0xDFFF).contains(&cp) || cp > 0x10FFFF {
        return (None, len);
    }
    (Some(cp), len)
}

/// Whether the code point is white space for the chunker.
pub fn is_space(cp: u32) -> bool {
    matches!(cp, 0x09..=0x0D | 0x20 | 0x85 | 0xA0 | 0x1680 | 0x2000..=0x200A | 0x2028 | 0x2029 | 0x202F | 0x205F | 0x3000)
}

/// Folds one code point into 1–2 bytes of the alphabet; returns the count.
pub fn fold_char(cp: u32, out: &mut [u8; 2]) -> usize {
    let one = |b: u8, out: &mut [u8; 2]| {
        out[0] = b;
        1
    };
    let two = |a: u8, b: u8, out: &mut [u8; 2]| {
        out[0] = a;
        out[1] = b;
        2
    };
    match cp {
        0x00..=0x1F => one(b' ', out),
        0x20..=0x7E => one(cp as u8, out),
        0xC6 => two(b'A', b'E', out),
        0xE6 => two(b'a', b'e', out),
        0xDE => two(b'T', b'h', out),
        0xFE => two(b't', b'h', out),
        0xDF => two(b's', b's', out),
        0xD7 => one(TIMES, out),
        0xF7 => one(DIVIDE, out),
        0xC0..=0xFF => one(LATIN1[(cp - 0xC0) as usize], out),
        0x152 => two(b'O', b'E', out),
        0x153 => two(b'o', b'e', out),
        0x132 => two(b'I', b'J', out),
        0x133 => two(b'i', b'j', out),
        0x100..=0x17F => one(LATIN_A[(cp - 0x100) as usize], out),
        0x20AC => one(EURO, out),
        0xA3 => one(POUND, out),
        0xA5 => one(YEN, out),
        0xA2 => one(CENT, out),
        0xB0 => one(DEG, out),
        0xA9 => one(COPY, out),
        0xAE => one(REG, out),
        0x2122 => one(TM, out),
        0xA7 => one(SECT, out),
        0xB6 => one(PARA, out),
        0xB1 => one(PLUSMINUS, out),
        0xB5 | 0x3BC => one(MICRO, out),
        0x2026 => one(ELLIPSIS, out),
        0x2010 | 0x2011 => one(b'-', out),
        0x2012 | 0x2013 => one(ENDASH, out),
        0x2014 | 0x2015 => one(EMDASH, out),
        0x2018 | 0x201A | 0x201B | 0x201C | 0x201E | 0x201F | 0xAB | 0x2039 => one(LQUOTE, out),
        0x2019 | 0x2032 => one(b'\'', out),
        0x201D | 0xBB | 0x203A | 0x2033 => one(RQUOTE, out),
        0xBD => one(HALF, out),
        0xBC => one(QUARTER, out),
        0xBE => one(THREEQ, out),
        0x2022 | 0xB7 | 0x2023 | 0x25E6 | 0x2043 => one(BULLET, out),
        0x2212 => one(MINUS, out),
        0xBF => one(INVQ, out),
        0xA1 => one(INVEXCL, out),
        0x2126 => one(OHM, out),
        0xB9 => one(b'1', out),
        0xB2 => one(b'2', out),
        0xB3 => one(b'3', out),
        0x391..=0x3A1 => one(GREEK + (cp - 0x391) as u8, out),
        0x3A3..=0x3A9 => one(GREEK + (cp - 0x392) as u8, out),
        0x3B1..=0x3C1 => one(GREEK + (cp - 0x3B1) as u8, out),
        0x3C2 => one(GREEK + 17, out),
        0x3C3..=0x3C9 => one(GREEK + (cp - 0x3B2) as u8, out),
        // Combining marks (accents written separately) vanish.
        0x300..=0x36F => 0,
        _ => one(SKIP, out),
    }
}

/// Folds UTF-8 `src` into `dst`; returns the length (stops when `dst` is
/// full). Invalid bytes become [`SKIP`].
pub fn fold(src: &[u8], dst: &mut [u8]) -> usize {
    let mut i = 0;
    let mut n = 0;
    while i < src.len() {
        let (cp, len) = decode(&src[i..]);
        i += len;
        let mut b = [0u8; 2];
        let k = match cp {
            Some(cp) => fold_char(cp, &mut b),
            None => {
                b[0] = SKIP;
                1
            }
        };
        if n + k > dst.len() {
            break;
        }
        dst[n..n + k].copy_from_slice(&b[..k]);
        n += k;
    }
    n
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(s: &str) -> ([u8; 64], usize) {
        let mut d = [0u8; 64];
        let n = fold(s.as_bytes(), &mut d);
        (d, n)
    }

    #[test]
    fn folding() {
        let (d, n) = f("Beyoncé Motörhead Œuvre straße Ærø");
        assert_eq!(&d[..n], b"Beyonce Motorhead OEuvre strasse AEro");
        let (d, n) = f("€5 ½ — “q” it’s αΩω");
        assert_eq!(
            &d[..n],
            &[
                EURO,
                b'5',
                b' ',
                HALF,
                b' ',
                EMDASH,
                b' ',
                LQUOTE,
                b'q',
                RQUOTE,
                b' ',
                b'i',
                b't',
                b'\'',
                b's',
                b' ',
                GREEK,
                GREEK + 23,
                GREEK + 23
            ]
        );
        let (d, n) = f("日本\u{1F600}");
        assert_eq!(&d[..n], &[SKIP, SKIP, SKIP]);
        let mut d = [0u8; 8];
        assert_eq!(fold(&[0xFF, 0xC3, b'a', 0xE2, 0x82], &mut d), 5);
        assert_eq!(&d[..5], &[SKIP, SKIP, b'a', SKIP, SKIP]);
        assert_eq!(GREEK_NAMES.len(), 24);
    }

    #[test]
    fn spaces() {
        assert!(is_space(0xA0) && is_space(u32::from(b'\n')) && !is_space(u32::from(b'a')));
    }
}
