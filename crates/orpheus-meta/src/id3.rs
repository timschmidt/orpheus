//! ID3v2.2, 2.3 and 2.4 (id3.org specifications), streaming.

use orpheus_audio::SeekSource;

use crate::io::{self, goto, peek, peek_logical, skip_logical, unsync_in_place};
use crate::text::{self, Encoding};
use crate::{
    BUF, Located, Lyrics, LyricsKind, MetaError, Mime, Opts, Picture, Progress, SCRATCH, Tags, apply_key,
    tryp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum P {
    Header,
    Ext,
    Frame,
    /// Skip this many logical bytes, then read the next frame.
    Skip(u64),
    Finish,
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct Id3 {
    start: u64,
    ver: u8,
    /// v2.2/2.3 whole-tag unsynchronisation: every read is de-unsynchronised.
    unsync_tag: bool,
    /// v2.4 tag flag: every frame is unsynchronised.
    unsync_frames: bool,
    left: u64,
    end: u64,
    phase: P,
    prev_ff: bool,
}

fn synchsafe(b: &[u8]) -> Option<u64> {
    b.iter().try_fold(0u64, |a, &x| (x & 0x80 == 0).then_some((a << 7) | u64::from(x)))
}

/// Frames whose content is examined.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Kind {
    Title,
    Artist,
    Album,
    AlbumArtist,
    Genre,
    Year,
    Track,
    Disc,
    Txxx,
    Comm,
    Pic,
    Uslt,
    Sylt,
}

fn kind(id: &[u8]) -> Option<Kind> {
    Some(match id {
        b"TIT2" | b"TT2" => Kind::Title,
        b"TPE1" | b"TP1" => Kind::Artist,
        b"TALB" | b"TAL" => Kind::Album,
        b"TPE2" | b"TP2" => Kind::AlbumArtist,
        b"TCON" | b"TCO" => Kind::Genre,
        b"TYER" | b"TYE" | b"TDRC" | b"TDOR" => Kind::Year,
        b"TRCK" | b"TRK" => Kind::Track,
        b"TPOS" | b"TPA" => Kind::Disc,
        b"TXXX" | b"TXX" => Kind::Txxx,
        b"COMM" | b"COM" => Kind::Comm,
        b"APIC" | b"PIC" => Kind::Pic,
        b"USLT" | b"ULT" => Kind::Uslt,
        b"SYLT" | b"SLT" => Kind::Sylt,
        _ => return None,
    })
}

/// Where a frame's content lives, to turn content offsets into
/// [`Located`]s.
struct Origin {
    /// Raw position of the content (after the frame header and prefix).
    pos: u64,
    /// Logical bytes before the content inside `pos` (unsync case).
    pre: u32,
    unsync: bool,
    /// Logical content length.
    len: u64,
}

impl Origin {
    fn at(&self, off: usize) -> Located {
        let len = self.len.saturating_sub(off as u64).min(u64::from(u32::MAX)) as u32;
        if self.unsync {
            Located { offset: self.pos, skip: self.pre + off as u32, len, unsync: true }
        } else {
            Located { offset: self.pos + u64::from(self.pre) + off as u64, skip: 0, len, unsync: false }
        }
    }
}

impl Id3 {
    pub(crate) fn new(start: u64) -> Self {
        Self {
            start,
            ver: 0,
            unsync_tag: false,
            unsync_frames: false,
            left: 0,
            end: start,
            phase: P::Header,
            prev_ff: false,
        }
    }

    /// Parse the tag at the read position; `fill` only sets empty fields.
    pub(crate) fn step(
        &mut self,
        src: &mut dyn SeekSource,
        tags: &mut Tags,
        opts: &Opts,
        scratch: &mut [u8; BUF],
        fill: bool,
    ) -> Result<Progress, MetaError> {
        loop {
            match self.phase {
                P::Header => {
                    let v = tryp!(peek(src, 10));
                    let h: [u8; 10] = v.array(0).unwrap_or_default();
                    let size = synchsafe(&h[6..10]);
                    let ok = &h[..3] == b"ID3" && (2..=4).contains(&h[3]) && h[4] != 0xFF;
                    let Some(size) = size.filter(|_| ok) else {
                        // Not a tag after all: step over the magic.
                        src.consume(3);
                        return Ok(Progress::Done);
                    };
                    self.ver = h[3];
                    let flags = h[5];
                    let footer = if self.ver == 4 && flags & 0x10 != 0 { 10 } else { 0 };
                    self.end = self.start + 10 + size + footer;
                    self.left = size;
                    self.unsync_tag = self.ver < 4 && flags & 0x80 != 0;
                    self.unsync_frames = self.ver == 4 && flags & 0x80 != 0;
                    src.consume(10);
                    self.phase = if self.ver == 2 && flags & 0x40 != 0 {
                        // v2.2 "compression": undefined, skip the tag.
                        P::Finish
                    } else if flags & 0x40 != 0 {
                        P::Ext
                    } else {
                        P::Frame
                    };
                }
                P::Ext => {
                    let mut b = [0u8; 4];
                    tryp!(peek_logical(src, self.unsync_tag, self.prev_ff, &mut b));
                    let total = if self.ver == 4 {
                        synchsafe(&b).unwrap_or(u64::MAX)
                    } else {
                        4 + u64::from(u32::from_be_bytes(b))
                    };
                    if total > self.left {
                        self.phase = P::Finish;
                        continue;
                    }
                    self.left -= total;
                    self.phase = P::Skip(total);
                }
                P::Skip(n) => {
                    if self.unsync_tag {
                        let k = skip_logical(src, n, &mut self.prev_ff);
                        if k < n {
                            self.phase = P::Skip(n - k);
                            if src.at_end() {
                                self.phase = P::Finish;
                                continue;
                            }
                            return Ok(Progress::Need(((n - k).min(4096) as usize).max(src.available() + 1)));
                        }
                    } else if io::skip(src, n).is_err() {
                        return Ok(Progress::Done);
                    }
                    self.phase = P::Frame;
                }
                P::Frame => {
                    let hl = if self.ver == 2 { 6 } else { 10 };
                    if self.left < hl as u64 {
                        self.phase = P::Finish;
                        continue;
                    }
                    let mut h = [0u8; 10];
                    let raw_h = tryp!(peek_logical(src, self.unsync_tag, self.prev_ff, &mut h[..hl]));
                    let idl = if self.ver == 2 { 3 } else { 4 };
                    if !h[..idl].iter().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit()) {
                        // Padding (zeros) or garbage: the frames are over.
                        self.phase = P::Finish;
                        continue;
                    }
                    let size = match self.ver {
                        2 => (u64::from(h[3]) << 16) | (u64::from(h[4]) << 8) | u64::from(h[5]),
                        3 => u64::from(u32::from_be_bytes([h[4], h[5], h[6], h[7]])),
                        // Some writers put plain sizes in v2.4 tags.
                        _ => synchsafe(&h[4..8])
                            .unwrap_or_else(|| u64::from(u32::from_be_bytes([h[4], h[5], h[6], h[7]]))),
                    };
                    let flags = if self.ver == 2 { 0 } else { u16::from_be_bytes([h[8], h[9]]) };
                    if size + hl as u64 > self.left {
                        self.phase = P::Finish;
                        continue;
                    }
                    if let Some(k) = kind(&h[..idl]) {
                        let want = (size as usize).min(SCRATCH);
                        tryp!(peek_logical(src, self.unsync_tag, self.prev_ff, &mut scratch[..hl + want]));
                        let body = src.position() + raw_h as u64;
                        self.frame(k, flags, &mut scratch[hl..hl + want], size, body, tags, opts, fill);
                    }
                    self.left -= hl as u64 + size;
                    self.phase = P::Skip(hl as u64 + size);
                }
                P::Finish => {
                    if goto(src, self.end).is_err() {
                        // A tag running past the end of the file.
                        let a = src.available();
                        src.consume(a);
                    }
                    return Ok(Progress::Done);
                }
            }
        }
    }

    #[allow(clippy::too_many_arguments)]
    fn frame(
        &self,
        k: Kind,
        flags: u16,
        content: &mut [u8],
        size: u64,
        body: u64,
        tags: &mut Tags,
        opts: &Opts,
        fill: bool,
    ) {
        // Frame format flags.
        let (compressed, encrypted, grouped, unsync, dli) = match self.ver {
            3 => (flags & 0x80 != 0, flags & 0x40 != 0, flags & 0x20 != 0, false, false),
            4 => (
                flags & 0x08 != 0,
                flags & 0x04 != 0,
                flags & 0x40 != 0,
                flags & 0x02 != 0,
                flags & 0x01 != 0,
            ),
            _ => (false, false, false, false, false),
        };
        if compressed || encrypted {
            return;
        }
        let pre = usize::from(grouped) + if dli { 4 } else { 0 };
        if content.len() < pre {
            return;
        }
        let frame_unsync = self.ver == 4 && (unsync || self.unsync_frames);
        // v2.4 data length indicator: the content's length once
        // unsynchronisation is removed.
        let logical = if dli {
            synchsafe(&content[usize::from(grouped)..usize::from(grouped) + 4]).unwrap_or(size)
        } else {
            size - pre as u64
        };
        let (c, origin) = if frame_unsync {
            let n = unsync_in_place(&mut content[pre..]);
            (&content[pre..pre + n], Origin { pos: body, pre: pre as u32, unsync: true, len: logical })
        } else if self.unsync_tag {
            (&content[pre..], Origin { pos: body, pre: pre as u32, unsync: true, len: size - pre as u64 })
        } else {
            (&content[pre..], Origin { pos: body, pre: pre as u32, unsync: false, len: size - pre as u64 })
        };
        let cp = opts.code_page;
        let Some((&e, rest)) = c.split_first() else { return };
        let Some(enc) = Encoding::from_id3(e) else { return };
        let set = |t: &mut crate::Text, v: crate::Text| {
            if !fill || t.is_empty() {
                *t = v;
            }
        };
        match k {
            Kind::Title => set(&mut tags.title, enc.decode(rest, cp)),
            Kind::Artist => set(&mut tags.artist, enc.decode(rest, cp)),
            Kind::Album => set(&mut tags.album, enc.decode(rest, cp)),
            Kind::AlbumArtist => set(&mut tags.album_artist, enc.decode(rest, cp)),
            Kind::Genre => set(&mut tags.genre, text::genre(&enc.decode(rest, cp))),
            Kind::Year => apply_key(tags, b"YEAR", enc.decode(rest, cp).as_str(), fill),
            Kind::Track => apply_key(tags, b"TRACK", enc.decode(rest, cp).as_str(), fill),
            Kind::Disc => apply_key(tags, b"DISC", enc.decode(rest, cp).as_str(), fill),
            Kind::Txxx => {
                let Some((dl, after)) = enc.terminated(rest) else { return };
                let desc = enc.decode(&rest[..dl], cp);
                apply_key(tags, desc.as_str().as_bytes(), enc.decode(&rest[after..], cp).as_str(), fill);
            }
            Kind::Comm => {
                let Some(r) = rest.get(3..) else { return };
                let Some((dl, after)) = enc.terminated(r) else { return };
                if enc.decode(&r[..dl], cp).as_str().eq_ignore_ascii_case("iTunSMPB") {
                    apply_key(tags, b"ITUNSMPB", enc.decode(&r[after..], cp).as_str(), fill);
                }
            }
            Kind::Pic => {
                // APIC: mime (Latin-1, NUL), type, description, data.
                // PIC (v2.2): 3-letter format, type, description, data.
                let (mime, after_mime) = if self.ver == 2 {
                    let Some(f) = rest.get(..3) else { return };
                    (Mime::from_bytes(f), 3)
                } else {
                    let Some((ml, a)) = Encoding::Latin1.terminated(rest) else { return };
                    (Mime::from_bytes(&rest[..ml]), a)
                };
                let Some(&ptype) = rest.get(after_mime) else { return };
                let d = &rest[after_mime + 1..];
                let Some((_, after_desc)) = enc.terminated(d) else { return };
                let off = 1 + after_mime + 1 + after_desc;
                let pic = Picture { at: origin.at(off), mime, kind: ptype };
                // Prefer the front cover (type 3), else the first picture.
                if tags.picture.is_none_or(|p| p.kind != 3 && ptype == 3) {
                    tags.picture = Some(pic);
                }
            }
            Kind::Uslt => {
                let Some(r) = rest.get(3..) else { return };
                let Some((_, after)) = enc.terminated(r) else { return };
                if tags.lyrics.is_none() {
                    tags.lyrics =
                        Some(Lyrics { at: origin.at(1 + 3 + after), kind: LyricsKind::Unsynced(enc) });
                }
            }
            Kind::Sylt => {
                if tags.synced_lyrics.is_none() {
                    tags.synced_lyrics = Some(Lyrics { at: origin.at(0), kind: LyricsKind::Synced });
                }
            }
        }
    }
}
