//! WAV-family decoder (04 §4.1 priority A): RIFF/WAVE (PCM 8/16/24/32-bit,
//! `WAVE_FORMAT_EXTENSIBLE`, IEEE float 32/64, A-law, µ-law, IMA ADPCM,
//! MS ADPCM), AIFF and AIFF-C (`NONE`/`twos`/`sowt`/`raw `/`fl32`/`fl64`/
//! `alaw`/`ulaw`/`ima4`) and Sun AU.
//!
//! Output: `i16` for sources of up to 16 bits (and the companded and ADPCM
//! encodings), left-justified `i32` for 24/32-bit and float sources. Up to
//! eight channels are passed through interleaved (MS ADPCM: two).
//!
//! Memory: one 3 KiB output buffer from the arena ([`ARENA`] bytes in all);
//! the decoder never copies the input except for a frame that straddles the
//! ring's wrap point (at most 64 bytes, on the stack).
//!
//! Written from the format descriptions; no third-party decoder is used (see
//! README.md).

#![warn(missing_docs)]
#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

mod adpcm;
mod conv;
mod header;

pub use conv::{alaw, f32_bits_to_i32, f64_bits_to_i32, ulaw};

use adpcm::{Ima, Ms};
use conv::Enc;
use header::{Kind, Layout};
use orpheus_audio::arena::{self, Arena};
use orpheus_audio::source::skip;
use orpheus_audio::{
    ByteSource, CodecId, ContainerInfo, DecodeError, Decoder, OpenError, PcmBlock, Plan, SampleKind,
    SeekSource, SeekStep, Step, StreamInfo, View,
};

/// Output buffer, in `i32` words (3 KiB).
const OUT_WORDS: usize = 768;

/// Arena bytes every WAV-family stream needs.
pub const ARENA: usize = arena::need::<i32>(OUT_WORDS);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    /// Looking for the data chunk: `skip` bytes still to step over; with
    /// `then` set, the audio starts after them (`then` = its length).
    Walk {
        skip: u64,
        then: Option<Option<u64>>,
    },
    Data,
    End,
}

/// Progress through one ADPCM block.
#[derive(Debug, Clone, Copy, Default)]
struct Blk {
    active: bool,
    len: u32,
    /// Frames produced from this block so far.
    pos: u32,
    frames: u32,
}

enum Fill {
    /// Output samples `out[start..end]`.
    Range(usize, usize),
    Need(usize),
    End,
    Corrupt,
}

/// An open WAV (RIFF PCM) stream.
pub struct WavDecoder<'a> {
    lay: Layout,
    out: &'a mut [i32],
    used: usize,
    phase: Phase,
    data_start: Option<u64>,
    data_len: Option<u64>,
    remaining: Option<u64>,
    discard: u64,
    blk: Blk,
    ima: [Ima; 8],
    ms: [Ms; 2],
}

fn stream_info(l: &Layout) -> StreamInfo {
    let frames = match l.data {
        Some((_, Some(len))) => Some(frames_in(l, len)),
        _ => l.frames,
    };
    let codec = match l.enc {
        Enc::Int { .. } => CodecId::Pcm,
        Enc::Float { .. } => CodecId::FloatPcm,
        Enc::ALaw => CodecId::ALaw,
        Enc::MuLaw => CodecId::MuLaw,
        Enc::Ima => CodecId::ImaAdpcm,
        Enc::ImaQt => CodecId::ImaQt,
        Enc::Ms => CodecId::MsAdpcm,
    };
    StreamInfo {
        rate: l.rate,
        channels: l.channels,
        bits: l.bits,
        sample: if l.enc.wide() { SampleKind::I32 } else { SampleKind::I16 },
        frames,
        delay: 0,
        padding: 0,
        bitrate: (u64::from(l.rate) * u64::from(l.block) * 8 / u64::from(l.spb.max(1)))
            .min(u64::from(u32::MAX)) as u32,
        codec,
    }
}

