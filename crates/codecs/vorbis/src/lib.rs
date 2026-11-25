//! Ogg Vorbis decoder (04 §4.1 priority A, WP AU-7), written from the
//! Xiph Vorbis I specification; `lewton` (MIT/Apache-2.0) was read for
//! cross-checking and its copy of the spec's floor1 inverse-dB table is
//! used verbatim (README.md).
//!
//! Low-memory design, everything in the codec arena:
//! * codebooks as binary decode trees (4 bytes per used entry) with the VQ
//!   multiplicands unexpanded — no per-entry vectors;
//! * per channel only the spectrum/DCT-IV buffer (n/2) and the quarter
//!   block the next overlap needs (n/4); the IMDCT output is never stored
//!   (see `audio.rs`);
//! * one quarter-wave cosine table (n_long + 1 floats) serves the FFT and
//!   the DCT-IV twiddles of both block sizes; two window slopes.
//!
//! Single-precision float (the M4F's FPU), no `f64`, no `libm`.
//!
//! Gapless: output is exact by the spec's block arithmetic (the first
//! packet yields nothing; each later one the span from the previous block's
//! centre to its own); the last page's granule position trims the end.
//! Seeking: bisection on page granule positions, then decoding from the
//! chosen page with output dropped until a granule position fixes the
//! sample position, then up to the exact target.
//!
//! Refused: floor type 0 (no encoder since 2001 writes it), more than 8
//! channels, codebooks with more than 32 767 entries.

#![warn(missing_docs)]
#![no_std]
#![forbid(unsafe_code)]
// Index loops mirror the specification's pseudocode.
#![allow(clippy::needless_range_loop)]

#[cfg(test)]
extern crate std;

mod audio;
mod bits;
mod math;
mod setup;

use audio::{Block, Buffers};
use bits::Bits;
use orpheus_audio::arena::{self, Arena};
use orpheus_audio::{
    ByteSource, CodecId, ContainerInfo, DecodeError, Decoder, MAX_PEEK, OpenError, PcmBlock, Plan,
    SampleKind, SeekSource, SeekStep, Step, StreamInfo, Unsupported, View,
};
use orpheus_ogg::ranges::{Next, Packet, PageErr, Reader, peek_packets};
use orpheus_ogg::{Bisect, NO_GRANULE};
use setup::{Book, Floor1, Ident, MAX_X, Mapping, Mode, Residue, Setup, SetupError, Sizes};

/// PCM handed out per call, in frames.
const OUT_FRAMES: usize = 512;
/// The arena a stream is planned with when its set-up header is not
/// visible at inspection (a huge comment packet before it).
pub const ARENA_MAX: usize = 72 * 1024;

fn buffers_need(id: &Ident) -> usize {
    let ch = usize::from(id.channels);
    let n0 = 1usize << id.bs[0];
    let n1 = 1usize << id.bs[1];
    arena::need::<f32>(ch * n1 / 2)
        + arena::need::<f32>(ch * n1 / 4)
        + arena::need::<[f32; 2]>(n1 / 4)
        + arena::need::<f32>(n1 + 1)
        + arena::need::<f32>(n0 / 2)
        + arena::need::<f32>(n1 / 2)
        + arena::need::<i32>(ch * MAX_X)
        + arena::need::<i16>(OUT_FRAMES * ch)
}

fn setup_need(s: &Sizes) -> usize {
    arena::need::<Book>(s.books)
        + arena::need::<[u16; 2]>(s.nodes.max(1))
        + arena::need::<u16>(s.vals.max(1))
        + arena::need::<Floor1>(s.floors)
        + arena::need::<Residue>(s.residues)
        + arena::need::<Mapping>(s.mappings)
        + arena::need::<Mode>(s.modes)
        + arena::need::<u8>(s.classes.max(1))
}

fn bad(e: SetupError) -> Unsupported {
    match e {
        SetupError::Invalid => Unsupported::BadHeader,
        SetupError::Unsupported => Unsupported::Encoding,
    }
}

fn info_of(id: &Ident) -> StreamInfo {
    StreamInfo {
        rate: id.rate,
        channels: id.channels,
        bits: 16,
        sample: SampleKind::I16,
        frames: None,
        delay: 0,
        padding: 0,
        bitrate: id.bitrate,
        codec: CodecId::Vorbis,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Comment,
    Setup,
    Audio,
    End,
}

/// The previous block, as far as the next overlap needs it.
#[derive(Debug, Clone, Copy, Default)]
struct Prev {
    n: usize,
    rs: usize,
    re: usize,
}

/// Output of the current packet still to hand out.
#[derive(Debug, Clone, Copy, Default)]
struct Emit {
    blk: Block,
    prev: Option<Prev>,
    /// Next sample index within the packet's span, and the end of the
    /// part handed out.
    k: usize,
    hi: usize,
    active: bool,
}

/// An open Ogg Vorbis stream; every buffer lives in the arena.
pub struct VorbisDecoder<'a> {
    arena: Arena<'a>,
    id: Ident,
    ogg: Reader,
    phase: Phase,
    setup: Option<Setup<'a>>,
    bufs: Buffers<'a>,
    out: &'a mut [i16],
    prev: Option<Prev>,
    emit: Emit,
    /// Absolute sample position of the next sample of the stream (None
    /// after a seek until a granule position says).
    pos: Option<u64>,
    /// Drop samples before this position.
    discard: u64,
    /// Where the audio packets start: source position and reader state.
    audio_at: Option<(u64, Reader)>,
    /// Bisection in progress (kept across `NeedData`).
    seek: Option<Bisect>,
}

