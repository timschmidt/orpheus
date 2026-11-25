//! Text encodings of plain-text and FB2 books (05 §7.15): detection from
//! the first bytes of a file and a streaming decoder to `char`s.
//!
//! Detection, in order: a byte-order mark (UTF-8, UTF-16 LE/BE); UTF-16
//! without a mark (zero bytes on one side of most pairs); an XML
//! declaration's `encoding="…"` (FB2); valid UTF-8; otherwise a single-byte
//! code page: Windows-1251 or KOI8-R when most letters are high bytes
//! (Russian text is mostly lower case, which is `0xE0–0xFF` in 1251 and
//! `0xC0–0xDF` in KOI8-R), else Latin-1 (decoded as Windows-1252, its
//! superset for printable text). Undecodable input becomes U+FFFD; the
//! decoder never fails and never stalls.

/// A text encoding.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
#[repr(u8)]
pub enum Encoding {
    #[default]
    Utf8 = 0,
    Utf16Le = 1,
    Utf16Be = 2,
    Cp1251 = 3,
    Koi8r = 4,
    /// ISO 8859-1, decoded as Windows-1252.
    Latin1 = 5,
}

impl Encoding {
    pub const ALL: [Encoding; 6] = [
        Encoding::Utf8,
        Encoding::Utf16Le,
        Encoding::Utf16Be,
        Encoding::Cp1251,
        Encoding::Koi8r,
        Encoding::Latin1,
    ];

    #[must_use]
    pub fn from_u8(v: u8) -> Encoding {
        Encoding::ALL.get(usize::from(v)).copied().unwrap_or_default()
    }

    /// A name as in an XML declaration (case-insensitive ASCII).
    #[must_use]
    pub fn from_label(label: &[u8]) -> Option<Encoding> {
        let eq = |s: &str| label.eq_ignore_ascii_case(s.as_bytes());
        Some(if eq("utf-8") || eq("utf8") {
            Encoding::Utf8
        } else if eq("utf-16le") {
            Encoding::Utf16Le
        } else if eq("utf-16be") || eq("utf-16") {
            Encoding::Utf16Be
        } else if eq("windows-1251") || eq("cp1251") || eq("win-1251") {
            Encoding::Cp1251
        } else if eq("koi8-r") || eq("koi8r") || eq("koi8-u") {
            Encoding::Koi8r
        } else if eq("iso-8859-1") || eq("latin1") || eq("windows-1252") || eq("cp1252") || eq("us-ascii") {
            Encoding::Latin1
        } else {
            return None;
        })
    }
}

/// A byte-order mark at the start: the encoding and the mark's length.
#[must_use]
pub fn bom(head: &[u8]) -> Option<(Encoding, usize)> {
    match head {
        [0xEF, 0xBB, 0xBF, ..] => Some((Encoding::Utf8, 3)),
        [0xFF, 0xFE, ..] => Some((Encoding::Utf16Le, 2)),
        [0xFE, 0xFF, ..] => Some((Encoding::Utf16Be, 2)),
        _ => None,
    }
}

/// `encoding="…"` of an XML declaration at the start of `head` (ASCII-
/// compatible encodings only; UTF-16 is found by [`detect`] before).
#[must_use]
pub fn xml_declared(head: &[u8]) -> Option<Encoding> {
    let start = head.iter().position(|&b| !b.is_ascii_whitespace())?;
    let rest = head.get(start..)?;
    if !rest.starts_with(b"<?xml") {
        return None;
    }
    let end = rest.iter().position(|&b| b == b'>')?;
    let decl = rest.get(..end)?;
    let at = decl.windows(8).position(|w| w == b"encoding")?;
    let mut i = at + 8;
    while decl.get(i).is_some_and(|b| b.is_ascii_whitespace() || *b == b'=') {
        i += 1;
    }
    let q = *decl.get(i)?;
    if q != b'"' && q != b'\'' {
        return None;
    }
    let from = i + 1;
    let len = decl.get(from..)?.iter().position(|&b| b == q)?;
    Encoding::from_label(decl.get(from..from + len)?)
}

/// Whether `b` is valid UTF-8, except for a sequence cut at the end
/// (`complete == false`: `b` is only the head of a file).
#[must_use]
pub fn is_utf8(b: &[u8], complete: bool) -> bool {
    match core::str::from_utf8(b) {
        Ok(_) => true,
        Err(e) => !complete && e.error_len().is_none(),
    }
}

