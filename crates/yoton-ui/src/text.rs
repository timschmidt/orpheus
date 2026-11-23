//! Text helpers without `core::fmt`: a fixed-capacity string, number
//! formatting, and visual ordering for right-to-left runs.

/// A UTF-8 string with fixed capacity `N` bytes, stored inline.
#[derive(Clone, Copy)]
pub struct StrBuf<const N: usize> {
    buf: [u8; N],
    len: u8,
}

impl<const N: usize> Default for StrBuf<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> core::fmt::Debug for StrBuf<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<const N: usize> Eq for StrBuf<N> {}

/// Build from a string, truncated at a character boundary if too long.
impl<const N: usize> From<&str> for StrBuf<N> {
    fn from(s: &str) -> Self {
        let mut b = Self::new();
        b.push_str(s);
        b
    }
}

impl<const N: usize> PartialEq for StrBuf<N> {
    fn eq(&self, o: &Self) -> bool {
        self.as_str() == o.as_str()
    }
}

impl<const N: usize> StrBuf<N> {
    #[must_use]
    pub const fn new() -> Self {
        assert!(N <= 255, "StrBuf capacity is at most 255 bytes");
        StrBuf { buf: [0; N], len: 0 }
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only whole characters are ever pushed, so this cannot fail.
        self.buf.get(..usize::from(self.len)).and_then(|b| core::str::from_utf8(b).ok()).unwrap_or("")
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    pub fn clear(&mut self) {
        self.len = 0;
    }

    /// Append as much of `s` as fits, whole characters only. Returns false if
    /// anything was dropped.
    pub fn push_str(&mut self, s: &str) -> bool {
        for c in s.chars() {
            if !self.push(c) {
                return false;
            }
        }
        true
    }

    pub fn push(&mut self, c: char) -> bool {
        let mut tmp = [0u8; 4];
        let enc = c.encode_utf8(&mut tmp).as_bytes();
        let at = usize::from(self.len);
        match self.buf.get_mut(at..at + enc.len()) {
            Some(dst) => {
                dst.copy_from_slice(enc);
                self.len = u8::try_from(at + enc.len()).unwrap_or(u8::MAX);
                true
            }
            None => false,
        }
    }

    /// Append a decimal number.
    pub fn push_u32(&mut self, v: u32) -> bool {
        let mut digits = [0u8; 10];
        let mut n = 0;
        let mut v = v;
        loop {
            digits[n] = b'0' + (v % 10) as u8;
            n += 1;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let mut ok = true;
        while n > 0 {
            n -= 1;
            ok &= self.push(char::from(digits[n]));
        }
        ok
    }

    pub fn push_i32(&mut self, v: i32) -> bool {
        if v < 0 {
            self.push('-') && self.push_u32(v.unsigned_abs())
        } else {
            self.push_u32(v.unsigned_abs())
        }
    }

    /// Append `v` with at least `width` digits, zero padded.
    pub fn push_u32_padded(&mut self, v: u32, width: u8) -> bool {
        let mut digits = 1u8;
        let mut t = v / 10;
        while t > 0 {
            digits += 1;
            t /= 10;
        }
        let mut ok = true;
        for _ in digits..width {
            ok &= self.push('0');
        }
        ok && self.push_u32(v)
    }

    /// Append a duration as `M:SS` or `H:MM:SS`.
    pub fn push_duration(&mut self, secs: u32) -> bool {
        let (h, m, s) = (secs / 3600, (secs / 60) % 60, secs % 60);
        let mut ok = true;
        if h > 0 {
            ok &= self.push_u32(h) && self.push(':') && self.push_u32_padded(m, 2);
        } else {
            ok &= self.push_u32(m);
        }
        ok && self.push(':') && self.push_u32_padded(s, 2)
    }
}

/// True for characters of right-to-left scripts (Hebrew, Arabic and their
/// presentation forms). No bidirectional algorithm in v1 (05 §4): a run of
/// such characters is shown reversed as a whole.
#[must_use]
pub fn is_rtl(c: char) -> bool {
    matches!(u32::from(c), 0x0590..=0x08FF | 0xFB1D..=0xFDFF | 0xFE70..=0xFEFF)
}

/// Characters that may sit inside an RTL run without ending it.
fn is_neutral(c: char) -> bool {
    c == ' ' || c == '\u{a0}' || (c.is_ascii_punctuation() && !matches!(c, '(' | ')' | '[' | ']'))
}

/// Call `f` for each character of `s` in visual (left-to-right drawing)
/// order: right-to-left runs are reversed. No allocation, no buffer.
pub fn for_each_visual(s: &str, mut f: impl FnMut(char) -> bool) {
    let mut rest = s;
    while let Some(c) = rest.chars().next() {
        if is_rtl(c) {
            // The run extends over RTL and neutral characters and ends after
            // the last RTL character.
            let mut end = 0;
            for (i, ch) in rest.char_indices() {
                if is_rtl(ch) {
                    end = i + ch.len_utf8();
                } else if !is_neutral(ch) {
                    break;
                }
            }
            let (run, tail) = rest.split_at(end);
            for ch in run.chars().rev() {
                if !f(ch) {
                    return;
                }
            }
            rest = tail;
        } else {
            if !f(c) {
                return;
            }
            rest = &rest[c.len_utf8()..];
        }
    }
}

/// Characters after which, or before which, a line may break in wrapped
/// text: spaces, and on either side of CJK ideographs, kana and Hangul.
#[must_use]
pub fn is_cjk(c: char) -> bool {
    matches!(u32::from(c), 0x2E80..=0x9FFF | 0xAC00..=0xD7AF | 0xF900..=0xFAFF | 0xFF00..=0xFFEF)
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::String;

    #[test]
    fn strbuf() {
        let mut b = StrBuf::<8>::new();
        assert!(b.push_str("ab"));
        assert!(b.push_u32(120));
        assert_eq!(b.as_str(), "ab120");
        assert!(!b.push_str("Жжж"));
        assert_eq!(b.as_str(), "ab120Ж");
        let mut d = StrBuf::<16>::new();
        d.push_duration(3725);
        assert_eq!(d.as_str(), "1:02:05");
        d.clear();
        d.push_duration(65);
        assert_eq!(d.as_str(), "1:05");
        d.clear();
        d.push_i32(-7);
        d.push_u32_padded(5, 3);
        assert_eq!(d.as_str(), "-7005");
    }

    #[test]
    fn rtl_runs() {
        let mut out = String::new();
        for_each_visual("ab שלום עולם cd", |c| {
            out.push(c);
            true
        });
        assert_eq!(out, "ab םלוע םולש cd");
    }
}