impl VorbisDecoder<'_> {
    fn header_packets(src: &dyn ByteSource) -> Result<(Ident, Option<Sizes>), OpenError> {
        let v = View::of(src);
        let mut pk = [Packet::default(); 3];
        match peek_packets(&v, src.at_end(), &mut pk[..1]) {
            Ok(()) => {}
            Err(PageErr::Need(n)) if n <= MAX_PEEK && !src.at_end() => return Err(OpenError::NeedData(n)),
            Err(_) => return Err(Unsupported::NotThisFormat.into()),
        }
        let id = setup::ident(&mut Bits::new(v, &pk[0])).map_err(bad)?;
        match peek_packets(&v, src.at_end(), &mut pk) {
            Ok(()) => {
                let sz = setup::setup(&mut Bits::new(v, &pk[2]), &id, None).map_err(bad)?;
                Ok((id, Some(sz)))
            }
            Err(PageErr::Need(n)) if n <= MAX_PEEK && !src.at_end() => Err(OpenError::NeedData(n)),
            // The comment packet is larger than the ring: plan for the
            // largest set-up instead.
            Err(_) => Ok((id, None)),
        }
    }

    /// Parse the set-up packet into the arena. Not inlined (nor
    /// [`Self::audio_packet`]): inlined into `step`, their temporaries sat
    /// above the Ogg reader's call chain (a 1.9 KB frame; VER-10).
    #[inline(never)]
    fn load_setup(&mut self, v: View<'_>, p: &Packet) -> Result<(), DecodeError> {
        let fail = |e: SetupError| DecodeError::Unsupported(bad(e));
        let sz = setup::setup(&mut Bits::new(v, p), &self.id, None).map_err(fail)?;
        let full = |e: orpheus_audio::ArenaFull| {
            DecodeError::Unsupported(Unsupported::ArenaTooSmall { needed: e.needed, capacity: e.capacity })
        };
        let ar = &mut self.arena;
        let mut s = Setup {
            books: ar.slice::<Book>(sz.books).map_err(full)?,
            nodes: ar.slice::<[u16; 2]>(sz.nodes.max(1)).map_err(full)?,
            vals: ar.slice::<u16>(sz.vals.max(1)).map_err(full)?,
            floors: ar.slice::<Floor1>(sz.floors).map_err(full)?,
            residues: ar.slice::<Residue>(sz.residues).map_err(full)?,
            mappings: ar.slice::<Mapping>(sz.mappings).map_err(full)?,
            modes: ar.slice::<Mode>(sz.modes).map_err(full)?,
        };
        self.bufs.classes = ar.slice::<u8>(sz.classes.max(1)).map_err(full)?;
        setup::setup(&mut Bits::new(v, p), &self.id, Some((&mut s, &sz))).map_err(fail)?;
        self.setup = Some(s);
        Ok(())
    }

    /// Hand out the next part of the current packet's output.
    fn emit_chunk(&mut self) -> usize {
        let e = self.emit;
        let ch = usize::from(self.id.channels);
        let n = (e.hi - e.k).min(OUT_FRAMES);
        let blk = e.blk;
        let stride = self.bufs.buf.len() / ch;
        let qstride = self.bufs.prev.len() / ch;
        let n0 = 1usize << self.id.bs[0];
        // Current block index of the first sample of this packet's span.
        let start = (blk.n / 4) as isize - e.prev.map_or(0, |p| p.n / 4) as isize;
        let lslope = blk.left_end - blk.left_start;
        let lwin = &self.bufs.win[usize::from(lslope != n0 / 2)];
        for f in 0..n {
            let k = e.k + f;
            let t = start + k as isize;
            for c in 0..ch {
                let u = &self.bufs.buf[c * stride..c * stride + blk.n / 2];
                let mut v = 0.0f32;
                if t >= blk.left_start as isize {
                    let t = t as usize;
                    let w = if t < blk.left_end {
                        lwin.get(t - blk.left_start).copied().unwrap_or(1.0)
                    } else {
                        1.0
                    };
                    v = w * audio::y(u, blk.n, t);
                }
                if let Some(p) = e.prev {
                    let tp = p.n / 2 + k;
                    if tp < p.re {
                        let up = &self.bufs.prev[c * qstride..c * qstride + p.n / 4];
                        let w = if tp < p.rs {
                            1.0
                        } else {
                            let l = p.re - p.rs;
                            let win = &self.bufs.win[usize::from(l != n0 / 2)];
                            win.get(l - 1 - (tp - p.rs)).copied().unwrap_or(0.0)
                        };
                        v += w * audio::y(up, p.n, tp);
                    }
                }
                let x = v * 32768.0;
                let r = if x >= 0.0 { (x + 0.5) as i32 } else { (x - 0.5) as i32 };
                self.out[f * ch + c] = r.clamp(-32768, 32767) as i16;
            }
        }
        self.emit.k += n;
        if self.emit.k >= self.emit.hi {
            self.finish_packet();
        }
        n * ch
    }

    /// Keep what the next overlap needs of the current block.
    fn finish_packet(&mut self) {
        if !self.emit.active {
            return;
        }
        let ch = usize::from(self.id.channels);
        let blk = self.emit.blk;
        let stride = self.bufs.buf.len() / ch;
        let qstride = self.bufs.prev.len() / ch;
        for c in 0..ch {
            let q = blk.n / 4;
            self.bufs.prev[c * qstride..c * qstride + q]
                .copy_from_slice(&self.bufs.buf[c * stride..c * stride + q]);
        }
        self.prev = Some(Prev { n: blk.n, rs: blk.right_start, re: blk.right_end });
        self.emit.active = false;
    }

    /// Decode an audio packet and set up its emission.
    #[inline(never)]
    fn audio_packet(&mut self, v: View<'_>, p: &Packet) -> bool {
        let Some(s) = self.setup.as_ref() else { return false };
        let ch = usize::from(self.id.channels);
        let mut b = Bits::new(v, p);
        let Some(blk) = audio::decode_packet(s, self.id.bs, ch, &mut self.bufs, &mut b) else { return false };
        let count = self.prev.map_or(0, |pv| pv.n / 4 + blk.n / 4);
        let p0 = match (self.pos, p.granule) {
            (Some(x), _) => Some(x),
            (None, g) if g != NO_GRANULE => Some(g.saturating_sub(count as u64)),
            _ => None,
        };
        let (mut lo, mut hi) = (count, count);
        if let Some(p0) = p0 {
            lo = (self.discard.saturating_sub(p0) as usize).min(count);
            if p.eos && p.granule != NO_GRANULE {
                hi = (p.granule.saturating_sub(p0) as usize).min(count);
            }
            self.pos = Some(p0 + count as u64);
        }
        self.emit = Emit { blk, prev: self.prev, k: lo, hi: hi.max(lo), active: true };
        if self.emit.k >= self.emit.hi {
            self.finish_packet();
        }
        true
    }

    fn reset_audio(&mut self) {
        self.prev = None;
        self.emit = Emit::default();
    }

    /// The decode loop without the PCM borrow; `Some(n)`: n samples ready.
    fn step(&mut self, src: &mut dyn ByteSource) -> Result<Result<usize, Step<'static>>, DecodeError> {
        loop {
            if self.emit.active && self.emit.k < self.emit.hi {
                return Ok(Ok(self.emit_chunk()));
            }
            if self.phase == Phase::End {
                return Ok(Err(Step::EndOfStream));
            }
            let p = match self.ogg.next(src) {
                Next::Packet(p) => p,
                Next::NeedData(n) => return Ok(Err(Step::NeedData(n))),
                Next::End => {
                    if self.phase != Phase::Audio {
                        return Err(DecodeError::Unsupported(Unsupported::BadHeader));
                    }
                    self.phase = Phase::End;
                    return Ok(Err(Step::EndOfStream));
                }
                Next::Corrupt => {
                    if self.phase == Phase::Comment {
                        // A comment packet too large for the ring was skipped.
                        self.phase = Phase::Setup;
                        continue;
                    }
                    self.reset_audio();
                    return Err(DecodeError::Corrupt);
                }
            };
            match self.phase {
                Phase::Comment => {
                    self.ogg.commit(src, &p);
                    self.phase = Phase::Setup;
                }
                Phase::Setup => {
                    let r = self.load_setup(View::of(src), &p);
                    self.ogg.commit(src, &p);
                    r?;
                    self.audio_at = Some((src.position(), self.ogg));
                    self.phase = Phase::Audio;
                }
                Phase::Audio => {
                    let ok = self.audio_packet(View::of(src), &p);
                    self.ogg.commit(src, &p);
                    if !ok {
                        // Undecodable: the overlap chain restarts.
                        self.prev = None;
                    }
                }
                Phase::End => {}
            }
        }
    }

    fn land(
        &mut self,
        src: &mut dyn SeekSource,
        at: Option<u64>,
        target: u64,
    ) -> Result<SeekStep, DecodeError> {
        self.reset_audio();
        self.discard = target;
        self.seek = None;
        self.phase = Phase::Audio;
        match at {
            Some(off) => {
                src.seek(off)?;
                self.ogg.reset();
                self.pos = None;
            }
            None => {
                let (off, r) = self.audio_at.unwrap_or((0, Reader::new()));
                src.seek(off)?;
                self.ogg = r;
                self.pos = Some(0);
            }
        }
        Ok(SeekStep::Done(target))
    }
}

