//! LRC lyric files (`.lrc` next to the track, or LRC text in a lyrics tag):
//! `[mm:ss.xx]text` lines, several time tags per line, `[offset:±ms]`.
//! Lines are read one at a time (at most [`MAX_LINE`] bytes examined);
//! each is UTF-8 if it decodes as UTF-8, else the configured code page.

use orpheus_audio::{ByteSource, View};

use crate::text::{CodePage, Encoding, Text};

/// Longest line examined; longer lines are cut.
pub const MAX_LINE: usize = 512;
/// Time tags kept per line.
pub const MAX_TIMES: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct LrcLine {
    /// Times in milliseconds (offset applied), `n` of them.
    pub times: [u32; MAX_TIMES],
    pub n: u8,
    pub text: Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LrcStep {
    Line(LrcLine),
    NeedData(usize),
    Done,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct LrcReader {
    offset_ms: i32,
    cp: CodePage,
    /// Bytes of an over-long line still to skip.
    skipping: bool,
}

/// `mm:ss`, `mm:ss.f`, `mm:ss.ff`, `mm:ss.fff` (also `:` before the
/// fraction) → milliseconds.
fn time(t: &[u8]) -> Option<u32> {
    let colon = t.iter().position(|&c| c == b':')?;
    let (m, rest) = (&t[..colon], &t[colon + 1..]);
    let num = |d: &[u8]| -> Option<u32> {
        if d.is_empty() || d.len() > 6 || !d.iter().all(u8::is_ascii_digit) {
            return None;
        }
        Some(d.iter().fold(0u32, |a, &c| a * 10 + u32::from(c - b'0')))
    };
    let sep = rest.iter().position(|&c| c == b'.' || c == b':');
    let (s, frac) = match sep {
        Some(i) => (&rest[..i], Some(&rest[i + 1..])),
        None => (rest, None),
    };
    let ms = match frac {
        Some(f) => {
            let v = num(f)?;
            match f.len() {
                1 => v * 100,
                2 => v * 10,
                3 => v,
                _ => return None,
            }
        }
        None => 0,
    };
    num(m)?.checked_mul(60_000)?.checked_add(num(s)?.checked_mul(1000)?)?.checked_add(ms)
}

impl LrcReader {
    #[must_use]
    pub fn new(cp: CodePage) -> Self {
        Self { offset_ms: 0, cp, skipping: false }
    }

    /// Parse one line (without its line break). `None` for lines with no
    /// time tag (metadata such as `[ar:…]`, blank lines); `[offset:…]` is
    /// remembered and applies to the lines after it.
    pub fn parse_line(&mut self, line: &[u8]) -> Option<LrcLine> {
        let line = line.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(line);
        let line = line.strip_suffix(b"\r").unwrap_or(line);
        let mut out = LrcLine::default();
        let mut rest = line;
        while let [b'[', tail @ ..] = rest {
            let Some(close) = tail.iter().position(|&c| c == b']') else { break };
            let tag = &tail[..close];
            if let Some(t) = time(tag) {
                if usize::from(out.n) < MAX_TIMES {
                    let shifted = i64::from(t) - i64::from(self.offset_ms);
                    out.times[usize::from(out.n)] = shifted.clamp(0, i64::from(u32::MAX)) as u32;
                    out.n += 1;
                }
            } else if tag.len() > 7 && tag[..7].eq_ignore_ascii_case(b"offset:") {
                let v = crate::text::decimal(core::str::from_utf8(&tag[7..]).unwrap_or(""), 0);
                self.offset_ms = v.unwrap_or(0).clamp(-3_600_000, 3_600_000) as i32;
            }
            rest = &tail[close + 1..];
        }
        if out.n == 0 {
            return None;
        }
        let enc = if core::str::from_utf8(rest).is_ok() { Encoding::Utf8 } else { Encoding::Latin1 };
        out.text = enc.decode(rest, self.cp);
        Some(out)
    }

    /// The next timed line from `src`.
    pub fn next(&mut self, src: &mut dyn ByteSource) -> LrcStep {
        loop {
            let avail = src.available();
            if avail == 0 {
                return if src.at_end() { LrcStep::Done } else { LrcStep::NeedData(1) };
            }
            let v = View::peek(src, avail.min(MAX_LINE)).unwrap_or_default();
            let nl = (0..v.len()).find(|&i| v.byte(i) == b'\n');
            let (len, consume) = match nl {
                Some(i) => (i, i + 1),
                None if v.len() >= MAX_LINE => (MAX_LINE, MAX_LINE),
                None if src.at_end() => (v.len(), v.len()),
                None => return LrcStep::NeedData(avail + 1),
            };
            let mut buf = [0u8; MAX_LINE];
            v.copy_to(0, &mut buf[..len]);
            let skipping = self.skipping;
            // A cut line's remainder is skipped up to its line break.
            self.skipping = nl.is_none() && len == MAX_LINE;
            src.consume(consume);
            if skipping {
                continue;
            }
            if let Some(l) = self.parse_line(&buf[..len]) {
                return LrcStep::Line(l);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_audio::SliceSource;

    #[test]
    fn lines_and_offset() {
        let file = "\u{feff}[ti:Song]\r\n[00:01.50]One\r\n[00:02.00][01:00.123]Two two\n[offset:+500]\n[00:03]Three\nnot a line\n[00:04.5]Ünïcode";
        let mut src = SliceSource::ring(file.as_bytes(), 16 * 1024);
        let mut r = LrcReader::new(CodePage::Latin1);
        let mut got = std::vec::Vec::new();
        loop {
            match r.next(&mut src) {
                LrcStep::Line(l) => got
                    .push((l.times[..usize::from(l.n)].to_vec(), std::string::String::from(l.text.as_str()))),
                LrcStep::NeedData(_) => src.refill(),
                LrcStep::Done => break,
            }
        }
        assert_eq!(got.len(), 4);
        assert_eq!(got[0], (std::vec![1500], "One".into()));
        assert_eq!(got[1], (std::vec![2000, 60123], "Two two".into()));
        assert_eq!(got[2], (std::vec![2500], "Three".into()));
        assert_eq!(got[3], (std::vec![4000], "Ünïcode".into()));
    }

    #[test]
    fn latin1_fallback() {
        let mut r = LrcReader::new(CodePage::Windows1251);
        let l = r.parse_line(b"[00:00.00]\xCF\xF0\xE8").unwrap();
        assert_eq!(l.text.as_str(), "При");
    }
}
