//! Bounded text fields, character-set conversion and the small numeric
//! parsers tags need (no floating point).

/// A UTF-8 string of at most 127 bytes (04 §4.4), truncated at a character
/// boundary.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text {
    buf: [u8; Text::CAP],
    len: u8,
}

impl Default for Text {
    fn default() -> Self {
        Self { buf: [0; Self::CAP], len: 0 }
    }
}

impl core::fmt::Debug for Text {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl Text {
    pub const CAP: usize = 127;

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only whole characters are ever pushed.
        core::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Append a character; `false` (and nothing appended) when it does not
    /// fit.
    pub fn push(&mut self, c: char) -> bool {
        let mut tmp = [0u8; 4];
        let s = c.encode_utf8(&mut tmp);
        let l = usize::from(self.len);
        if l + s.len() > Self::CAP {
            return false;
        }
        self.buf[l..l + s.len()].copy_from_slice(s.as_bytes());
        self.len += s.len() as u8;
        true
    }

    /// Text from a decoded string, trimmed of surrounding whitespace.
    #[must_use]
    pub fn from_chars(it: impl Iterator<Item = char>) -> Self {
        let mut t = Self::default();
        let mut pending_space = 0usize;
        for c in it {
            if c == '\0' {
                break;
            }
            if c.is_whitespace() {
                if !t.is_empty() {
                    pending_space += 1;
                }
                continue;
            }
            for _ in 0..pending_space {
                if !t.push(' ') {
                    return t;
                }
            }
            pending_space = 0;
            if !t.push(c) {
                break;
            }
        }
        t
    }
}

/// The 8-bit character set assumed for ID3 "ISO-8859-1" text and LRC files
/// that are not UTF-8 (04 §4.4: configurable code page).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodePage {
    /// ISO-8859-1 (what the ID3 standard says).
    #[default]
    Latin1,
    /// Windows-1252 (Western; what most "Latin-1" tags really are).
    Windows1252,
    /// Windows-1251 (Cyrillic).
    Windows1251,
}

const CP1252_80: [u16; 32] = [
    0x20AC, 0x81, 0x201A, 0x0192, 0x201E, 0x2026, 0x2020, 0x2021, 0x02C6, 0x2030, 0x0160, 0x2039, 0x0152,
    0x8D, 0x017D, 0x8F, 0x90, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x02DC, 0x2122, 0x0161,
    0x203A, 0x0153, 0x9D, 0x017E, 0x0178,
];

const CP1251_80: [u16; 64] = [
    0x0402, 0x0403, 0x201A, 0x0453, 0x201E, 0x2026, 0x2020, 0x2021, 0x20AC, 0x2030, 0x0409, 0x2039, 0x040A,
    0x040C, 0x040B, 0x040F, 0x0452, 0x2018, 0x2019, 0x201C, 0x201D, 0x2022, 0x2013, 0x2014, 0x98, 0x2122,
    0x0459, 0x203A, 0x045A, 0x045C, 0x045B, 0x045F, 0x00A0, 0x040E, 0x045E, 0x0408, 0x00A4, 0x0490, 0x00A6,
    0x00A7, 0x0401, 0x00A9, 0x0404, 0x00AB, 0x00AC, 0x00AD, 0x00AE, 0x0407, 0x00B0, 0x00B1, 0x0406, 0x0456,
    0x0491, 0x00B5, 0x00B6, 0x00B7, 0x0451, 0x2116, 0x0454, 0x00BB, 0x0458, 0x0405, 0x0455, 0x0457,
];

impl CodePage {
    #[must_use]
    pub fn decode(self, b: u8) -> char {
        let cp = match (self, b) {
            (_, 0..=0x7F) | (Self::Latin1, _) => u32::from(b),
            (Self::Windows1252, 0x80..=0x9F) => u32::from(CP1252_80[usize::from(b - 0x80)]),
            (Self::Windows1252, _) => u32::from(b),
            (Self::Windows1251, 0x80..=0xBF) => u32::from(CP1251_80[usize::from(b - 0x80)]),
            // А..я
            (Self::Windows1251, _) => 0x0410 + u32::from(b - 0xC0),
        };
        char::from_u32(cp).unwrap_or('\u{FFFD}')
    }
}

/// ID3v2 text encodings (and the same set for other formats).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoding {
    /// 8-bit, in the configured code page.
    Latin1,
    /// UTF-16 with a byte-order mark (little-endian if none).
    Utf16,
    Utf16Be,
    Utf8,
}

impl Encoding {
    #[must_use]
    pub fn from_id3(b: u8) -> Option<Self> {
        match b {
            0 => Some(Self::Latin1),
            1 => Some(Self::Utf16),
            2 => Some(Self::Utf16Be),
            3 => Some(Self::Utf8),
            _ => None,
        }
    }

