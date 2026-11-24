//! FLAC decoder (04 §4.1 priority A, WP AU-3), written from RFC 9639.
//!
//! * Native FLAC (`fLaC`); all block sizes up to the arena's limit (4608
//!   in the 38 KiB budget, 8192 in 66 KiB; larger STREAMINFO maxima are
//!   refused), fixed and variable blocking, 4–24 bits per sample, 1–8
//!   channels (the pipeline down-mixes beyond two), every channel
//!   decorrelation mode, fixed and LPC prediction up to order 32, Rice and
//!   Rice2 residuals with escape partitions, wasted bits.
//! * Every frame's CRC-8 and CRC-16 are checked; a bad frame is skipped
//!   with a resync on the next valid header (`DecodeError::Corrupt`).
//! * Seeking: SEEKTABLE points (up to [`SEEK_POINTS`] kept, evenly
//!   subsampled) narrow the range, then interpolation search on frame
//!   headers (their sample numbers make it exact), then a header walk.
//! * Refused: 32-bit samples (`Unsupported::BitDepth`); Ogg FLAC needs the
//!   Ogg layer (not yet built).
//!
//! Memory: one decoded block, planar `i32` (channels × max block size from
//! STREAMINFO), a 1 KiB interleaving buffer and the seek points, all from
//! the arena. Output: `i16` for ≤ 16-bit streams, left-justified `i32`
//! above.

#![warn(missing_docs)]
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod frame;

use frame::{FrameError, FrameHeader, Params};
use orpheus_audio::arena::{self, Arena};
use orpheus_audio::source::skip;
use orpheus_audio::{
    ByteSource, CodecId, ContainerInfo, DecodeError, Decoder, MAX_PEEK, OpenError, PcmBlock, Plan,
    SampleKind, SeekSource, SeekStep, Step, StreamInfo, Unsupported, View,
};

/// Interleaving buffer, in `i32` words (1 KiB).
const OUT_WORDS: usize = 256;
/// SEEKTABLE points kept.
pub const SEEK_POINTS: usize = 32;
/// Largest STREAMINFO maximum block size accepted.
pub const MAX_BLOCK: u32 = 8192;

/// Arena bytes for a stream with this many channels and maximum block size.
#[must_use]
pub const fn arena_for(channels: usize, max_block: usize) -> usize {
    arena::need::<i32>(channels * max_block)
        + arena::need::<i32>(OUT_WORDS)
        + arena::need::<[u64; 2]>(SEEK_POINTS)
}

/// The smallest arena any stream opens with (mono, 16-sample blocks).
/// Real streams need more and state it in `Plan::arena`: stereo 4608-sample
/// blocks fit the 38 KiB budget, 8192-sample blocks the 66 KiB one.
pub const ARENA_MIN: usize = arena_for(1, 16);

#[derive(Debug, Clone, Copy)]
struct StreamHead {
    params: Params,
    min_block: u32,
    max_frame: u32,
    total: u64,
    info: StreamInfo,
    /// Bytes of `fLaC` + STREAMINFO block.
    len: usize,
    /// STREAMINFO was the last metadata block.
    last: bool,
}