/// UTF-16 without a mark: most pairs have a zero byte on the same side
/// (Latin or Cyrillic text). `Some(LE)` if the zeros are the odd bytes.
fn utf16_unmarked(head: &[u8]) -> Option<Encoding> {
    let pairs = head.len() / 2;
    if pairs < 4 {
        return None;
    }
    let (mut even, mut odd) = (0usize, 0usize);
    for p in head.chunks_exact(2) {
        if p[0] == 0 {
            even += 1;
        }
        if p[1] == 0 {
            odd += 1;
        }
    }
    // Cyrillic in UTF-16 has 0x04 rather than 0x00 in the high byte:
    // count those too.
    let (mut even4, mut odd4) = (0usize, 0usize);
    for p in head.chunks_exact(2) {
        if p[0] == 0x04 {
            even4 += 1;
        }
        if p[1] == 0x04 {
            odd4 += 1;
        }
    }
    let (e, o) = (even + even4, odd + odd4);
    if o * 10 >= pairs * 7 && even * 10 < pairs {
        Some(Encoding::Utf16Le)
    } else if e * 10 >= pairs * 7 && odd * 10 < pairs {
        Some(Encoding::Utf16Be)
    } else {
        None
    }
}

/// The encoding of a file from its first bytes, and how many of them to
/// skip (a byte-order mark). `complete`: `head` is the whole file.
#[must_use]
pub fn detect(head: &[u8], complete: bool) -> (Encoding, usize) {
    if let Some(b) = bom(head) {
        return b;
    }
    if let Some(e) = utf16_unmarked(head) {
        return (e, 0);
    }
    if let Some(e) = xml_declared(head) {
        return (e, 0);
    }
    if is_utf8(head, complete) {
        return (Encoding::Utf8, 0);
    }
    (single_byte(head), 0)
}

/// Windows-1251, KOI8-R or Latin-1 for text that is not UTF-8.
#[must_use]
pub fn single_byte(head: &[u8]) -> Encoding {
    let (mut high, mut ascii_letters, mut low1251, mut lowkoi) = (0usize, 0usize, 0usize, 0usize);
    for &b in head {
        match b {
            b'a'..=b'z' | b'A'..=b'Z' => ascii_letters += 1,
            0xC0..=0xDF => {
                high += 1;
                lowkoi += 1;
            }
            0xE0..=0xFF => {
                high += 1;
                low1251 += 1;
            }
            0x80..=0xBF => high += 1,
            _ => {}
        }
    }
    // Cyrillic text: nearly every letter is a high byte. Latin-1 text
    // (French, German, Icelandic …): a few accented letters.
    if high * 2 > ascii_letters + high {
        if low1251 >= lowkoi { Encoding::Cp1251 } else { Encoding::Koi8r }
    } else {
        Encoding::Latin1
    }
}

/// The replacement character for undecodable input.
pub const REPLACEMENT: char = '\u{FFFD}';

#[rustfmt::skip]
static CP1251: [u16; 128] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021,
    0x20AC, 0x2030, 0x0409, 0x2039, 0x040A, 0x040C, 0x040B, 0x040F,
    0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0xFFFD, 0x2122, 0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F,
    0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6, 0x00A7,
    0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407,
    0x00B0, 0x00B1, 0x0406, 0x0456, 0x0491, 0x00B5, 0x00B6, 0x00B7,
    0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
    0x0410, 0x0411, 0x0412, 0x0413, 0x0414, 0x0415, 0x0416, 0x0417,
    0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E, 0x041F,
    0x0420, 0x0421, 0x0422, 0x0423, 0x0424, 0x0425, 0x0426, 0x0427,
    0x0428, 0x0429, 0x042A, 0x042B, 0x042C, 0x042D, 0x042E, 0x042F,
    0x0430, 0x0431, 0x0432, 0x0433, 0x0434, 0x0435, 0x0436, 0x0437,
    0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E, 0x043F,
    0x0440, 0x0441, 0x0442, 0x0443, 0x0444, 0x0445, 0x0446, 0x0447,
    0x0448, 0x0449, 0x044A, 0x044B, 0x044C, 0x044D, 0x044E, 0x044F,
];

