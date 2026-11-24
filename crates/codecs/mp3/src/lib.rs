//! MP3 (MPEG-1/2/2.5 Layer III) decoder (04 §4.1 priority A, WP AU-2).
//!
//! * Frame decoding: minimp3's algorithm in safe Rust, vendored from mbop3
//!   0.1.0 (MIT OR Apache-2.0; minimp3 is CC0) with its state made a `Pod`
//!   struct that lives in the codec arena (`state.rs`; `bits.rs`,
//!   `header.rs`, `layer3.rs`, `synth.rs`, `tables.rs` are mbop3's files
//!   with the changes marked `orpheus:`). Single-precision float: the M4F's
//!   FPU does the IMDCT and synthesis; no `f64` anywhere.
//! * Stream layer (this file, written for orpheus): frame sync over the ring
//!   (a frame is accepted when the next header matches, or once locked),
//!   ID3v1/ID3v2/APE tags in the stream skipped, Xing/Info/VBRI frames
//!   parsed and not decoded, gapless trims from the LAME tag applied exactly
//!   as ffmpeg does (start: encoder delay + 529; end: padding − 529),
//!   sample-exact seeking through a sparse frame index plus header walking,
//!   with the bit reservoir and overlap primed by decoding (and
//!   discarding) enough frames before the target: at least a full
//!   reservoir's worth of main data before the two frames that precede it.
//!
//! Layers I and II and free-format streams are refused
//! (`Unsupported::Encoding`).

#![warn(missing_docs)]
#![no_std]
#![forbid(unsafe_code)]
// The vendored files keep minimp3's constants and indexing.
#![allow(
    clippy::excessive_precision,
    clippy::approx_constant,
    clippy::needless_range_loop,
    clippy::cast_lossless,
    clippy::manual_range_contains,
    clippy::unreadable_literal
)]

#[cfg(test)]
extern crate std;

mod bits;
mod header;
mod layer3;
mod state;
mod synth;
mod tables;

use orpheus_audio::arena::{self, Arena};
use orpheus_audio::probe::{MpegHeader, PROBE_WINDOW, Sync, find_mpeg_sync, id3v2_len};
use orpheus_audio::source::skip;
use orpheus_audio::{
    ByteSource, CodecId, ContainerInfo, DecodeError, Decoder, MAX_PEEK, OpenError, PcmBlock, Plan,
    SampleKind, SeekSource, SeekStep, Step, StreamInfo, Unsupported, View,
};
use state::{MAX_SAMPLES_PER_FRAME, State};

/// An output sample type of the vendored synthesis (`i16` only here).
pub(crate) trait Sample: Copy + Default {
    fn from_synth(sample: f32) -> Self;
}

impl Sample for i16 {
    #[inline]
    fn from_synth(sample: f32) -> i16 {
        // minimp3 rounding: (sample + 0.5) truncated, minus 1 if negative,
        // saturated (mbop3's float-compare-free form).
        let s = ((sample + 0.5) as i32).clamp(-32767, 32767);
        (s - i32::from(s < 0)) as i16
    }
}

/// mbop3's profiling hook, compiled out.
macro_rules! timed {
    ($stage:expr, $e:expr) => {{ $e }};
}
pub(crate) use timed;

/// Decoder delay of the hybrid filterbank, in samples (ffmpeg: 528 + 1).
const DECODER_DELAY: u64 = 529;
/// Largest Layer III frame accepted (320 kbit/s at 32 kHz is 1441 bytes).
const MAX_FRAME: usize = 1536;
/// Sparse seek index entries (frame number, byte offset).
const INDEX_LEN: usize = 256;
/// Frames remembered while walking towards a seek target, to choose where
/// priming starts (the newest ones before the target).
const WALK_RING: usize = 64;

/// Arena bytes for any stream.
pub const ARENA: usize = arena::need::<State>(1)
    + arena::need::<u8>(MAX_FRAME)
    + arena::need::<i16>(MAX_SAMPLES_PER_FRAME)
    + arena::need::<[u32; 2]>(INDEX_LEN)
    + arena::need::<[u32; 2]>(WALK_RING);

/// What the first frame's Xing/Info/VBRI header says.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct VbrTag {
    /// The header frame itself carries no audio.
    present: bool,
    frames: Option<u32>,
    bytes: Option<u32>,
    /// LAME encoder delay and padding.
    gapless: Option<(u32, u32)>,
}