fn parse_head(src: &dyn ByteSource) -> Result<StreamHead, OpenError> {
    let v = View::of(src);
    if v.len() < 42 {
        return Err(if src.at_end() { Unsupported::BadHeader.into() } else { OpenError::NeedData(42) });
    }
    if !v.starts_with_at(0, b"fLaC") {
        return Err(Unsupported::NotThisFormat.into());
    }
    let bh = v.byte(4);
    if bh & 0x7F != 0 || v.u32_be(4).unwrap_or(0) & 0xFF_FFFF < 34 {
        return Err(Unsupported::BadHeader.into());
    }
    let si: [u8; 34] = v.array(8).ok_or(Unsupported::BadHeader)?;
    let min_block = u32::from(u16::from_be_bytes([si[0], si[1]]));
    let max_block = u32::from(u16::from_be_bytes([si[2], si[3]]));
    let max_frame = (u32::from(si[7]) << 16) | (u32::from(si[8]) << 8) | u32::from(si[9]);
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | u32::from(si[12] >> 4);
    let channels = ((si[12] >> 1) & 7) + 1;
    let bps = (((si[12] & 1) << 4) | (si[13] >> 4)) + 1;
    let total =
        (u64::from(si[13] & 0x0F) << 32) | u64::from(u32::from_be_bytes([si[14], si[15], si[16], si[17]]));
    if rate == 0 || max_block < 16 || min_block > max_block {
        return Err(Unsupported::BadHeader.into());
    }
    if max_block > MAX_BLOCK {
        return Err(Unsupported::FrameTooLarge.into());
    }
    if !(4..=24).contains(&bps) {
        return Err(Unsupported::BitDepth(bps).into());
    }
    if max_frame as usize > MAX_PEEK {
        return Err(Unsupported::FrameTooLarge.into());
    }
    let info = StreamInfo {
        rate,
        channels,
        bits: bps,
        sample: if bps > 16 { SampleKind::I32 } else { SampleKind::I16 },
        frames: (total != 0).then_some(total),
        delay: 0,
        padding: 0,
        bitrate: 0,
        codec: CodecId::Flac,
    };
    let params = Params { rate, channels, bps, max_block, nominal_block: max_block };
    Ok(StreamHead { params, min_block, max_frame, total, info, len: 42, last: bh & 0x80 != 0 })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Meta {
    /// Expect a metadata block header.
    Header,
    /// Skip this many bytes of a block.
    Skip(u64),
    /// Read seek points: bytes left in the table, keep every `step`-th.
    SeekTable {
        left: u64,
        step: u64,
        index: u64,
    },
    Done,
}

#[derive(Debug, Clone, Copy)]
struct SeekSt {
    target: u64,
    lo: u64,
    lo_s: u64,
    hi: u64,
    hi_s: u64,
    probes: u32,
    /// Where the last probe started.
    at: u64,
    /// Walking forward from a known frame start.
    linear: bool,
    started: bool,
}

/// An open FLAC stream (native FLAC or FLAC in Ogg).
pub struct FlacDecoder<'a> {
    head: StreamHead,
    planar: &'a mut [i32],
    out: &'a mut [i32],
    points: &'a mut [[u64; 2]],
    n_points: usize,
    stride: usize,
    used: usize,
    meta: Meta,
    last_block: bool,
    audio_start: u64,
    /// The decoded block: its length, how much has been handed out.
    block: usize,
    cursor: usize,
    /// Samples still to drop after a seek.
    discard: u64,
    seek: Option<SeekSt>,
    ended: bool,
    /// Last good frame's first sample + block (to detect gaps; unused for
    /// output).
    next_sample: u64,
}

enum Got {
    Block,
    Need(usize),
    End,
    Corrupt,
}

