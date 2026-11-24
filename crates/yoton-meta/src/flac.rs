//! FLAC metadata blocks: VORBIS_COMMENT and PICTURE (RFC 9639 §8.6, §8.8).

use yoton_audio::SeekSource;

use crate::io::{goto, peek};
use crate::vorbis::Comment;
use crate::{BUF, Located, MetaError, Mime, Opts, Picture, Progress, SCRATCH, Tags, tryp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum F {
    Magic,
    Block,
    Comments(Comment),
    Picture,
    SkipTo(u64),
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Flac {
    phase: F,
    last: bool,
    end: u64,
}

impl Flac {
    pub(crate) fn new() -> Self {
        Self { phase: F::Magic, last: false, end: 0 }
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
                F::Magic => {
                    let v = tryp!(peek(src, 4));
                    if !v.starts_with_at(0, b"fLaC") {
                        return Ok(Progress::Done);
                    }
                    src.consume(4);
                    self.phase = F::Block;
                }
                F::Block => {
                    let v = tryp!(peek(src, 4));
                    let h = v.u32_be(0).unwrap_or(0);
                    src.consume(4);
                    self.last = h & 0x8000_0000 != 0;
                    self.end = src.position() + u64::from(h & 0xFF_FFFF);
                    self.phase = match (h >> 24) & 0x7F {
                        4 => F::Comments(Comment::new(self.end)),
                        6 => F::Picture,
                        // An invalid type ends the metadata.
                        127 => return Ok(Progress::Done),
                        _ => F::SkipTo(self.end),
                    };
                }
                F::Comments(mut c) => {
                    let p = c.step(src, tags, opts, scratch, false)?;
                    self.phase = F::Comments(c);
                    match p {
                        Progress::Need(n) => return Ok(Progress::Need(n)),
                        Progress::Done => self.phase = F::SkipTo(self.end),
                    }
                }
                F::Picture => {
                    if let Some(p) = tryp!(picture(src, self.end))
                        && tags.picture.is_none_or(|q| q.kind != 3 && p.kind == 3)
                    {
                        tags.picture = Some(p);
                    }
                    self.phase = F::SkipTo(self.end);
                }
                F::SkipTo(pos) => {
                    if goto(src, pos).is_err() || self.last {
                        return Ok(Progress::Done);
                    }
                    self.phase = F::Block;
                }
            }
        }
    }
}

/// A PICTURE block body at the read position (also the decoded form of
/// Vorbis `METADATA_BLOCK_PICTURE`): type, MIME, description, geometry,
/// data. `end` bounds it.
fn picture(src: &mut dyn SeekSource, end: u64) -> Result<Option<Picture>, crate::io::Pending> {
    let v = peek(src, 8)?;
    let kind = v.u32_be(0).unwrap_or(0);
    let ml = v.u32_be(4).unwrap_or(u32::MAX) as usize;
    if ml > 256 {
        return Ok(None);
    }
    let v = peek(src, 12 + ml)?;
    let mut mime = [0u8; 256];
    v.copy_to(8, &mut mime[..ml]);
    let dl = v.u32_be(8 + ml).unwrap_or(u32::MAX) as usize;
    if dl > SCRATCH * 8 {
        return Ok(None);
    }
    let hdr = 12 + ml + dl + 16 + 4;
    let v = peek(src, hdr)?;
    let len = v.u32_be(hdr - 4).unwrap_or(0);
    let offset = src.position() + hdr as u64;
    if offset + u64::from(len) > end {
        return Ok(None);
    }
    Ok(Some(Picture {
        at: Located { offset, skip: 0, len, unsync: false },
        mime: Mime::from_bytes(&mime[..ml]),
        kind: kind.min(255) as u8,
    }))
}