/// Side information length (bytes after the header and optional CRC).
fn side_info_len(h: &MpegHeader) -> usize {
    match (h.version == 1, h.channels == 1) {
        (true, true) => 17,
        (true, false) => 32,
        (false, true) => 9,
        (false, false) => 17,
    }
}

fn parse_vbr_tag(frame: &View<'_>, h: &MpegHeader) -> VbrTag {
    let mut t = VbrTag::default();
    let off = 4 + if h.crc { 2 } else { 0 } + side_info_len(h);
    if frame.starts_with_at(off, b"Xing") || frame.starts_with_at(off, b"Info") {
        t.present = true;
        let flags = frame.u32_be(off + 4).unwrap_or(0);
        let mut p = off + 8;
        if flags & 1 != 0 {
            t.frames = frame.u32_be(p);
            p += 4;
        }
        if flags & 2 != 0 {
            t.bytes = frame.u32_be(p);
            p += 4;
        }
        if flags & 4 != 0 {
            p += 100;
        }
        if flags & 8 != 0 {
            p += 4;
        }
        // LAME extension: the same encoders ffmpeg trusts for gapless data.
        if (frame.starts_with_at(p, b"LAME")
            || frame.starts_with_at(p, b"Lavf")
            || frame.starts_with_at(p, b"Lavc"))
            && let Some(d) = frame.array::<3>(p + 21)
        {
            let v = (u32::from(d[0]) << 16) | (u32::from(d[1]) << 8) | u32::from(d[2]);
            t.gapless = Some((v >> 12, v & 0xFFF));
        }
    } else if frame.starts_with_at(36, b"VBRI") {
        t.present = true;
        t.bytes = frame.u32_be(36 + 10);
        t.frames = frame.u32_be(36 + 14);
    }
    t
}

/// The stream as found by `inspect`/`open`.
struct Head {
    /// Offset of the first frame.
    off: usize,
    hdr: MpegHeader,
    len: usize,
    tag: VbrTag,
    info: StreamInfo,
    /// Decoder samples to drop at the start, and the end position (if
    /// known) — both in decoder output samples per channel.
    delay: u64,
    end: Option<u64>,
}

fn parse_head(src: &dyn ByteSource, hint: &ContainerInfo) -> Result<Head, OpenError> {
    let v = View::of(src);
    let at_end = src.at_end();
    let off = match find_mpeg_sync(&v, at_end, 2) {
        Sync::Found(o) => o,
        Sync::NeedData(n) => return Err(OpenError::NeedData(n)),
        Sync::None if !at_end && v.len() < PROBE_WINDOW => {
            return Err(OpenError::NeedData((v.len() + 4096).min(PROBE_WINDOW)));
        }
        Sync::None => return Err(OpenError::Unsupported(Unsupported::NotThisFormat)),
    };
    let hdr = v.array::<4>(off).and_then(MpegHeader::parse).ok_or(Unsupported::BadHeader)?;
    if hdr.layer != 3 {
        return Err(OpenError::Unsupported(Unsupported::Encoding));
    }
    let len = hdr.frame_len(0) as usize;
    if len > MAX_FRAME || len < 4 + side_info_len(&hdr) {
        return Err(OpenError::Unsupported(Unsupported::FrameTooLarge));
    }
    let Some(frame) = v.range(off, len) else {
        return Err(if at_end { Unsupported::BadHeader.into() } else { OpenError::NeedData(off + len) });
    };
    let tag = parse_vbr_tag(&frame, &hdr);
    let spf = u64::from(hdr.samples);
    let (delay, end) = match (tag.gapless, hint.gapless) {
        (Some((d, p)), _) => {
            let delay = u64::from(d) + DECODER_DELAY;
            let end = tag.frames.map(|f| (u64::from(f) * spf + DECODER_DELAY).saturating_sub(u64::from(p)));
            (delay, end)
        }
        // Delay and padding from outside the stream (iTunSMPB): taken as
        // totals including the decoder delay.
        (None, Some(g)) => {
            let end = tag.frames.map(|f| (u64::from(f) * spf).saturating_sub(u64::from(g.padding)));
            (u64::from(g.delay), end)
        }
        (None, None) => (0, tag.frames.map(|f| u64::from(f) * spf)),
    };
    let frames = end.map(|e| e.saturating_sub(delay));
    let bitrate = match (tag.bytes, tag.frames) {
        (Some(b), Some(f)) if f > 0 => (u64::from(b) * 8 * u64::from(hdr.rate) / (u64::from(f) * spf)) as u32,
        _ => hdr.bitrate * 1000,
    };
    let info = StreamInfo {
        rate: hdr.rate,
        channels: hdr.channels,
        bits: 16,
        sample: SampleKind::I16,
        frames,
        delay: delay.min(u64::from(u32::MAX)) as u32,
        padding: match (end, tag.frames) {
            (Some(e), Some(f)) => (u64::from(f) * spf).saturating_sub(e) as u32,
            _ => 0,
        },
        bitrate,
        codec: CodecId::Mp3,
    };
    Ok(Head { off, hdr, len, tag, info, delay, end })
}