impl FlacDecoder<'_> {
    /// Walk the metadata blocks after STREAMINFO (seek table kept, the rest
    /// skipped as it streams past).
    fn metadata(&mut self, src: &mut dyn ByteSource) -> Option<usize> {
        loop {
            match self.meta {
                Meta::Done => return None,
                Meta::Skip(n) => {
                    let left = n - skip(src, n);
                    if left > 0 {
                        self.meta = Meta::Skip(left);
                        return if src.at_end() { None } else { Some(left.min(4096) as usize) };
                    }
                    self.meta = if self.last_block { Meta::Done } else { Meta::Header };
                    if self.meta == Meta::Done {
                        self.audio_start = src.position();
                    }
                }
                Meta::Header => {
                    let Some(v) = View::peek(src, 4) else {
                        return if src.at_end() { None } else { Some(4) };
                    };
                    let h = v.u32_be(0).unwrap_or(0);
                    src.consume(4);
                    self.last_block = h & 0x8000_0000 != 0;
                    let len = u64::from(h & 0xFF_FFFF);
                    self.meta = if (h >> 24) & 0x7F == 3 {
                        let n = len / 18;
                        Meta::SeekTable { left: len, step: n.div_ceil(SEEK_POINTS as u64).max(1), index: 0 }
                    } else {
                        Meta::Skip(len)
                    };
                }
                Meta::SeekTable { left, step, index } => {
                    if left < 18 {
                        self.meta = Meta::Skip(left);
                        continue;
                    }
                    let Some(v) = View::peek(src, 18) else {
                        return if src.at_end() { None } else { Some(18) };
                    };
                    let sample = v.u64_be(0).unwrap_or(u64::MAX);
                    let offset = v.u64_be(8).unwrap_or(0);
                    src.consume(18);
                    if sample != u64::MAX && index % step == 0 && self.n_points < self.points.len() {
                        let ok = self.n_points == 0 || self.points[self.n_points - 1][0] < sample;
                        if ok {
                            self.points[self.n_points] = [sample, offset];
                            self.n_points += 1;
                        }
                    }
                    self.meta = Meta::SeekTable { left: left - 18, step, index: index + 1 };
                }
            }
        }
    }

    /// Bytes to have buffered before trying a frame.
    fn want(&self) -> usize {
        let p = &self.head.params;
        let bound = if self.head.max_frame > 0 {
            self.head.max_frame as usize
        } else {
            // A verbatim frame plus headers.
            p.max_block as usize * usize::from(p.channels) * (usize::from(p.bps) + 1) / 8 + 64
        };
        bound.min(MAX_PEEK)
    }

    /// Find the next frame header at or after the read position, consuming
    /// what precedes it. `Ok(header)` when one starts at the read position.
    fn sync(&mut self, src: &mut dyn ByteSource) -> Result<FrameHeader, Got> {
        let mut junk = false;
        loop {
            let avail = src.available();
            if avail < 2 || (avail < 16 && !src.at_end()) {
                if src.at_end() {
                    src.consume(avail);
                    return Err(Got::End);
                }
                return Err(if junk { Got::Corrupt } else { Got::Need(16) });
            }
            let v = View::peek(src, avail).unwrap_or_default();
            if frame::is_sync(&v, 0)
                && let Some(h) = frame::header(&v, &self.head.params)
            {
                return if junk { Err(Got::Corrupt) } else { Ok(h) };
            }
            let k = (1..v.len()).find(|&i| v.byte(i) == 0xFF).unwrap_or(v.len());
            src.consume(k);
            junk = true;
        }
    }

    /// Decode the next frame into `planar`.
    fn next_frame(&mut self, src: &mut dyn ByteSource) -> Got {
        let h = match self.sync(src) {
            Ok(h) => h,
            Err(g) => return g,
        };
        let want = self.want();
        if src.available() < want && !src.at_end() {
            return Got::Need(want);
        }
        let v = View::of(src);
        match frame::decode(&v, &h, self.planar, self.stride) {
            Ok(len) => {
                src.consume(len);
                self.block = h.block as usize;
                self.cursor = 0;
                self.next_sample = h.first + u64::from(h.block);
                Got::Block
            }
            Err(FrameError::Short) if !src.at_end() && v.len() < MAX_PEEK => {
                Got::Need((v.len() + 4096).min(MAX_PEEK))
            }
            Err(_) => {
                // Skip the sync code; the next call resynchronises.
                src.consume(2);
                Got::Corrupt
            }
        }
    }

    /// Interleave the next part of the decoded block into `out`; returns
    /// the sample count.
    fn emit(&mut self) -> usize {
        let ch = usize::from(self.head.params.channels);
        let left = self.block - self.cursor;
        let d = (self.discard.min(left as u64)) as usize;
        self.discard -= d as u64;
        self.cursor += d;
        let wide = self.head.params.bps > 16;
        let cap = if wide { self.out.len() } else { self.out.len() * 2 } / ch;
        let n = (self.block - self.cursor).min(cap);
        let c0 = self.cursor;
        if wide {
            let shift = 32 - u32::from(self.head.params.bps);
            for (f, o) in self.out[..n * ch].chunks_exact_mut(ch).enumerate() {
                for (c, s) in o.iter_mut().enumerate() {
                    *s = self.planar[c * self.stride + c0 + f] << shift;
                }
            }
        } else {
            let shift = 16 - u32::from(self.head.params.bps);
            let out = arena::as_i16(self.out);
            if ch == 2 {
                let (l, r) = self.planar.split_at(self.stride);
                for ((o, &a), &b) in
                    out[..2 * n].as_chunks_mut::<2>().0.iter_mut().zip(&l[c0..c0 + n]).zip(&r[c0..c0 + n])
                {
                    o[0] = (a << shift) as i16;
                    o[1] = (b << shift) as i16;
                }
            } else {
                for (f, o) in out[..n * ch].chunks_exact_mut(ch).enumerate() {
                    for (c, s) in o.iter_mut().enumerate() {
                        *s = (self.planar[c * self.stride + c0 + f] << shift) as i16;
                    }
                }
            }
        }
        self.cursor += n;
        n * ch
    }

    /// One step of the seek search.
    fn seek_step(&mut self, src: &mut dyn SeekSource, mut st: SeekSt) -> Result<SeekStep, DecodeError> {
        let nominal = u64::from(self.head.params.max_block);
        loop {
            if !st.started {
                st.started = true;
                let pos = if st.linear || st.hi <= st.lo + 2 * self.want() as u64 {
                    st.linear = true;
                    st.lo
                } else if st.probes == 0 && st.lo_s <= st.target {
                    // First probe at the seek point itself.
                    st.lo
                } else {
                    let span = st.hi - st.lo;
                    let frac = if st.hi_s > st.lo_s {
                        (u128::from(st.target - st.lo_s.min(st.target)) * u128::from(span)
                            / u128::from(st.hi_s - st.lo_s)) as u64
                    } else {
                        span / 2
                    };
                    // Aim a little early: landing before the target walks,
                    // landing after it bisects again.
                    st.lo + frac.saturating_sub(self.want() as u64).min(span.saturating_sub(1))
                };
                st.probes += 1;
                if st.probes > 64 {
                    st.linear = true;
                }
                st.at = pos;
                src.seek(pos)?;
            }
            if !st.linear && src.position() >= st.hi && st.at < st.hi {
                // The probe scanned past the bracket without a header: no
                // frame starts between the probe and `hi`.
                st.hi = st.at.max(st.lo);
                st.linear = st.hi <= st.lo + 1;
                st.started = false;
                continue;
            }
            match self.sync(src) {
                Ok(h) => {
                    let at = src.position();
                    if h.first <= st.target && st.target < h.first + u64::from(h.block) {
                        self.discard = st.target - h.first;
                        self.seek = None;
                        self.block = 0;
                        self.cursor = 0;
                        self.ended = false;
                        return Ok(SeekStep::Done(st.target));
                    }
                    if h.first > st.target {
                        if st.linear || at <= st.lo {
                            // The walk passed the target (a gap): land here.
                            self.seek = None;
                            self.block = 0;
                            self.cursor = 0;
                            self.discard = 0;
                            self.ended = false;
                            return Ok(SeekStep::Done(h.first));
                        }
                        st.hi = at;
                        st.hi_s = h.first;
                        st.started = false;
                        continue;
                    }
                    // Before the target.
                    st.lo = at;
                    st.lo_s = h.first + u64::from(h.block);
                    if st.linear || st.target - h.first < 8 * nominal {
                        st.linear = true;
                        // Step over this header; the next sync is the next frame.
                        src.consume(2);
                        continue;
                    }
                    st.started = false;
                }
                Err(Got::Need(n)) => {
                    self.seek = Some(st);
                    return Ok(SeekStep::NeedData(n));
                }
                Err(Got::Corrupt) => {}
                Err(Got::End | Got::Block) => {
                    if st.linear {
                        // Ran off the end: the target is past the last frame.
                        self.seek = None;
                        self.ended = true;
                        self.block = 0;
                        self.cursor = 0;
                        return Ok(SeekStep::Done(st.target.min(self.next_sample.max(st.lo_s))));
                    }
                    // No frame between the probe and the end: search below.
                    st.hi = st.at.max(st.lo);
                    st.linear = st.hi <= st.lo + 1;
                    st.started = false;
                }
            }
        }
    }
}