#[rustfmt::skip]
static KOI8R: [u16; 128] = [
    0x2500, 0x2502, 0x250C, 0x2510, 0x2514, 0x2518, 0x251C, 0x2524,
    0x252C, 0x2534, 0x253C, 0x2580, 0x2584, 0x2588, 0x258C, 0x2590,
    0x2591, 0x2592, 0x2593, 0x2320, 0x25A0, 0x2219, 0x221A, 0x2248,
    0x2264, 0x2265, 0x00A0, 0x2321, 0x00B0, 0x00B2, 0x00B7, 0x00F7,
    0x2550, 0x2551, 0x2552, 0x0451, 0x2553, 0x2554, 0x2555, 0x2556,
    0x2557, 0x2558, 0x2559, 0x255A, 0x255B, 0x255C, 0x255D, 0x255E,
    0x255F, 0x2560, 0x2561, 0x0401, 0x2562, 0x2563, 0x2564, 0x2565,
    0x2566, 0x2567, 0x2568, 0x2569, 0x256A, 0x256B, 0x256C, 0x00A9,
    0x044E, 0x0430, 0x0431, 0x0446, 0x0434, 0x0435, 0x0444, 0x0433,
    0x0445, 0x0438, 0x0439, 0x043A, 0x043B, 0x043C, 0x043D, 0x043E,
    0x043F, 0x044F, 0x0440, 0x0441, 0x0442, 0x0443, 0x0436, 0x0432,
    0x044C, 0x044B, 0x0437, 0x0448, 0x044D, 0x0449, 0x0447, 0x044A,
    0x042E, 0x0410, 0x0411, 0x0426, 0x0414, 0x0415, 0x0424, 0x0413,
    0x0425, 0x0418, 0x0419, 0x041A, 0x041B, 0x041C, 0x041D, 0x041E,
    0x041F, 0x042F, 0x0420, 0x0421, 0x0422, 0x0423, 0x0416, 0x0412,
    0x042C, 0x042B, 0x0417, 0x0428, 0x042D, 0x0429, 0x0427, 0x042A,
];

/// Windows-1252 0x80–0x9F (0xA0–0xFF are Latin-1 itself).
#[rustfmt::skip]
static CP1252: [u16; 32] = [
    0x20AC, 0xFFFD, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021,
    0x02C6, 0x2030, 0x0160, 0x2039, 0x0152, 0xFFFD, 0x017D, 0xFFFD,
    0xFFFD, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014,
    0x02DC, 0x2122, 0x0161, 0x203A, 0x0153, 0xFFFD, 0x017E, 0x0178,
];

fn table_char(t: &[u16], i: u8) -> char {
    t.get(usize::from(i)).and_then(|&u| char::from_u32(u32::from(u))).unwrap_or(REPLACEMENT)
}

/// A streaming decoder: bytes in any pieces, `char`s out. Keeps at most
/// three bytes of an unfinished sequence between pieces (8 bytes of RAM).
#[derive(Clone, Copy, Debug)]
pub struct Decoder {
    enc: Encoding,
    pend: [u8; 4],
    n: u8,
    /// UTF-8: bytes the sequence needs in all. UTF-16: a high surrogate.
    need: u8,
    high: u16,
}

impl Decoder {
    #[must_use]
    pub const fn new(enc: Encoding) -> Decoder {
        Decoder { enc, pend: [0; 4], n: 0, need: 0, high: 0 }
    }

    #[must_use]
    pub fn encoding(&self) -> Encoding {
        self.enc
    }

    /// Decode `input`; `f` gets every complete character.
    pub fn feed(&mut self, input: &[u8], f: &mut impl FnMut(char)) {
        for &b in input {
            self.byte(b, f);
        }
    }

    /// End of input: an unfinished sequence becomes U+FFFD.
    pub fn finish(&mut self, f: &mut impl FnMut(char)) {
        if self.n > 0 || self.high != 0 {
            f(REPLACEMENT);
        }
        self.n = 0;
        self.need = 0;
        self.high = 0;
    }

    fn byte(&mut self, b: u8, f: &mut impl FnMut(char)) {
        match self.enc {
            Encoding::Utf8 => self.utf8(b, f),
            Encoding::Utf16Le | Encoding::Utf16Be => {
                if self.n == 0 {
                    self.pend[0] = b;
                    self.n = 1;
                    return;
                }
                self.n = 0;
                let u = if self.enc == Encoding::Utf16Le {
                    u16::from_le_bytes([self.pend[0], b])
                } else {
                    u16::from_be_bytes([self.pend[0], b])
                };
                self.unit16(u, f);
            }
            Encoding::Cp1251 | Encoding::Koi8r | Encoding::Latin1 => f(self.single(b)),
        }
    }