#[derive(Debug, Clone, Copy)]
struct SeekState {
    k: u64,
    target: u64,
    from: u64,
}

/// Outcome of looking for the next frame at the read position.
enum Found {
    /// A frame of this length starts at the read position and is buffered.
    Frame(usize, MpegHeader),
    Need(usize),
    End,
    /// Bytes were skipped to regain sync.
    Junk,
}

/// An open MPEG audio (MP3) stream; its state lives in the arena.
pub struct Mp3Decoder<'a> {
    st: &'a mut State,
    fbuf: &'a mut [u8],
    pcm: &'a mut [i16],
    index: &'a mut [[u32; 2]],
    /// While walking to a seek target: (offset, main-data bytes) of the
    /// last frames, by frame number modulo `WALK_RING`.
    walk: &'a mut [[u32; 2]],
    n_index: usize,
    stride: u32,
    info: StreamInfo,
    used: usize,
    /// The stream's header (version, layer, rate) once a frame was accepted.
    lock: Option<MpegHeader>,
    spf: u64,
    audio_start: u64,
    /// Next frame's number (0 = first audio frame).
    frame_no: u64,
    /// Decoder output position (samples per channel) at the next frame.
    pos: u64,
    delay: u64,
    end: Option<u64>,
    /// After a seek: decoder position from which output is handed out.
    skip_to: u64,
    /// Pending output range in `pcm`.
    out: (usize, usize),
    /// Bytes of an in-stream tag still to skip.
    skip: u64,
    /// A seek in progress: walk to frame `k` (the one holding the target),
    /// then land on playable frame `target`; `from` is where the walk began.
    seeking: Option<SeekState>,
    ended: bool,
}

