//! Tags inside RIFF/WAVE (`LIST`/`INFO` text chunks, an `id3 ` chunk) and
//! AIFF (`NAME`, `AUTH`, an `ID3 ` chunk).

use yoton_audio::SeekSource;

use crate::id3::Id3;
use crate::io::{goto, peek};
use crate::text::{Encoding, Text};
use crate::{BUF, MetaError, Opts, Progress, SCRATCH, Tags, apply_key, tryp};

#[derive(Debug, Clone, Copy)]
enum R {
    Magic,
    Chunk,
    /// Inside a `LIST/INFO` chunk ending at `end`.
    Info {
        end: u64,
    },
    Id3 {
        tag: Id3,
        end: u64,
    },
    SkipTo(u64),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Riff {
    aiff: bool,
    phase: R,
}

fn pad(n: u64) -> u64 {
    n + (n & 1)
}

impl Riff {
    pub(crate) fn new(aiff: bool) -> Self {
        Self { aiff, phase: R::Magic }
    }

    fn size(&self, b: [u8; 4]) -> u64 {
        u64::from(if self.aiff { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
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
                R::Magic => {
                    let v = tryp!(peek(src, 12));
                    let ok = if self.aiff {
                        v.starts_with_at(0, b"FORM")
                    } else {
                        v.starts_with_at(0, b"RIFF") && v.starts_with_at(8, b"WAVE")
                    };
                    if !ok {
                        return Ok(Progress::Done);
                    }
                    src.consume(12);
                    self.phase = R::Chunk;
                }
                R::Chunk => {
                    let v = match peek(src, 12) {
                        Err(crate::io::Pending::Eof) => tryp!(peek(src, 8)),
                        r => tryp!(r),
                    };
                    let id: [u8; 4] = v.array(0).unwrap_or_default();
                    let size = self.size(v.array(4).unwrap_or_default());
                    let body = src.position() + 8;
                    let end = body + pad(size);
                    match (&id, self.aiff) {
                        (b"LIST", false) if v.starts_with_at(8, b"INFO") => {
                            src.consume(12);
                            self.phase = R::Info { end: body + size };
                        }
                        (b"id3 " | b"ID3 ", _) => {
                            src.consume(8);
                            self.phase = R::Id3 { tag: Id3::new(body), end };
                        }
                        (b"NAME" | b"AUTH", true) => {
                            let want = (size as usize).min(SCRATCH);
                            let v = tryp!(peek(src, 8 + want));
                            v.copy_to(8, &mut scratch[..want]);
                            let t = Encoding::Latin1.decode(&scratch[..want], opts.code_page);
                            let field = if &id == b"NAME" { &mut tags.title } else { &mut tags.artist };
                            if field.is_empty() {
                                *field = t;
                            }
                            self.phase = R::SkipTo(end);
                        }
                        _ => self.phase = R::SkipTo(end),
                    }
                }
                R::Info { end } => {
                    let pos = src.position();
                    if pos + 8 > end {
                        self.phase = R::SkipTo(pad(end));
                        continue;
                    }
                    let v = tryp!(peek(src, 8));
                    let id: [u8; 4] = v.array(0).unwrap_or_default();
                    let size = u64::from(v.u32_le(4).unwrap_or(0));
                    let want = (size.min(end - pos - 8) as usize).min(SCRATCH);
                    let v = tryp!(peek(src, 8 + want));
                    v.copy_to(8, &mut scratch[..want]);
                    let t: Text = Encoding::Latin1.decode(&scratch[..want], opts.code_page);
                    let key: &[u8] = match &id {
                        b"INAM" => b"TITLE",
                        b"IART" => b"ARTIST",
                        b"IPRD" => b"ALBUM",
                        b"IGNR" => b"GENRE",
                        b"ICRD" => b"DATE",
                        b"ITRK" | b"IPRT" => b"TRACKNUMBER",
                        _ => b"",
                    };
                    apply_key(tags, key, t.as_str(), true);
                    if goto(src, pos + 8 + pad(size)).is_err() {
                        return Ok(Progress::Done);
                    }
                }
                R::Id3 { mut tag, end } => {
                    let p = tag.step(src, tags, opts, scratch, false)?;
                    self.phase = R::Id3 { tag, end };
                    match p {
                        Progress::Need(n) => return Ok(Progress::Need(n)),
                        Progress::Done => self.phase = R::SkipTo(end),
                    }
                }
                R::SkipTo(pos) => {
                    if goto(src, pos).is_err() || crate::at_eof(src) {
                        return Ok(Progress::Done);
                    }
                    self.phase = R::Chunk;
                }
            }
        }
    }
}