/// Frames decoded from `len` bytes of audio data (whole blocks plus what a
/// final partial block yields).
fn frames_in(l: &Layout, len: u64) -> u64 {
    let block = u64::from(l.block.max(1));
    let full = len / block * u64::from(l.spb);
    let last = len % block;
    let ch = u64::from(l.channels);
    let part = match l.enc {
        Enc::Ima if last >= 4 * ch => 1 + (last - 4 * ch) / (4 * ch) * 8,
        Enc::Ms if last >= 7 * ch => (last - 6 * ch) * 2 / ch,
        _ => 0,
    };
    full + part
}

impl WavDecoder<'_> {
    /// Start the audio at `off` bytes from the read position (skipping what
    /// is not buffered yet).
    fn enter(&mut self, src: &mut dyn ByteSource, off: u64, len: Option<u64>) {
        let k = skip(src, off);
        if k == off {
            self.start_data(src, len);
        } else {
            self.phase = Phase::Walk { skip: off - k, then: Some(len) };
        }
    }

    fn start_data(&mut self, src: &dyn ByteSource, len: Option<u64>) {
        let start = src.position();
        let len = match (len, src.len()) {
            (Some(l), Some(total)) => Some(l.min(total.saturating_sub(start))),
            (l, _) => l,
        };
        self.data_start = Some(start);
        self.data_len = len;
        self.remaining = len;
        self.phase = Phase::Data;
    }

    /// Walk chunks until the audio data starts.
    fn walk(&mut self, src: &mut dyn ByteSource) -> Fill {
        loop {
            let Phase::Walk { skip: mut left, then } = self.phase else {
                return if self.phase == Phase::End { Fill::End } else { Fill::Range(0, 0) };
            };
            if left > 0 {
                left -= skip(src, left);
                self.phase = Phase::Walk { skip: left, then };
                if left > 0 {
                    if src.at_end() {
                        self.phase = Phase::End;
                        return Fill::End;
                    }
                    return Fill::Need(left.min(4096) as usize);
                }
            }
            if let Some(len) = then {
                self.start_data(src, len);
                return Fill::Range(0, 0);
            }
            let hdr_len = if self.lay.kind == Kind::Aiff { 16 } else { 8 };
            let Some(v) = View::peek(src, hdr_len) else {
                if src.at_end() {
                    self.phase = Phase::End;
                    return Fill::End;
                }
                return Fill::Need(hdr_len);
            };
            let id: [u8; 4] = v.array(0).unwrap_or_default();
            if self.lay.kind == Kind::Aiff {
                let size = u64::from(v.u32_be(4).unwrap_or(0));
                if &id == b"SSND" {
                    let off = u64::from(v.u32_be(8).unwrap_or(0));
                    src.consume(16);
                    self.phase = Phase::Walk { skip: off, then: Some(size.checked_sub(8 + off)) };
                } else {
                    src.consume(8);
                    self.phase = Phase::Walk { skip: size + (size & 1), then: None };
                }
            } else {
                let size = u64::from(v.u32_le(4).unwrap_or(0));
                src.consume(8);
                if &id == b"data" {
                    let len = if size == 0 || size == u64::from(u32::MAX) { None } else { Some(size) };
                    self.start_data(src, len);
                    return Fill::Range(0, 0);
                }
                self.phase = Phase::Walk { skip: size + (size & 1), then: None };
            }
        }
    }

    fn take_remaining(&mut self, n: usize) {
        if let Some(r) = &mut self.remaining {
            *r = r.saturating_sub(n as u64);
        }
    }

    /// Drop frames still owed to a seek from the `n` frames just produced;
    /// `Some(range)` if any remain.
    fn after_discard(&mut self, n: usize) -> Option<(usize, usize)> {
        let ch = usize::from(self.lay.channels);
        let d = (self.discard.min(n as u64)) as usize;
        self.discard -= d as u64;
        (d < n).then_some((d * ch, n * ch))
    }

    fn fill_pcm(&mut self, src: &mut dyn ByteSource) -> Fill {
        let enc = self.lay.enc;
        let ch = usize::from(self.lay.channels);
        let fb = self.lay.block as usize;
        let ss = fb / ch;
        let cap = if enc.wide() { self.out.len() } else { self.out.len() * 2 } / ch;
        loop {
            let rem = self.remaining.map(|r| r / fb as u64);
            if rem == Some(0) {
                return Fill::End;
            }
            let avail = src.available() / fb;
            if avail == 0 {
                return if src.at_end() { Fill::End } else { Fill::Need(fb) };
            }
            let mut want = avail.min(cap);
            if let Some(r) = rem {
                want = want.min(r as usize);
            }
            let Some((a, b)) = src.peek(want * fb) else { return Fill::Need(want * fb) };
            let mut tmp = [0u8; 64];
            let (n, bytes): (usize, &[u8]) = if a.len() >= fb {
                let n = (a.len() / fb).min(want);
                (n, &a[..n * fb])
            } else {
                // One frame straddles the wrap point.
                tmp[..a.len()].copy_from_slice(a);
                tmp[a.len()..fb].copy_from_slice(&b[..fb - a.len()]);
                (1, &tmp[..fb])
            };
            if enc.wide() {
                convert_wide(enc, ss, bytes, &mut self.out[..n * ch]);
            } else {
                convert_narrow(enc, ss, bytes, &mut arena::as_i16(self.out)[..n * ch]);
            }
            src.consume(n * fb);
            self.take_remaining(n * fb);
            if let Some((s, e)) = self.after_discard(n) {
                return Fill::Range(s, e);
            }
        }
    }

    fn fill_adpcm(&mut self, src: &mut dyn ByteSource) -> Fill {
        let ch = usize::from(self.lay.channels);
        loop {
            if !self.blk.active {
                if self.remaining == Some(0) {
                    return Fill::End;
                }
                let full = u64::from(self.lay.block);
                let mut len = self.remaining.map_or(full, |r| r.min(full)) as usize;
                if src.available() < len {
                    if !src.at_end() {
                        return Fill::Need(len);
                    }
                    len = src.available();
                }
                let frames = frames_in(&self.lay, len as u64) as u32;
                if frames == 0 {
                    // Trailing bytes too short to hold a block header.
                    src.consume(len);
                    self.remaining = Some(0);
                    return Fill::End;
                }
                self.blk = Blk { active: true, len: len as u32, pos: 0, frames };
            }
            let Some(v) = View::peek(src, self.blk.len as usize) else {
                return Fill::Need(self.blk.len as usize);
            };
            let out = arena::as_i16(self.out);
            let cap = out.len() / ch;
            let produced = match self.lay.enc {
                Enc::Ima => ima_wav(&v, &mut self.blk, &mut self.ima[..ch], out, cap),
                Enc::Ms => ms(&v, &mut self.blk, &mut self.ms[..ch], out, cap),
                _ => ima_qt(&v, &mut self.blk, &mut self.ima[..ch], out),
            };
            let Some(n) = produced else {
                let len = self.blk.len as usize;
                src.consume(len);
                self.take_remaining(len);
                self.blk.active = false;
                return Fill::Corrupt;
            };
            if self.blk.pos >= self.blk.frames {
                let len = self.blk.len as usize;
                src.consume(len);
                self.take_remaining(len);
                self.blk.active = false;
            }
            if n > 0
                && let Some((s, e)) = self.after_discard(n)
            {
                return Fill::Range(s, e);
            }
        }
    }

    fn fill(&mut self, src: &mut dyn ByteSource) -> Fill {
        loop {
            match self.phase {
                Phase::End => return Fill::End,
                Phase::Walk { .. } => match self.walk(src) {
                    Fill::Range(..) => {}
                    other => return other,
                },
                Phase::Data => break,
            }
        }
        let r = match self.lay.enc {
            Enc::Ima | Enc::Ms | Enc::ImaQt => self.fill_adpcm(src),
            _ => self.fill_pcm(src),
        };
        if let Fill::End = r {
            self.phase = Phase::End;
        }
        r
    }
}