impl Mp3Decoder<'_> {
    fn record(&mut self, frame_no: u64, offset: u64) {
        let (Ok(f), Ok(o)) = (u32::try_from(frame_no), u32::try_from(offset)) else { return };
        if f != self.n_index as u32 * self.stride {
            return;
        }
        if self.n_index == self.index.len() {
            // Full: keep every other entry and double the stride.
            for i in 0..self.index.len() / 2 {
                self.index[i] = self.index[2 * i];
            }
            self.n_index = self.index.len() / 2;
            self.stride *= 2;
            if f != self.n_index as u32 * self.stride {
                return;
            }
        }
        self.index[self.n_index] = [f, o];
        self.n_index += 1;
    }

    /// Length of a tag found inside the stream (ID3v1, ID3v2, APEv2).
    fn tag_len(v: &View<'_>) -> Option<u64> {
        if v.starts_with_at(0, b"TAG") {
            Some(128)
        } else if let Some(n) = id3v2_len(v) {
            Some(n)
        } else if v.starts_with_at(0, b"APETAGEX") {
            // A footer's size covers the items and the footer; a header
            // (flag bit 31) adds 32 bytes. Reading from a header (flag bit
            // 29) skips just the header: the items fail sync and the footer
            // is found again.
            let size = u64::from(v.u32_le(12).unwrap_or(0));
            let flags = v.u32_le(20).unwrap_or(0);
            Some(if flags & (1 << 29) != 0 { 32 } else { size + if flags & (1 << 31) != 0 { 32 } else { 0 } })
        } else {
            None
        }
    }

    /// Find the next frame at the read position (consuming junk and tags).
    fn find(&mut self, src: &mut dyn ByteSource) -> Found {
        let mut junk = false;
        loop {
            if self.skip > 0 {
                self.skip -= skip(src, self.skip);
                if self.skip > 0 {
                    return if src.at_end() {
                        Found::End
                    } else {
                        Found::Need((self.skip.min(4096)) as usize)
                    };
                }
            }
            let avail = src.available();
            let at_end = src.at_end();
            if avail < 4 {
                if at_end {
                    src.consume(avail);
                    return Found::End;
                }
                return Found::Need(4);
            }
            let v = View::peek(src, avail.min(MAX_PEEK)).unwrap_or_default();
            let h = v.array::<4>(0).and_then(MpegHeader::parse);
            let ok = |h: &MpegHeader, lock: Option<MpegHeader>| {
                h.layer == 3 && h.bitrate != 0 && lock.is_none_or(|l| l.compatible(h))
            };
            if let Some(h) = h.filter(|h| ok(h, self.lock)) {
                let len = h.frame_len(0) as usize;
                if len <= MAX_FRAME && len >= 4 + side_info_len(&h) {
                    if avail < len {
                        if at_end {
                            // A truncated last frame.
                            src.consume(avail);
                            return Found::End;
                        }
                        return Found::Need(len);
                    }
                    if self.lock.is_some() {
                        return if junk { Found::Junk } else { Found::Frame(len, h) };
                    }
                    // Not locked yet: the next header must agree.
                    if avail < len + 4 && !at_end {
                        return Found::Need(len + 4);
                    }
                    let next = v.array::<4>(len).and_then(MpegHeader::parse);
                    if avail == len || next.is_some_and(|n| h.compatible(&n)) {
                        self.lock = Some(h);
                        return if junk { Found::Junk } else { Found::Frame(len, h) };
                    }
                }
            }
            let tag = Self::tag_len(&v);
            // Lost sync: skip to the next possible header byte.
            let k = (1..v.len()).find(|&i| v.byte(i) == 0xFF).unwrap_or(v.len());
            if let Some(n) = tag {
                self.skip = n.max(1) - skip(src, n.max(1));
                continue;
            }
            src.consume(k);
            // Junk before the first frame is not an error.
            junk |= self.lock.is_some();
        }
    }

    fn decode_frame(&mut self, src: &mut dyn ByteSource, len: usize, h: &MpegHeader) -> usize {
        let Some(v) = View::peek(src, len) else { return 0 };
        let frame: &[u8] = if v.b.is_empty() {
            v.a
        } else {
            v.copy_to(0, &mut self.fbuf[..len]);
            &self.fbuf[..len]
        };
        let pcm: &mut [i16; MAX_SAMPLES_PER_FRAME] = match (&mut self.pcm[..MAX_SAMPLES_PER_FRAME]).try_into()
        {
            Ok(p) => p,
            Err(_) => return 0,
        };
        let n = self.st.decode_frame(frame, pcm);
        let spf = self.spf as usize;
        let fch = usize::from(h.channels);
        let och = usize::from(self.info.channels);
        if n == 0 {
            // Missing reservoir: the frame still occupies its time.
            self.pcm[..spf * och].fill(0);
        } else if fch != och {
            // A mono/stereo switch inside the stream: convert to the
            // stream's channel count.
            if fch == 1 {
                for i in (0..spf).rev() {
                    let s = self.pcm[i];
                    self.pcm[2 * i] = s;
                    self.pcm[2 * i + 1] = s;
                }
            } else {
                for i in 0..spf {
                    self.pcm[i] = self.pcm[2 * i];
                }
            }
        }
        spf
    }
}