impl<'a> Decoder<'a> for FlacDecoder<'a> {
    const ARENA_MIN: usize = ARENA_MIN;

    fn inspect(src: &dyn ByteSource, _hint: &ContainerInfo) -> Result<Plan, OpenError> {
        let h = parse_head(src)?;
        let arena = arena_for(usize::from(h.params.channels), h.params.max_block as usize);
        Ok(Plan { info: h.info, arena })
    }

    fn open(
        src: &mut dyn ByteSource,
        arena: &'a mut [u8],
        _hint: &ContainerInfo,
    ) -> Result<(Self, StreamInfo), OpenError> {
        let mut head = parse_head(src)?;
        // Fixed-blocksize streams number frames; the nominal size turns a
        // frame number into a sample number.
        head.params.nominal_block = if head.min_block == head.params.max_block {
            head.params.max_block
        } else {
            head.min_block.max(16)
        };
        let stride = head.params.max_block as usize;
        let mut ar = Arena::new(arena);
        let planar = ar.slice::<i32>(usize::from(head.params.channels) * stride)?;
        let out = ar.slice::<i32>(OUT_WORDS)?;
        let points = ar.slice::<[u64; 2]>(SEEK_POINTS)?;
        src.consume(head.len);
        let audio_start = src.position();
        let d = FlacDecoder {
            head,
            planar,
            out,
            points,
            n_points: 0,
            stride,
            used: ar.used(),
            meta: if head.last { Meta::Done } else { Meta::Header },
            last_block: head.last,
            audio_start,
            block: 0,
            cursor: 0,
            discard: 0,
            seek: None,
            ended: false,
            next_sample: 0,
        };
        Ok((d, head.info))
    }

    fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError> {
        if let Some(n) = self.metadata(src) {
            return Ok(Step::NeedData(n));
        }
        if self.meta != Meta::Done {
            self.ended = true;
        }
        loop {
            if self.cursor < self.block {
                let n = self.emit();
                if n > 0 {
                    return Ok(Step::Pcm(if self.head.params.bps > 16 {
                        PcmBlock::I32(&mut self.out[..n])
                    } else {
                        PcmBlock::I16(&mut arena::as_i16(self.out)[..n])
                    }));
                }
                continue;
            }
            if self.ended {
                return Ok(Step::EndOfStream);
            }
            match self.next_frame(src) {
                Got::Block => {}
                Got::Need(n) => return Ok(Step::NeedData(n)),
                Got::End => {
                    self.ended = true;
                    return Ok(Step::EndOfStream);
                }
                Got::Corrupt => return Err(DecodeError::Corrupt),
            }
        }
    }

    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError> {
        if let Some(n) = self.metadata(src) {
            return Ok(SeekStep::NeedData(n));
        }
        let st = match self.seek {
            Some(st) if st.target == frame.min(self.total_or_max()) => st,
            _ => {
                let target = frame.min(self.total_or_max());
                if self.head.total != 0 && target >= self.head.total {
                    self.ended = true;
                    self.block = 0;
                    self.cursor = 0;
                    return Ok(SeekStep::Done(self.head.total));
                }
                let end = src.len().unwrap_or(u64::MAX).max(self.audio_start);
                let (mut lo, mut lo_s, mut hi, mut hi_s) = (self.audio_start, 0, end, self.head.total);
                for p in &self.points[..self.n_points] {
                    let at = self.audio_start.saturating_add(p[1]);
                    if p[0] <= target && at >= lo && at < end {
                        lo = at;
                        lo_s = p[0];
                    } else if p[0] > target && at < hi {
                        hi = at;
                        hi_s = p[0];
                        break;
                    }
                }
                if hi == u64::MAX {
                    hi = lo;
                }
                SeekSt { target, lo, lo_s, hi, hi_s, probes: 0, at: lo, linear: hi <= lo, started: false }
            }
        };
        self.seek_step(src, st)
    }

    fn arena_used(&self) -> usize {
        self.used
    }
}

impl FlacDecoder<'_> {
    fn total_or_max(&self) -> u64 {
        if self.head.total != 0 { self.head.total } else { u64::MAX }
    }
}

#[cfg(test)]
mod tests;