fn convert_narrow(enc: Enc, ss: usize, bytes: &[u8], out: &mut [i16]) {
    let it = bytes.chunks_exact(ss).zip(out.iter_mut());
    match enc {
        Enc::Int { bytes: 2, be: false, .. } => it.for_each(|(s, o)| *o = i16::from_le_bytes([s[0], s[1]])),
        _ => it.for_each(|(s, o)| *o = conv::to_i16(enc, s)),
    }
}

fn convert_wide(enc: Enc, ss: usize, bytes: &[u8], out: &mut [i32]) {
    let it = bytes.chunks_exact(ss).zip(out.iter_mut());
    match enc {
        Enc::Int { bytes: 3, be: false, .. } => {
            it.for_each(|(s, o)| *o = i32::from_le_bytes([0, s[0], s[1], s[2]]));
        }
        _ => it.for_each(|(s, o)| *o = conv::to_i32(enc, s)),
    }
}

/// IMA ADPCM, WAV layout: per channel a 4-byte header (predictor, index),
/// then groups of 4 bytes (8 samples) per channel in turn. Produces whole
/// groups while `cap` frames fit. `None`: corrupt header.
fn ima_wav(v: &View<'_>, blk: &mut Blk, st: &mut [Ima], out: &mut [i16], cap: usize) -> Option<usize> {
    let ch = st.len();
    let mut n = 0usize;
    if blk.pos == 0 {
        for (i, s) in st.iter_mut().enumerate() {
            s.predictor = i32::from(v.u16_le(4 * i)? as i16);
            s.index = i32::from(v.u16_le(4 * i + 2)? as i16);
            if !(0..=88).contains(&s.index) {
                return None;
            }
            out[i] = s.predictor as i16;
        }
        n = 1;
        blk.pos = 1;
    }
    while blk.pos < blk.frames && n + 8 <= cap {
        let g = (blk.pos as usize - 1) / 8;
        for (i, s) in st.iter_mut().enumerate() {
            let base = 4 * ch + g * 4 * ch + 4 * i;
            for k in 0..4 {
                let byte = v.byte(base + k);
                out[(n + 2 * k) * ch + i] = s.expand(byte & 0x0F);
                out[(n + 2 * k + 1) * ch + i] = s.expand(byte >> 4);
            }
        }
        n += 8;
        blk.pos += 8;
    }
    Some(n)
}