impl<'a> Decoder<'a> for Mp3Decoder<'a> {
    const ARENA_MIN: usize = ARENA;

    fn inspect(src: &dyn ByteSource, hint: &ContainerInfo) -> Result<Plan, OpenError> {
        let h = parse_head(src, hint)?;
        Ok(Plan { info: h.info, arena: ARENA })
    }

    fn open(
        src: &mut dyn ByteSource,
        arena: &'a mut [u8],
        hint: &ContainerInfo,
    ) -> Result<(Self, StreamInfo), OpenError> {
        let h = parse_head(src, hint)?;
        let mut ar = Arena::new(arena);
        let st = ar.one::<State>()?;
        let fbuf = ar.slice::<u8>(MAX_FRAME)?;
        let pcm = ar.slice::<i16>(MAX_SAMPLES_PER_FRAME)?;
        let index = ar.slice::<[u32; 2]>(INDEX_LEN)?;
        let walk = ar.slice::<[u32; 2]>(WALK_RING)?;
        src.consume(h.off);
        if h.tag.present {
            src.consume(h.len);
        }
        let d = Mp3Decoder {
            st,
            fbuf,
            pcm,
            index,
            walk,
            n_index: 0,
            stride: 1,
            info: h.info,
            used: ar.used(),
            lock: Some(h.hdr),
            spf: u64::from(h.hdr.samples),
            audio_start: src.position(),
            frame_no: 0,
            pos: 0,
            delay: h.delay,
            end: h.end,
            skip_to: 0,
            out: (0, 0),
            skip: 0,
            seeking: None,
            ended: false,
        };
        Ok((d, h.info))
    }

    fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError> {
        loop {
            if self.out.0 < self.out.1 {
                let (s, e) = self.out;
                self.out = (0, 0);
                return Ok(Step::Pcm(PcmBlock::I16(&mut self.pcm[s..e])));
            }
            if self.ended {
                return Ok(Step::EndOfStream);
            }
            let (len, h) = match self.find(src) {
                Found::Frame(len, h) => (len, h),
                Found::Need(n) => return Ok(Step::NeedData(n)),
                Found::End => {
                    self.ended = true;
                    return Ok(Step::EndOfStream);
                }
                Found::Junk => return Err(DecodeError::Corrupt),
            };
            let offset = src.position() - self.audio_start;
            let spf = self.decode_frame(src, len, &h);
            src.consume(len);
            self.record(self.frame_no, offset);
            self.frame_no += 1;
            let start = self.pos;
            self.pos += self.spf;
            let lo = self.delay.max(self.skip_to);
            let hi = self.end.unwrap_or(u64::MAX);
            if start >= hi {
                self.ended = true;
                return Ok(Step::EndOfStream);
            }
            let s = lo.saturating_sub(start).min(spf as u64) as usize;
            let e = hi.saturating_sub(start).min(spf as u64) as usize;
            if e > s {
                let ch = usize::from(self.info.channels);
                self.out = (s * ch, e * ch);
            }
        }
    }

    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError> {
        let ring = WALK_RING as u64;
        if self.seeking.is_none() {
            let target = self.info.frames.map_or(frame, |t| frame.min(t));
            let k = (target + self.delay) / self.spf;
            // Start the walk far enough before `k` for the ring to hold the
            // priming frames: from the current position if possible, else
            // from the index entry at or before that point.
            let want = k.saturating_sub(ring);
            let entry = self.index[..self.n_index].iter().rev().find(|e| u64::from(e[0]) <= want).copied();
            let here_ok = self.frame_no <= want && !self.ended;
            match entry {
                Some([f, _]) if here_ok && self.frame_no >= u64::from(f) => {}
                Some([f, o]) => {
                    src.seek(self.audio_start + u64::from(o))?;
                    self.frame_no = u64::from(f);
                }
                None => {
                    src.seek(self.audio_start)?;
                    self.frame_no = 0;
                }
            }
            self.skip = 0;
            self.seeking = Some(SeekState { k, target, from: self.frame_no });
        }
        let Some(sk) = self.seeking else { return Ok(SeekStep::Done(frame)) };
        while self.frame_no < sk.k {
            match self.find(src) {
                Found::Frame(len, h) => {
                    let offset = src.position() - self.audio_start;
                    let main = len.saturating_sub(4 + side_info_len(&h) + if h.crc { 2 } else { 0 });
                    self.walk[(self.frame_no % ring) as usize] = [offset as u32, main as u32];
                    src.consume(len);
                    self.record(self.frame_no, offset);
                    self.frame_no += 1;
                }
                Found::Need(n) => return Ok(SeekStep::NeedData(n)),
                Found::Junk => {}
                Found::End => break,
            }
        }
        // Priming: frames k-2 and k-1 must decode exactly, so the frames
        // before them must supply a full bit reservoir (511 bytes for
        // MPEG-1, 255 for MPEG-2/2.5).
        let reservoir = if self.lock.is_some_and(|l| l.version == 1) { 511 } else { 255 };
        let end = self.frame_no;
        let oldest = sk.from.max(end.saturating_sub(ring));
        let mut start = end.saturating_sub(2).max(oldest);
        let mut acc = 0u32;
        while start > oldest && acc < reservoir {
            start -= 1;
            acc += self.walk[(start % ring) as usize][1];
        }
        if start < end {
            let off = u64::from(self.walk[(start % ring) as usize][0]);
            src.seek(self.audio_start + off)?;
            self.frame_no = start;
        }
        self.seeking = None;
        self.st.reset();
        self.pos = self.frame_no * self.spf;
        self.skip_to = sk.target + self.delay;
        self.out = (0, 0);
        self.ended = false;
        Ok(SeekStep::Done(sk.target))
    }

    fn arena_used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests;