    fn single(&self, b: u8) -> char {
        if b < 0x80 {
            return char::from(b);
        }
        let i = b - 0x80;
        match self.enc {
            Encoding::Cp1251 => table_char(&CP1251, i),
            Encoding::Koi8r => table_char(&KOI8R, i),
            _ if b < 0xA0 => table_char(&CP1252, i),
            _ => char::from(b),
        }
    }

    fn unit16(&mut self, u: u16, f: &mut impl FnMut(char)) {
        match u {
            0xD800..=0xDBFF => {
                if self.high != 0 {
                    f(REPLACEMENT);
                }
                self.high = u;
            }
            0xDC00..=0xDFFF => {
                if self.high == 0 {
                    f(REPLACEMENT);
                } else {
                    let c = 0x10000 + ((u32::from(self.high) - 0xD800) << 10) + (u32::from(u) - 0xDC00);
                    f(char::from_u32(c).unwrap_or(REPLACEMENT));
                    self.high = 0;
                }
            }
            _ => {
                if self.high != 0 {
                    f(REPLACEMENT);
                    self.high = 0;
                }
                f(char::from_u32(u32::from(u)).unwrap_or(REPLACEMENT));
            }
        }
    }

    fn utf8(&mut self, b: u8, f: &mut impl FnMut(char)) {
        if self.n > 0 {
            let first = self.pend[0];
            let ok = match (first, self.n) {
                (0xE0, 1) => (0xA0..=0xBF).contains(&b),
                (0xED, 1) => (0x80..=0x9F).contains(&b),
                (0xF0, 1) => (0x90..=0xBF).contains(&b),
                (0xF4, 1) => (0x80..=0x8F).contains(&b),
                _ => (0x80..=0xBF).contains(&b),
            };
            if ok {
                self.pend[usize::from(self.n)] = b;
                self.n += 1;
                if self.n == self.need {
                    let s = core::str::from_utf8(&self.pend[..usize::from(self.n)]).ok();
                    f(s.and_then(|s| s.chars().next()).unwrap_or(REPLACEMENT));
                    self.n = 0;
                }
                return;
            }
            // A sequence broken off: one replacement, then `b` anew.
            f(REPLACEMENT);
            self.n = 0;
        }
        let need = match b {
            0x00..=0x7F => {
                f(char::from(b));
                return;
            }
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => {
                f(REPLACEMENT);
                return;
            }
        };
        self.pend[0] = b;
        self.n = 1;
        self.need = need;
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    fn decode(enc: Encoding, b: &[u8], piece: usize) -> String {
        let mut d = Decoder::new(enc);
        let mut s = String::new();
        let mut f = |c| s.push(c);
        for p in b.chunks(piece.max(1)) {
            d.feed(p, &mut f);
        }
        d.finish(&mut f);
        s
    }

    fn cp1251(s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| {
                if (c as u32) < 0x80 {
                    c as u8
                } else {
                    let i = CP1251.iter().position(|&u| u32::from(u) == c as u32).expect("in 1251");
                    0x80 + i as u8
                }
            })
            .collect()
    }

    fn koi8(s: &str) -> Vec<u8> {
        s.chars()
            .map(|c| {
                if (c as u32) < 0x80 {
                    c as u8
                } else {
                    let i = KOI8R.iter().position(|&u| u32::from(u) == c as u32).expect("in koi8");
                    0x80 + i as u8
                }
            })
            .collect()
    }

    const RU: &str =
        "Так говорила в июле 1805 года известная Анна Павловна Шерер, фрейлина и приближенная императрицы.";

    #[test]
    fn boms() {
        assert_eq!(detect(b"\xEF\xBB\xBFabc", false), (Encoding::Utf8, 3));
        assert_eq!(detect(b"\xFF\xFEa\0b\0", false), (Encoding::Utf16Le, 2));
        assert_eq!(detect(b"\xFE\xFF\0a\0b", false), (Encoding::Utf16Be, 2));
    }