/// Microsoft ADPCM: per channel predictor index, delta, two history
/// samples (interleaved by field), then one byte per stereo frame or two mono
/// frames, high nibble first.
fn ms(v: &View<'_>, blk: &mut Blk, st: &mut [Ms], out: &mut [i16], cap: usize) -> Option<usize> {
    let ch = st.len();
    let mut n = 0usize;
    if blk.pos == 0 {
        for (i, s) in st.iter_mut().enumerate() {
            if !s.predictor(v.get(i)?) {
                return None;
            }
            s.idelta = i32::from(v.u16_le(ch + 2 * i)? as i16);
            s.s1 = i32::from(v.u16_le(3 * ch + 2 * i)? as i16);
            s.s2 = i32::from(v.u16_le(5 * ch + 2 * i)? as i16);
            out[i] = s.s2 as i16;
            out[ch + i] = s.s1 as i16;
        }
        n = 2;
        blk.pos = 2;
    }
    let hdr = 7 * ch;
    if ch == 2 {
        while blk.pos < blk.frames && n < cap {
            let byte = v.byte(hdr + blk.pos as usize - 2);
            out[2 * n] = st[0].expand(byte >> 4);
            out[2 * n + 1] = st[1].expand(byte & 0x0F);
            n += 1;
            blk.pos += 1;
        }
    } else {
        while blk.pos + 1 < blk.frames && n + 2 <= cap {
            let byte = v.byte(hdr + (blk.pos as usize - 2) / 2);
            out[n] = st[0].expand(byte >> 4);
            out[n + 1] = st[0].expand(byte & 0x0F);
            n += 2;
            blk.pos += 2;
        }
        if blk.pos + 1 == blk.frames {
            blk.pos = blk.frames;
        }
    }
    Some(n)
}

