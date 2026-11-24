//! ID3 `SYLT` (synchronised lyrics) frame bodies: encoding, language,
//! time-stamp format, content type, descriptor, then (text, time) pairs.
//! Start the reader at [`crate::Lyrics::at`] of a `Synced` lyrics location
//! (`at.unsync` set: the caller removes unsynchronisation first).

use orpheus_audio::{ByteSource, View};

use crate::text::{CodePage, Encoding, Text};

const MAX_ITEM: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyltEntry {
    /// Milliseconds if [`SyltReader::ms`], else MPEG frames.
    pub time: u32,
    pub text: Text,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SyltStep {
    Entry(SyltEntry),
    NeedData(usize),
    Done,
}

#[derive(Debug, Clone, Copy)]
pub struct SyltReader {
    left: u64,
    enc: Option<Encoding>,
    /// Time stamps are milliseconds (format 2); else MPEG frames (1).
    pub ms: bool,
    cp: CodePage,
}

impl SyltReader {
    /// A reader for a frame body of `len` bytes.
    #[must_use]
    pub fn new(len: u32, cp: CodePage) -> Self {
        Self { left: u64::from(len), enc: None, ms: true, cp }
    }

    pub fn next(&mut self, src: &mut dyn ByteSource) -> SyltStep {
        loop {
            if let Some(step) = self.item(src) {
                return step;
            }
        }
    }

    /// One header or entry; `None` after the header (read on).
    fn item(&mut self, src: &mut dyn ByteSource) -> Option<SyltStep> {
        let want = (self.left as usize).min(MAX_ITEM + 6);
        if want == 0 {
            return Some(SyltStep::Done);
        }
        let Some(v) = View::peek(src, want) else {
            return Some(if src.at_end() { SyltStep::Done } else { SyltStep::NeedData(want) });
        };
        let mut b = [0u8; MAX_ITEM + 6];
        v.copy_to(0, &mut b[..want]);
        let b = &b[..want];
        let Some(enc) = self.enc else {
            let Some(enc) = b.first().copied().and_then(Encoding::from_id3) else {
                self.left = 0;
                return Some(SyltStep::Done);
            };
            let Some((_, after)) = b.get(6..).and_then(|d| enc.terminated(d)) else {
                self.left = 0;
                return Some(SyltStep::Done);
            };
            self.ms = b[4] == 2;
            self.enc = Some(enc);
            src.consume(6 + after);
            self.left -= (6 + after) as u64;
            return None;
        };
        let Some((tl, after)) = enc.terminated(b) else {
            self.left = 0;
            return Some(SyltStep::Done);
        };
        let Some(t) = b.get(after..after + 4) else {
            self.left = 0;
            return Some(SyltStep::Done);
        };
        let time = u32::from_be_bytes([t[0], t[1], t[2], t[3]]);
        let text = enc.decode(&b[..tl], self.cp);
        src.consume(after + 4);
        self.left -= (after + 4) as u64;
        Some(SyltStep::Entry(SyltEntry { time, text }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use orpheus_audio::SliceSource;

    #[test]
    fn entries() {
        let mut body = std::vec::Vec::new();
        body.extend_from_slice(&[3, b'e', b'n', b'g', 2, 1]);
        body.extend_from_slice(b"desc\0");
        for (t, s) in [(1000u32, "one"), (2500, "two")] {
            body.extend_from_slice(s.as_bytes());
            body.push(0);
            body.extend_from_slice(&t.to_be_bytes());
        }
        let mut src = SliceSource::new(&body);
        let mut r = SyltReader::new(body.len() as u32, CodePage::Latin1);
        let mut got = std::vec::Vec::new();
        while let SyltStep::Entry(e) = r.next(&mut src) {
            got.push((e.time, std::string::String::from(e.text.as_str())));
        }
        assert!(r.ms);
        assert_eq!(got, std::vec![(1000, "one".into()), (2500, "two".into())]);
    }
}