    /// Bytes of a terminating NUL.
    #[must_use]
    pub fn nul_len(self) -> usize {
        match self {
            Self::Utf16 | Self::Utf16Be => 2,
            _ => 1,
        }
    }

    /// Length of the NUL-terminated string at the start of `b` and the
    /// offset after its terminator (`None` if no terminator in `b`).
    #[must_use]
    pub fn terminated(self, b: &[u8]) -> Option<(usize, usize)> {
        let n = self.nul_len();
        let mut i = 0;
        while i + n <= b.len() {
            if b[i..i + n].iter().all(|&x| x == 0) {
                return Some((i, i + n));
            }
            i += n;
        }
        None
    }

    /// Decode `b` (up to the first NUL) into a bounded text.
    #[must_use]
    pub fn decode(self, b: &[u8], cp: CodePage) -> Text {
        match self {
            Self::Latin1 => Text::from_chars(b.iter().map(|&x| cp.decode(x))),
            Self::Utf8 => {
                let b = b.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(b);
                Text::from_chars(Utf8Chars { b })
            }
            Self::Utf16 | Self::Utf16Be => {
                let (be, b) = match b {
                    [0xFE, 0xFF, rest @ ..] => (true, rest),
                    [0xFF, 0xFE, rest @ ..] => (false, rest),
                    _ => (self == Self::Utf16Be, b),
                };
                let units = b
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&c| if be { u16::from_be_bytes(c) } else { u16::from_le_bytes(c) });
                Text::from_chars(char::decode_utf16(units).map(|r| r.unwrap_or('\u{FFFD}')))
            }
        }
    }
}

/// Lossy UTF-8 decoding without allocation (invalid sequences become
/// U+FFFD, one per bad byte).
struct Utf8Chars<'a> {
    b: &'a [u8],
}

impl Iterator for Utf8Chars<'_> {
    type Item = char;
    fn next(&mut self) -> Option<char> {
        let first = *self.b.first()?;
        let n = match first {
            0x00..=0x7F => 1,
            0xC2..=0xDF => 2,
            0xE0..=0xEF => 3,
            0xF0..=0xF4 => 4,
            _ => 0,
        };
        if n > 0
            && let Some(s) = self.b.get(..n)
            && let Ok(st) = core::str::from_utf8(s)
            && let Some(c) = st.chars().next()
        {
            self.b = &self.b[n..];
            return Some(c);
        }
        self.b = &self.b[1..];
        Some('\u{FFFD}')
    }
}

/// Leading decimal number of `s` (after optional spaces); `None` if none.
#[must_use]
pub fn leading_u32(s: &str) -> Option<u32> {
    let s = s.trim_start();
    let digits = s.bytes().take_while(u8::is_ascii_digit).take(9);
    let mut v: Option<u32> = None;
    for d in digits {
        v = Some(v.unwrap_or(0) * 10 + u32::from(d - b'0'));
    }
    v
}

/// "3/12" → (3, 12); "7" → (7, 0).
#[must_use]
pub fn number_pair(s: &str) -> (u16, u16) {
    let mut it = s.splitn(2, '/');
    let a = it.next().and_then(leading_u32).unwrap_or(0);
    let b = it.next().and_then(leading_u32).unwrap_or(0);
    (a.min(u32::from(u16::MAX)) as u16, b.min(u32::from(u16::MAX)) as u16)
}

/// A decimal like "-6.48 dB" or "0.988525" as an integer scaled by
/// `10^frac_digits` (rounded towards zero), without floating point.
#[must_use]
pub fn decimal(s: &str, frac_digits: u32) -> Option<i64> {
    let s = s.trim();
    let (neg, s) = match s.as_bytes().first()? {
        b'-' => (true, &s[1..]),
        b'+' => (false, &s[1..]),
        _ => (false, s),
    };
    let mut int: i64 = 0;
    let mut frac: i64 = 0;
    let mut seen = false;
    let mut digits = 0u32;
    let mut in_frac = false;
    for c in s.bytes() {
        match c {
            b'0'..=b'9' => {
                seen = true;
                if in_frac {
                    if digits < frac_digits {
                        frac = frac * 10 + i64::from(c - b'0');
                        digits += 1;
                    }
                } else if int < 1_000_000_000 {
                    int = int * 10 + i64::from(c - b'0');
                }
            }
            b'.' | b',' if !in_frac => in_frac = true,
            _ => break,
        }
    }
    if !seen {
        return None;
    }
    while digits < frac_digits {
        frac *= 10;
        digits += 1;
    }
    let v = int * 10i64.pow(frac_digits) + frac;
    Some(if neg { -v } else { v })
}