impl<'a> Decoder<'a> for VorbisDecoder<'a> {
    const ARENA_MIN: usize = 16 * 1024;

    fn inspect(src: &dyn ByteSource, _hint: &ContainerInfo) -> Result<Plan, OpenError> {
        let (id, sz) = Self::header_packets(src)?;
        let arena = match sz {
            Some(sz) => buffers_need(&id) + setup_need(&sz),
            None => ARENA_MAX,
        };
        Ok(Plan { info: info_of(&id), arena })
    }

    fn open(
        src: &mut dyn ByteSource,
        arena: &'a mut [u8],
        _hint: &ContainerInfo,
    ) -> Result<(Self, StreamInfo), OpenError> {
        let (id, _) = Self::header_packets(src)?;
        let info = info_of(&id);
        let mut ar = Arena::new(arena);
        let ch = usize::from(id.channels);
        let n0 = 1usize << id.bs[0];
        let n1 = 1usize << id.bs[1];
        let buf = ar.slice::<f32>(ch * n1 / 2)?;
        let prev = ar.slice::<f32>(ch * n1 / 4)?;
        let scratch = ar.slice::<[f32; 2]>(n1 / 4)?;
        let quarter = ar.slice::<f32>(n1 + 1)?;
        let w0 = ar.slice::<f32>(n0 / 2)?;
        let w1 = ar.slice::<f32>(n1 / 2)?;
        let fy = ar.slice::<i32>(ch * MAX_X)?;
        let out = ar.slice::<i16>(OUT_FRAMES * ch)?;
        math::fill_quarter(quarter);
        math::window(w0);
        math::window(w1);
        let mut ogg = Reader::new();
        // The identification packet.
        loop {
            match ogg.next(src) {
                Next::Packet(p) => {
                    ogg.commit(src, &p);
                    break;
                }
                Next::Corrupt => {}
                Next::NeedData(n) => return Err(OpenError::NeedData(n)),
                Next::End => return Err(Unsupported::BadHeader.into()),
            }
        }
        let d = VorbisDecoder {
            arena: ar,
            id,
            ogg,
            phase: Phase::Comment,
            setup: None,
            bufs: Buffers { buf, prev, scratch, quarter, win: [w0, w1], fy, classes: &mut [] },
            out,
            prev: None,
            emit: Emit::default(),
            pos: Some(0),
            discard: 0,
            audio_at: None,
            seek: None,
        };
        Ok((d, info))
    }

    fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError> {
        match self.step(src)? {
            Ok(n) => Ok(Step::Pcm(PcmBlock::I16(&mut self.out[..n]))),
            Err(Step::NeedData(n)) => Ok(Step::NeedData(n)),
            Err(_) => Ok(Step::EndOfStream),
        }
    }

    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError> {
        // The headers must be read first.
        while self.phase != Phase::Audio && self.phase != Phase::End {
            match self.step(src)? {
                Ok(_) => {}
                Err(Step::NeedData(n)) => return Ok(SeekStep::NeedData(n)),
                Err(_) => return Ok(SeekStep::Done(frame)),
            }
        }
        let Some((audio_off, _)) = self.audio_at else { return Ok(SeekStep::Done(frame)) };
        let Some(serial) = self.ogg.serial() else { return Ok(SeekStep::Done(frame)) };
        let mut b = match self.seek {
            Some(b) if b.target() == frame => b,
            _ => Bisect::new(audio_off, src.len().unwrap_or(u64::MAX), frame, serial),
        };
        match b.step(src)? {
            orpheus_ogg::SeekStep::NeedData(n) => {
                self.seek = Some(b);
                Ok(SeekStep::NeedData(n))
            }
            // Pages of granule 0 (only the first audio packet ends there)
            // position nothing: start from the first audio packet instead.
            orpheus_ogg::SeekStep::Done(found) => {
                let at = found.filter(|&(_, g)| g > 0).map(|(off, _)| off);
                self.land(src, at, frame)
            }
        }
    }

    fn arena_used(&self) -> usize {
        self.arena.used()
    }
}

#[cfg(test)]
mod tests;
