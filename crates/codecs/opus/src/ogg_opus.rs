//! Ogg Opus (RFC 7845) on top of [`crate::opus_dec`]: identification header,
//! comment header skipping, pre-skip and end trimming by granule position,
//! the header output gain, seeking by granule bisection with pre-roll.
//!
//! Supported: channel mapping family 0 (mono, stereo) and family 1 with one
//! stream of one or two channels. Multistream (surround) files are refused
//! (`Unsupported::Channels`). A chained file ends at the end of its first
//! logical stream.

use yoton_audio::arena::{self, Arena};
use yoton_audio::{
    ByteSource, CodecId, ContainerInfo, DecodeError, Decoder, OpenError, PcmBlock, Plan, SampleKind,
    SeekSource, SeekStep, Step, StreamInfo, Unsupported, View,
};
use yoton_ogg::assembled::{Next, PacketReader};
use yoton_ogg::{Bisect, HEADER_MIN, PageHeader, Parse};

use crate::celt::{CeltState, MEM_PER_CH};
use crate::opus_dec::{OpusDec, OpusState, REGION_WORDS_PER_CH, scratch_words};
use crate::packet::{self, Frames};
use crate::silk::SilkDecoder;

/// Largest Opus packet kept (a larger one is concealed as lost). 120 ms of
/// stereo audio at 256 kbit/s is 3840 bytes.
pub const PACKET_MAX: usize = 4096;
/// Decoding starts this many samples (48 kHz) before a seek target so the
/// decoder state has converged (RFC 7845 §4.6 asks for at least 80 ms).
pub const PREROLL: u64 = 48 * 400;

/// Parsed `OpusHead`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OpusHead {
    pub channels: u8,
    pub pre_skip: u16,
    pub input_rate: u32,
    /// Output gain, Q7.8 dB.
    pub gain: i16,
    pub serial: u32,
}

/// Arena bytes for a stream of `channels`.
#[must_use]
pub const fn arena_for(channels: usize) -> usize {
    arena::need::<OpusState>(1)
        + arena::need::<CeltState>(1)
        + arena::need::<f32>(channels * MEM_PER_CH)
        + arena::need::<SilkDecoder>(1)
        + arena::need::<u64>(scratch_words_const())
        + arena::need::<u32>(REGION_WORDS_PER_CH * channels)
        + 2 * arena::need::<f32>(240 * channels)
        + arena::need::<u8>(PACKET_MAX)
}

const fn scratch_words_const() -> usize {
    let celt = crate::celt::SCRATCH_LEN * 4;
    let silk = core::mem::size_of::<crate::silk::SilkScratch>();
    (if celt > silk { celt } else { silk }).div_ceil(8)
}

pub const ARENA_MIN: usize = arena_for(1);

fn parse_head(src: &dyn ByteSource) -> Result<OpusHead, OpenError> {
    let v = View::of(src);
    let h = match PageHeader::parse(&v) {
        Parse::Page(h) => h,
        Parse::Need(n) => {
            return Err(if src.at_end() {
                Unsupported::BadHeader.into()
            } else {
                OpenError::NeedData(n.max(HEADER_MIN))
            });
        }
        Parse::NotPage => return Err(Unsupported::NotThisFormat.into()),
    };
    if v.len() < h.total() {
        return Err(if src.at_end() {
            Unsupported::BadHeader.into()
        } else {
            OpenError::NeedData(h.total())
        });
    }
    if yoton_ogg::page_crc(&v, h.total()) != h.crc || !h.bos() {
        return Err(Unsupported::BadHeader.into());
    }
    let b = v.skip(h.header_len).take(h.body_len);
    if !b.starts_with_at(0, b"OpusHead") {
        return Err(Unsupported::NotThisFormat.into());
    }
    if b.len() < 19 {
        return Err(Unsupported::BadHeader.into());
    }
    let version = b.byte(8);
    if version >> 4 != 0 {
        return Err(Unsupported::Encoding.into());
    }
    let channels = b.byte(9);
    let pre_skip = b.u16_le(10).unwrap_or(0);
    let input_rate = b.u32_le(12).unwrap_or(0);
    let gain = b.u16_le(16).unwrap_or(0) as i16;
    let family = b.byte(18);
    match family {
        0 if (1..=2).contains(&channels) => {}
        1 if (1..=2).contains(&channels) => {
            if b.len() < 21 + usize::from(channels) {
                return Err(Unsupported::BadHeader.into());
            }
            let streams = b.byte(19);
            let coupled = b.byte(20);
            let ok = streams == 1
                && coupled == channels - 1
                && (0..channels).all(|i| b.byte(21 + usize::from(i)) == i);
            if !ok {
                return Err(Unsupported::Channels(channels).into());
            }
        }
        0 | 1 => return Err(Unsupported::Channels(channels).into()),
        _ => return Err(Unsupported::Channels(channels).into()),
    }
    Ok(OpusHead { channels, pre_skip, input_rate, gain, serial: h.serial })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Stage {
    Tags,
    Audio,
}

pub struct OpusDecoder<'a> {
    head: OpusHead,
    dec: OpusDec<'a>,
    pkt: &'a mut [u8],
    reader: PacketReader,
    stage: Stage,
    audio_start: u64,
    used: usize,
    /// The packet being decoded.
    frames: Option<Frames>,
    frame_idx: usize,
    pkt_len: usize,
    /// Samples still to conceal (a lost or broken packet).
    conceal: usize,
    /// Granule position of the next decoded sample.
    pos: u64,
    /// First granule position to output.
    keep_from: u64,
    /// Granule position after the last sample to output.
    end: Option<u64>,
    ended: bool,
    seek: Option<Bisect>,
    seek_target: u64,
    preroll: u64,
}