/// The 80 genres of ID3v1.
pub const ID3V1_GENRES: [&str; 80] = [
    "Blues",
    "Classic Rock",
    "Country",
    "Dance",
    "Disco",
    "Funk",
    "Grunge",
    "Hip-Hop",
    "Jazz",
    "Metal",
    "New Age",
    "Oldies",
    "Other",
    "Pop",
    "R&B",
    "Rap",
    "Reggae",
    "Rock",
    "Techno",
    "Industrial",
    "Alternative",
    "Ska",
    "Death Metal",
    "Pranks",
    "Soundtrack",
    "Euro-Techno",
    "Ambient",
    "Trip-Hop",
    "Vocal",
    "Jazz+Funk",
    "Fusion",
    "Trance",
    "Classical",
    "Instrumental",
    "Acid",
    "House",
    "Game",
    "Sound Clip",
    "Gospel",
    "Noise",
    "AlternRock",
    "Bass",
    "Soul",
    "Punk",
    "Space",
    "Meditative",
    "Instrumental Pop",
    "Instrumental Rock",
    "Ethnic",
    "Gothic",
    "Darkwave",
    "Techno-Industrial",
    "Electronic",
    "Pop-Folk",
    "Eurodance",
    "Dream",
    "Southern Rock",
    "Comedy",
    "Cult",
    "Gangsta",
    "Top 40",
    "Christian Rap",
    "Pop/Funk",
    "Jungle",
    "Native American",
    "Cabaret",
    "New Wave",
    "Psychadelic",
    "Rave",
    "Showtunes",
    "Trailer",
    "Lo-Fi",
    "Tribal",
    "Acid Punk",
    "Acid Jazz",
    "Polka",
    "Retro",
    "Musical",
    "Rock & Roll",
    "Hard Rock",
];

/// ID3v2 `TCON`: "(17)", "(17)Rock", "17" or a name.
#[must_use]
pub fn genre(t: &Text) -> Text {
    let s = t.as_str();
    let inner = s.strip_prefix('(').and_then(|r| r.split_once(')'));
    let (num, rest) = match inner {
        Some((n, rest)) => (leading_u32(n), rest),
        None => (s.parse::<u32>().ok(), ""),
    };
    if !rest.trim().is_empty() {
        return Text::from_chars(rest.chars());
    }
    match num.and_then(|n| ID3V1_GENRES.get(n as usize)) {
        Some(g) => Text::from_chars(g.chars()),
        None => *t,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn truncation_and_codepages() {
        let long: std::string::String = core::iter::repeat_n('é', 100).collect();
        let t = Text::from_chars(long.chars());
        assert_eq!(t.as_str().len(), 126);
        assert_eq!(Encoding::Latin1.decode(b"Caf\xE9\0junk", CodePage::Latin1).as_str(), "Café");
        assert_eq!(
            Encoding::Latin1.decode(b"\xCF\xF0\xE8\xE2\xE5\xF2", CodePage::Windows1251).as_str(),
            "Привет"
        );
        assert_eq!(Encoding::Latin1.decode(b"\x80 5", CodePage::Windows1252).as_str(), "€ 5");
        assert_eq!(Encoding::Utf16.decode(b"\xFF\xFEA\0b\0", CodePage::Latin1).as_str(), "Ab");
        assert_eq!(Encoding::Utf16Be.decode(b"\0A\0b", CodePage::Latin1).as_str(), "Ab");
        assert_eq!(Encoding::Utf8.decode("  ünï  côde ".as_bytes(), CodePage::Latin1).as_str(), "ünï  côde");
        assert_eq!(Encoding::Utf8.decode(b"a\xFFb", CodePage::Latin1).as_str(), "a\u{FFFD}b");
    }

    #[test]
    fn numbers() {
        assert_eq!(number_pair("3/12"), (3, 12));
        assert_eq!(number_pair(" 7"), (7, 0));
        assert_eq!(decimal("-6.48 dB", 2), Some(-648));
        assert_eq!(decimal("+1.5 dB", 2), Some(150));
        assert_eq!(decimal("0.988525", 6), Some(988_525));
        assert_eq!(decimal("dB", 2), None);
        assert_eq!(genre(&Text::from_chars("(17)".chars())).as_str(), "Rock");
        assert_eq!(genre(&Text::from_chars("(17)Krautrock".chars())).as_str(), "Krautrock");
        assert_eq!(genre(&Text::from_chars("Jazz".chars())).as_str(), "Jazz");
    }

    #[test]
    fn terminators() {
        assert_eq!(Encoding::Utf16.terminated(b"a\0\0\0x"), Some((2, 4)));
        assert_eq!(Encoding::Latin1.terminated(b"ab\0c"), Some((2, 3)));
        assert_eq!(Encoding::Utf8.terminated(b"abc"), None);
    }
}