    #[test]
    fn utf16_without_mark() {
        let le: Vec<u8> = "Hello, world".encode_utf16().flat_map(u16::to_le_bytes).collect();
        let be: Vec<u8> = RU.encode_utf16().flat_map(u16::to_be_bytes).collect();
        assert_eq!(detect(&le, true).0, Encoding::Utf16Le);
        assert_eq!(detect(&be, true).0, Encoding::Utf16Be);
        assert_eq!(decode(Encoding::Utf16Be, &be, 3), RU);
    }

    #[test]
    fn utf8_and_cut_heads() {
        let b = "Мир — ок".as_bytes();
        assert_eq!(detect(b, true).0, Encoding::Utf8);
        // Cut inside "—": still UTF-8 as a head, not as a whole file.
        let cut = &b[..8];
        assert!(is_utf8(cut, false) && !is_utf8(cut, true));
        assert_eq!(detect(cut, false).0, Encoding::Utf8);
        for piece in [1, 2, 3, 5, 64] {
            assert_eq!(decode(Encoding::Utf8, b, piece), "Мир — ок");
        }
    }

    #[test]
    fn cyrillic_code_pages() {
        let w = cp1251(RU);
        let k = koi8(RU);
        assert_eq!(detect(&w, true).0, Encoding::Cp1251);
        assert_eq!(detect(&k, true).0, Encoding::Koi8r);
        assert_eq!(decode(Encoding::Cp1251, &w, 7), RU);
        assert_eq!(decode(Encoding::Koi8r, &k, 1), RU);
    }

    #[test]
    fn latin1() {
        let b = b"Le c\xF4t\xE9 de chez Swann, \x93caf\xE9\x94 \x80";
        assert_eq!(detect(b, true).0, Encoding::Latin1);
        assert_eq!(decode(Encoding::Latin1, b, 4), "Le côté de chez Swann, \u{201C}café\u{201D} €");
    }

    #[test]
    fn xml_declaration() {
        let h = b"<?xml version=\"1.0\" encoding=\"windows-1251\"?>\n<FictionBook>";
        assert_eq!(xml_declared(h), Some(Encoding::Cp1251));
        assert_eq!(detect(h, true).0, Encoding::Cp1251);
        assert_eq!(xml_declared(b"  <?xml version='1.0' encoding='UTF-8'?>"), Some(Encoding::Utf8));
        assert_eq!(xml_declared(b"<?xml version='1.0'?>"), None);
        assert_eq!(xml_declared(b"<?xml encoding=\"klingon\"?>"), None);
        assert_eq!(xml_declared(b"<?xml encoding=\"koi8"), None, "cut");
    }

    /// Malformed input: every bad byte or broken sequence becomes one
    /// U+FFFD and decoding goes on with the next byte.
    #[test]
    fn malformed_utf8() {
        let b = b"a\xC3b\xE2\x82c\xF0\x9F\x98\xFF\xFEd\xED\xA0\x80e\xC0\xAF";
        let s = decode(Encoding::Utf8, b, 1);
        assert_eq!(
            s,
            "a\u{FFFD}b\u{FFFD}c\u{FFFD}\u{FFFD}\u{FFFD}d\u{FFFD}\u{FFFD}\u{FFFD}e\u{FFFD}\u{FFFD}"
        );
        assert_eq!(decode(Encoding::Utf8, b, 1), decode(Encoding::Utf8, b, 5), "piece size does not matter");
        assert_eq!(decode(Encoding::Utf8, b"ok\xE2\x82", 1), "ok\u{FFFD}", "cut at the end");
    }

    #[test]
    fn malformed_utf16() {
        // Lone low surrogate, lone high surrogate before a letter, odd byte.
        let mut b = Vec::new();
        for u in [0x0041u16, 0xDC00, 0xD83D, 0x0042, 0xD83D, 0xDE00] {
            b.extend(u.to_le_bytes());
        }
        b.push(0x43);
        assert_eq!(decode(Encoding::Utf16Le, &b, 3), "A\u{FFFD}\u{FFFD}B😀\u{FFFD}");
    }

    #[test]
    fn labels() {
        assert_eq!(Encoding::from_label(b"KOI8-R"), Some(Encoding::Koi8r));
        assert_eq!(Encoding::from_label(b"Windows-1251"), Some(Encoding::Cp1251));
        for e in Encoding::ALL {
            assert_eq!(Encoding::from_u8(e as u8), e);
        }
        assert_eq!(Encoding::from_u8(99), Encoding::Utf8);
    }
}
