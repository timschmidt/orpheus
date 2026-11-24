//! Tags at the end of a file: APEv2 (footer, then items) and ID3v1/1.1.
//! Both only fill fields a leading tag left empty.

use orpheus_audio::{Format, SeekSource};

use crate::io::{goto, peek};
use crate::text::{self, Encoding, ID3V1_GENRES, Text};
use crate::{
    BUF, Located, Lyrics, LyricsKind, MetaError, Mime, Opts, Picture, Progress, SCRATCH, Tags, apply_key,
    tryp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum T {
    Start,
    V1Probe,
    ApeFooter,
    Items { left: u32, end: u64 },
    V1,
    Done,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Tail {
    phase: T,
    len: u64,
    has_v1: bool,
}

impl Tail {
    pub(crate) fn new(_format: Format) -> Self {
        Self { phase: T::Start, len: 0, has_v1: false }
    }

    pub(crate) fn step(
        &mut self,
        src: &mut dyn SeekSource,
        tags: &mut Tags,
        opts: &Opts,
        scratch: &mut [u8; BUF],
    ) -> Result<Progress, MetaError> {
        loop {
            match self.phase {
                T::Start => {
                    let Some(len) = src.len() else { return Ok(Progress::Done) };
                    self.len = len;
                    self.phase = if len >= 128 { T::V1Probe } else { T::ApeFooter };
                    if len >= 128 && goto(src, len - 128).is_err() {
                        return Ok(Progress::Done);
                    }
                }
                T::V1Probe => {
                    let v = tryp!(peek(src, 3));
                    self.has_v1 = v.starts_with_at(0, b"TAG");
                    self.phase = T::ApeFooter;
                }
                T::ApeFooter => {
                    let footer = self.len.checked_sub(32 + if self.has_v1 { 128 } else { 0 });
                    let Some(footer) = footer else {
                        self.phase = T::V1;
                        continue;
                    };
                    if src.position() != footer && goto(src, footer).is_err() {
                        self.phase = T::V1;
                        continue;
                    }
                    let v = tryp!(peek(src, 32));
                    let size = u64::from(v.u32_le(12).unwrap_or(0));
                    let count = v.u32_le(16).unwrap_or(0);
                    let items = (footer + 32).checked_sub(size);
                    match items {
                        Some(start)
                            if v.starts_with_at(0, b"APETAGEX") && size >= 32 && goto(src, start).is_ok() =>
                        {
                            self.phase = T::Items { left: count, end: footer };
                        }
                        _ => self.phase = T::V1,
                    }
                }
                T::Items { left, end } => {
                    let pos = src.position();
                    if left == 0 || pos + 8 > end {
                        self.phase = T::V1;
                        continue;
                    }
                    let v = tryp!(peek(src, (end - pos).min(8 + 256 + SCRATCH as u64) as usize));
                    let vlen = u64::from(v.u32_le(0).unwrap_or(0));
                    let flags = v.u32_le(4).unwrap_or(0);
                    let mut key = [0u8; 256];
                    let mut kl = 0;
                    while kl < 255 && v.byte(8 + kl) != 0 && 8 + kl < v.len() {
                        key[kl] = v.byte(8 + kl);
                        kl += 1;
                    }
                    let vstart = pos + 8 + kl as u64 + 1;
                    let next = vstart + vlen;
                    if 8 + kl >= v.len() || next > end {
                        self.phase = T::V1;
                        continue;
                    }
                    let key = &key[..kl];
                    let want = (vlen as usize).min(SCRATCH);
                    let off = 8 + kl + 1;
                    let got = v.len().saturating_sub(off).min(want);
                    v.copy_to(off, &mut scratch[..got]);
                    let value = &scratch[..got];
                    match (flags >> 1) & 3 {
                        0 if key.eq_ignore_ascii_case(b"Lyrics") => {
                            if tags.lyrics.is_none() {
                                let at = Located { offset: vstart, skip: 0, len: vlen as u32, unsync: false };
                                tags.lyrics = Some(Lyrics { at, kind: LyricsKind::Utf8 });
                            }
                        }
                        0 => {
                            let t = Encoding::Utf8.decode(value, opts.code_page);
                            apply_key(tags, key, t.as_str(), true);
                        }
                        1 if key.eq_ignore_ascii_case(b"Cover Art (Front)") && tags.picture.is_none() => {
                            // "file name" NUL data
                            if let Some(n) = value.iter().position(|&c| c == 0) {
                                let ext = value[..n].rsplit(|&c| c == b'.').next().unwrap_or(&[]);
                                let at = Located {
                                    offset: vstart + n as u64 + 1,
                                    skip: 0,
                                    len: (vlen - n as u64 - 1) as u32,
                                    unsync: false,
                                };
                                tags.picture = Some(Picture { at, mime: Mime::from_bytes(ext), kind: 3 });
                            }
                        }
                        _ => {}
                    }
                    self.phase = T::Items { left: left - 1, end };
                    if goto(src, next).is_err() {
                        self.phase = T::V1;
                    }
                }
                T::V1 => {
                    if !self.has_v1 || goto(src, self.len - 128).is_err() {
                        self.phase = T::Done;
                        continue;
                    }
                    let v = tryp!(peek(src, 128));
                    let mut b = [0u8; 128];
                    v.copy_to(0, &mut b);
                    id3v1(&b, tags, opts);
                    self.phase = T::Done;
                }
                T::Done => return Ok(Progress::Done),
            }
        }
    }
}

/// ID3v1/1.1: fixed 30-byte fields, filling only empty ones.
fn id3v1(b: &[u8; 128], tags: &mut Tags, opts: &Opts) {
    let field = |r: core::ops::Range<usize>| Encoding::Latin1.decode(&b[r], opts.code_page);
    let fill = |t: &mut Text, v: Text| {
        if t.is_empty() {
            *t = v;
        }
    };
    fill(&mut tags.title, field(3..33));
    fill(&mut tags.artist, field(33..63));
    fill(&mut tags.album, field(63..93));
    if tags.year == 0 {
        let y = field(93..97);
        tags.year = text::leading_u32(y.as_str()).unwrap_or(0).min(9999) as u16;
    }
    if b[125] == 0 && b[126] != 0 && tags.track == 0 {
        tags.track = u16::from(b[126]);
    }
    if tags.genre.is_empty()
        && let Some(g) = ID3V1_GENRES.get(usize::from(b[127]))
    {
        tags.genre = Text::from_chars(g.chars());
    }
}