impl OpusDecoder<'_> {
    #[must_use]
    pub fn head(&self) -> OpusHead {
        self.head
    }

    /// Change the seek pre-roll (48 kHz samples; tests and measurements).
    pub fn set_preroll(&mut self, samples: u64) {
        self.preroll = samples;
    }

    /// Output range of the frame just decoded at `pos` with `n` samples.
    fn emit(&mut self, n: usize) -> Option<(usize, usize)> {
        let start = self.pos;
        let stop = start + n as u64;
        self.pos = stop;
        let a = self.keep_from.max(start);
        let b = self.end.map_or(stop, |e| e.min(stop));
        if b > a { Some(((a - start) as usize, (b - start) as usize)) } else { None }
    }

    fn reset_stream(&mut self) {
        self.dec.reset();
        self.frames = None;
        self.frame_idx = 0;
        self.conceal = 0;
        self.ended = false;
    }
}

impl<'a> Decoder<'a> for OpusDecoder<'a> {
    const ARENA_MIN: usize = ARENA_MIN;

    fn inspect(src: &dyn ByteSource, _hint: &ContainerInfo) -> Result<Plan, OpenError> {
        let h = parse_head(src)?;
        Ok(Plan { info: info_of(&h), arena: arena_for(usize::from(h.channels)) })
    }

    fn open(
        src: &mut dyn ByteSource,
        arena: &'a mut [u8],
        _hint: &ContainerInfo,
    ) -> Result<(Self, StreamInfo), OpenError> {
        let head = parse_head(src)?;
        let ch = usize::from(head.channels);
        let mut ar = Arena::new(arena);
        let st = ar.one::<OpusState>()?;
        let celt = ar.one::<CeltState>()?;
        let celt_mem = ar.slice::<f32>(ch * MEM_PER_CH)?;
        let silk = ar.one::<SilkDecoder>()?;
        let scratch = ar.slice::<u64>(scratch_words())?;
        let region = ar.slice::<u32>(REGION_WORDS_PER_CH * ch)?;
        let transition = ar.slice::<f32>(240 * ch)?;
        let redundant = ar.slice::<f32>(240 * ch)?;
        let pkt = ar.slice::<u8>(PACKET_MAX)?;
        let used = ar.used();
        let mut dec = OpusDec { st, celt, celt_mem, silk, scratch, region, transition, redundant };
        dec.init(ch, i32::from(head.gain));
        let mut reader = PacketReader::new();
        // The identification header page (complete: `parse_head` saw it).
        match reader.next_packet(src, pkt) {
            Next::Packet(p) if p.bos => reader.select(p.serial),
            _ => return Err(Unsupported::BadHeader.into()),
        }
        let info = info_of(&head);
        let d = OpusDecoder {
            head,
            dec,
            pkt,
            reader,
            stage: Stage::Tags,
            audio_start: src.position(),
            used,
            frames: None,
            frame_idx: 0,
            pkt_len: 0,
            conceal: 0,
            pos: 0,
            keep_from: u64::from(head.pre_skip),
            end: None,
            ended: false,
            seek: None,
            seek_target: 0,
            preroll: PREROLL,
        };
        Ok((d, info))
    }

    fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError> {
        let ch = usize::from(self.head.channels);
        loop {
            // Concealment of a lost packet, 20 ms at a time.
            if self.conceal > 0 {
                let n = self.dec.conceal(self.conceal);
                self.conceal = self.conceal.saturating_sub(n.max(1));
                if let Some((a, b)) = self.emit(n) {
                    let out = self.dec.output(n);
                    return Ok(Step::Pcm(PcmBlock::I16(&mut out[a * ch..b * ch])));
                }
                continue;
            }
            if let Some(f) = self.frames {
                if self.frame_idx < f.count {
                    let i = self.frame_idx;
                    self.frame_idx += 1;
                    let n = self.dec.decode_packet_frame(&self.pkt[..self.pkt_len], &f, i);
                    // A DTX frame longer than 20 ms is concealed in steps.
                    self.conceal += f.toc.frame_size.saturating_sub(n);
                    if let Some((a, b)) = self.emit(n) {
                        let out = self.dec.output(n);
                        return Ok(Step::Pcm(PcmBlock::I16(&mut out[a * ch..b * ch])));
                    }
                    continue;
                }
                self.frames = None;
            }
            if self.ended || self.end.is_some_and(|e| self.pos >= e) {
                self.ended = true;
                return Ok(Step::EndOfStream);
            }
            match self.reader.next_packet(src, self.pkt) {
                Next::NeedData(n) => return Ok(Step::NeedData(n)),
                Next::End => {
                    self.ended = true;
                    return Ok(Step::EndOfStream);
                }
                Next::Corrupt => return Err(DecodeError::Corrupt),
                Next::Packet(p) => {
                    if Some(p.serial) != self.reader.selected() {
                        // A chained stream follows: not decoded.
                        self.ended = true;
                        return Ok(Step::EndOfStream);
                    }
                    if self.stage == Stage::Tags {
                        self.stage = Stage::Audio;
                        self.audio_start = src.position();
                        continue;
                    }
                    let samples = if p.truncated { None } else { packet::samples(&self.pkt[..p.len]) };
                    let parsed = if p.truncated { None } else { packet::parse(&self.pkt[..p.len]) };
                    if p.eos
                        && let Some(g) = p.granule
                    {
                        self.end = Some(g);
                    }
                    match (parsed, samples) {
                        (Some(f), Some(_)) => {
                            self.frames = Some(f);
                            self.frame_idx = 0;
                            self.pkt_len = p.len;
                        }
                        (_, Some(n)) => self.conceal = n,
                        _ => {
                            // Unknown duration: assume 20 ms.
                            self.conceal = 960;
                        }
                    }
                }
            }
        }
    }

    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError> {
        let pre_skip = u64::from(self.head.pre_skip);
        let target = frame.saturating_add(pre_skip);
        if self.stage == Stage::Tags {
            // Still in the headers: read through them first.
            loop {
                match self.reader.next_packet(src, self.pkt) {
                    Next::NeedData(n) => return Ok(SeekStep::NeedData(n)),
                    Next::Packet(_) => {
                        self.stage = Stage::Audio;
                        self.audio_start = src.position();
                        break;
                    }
                    Next::End => {
                        self.ended = true;
                        return Ok(SeekStep::Done(frame));
                    }
                    Next::Corrupt => {}
                }
            }
        }
        let mut b = match self.seek {
            Some(b) if self.seek_target == target => b,
            _ => {
                self.seek_target = target;
                let start = target.saturating_sub(self.preroll);
                // `open` selected the stream.
                let serial = self.reader.selected().unwrap_or(0);
                Bisect::new(self.audio_start, src.len().unwrap_or(u64::MAX), start, serial)
            }
        };
        let r = b.step(src).map_err(DecodeError::Source)?;
        match r {
            yoton_ogg::SeekStep::NeedData(n) => {
                self.seek = Some(b);
                Ok(SeekStep::NeedData(n))
            }
            yoton_ogg::SeekStep::Done(found) => {
                self.seek = None;
                self.reset_stream();
                match found {
                    Some((page, g)) => {
                        src.seek(page)?;
                        self.reader.restart(true);
                        self.pos = g;
                    }
                    None => {
                        src.seek(self.audio_start)?;
                        self.reader.restart(false);
                        self.pos = 0;
                    }
                }
                self.keep_from = target.max(pre_skip);
                let landed = self.keep_from - pre_skip;
                if self.end.is_some_and(|e| self.keep_from >= e) {
                    self.ended = true;
                }
                Ok(SeekStep::Done(landed))
            }
        }
    }

    fn arena_used(&self) -> usize {
        self.used
    }
}

fn info_of(h: &OpusHead) -> StreamInfo {
    StreamInfo {
        rate: 48000,
        channels: h.channels,
        bits: 16,
        sample: SampleKind::I16,
        frames: None,
        delay: u32::from(h.pre_skip),
        padding: 0,
        bitrate: 0,
        codec: CodecId::Opus,
    }
}
