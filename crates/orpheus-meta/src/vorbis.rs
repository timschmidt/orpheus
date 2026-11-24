//! Vorbis comments (`vendor`, then `KEY=value` items, little-endian
//! lengths) as found in FLAC (and, with the Ogg layer, Vorbis/Opus).

use orpheus_audio::SeekSource;

use crate::io::{goto, peek};
use crate::text::Encoding;
use crate::{BUF, Located, Lyrics, LyricsKind, MetaError, Opts, Progress, SCRATCH, Tags, apply_key, tryp};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum C {
    Vendor,
    Count,
    Item,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Comment {
    end: u64,
    phase: C,
    left: u32,
}

impl Comment {
    /// Comments at the read position, ending at absolute `end`.
    pub(crate) fn new(end: u64) -> Self {
        Self { end, phase: C::Vendor, left: 0 }
    }

    pub(crate) fn step(
        &mut self,
        src: &mut dyn SeekSource,
        tags: &mut Tags,
        _opts: &Opts,
        scratch: &mut [u8; BUF],
        fill: bool,
    ) -> Result<Progress, MetaError> {
        loop {
            let pos = src.position();
            match self.phase {
                C::Vendor => {
                    let v = tryp!(peek(src, 4));
                    let vl = u64::from(v.u32_le(0).unwrap_or(u32::MAX));
                    if pos + 4 + vl > self.end || goto(src, pos + 4 + vl).is_err() {
                        return Ok(Progress::Done);
                    }
                    self.phase = C::Count;
                }
                C::Count => {
                    let v = tryp!(peek(src, 4));
                    self.left = v.u32_le(0).unwrap_or(0);
                    src.consume(4);
                    self.phase = C::Item;
                }
                C::Item => {
                    if self.left == 0 || pos + 4 > self.end {
                        return Ok(Progress::Done);
                    }
                    let v = tryp!(peek(src, 4));
                    let len = v.u32_le(0).unwrap_or(u32::MAX) as usize;
                    let next = pos + 4 + len as u64;
                    if next > self.end {
                        return Ok(Progress::Done);
                    }
                    let want = len.min(SCRATCH);
                    let v = tryp!(peek(src, 4 + want));
                    v.copy_to(4, &mut scratch[..want]);
                    let item = &scratch[..want];
                    if let Some(eq) = item.iter().position(|&c| c == b'=') {
                        let key = &item[..eq];
                        if key.eq_ignore_ascii_case(b"LYRICS") || key.eq_ignore_ascii_case(b"UNSYNCEDLYRICS")
                        {
                            if tags.lyrics.is_none() {
                                let at = Located {
                                    offset: pos + 4 + eq as u64 + 1,
                                    skip: 0,
                                    len: (len - eq - 1) as u32,
                                    unsync: false,
                                };
                                tags.lyrics = Some(Lyrics { at, kind: LyricsKind::Utf8 });
                            }
                        } else {
                            let value = Encoding::Utf8.decode(&item[eq + 1..], Default::default());
                            apply_key(tags, key, value.as_str(), fill);
                        }
                    }
                    self.left -= 1;
                    if goto(src, next).is_err() {
                        return Ok(Progress::Done);
                    }
                }
            }
        }
    }
}