/// IMA ADPCM, QuickTime layout: per channel a 34-byte packet (2-byte header,
/// 64 samples); the channels' packets follow each other.
fn ima_qt(v: &View<'_>, blk: &mut Blk, st: &mut [Ima], out: &mut [i16]) -> Option<usize> {
    let ch = st.len();
    for (i, s) in st.iter_mut().enumerate() {
        let base = 34 * i;
        if !s.qt_header(v.u16_be(base)?) {
            return None;
        }
        for k in 0..32 {
            let byte = v.byte(base + 2 + k);
            out[(2 * k) * ch + i] = s.expand_qt(byte & 0x0F);
            out[(2 * k + 1) * ch + i] = s.expand_qt(byte >> 4);
        }
    }
    blk.pos = blk.frames;
    Some(64)
}

impl<'a> Decoder<'a> for WavDecoder<'a> {
    const ARENA_MIN: usize = ARENA;

    fn inspect(src: &dyn ByteSource, _hint: &ContainerInfo) -> Result<Plan, OpenError> {
        let lay = header::parse(&View::of(src), src.at_end())?;
        Ok(Plan { info: stream_info(&lay), arena: ARENA })
    }

    fn open(
        src: &mut dyn ByteSource,
        arena: &'a mut [u8],
        _hint: &ContainerInfo,
    ) -> Result<(Self, StreamInfo), OpenError> {
        let lay = header::parse(&View::of(src), src.at_end())?;
        let info = stream_info(&lay);
        let mut ar = Arena::new(arena);
        let out = ar.slice::<i32>(OUT_WORDS)?;
        let mut d = WavDecoder {
            lay,
            out,
            used: ar.used(),
            phase: Phase::Walk { skip: 0, then: None },
            data_start: None,
            data_len: None,
            remaining: None,
            discard: 0,
            blk: Blk::default(),
            ima: [Ima::default(); 8],
            ms: [Ms::default(); 2],
        };
        match lay.data {
            Some((off, len)) => d.enter(src, off, len),
            None => {
                let k = skip(src, lay.resume);
                d.phase = Phase::Walk { skip: lay.resume - k, then: None };
            }
        }
        Ok((d, info))
    }

    fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError> {
        match self.fill(src) {
            Fill::Range(s, e) => Ok(Step::Pcm(if self.lay.enc.wide() {
                PcmBlock::I32(&mut self.out[s..e])
            } else {
                PcmBlock::I16(&mut arena::as_i16(self.out)[s..e])
            })),
            Fill::Need(n) => Ok(Step::NeedData(n)),
            Fill::End => Ok(Step::EndOfStream),
            Fill::Corrupt => Err(DecodeError::Corrupt),
        }
    }

    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError> {
        if self.data_start.is_none() {
            match self.walk(src) {
                Fill::Need(n) => return Ok(SeekStep::NeedData(n)),
                Fill::End => return Ok(SeekStep::Done(0)),
                _ => {}
            }
        }
        let Some(start) = self.data_start else { return Ok(SeekStep::Done(0)) };
        let total = self.data_len.map(|l| frames_in(&self.lay, l));
        let target = total.map_or(frame, |t| frame.min(t));
        let spb = u64::from(self.lay.spb.max(1));
        // QuickTime IMA keeps the full predictor across packets while the
        // packet header holds only its top 9 bits, so a decode that starts
        // mid-stream carries a constant offset below 128 LSB (a DC shift)
        // that no amount of priming removes; only a decode from the start
        // is exact. Seeks there are approximate by that much.
        let blk = target / spb;
        let disc = target % spb;
        let off = blk * u64::from(self.lay.block);
        src.seek(start + off)?;
        self.remaining = self.data_len.map(|l| l.saturating_sub(off));
        self.discard = disc;
        self.blk = Blk::default();
        self.ima = [Ima::default(); 8];
        self.phase = Phase::Data;
        Ok(SeekStep::Done(target))
    }

    fn arena_used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests;
