//! Speex decoder for Ogg Speex (04 §4.1 priority A, WP AU-9).
//!
//! A port of the libspeex 1.2.1 decoder (BSD-3-Clause; Xiph.Org
//! Foundation, Jean-Marc Valin, Analog Devices, CSIRO, David Rowe and the
//! other authors in `LICENSE-XIPH-SPEEX`): `nb_celp.c`, `sb_celp.c`,
//! `modes*.c`, `quant_lsp.c`, `lsp.c`, `ltp.c`, `cb_search.c`,
//! `filters.c`, `stereo.c`, `bits.c`, `speex_header.c`, the in-band handler
//! of `speex_callbacks.c` and the codebook tables, decoder side only.
//!
//! * Numbers: libspeex's **floating-point** build, in `f32` throughout (no
//!   `f64`: the M4F has a single-precision FPU only). The conformance
//!   reference is ffmpeg's `libspeex` decoder on Fedora's libspeex 1.2.0,
//!   a float build with SSE, so this port follows that build operation by
//!   operation: `inner_prod` sums in the SSE build's order (four partial
//!   sums), cosines come from libspeex's own float `spx_cos` polynomial
//!   (the float build calls no libm `cos`), `exp` of quantised indices is
//!   tabulated with the values the C code rounds to, square roots are
//!   correctly rounded. Where the C code evaluates in double precision (a
//!   `double` constant in a `float` expression) this port rounds to `f32`.
//!   Measured against the reference: −97 to −120 dBFS RMS difference, at
//!   most 5 LSB (31-file corpus). With libm's `cos` in place of `spx_cos`
//!   the difference was −79 dBFS: the reference really is the float build.
//! * Modes: narrowband (8 kHz), wideband (16 kHz) and ultra-wideband
//!   (32 kHz), every sub-mode (CBR, VBR, ABR streams are all just sub-mode
//!   choices per frame), the vocoder and null modes, DTX, the perceptual
//!   enhancer (on: libspeex initialises the narrowband layer with it on and
//!   ffmpeg's wrapper never changes it; checked against the reference — off
//!   differs by −31 dBFS), in-band requests (the stereo request is applied
//!   for stereo streams, as ffmpeg registers only that handler; the rest are
//!   skipped as libspeex skips unregistered requests), intensity stereo
//!   (`speex_decode_stereo_int`), any number of frames per packet.
//! * Packets are decoded the way ffmpeg's `libspeexdec` does: one frame per
//!   call; a packet is finished when fewer than 5 bits remain or the next 5
//!   bits are a terminator (`0b01111`). Samples are converted to `i16` with
//!   `speex_decode_int`'s rounding (`floor(0.5 + x)`, saturated); the
//!   stereo expansion truncates as C's float-to-`short` conversion does.
//! * Length (gapless): ffmpeg applies no trimming to Speex — neither the
//!   encoder look-ahead at the start (granule positions start at
//!   −look-ahead) nor the end granule position — and outputs every decoded
//!   frame (checked empirically: the reference length is always
//!   `frames × frame size`, e.g. 12160 samples for a 12030-sample source).
//!   This decoder does the same, so its length equals ffmpeg's exactly.
//!   `StreamInfo::frames` is `None` (granule positions written by
//!   different muxers disagree on the look-ahead; the length is known only
//!   at the end).
//! * Seeking: the sample-frame time line above is mapped to granule
//!   positions with the offset seen on the first audio page (granule −
//!   decoded samples at that page's end; ffmpeg writes −look-ahead). The
//!   decoder skims (parses without decoding) packets up to
//!   [`PREROLL_FRAMES`] frames before the target, starting from the first
//!   audio page for streams of up to [`SKIM_LIMIT`] bytes and otherwise
//!   from the page an Ogg bisection (`orpheus-ogg`) finds; it then decodes
//!   the pre-roll from a fresh state and drops the samples before the
//!   target. Skimming keeps what never converges by itself: the noise
//!   seeds, the vocoder's memories and pulse phase, the DTX flag and the
//!   in-band stereo parameters. The pre-roll is long (64 frames, 1.28 s)
//!   because CELP's pitch predictor has gains near or above 1 on periodic
//!   signals (the corpus's sweeps): over 19 seek targets in each of the 31
//!   corpus files, the worst RMS difference to the linear decode (2048
//!   samples after the target) with 16/24/32/48/64 frames of pre-roll was
//!   −38/−52/−57/−57/−101 dBFS (−80 is the pass mark). Streams
//!   above `SKIM_LIMIT` keep fresh seeds after a seek, so their
//!   noise-excited frames (vocoder mode, null mode, DTX comfort noise)
//!   carry different (statistically identical) noise than a linear decode;
//!   state carried through a long null-mode stretch (more than the
//!   pre-roll) is likewise not reproduced.
//!
//! Memory: all state in the arena — the narrowband state (2.2 KiB), one
//! high-band state per sub-band layer (0.7 KiB each), the frame buffer
//! (`f32`, reused for the `i16` output in place), the QMF scratch and a
//! 2 KiB packet buffer: 4872 B narrowband, 7768 B wideband, 11048 B
//! ultra-wideband (mono and stereo alike) of the 12 KiB budget.

#![no_std]
#![forbid(unsafe_code)]
#![allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_range_loop,
    clippy::cast_lossless,
    clippy::excessive_precision,
    clippy::unreadable_literal,
    clippy::approx_constant
)]

#[cfg(test)]
extern crate std;

mod bits;
mod dsp;
mod math;
mod modes;
mod nb;
mod sb;
mod stereo;
mod tables;

use bits::Bits;
use nb::NbState;
use orpheus_audio::arena::{self, Arena};
use orpheus_audio::{
    ByteSource, CodecId, ContainerInfo, DecodeError, Decoder, MAX_PEEK, OpenError, PcmBlock, Plan,
    SampleKind, SeekSource, SeekStep, Step, StreamInfo, Unsupported, View,
};
use orpheus_ogg::assembled::{Next, PacketReader};
use orpheus_ogg::{Bisect, HEADER_MIN, PageHeader, Parse, page_crc};
use sb::{Low, SbState};
use stereo::Stereo;

/// Packet buffer: ten ultra-wideband stereo frames at the highest rate
/// take about 1.2 KB; longer packets are truncated (the frames in the kept
/// part still decode).
pub const PACKET_BYTES: usize = 2048;

/// Frames decoded and dropped before a seek target (chosen by measurement,
/// see the crate documentation and the `preroll_survey` development test).
pub const PREROLL_FRAMES: u64 = 64;

/// Streams up to this many bytes of audio are skimmed from their start on
/// a seek (no bisection), so that the noise generators' seeds — which
/// never converge — are those of a linear decode. Longer streams bisect;
/// after a seek their noise-excited frames (vocoder mode, DTX comfort
/// noise) differ from a linear decode in their (random) noise only.
pub const SKIM_LIMIT: u64 = 1 << 20;

/// Size of the Speex identification header.
const HEADER_LEN: usize = 80;

/// Samples per frame for a mode (0 NB, 1 WB, 2 UWB).
const fn frame_size(mode: u8) -> usize {
    nb::FRAME << mode
}

/// QMF scratch values for a mode.
const fn qmf_scratch(mode: u8) -> usize {
    if mode == 0 { 0 } else { 2 * (frame_size(mode) / 2 + dsp::QMF_M2) }
}

/// Arena bytes for a stream of this mode (mono and stereo need the same:
/// the stereo expansion happens in place in the frame buffer).
#[must_use]
pub const fn arena_for(mode: u8) -> usize {
    arena::need::<NbState>(1)
        + mode as usize * arena::need::<SbState>(1)
        + arena::need::<f32>(frame_size(mode))
        + arena::need::<f32>(qmf_scratch(mode))
        + arena::need::<u8>(PACKET_BYTES)
}

/// The smallest arena (narrowband).
pub const ARENA_MIN: usize = arena_for(0);

#[derive(Debug, Clone, Copy)]
struct Head {
    mode: u8,
    rate: u32,
    channels: u8,
    frames_per_packet: u32,
    extra_headers: u32,
    bitrate: u32,
}

impl Head {
    fn info(&self) -> StreamInfo {
        StreamInfo {
            rate: self.rate,
            channels: self.channels,
            bits: 16,
            sample: SampleKind::I16,
            frames: None,
            delay: 0,
            padding: 0,
            bitrate: self.bitrate,
            codec: CodecId::Speex,
        }
    }

    /// Samples per packet (per channel), as granule positions count them.
    fn packet_samples(&self) -> u64 {
        u64::from(self.frames_per_packet) * frame_size(self.mode) as u64
    }
}

/// Parse the first page and its Speex identification header without
/// consuming anything.
fn parse_head(src: &dyn ByteSource) -> Result<Head, OpenError> {
    let v = View::of(src);
    let h = match PageHeader::parse(&v) {
        Parse::Need(n) => {
            return Err(if src.at_end() {
                Unsupported::BadHeader.into()
            } else {
                OpenError::NeedData(n.min(MAX_PEEK))
            });
        }
        Parse::NotPage => return Err(Unsupported::NotThisFormat.into()),
        Parse::Page(h) => h,
    };
    let total = h.total();
    if total > MAX_PEEK {
        return Err(Unsupported::BadHeader.into());
    }
    if v.len() < total {
        return Err(if src.at_end() { Unsupported::BadHeader.into() } else { OpenError::NeedData(total) });
    }
    if page_crc(&v, total) != h.crc || !h.bos() {
        return Err(Unsupported::BadHeader.into());
    }
    // The first packet must end on this page.
    let mut len = 0usize;
    let mut complete = false;
    for i in 0..usize::from(h.segments) {
        let l = usize::from(v.byte(HEADER_MIN + i));
        len += l;
        if l < 255 {
            complete = true;
            break;
        }
    }
    let p = v.skip(h.header_len);
    if !complete || len < HEADER_LEN || !p.starts_with_at(0, b"Speex   ") {
        return Err(Unsupported::NotThisFormat.into());
    }
    let field = |i: usize| p.u32_le(28 + 4 * i).unwrap_or(0) as i32;
    let (rate, mode, bitstream, channels, bitrate) = (field(2), field(3), field(4), field(5), field(6));
    let (fpp, extra) = (field(9), field(10));
    if !(0..=2).contains(&mode) || bitstream != 4 {
        return Err(Unsupported::Encoding.into());
    }
    if !(1000..=96_000).contains(&rate) {
        return Err(Unsupported::Rate(rate.max(0) as u32).into());
    }
    if !(0..=64).contains(&fpp) || !(0..=255).contains(&extra) {
        return Err(Unsupported::BadHeader.into());
    }
    Ok(Head {
        mode: mode as u8,
        rate: rate as u32,
        // libspeex clamps the channel count to 1–2.
        channels: channels.clamp(1, 2) as u8,
        frames_per_packet: fpp.max(1) as u32,
        extra_headers: extra as u32,
        bitrate: bitrate.max(0) as u32,
    })
}

#[derive(Debug, Clone, Copy)]
enum SeekPhase {
    Start,
    /// Reading from the stream start for the first audio granule position.
    FindOffset,
    Begin,
    Bisect(Bisect),
    /// Stepping over packets (without decoding) from output sample `at`
    /// to the pre-roll start.
    Skim {
        at: u64,
    },
}

#[derive(Debug, Clone, Copy)]
struct SeekSt {
    target: u64,
    phase: SeekPhase,
}

pub struct SpeexDecoder<'a> {
    head: Head,
    reader: PacketReader,
    pkt: &'a mut [u8],
    pkt_len: usize,
    bits: Bits,
    /// A packet is loaded in `pkt`.
    loaded: bool,
    nb: &'a mut NbState,
    wb: Option<&'a mut SbState>,
    uwb: Option<&'a mut SbState>,
    out: &'a mut [f32],
    scratch: &'a mut [f32],
    stereo: Stereo,
    used: usize,
    /// Header packets (comment, extra headers) still to skip.
    headers_left: u32,
    /// Where the stream (its first page) starts.
    stream_start: u64,
    /// Where the first audio page starts, once known.
    audio_start: Option<u64>,
    /// Audio packets read since the stream start (until `offset` is known).
    packets: u64,
    /// Granule position minus output position, once known.
    offset: Option<i64>,
    /// Samples (per channel) still to drop after a seek.
    discard: u64,
    /// Samples decoded (and dropped) before a seek target.
    preroll: u64,
    /// Streams with at most this many bytes of audio are skimmed from the
    /// start on a seek ([`SKIM_LIMIT`]).
    skim_limit: u64,
    seek: Option<SeekSt>,
    ended: bool,
}

impl SpeexDecoder<'_> {
    /// Reset the codec state (after a seek).
    fn reset_codec(&mut self) {
        self.nb.init(self.head.mode > 0);
        if let Some(wb) = self.wb.as_deref_mut() {
            wb.init(&modes::WB_MODE);
        }
        if let Some(uwb) = self.uwb.as_deref_mut() {
            uwb.init(&modes::UWB_MODE);
        }
        self.stereo = Stereo::new();
        self.loaded = false;
    }

    /// Account for a packet the reader delivered: header skipping, offset
    /// discovery. `true` when it is an audio packet.
    fn take_packet(&mut self, p: &orpheus_ogg::assembled::Packet, pos: u64) -> bool {
        if self.headers_left > 0 {
            self.headers_left -= 1;
            if self.headers_left == 0 && self.audio_start.is_none() {
                self.audio_start = Some(pos);
            }
            return false;
        }
        self.packets += 1;
        if self.offset.is_none()
            && let Some(g) = p.granule
        {
            let done = self.packets.saturating_mul(self.head.packet_samples());
            self.offset = Some((g as i64).wrapping_sub(done as i64));
        }
        true
    }

    /// Decode one frame from the loaded packet into `out`.
    fn decode_frame(&mut self) -> nb::Ret {
        let buf = &self.pkt[..self.pkt_len];
        let bits = &mut self.bits;
        let stereo = if self.head.channels == 2 { Some(&mut self.stereo) } else { None };
        let nb = &mut *self.nb;
        match (self.wb.as_deref_mut(), self.uwb.as_deref_mut()) {
            (None, _) => nb::decode(nb, buf, bits, stereo, &mut self.out[..nb::FRAME], None),
            (Some(wb), None) => {
                let (lo, hi) = self.out.split_at_mut(nb::FRAME);
                let ret = nb::decode(nb, buf, bits, stereo, lo, Some(&mut hi[..nb::FRAME]));
                if ret != 0 {
                    return ret;
                }
                let low = nb_low(nb);
                sb::decode(
                    wb,
                    &modes::WB_MODE,
                    &low,
                    buf,
                    bits,
                    &mut self.out[..2 * nb::FRAME],
                    None,
                    self.scratch,
                )
            }
            (Some(wb), Some(uwb)) => {
                let (a, b) = self.out.split_at_mut(2 * nb::FRAME);
                {
                    let (lo, hi) = a.split_at_mut(nb::FRAME);
                    let ret = nb::decode(nb, buf, bits, stereo, lo, Some(hi));
                    if ret != 0 {
                        return ret;
                    }
                }
                let low = nb_low(nb);
                let ret = sb::decode(wb, &modes::WB_MODE, &low, buf, bits, a, Some(b), self.scratch);
                if ret != 0 {
                    return ret;
                }
                let low = wb.as_low(nb.dtx != 0);
                sb::decode(
                    uwb,
                    &modes::UWB_MODE,
                    &low,
                    buf,
                    bits,
                    &mut self.out[..4 * nb::FRAME],
                    None,
                    self.scratch,
                )
            }
        }
    }

    /// Convert the decoded frame to `i16` in place (stereo: expanded);
    /// returns the number of samples.
    fn emit_pcm(&mut self) -> usize {
        let n = frame_size(self.head.mode);
        let h: &mut [i16] = bytemuck::cast_slice_mut(&mut self.out[..]);
        for k in 0..n {
            let x: f32 = bytemuck::cast([h[2 * k], h[2 * k + 1]]);
            h[k] = math::to_i16(x);
        }
        if self.head.channels == 2 {
            self.stereo.decode(h, n);
        }
        n * usize::from(self.head.channels)
    }

    /// Step over the frames of a packet of `len` bytes now in `pkt`
    /// without decoding them, as `decode` would go through them; returns
    /// the frames that would have produced output.
    fn skim_packet(&mut self, len: usize) -> u64 {
        if len == 0 {
            return 0;
        }
        self.pkt_len = len.min(self.pkt.len());
        let buf = &self.pkt[..self.pkt_len];
        let mut bits = Bits::new(self.pkt_len);
        let mut frames = 0;
        loop {
            let stereo = if self.head.channels == 2 { Some(&mut self.stereo) } else { None };
            let mut ret = nb::skim(self.nb, buf, &mut bits, stereo);
            let dtx = self.nb.dtx != 0;
            if ret == 0
                && let Some(wb) = self.wb.as_deref_mut()
            {
                ret = sb::skim(wb, &modes::WB_MODE, dtx, buf, &mut bits);
            }
            if ret == 0
                && let Some(uwb) = self.uwb.as_deref_mut()
            {
                ret = sb::skim(uwb, &modes::UWB_MODE, dtx, buf, &mut bits);
            }
            if ret != 0 {
                return frames;
            }
            frames += 1;
            if bits.remaining() < 5 || bits.peek_n(buf, 5) == 0xF {
                return frames;
            }
        }
    }

    /// The decoder is at output sample `at` (a packet boundary the reader
    /// is now positioned at, the codec state fresh apart from what
    /// skimming tracked) on the way to `target`.
    fn landed(&mut self, at: u64, target: u64) {
        self.loaded = false;
        self.discard = target.saturating_sub(at);
        self.headers_left = 0;
        self.ended = false;
        self.seek = None;
    }
}

fn nb_low(nb: &NbState) -> Low {
    Low { pi_gain: nb.pi_gain, exc_rms: core::array::from_fn(|i| nb.exc_rms(i)), dtx: nb.dtx != 0 }
}

impl<'a> Decoder<'a> for SpeexDecoder<'a> {
    const ARENA_MIN: usize = ARENA_MIN;

    fn inspect(src: &dyn ByteSource, _hint: &ContainerInfo) -> Result<Plan, OpenError> {
        let h = parse_head(src)?;
        Ok(Plan { info: h.info(), arena: arena_for(h.mode) })
    }

    fn open(
        src: &mut dyn ByteSource,
        arena: &'a mut [u8],
        _hint: &ContainerInfo,
    ) -> Result<(Self, StreamInfo), OpenError> {
        let head = parse_head(src)?;
        let mut ar = Arena::new(arena);
        let nb = ar.one::<NbState>()?;
        nb.init(head.mode > 0);
        let wb = if head.mode >= 1 {
            let s = ar.one::<SbState>()?;
            s.init(&modes::WB_MODE);
            Some(s)
        } else {
            None
        };
        let uwb = if head.mode >= 2 {
            let s = ar.one::<SbState>()?;
            s.init(&modes::UWB_MODE);
            Some(s)
        } else {
            None
        };
        let out = ar.slice::<f32>(frame_size(head.mode))?;
        let scratch = ar.slice::<f32>(qmf_scratch(head.mode))?;
        let pkt = ar.slice::<u8>(PACKET_BYTES)?;
        let stream_start = src.position();
        let mut reader = PacketReader::new();
        // `parse_head` saw the whole first page with a good CRC: the
        // identification packet comes out at once.
        match reader.next_packet(src, pkt) {
            Next::Packet(p) if p.bos => reader.select(p.serial),
            _ => return Err(Unsupported::BadHeader.into()),
        }
        let d = SpeexDecoder {
            head,
            reader,
            pkt,
            pkt_len: 0,
            bits: Bits::default(),
            loaded: false,
            nb,
            wb,
            uwb,
            out,
            scratch,
            stereo: Stereo::new(),
            used: ar.used(),
            headers_left: 1 + head.extra_headers,
            stream_start,
            audio_start: None,
            packets: 0,
            offset: None,
            discard: 0,
            preroll: PREROLL_FRAMES * frame_size(head.mode) as u64,
            skim_limit: SKIM_LIMIT,
            seek: None,
            ended: false,
        };
        Ok((d, head.info()))
    }

    fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError> {
        loop {
            if self.ended {
                return Ok(Step::EndOfStream);
            }
            let fresh = !self.loaded || self.bits.remaining() < 5 || self.bits.peek_n(self.pkt, 5) == 0xF;
            if fresh {
                self.loaded = false;
                match self.reader.next_packet(src, self.pkt) {
                    Next::Packet(p) => {
                        if p.bos && Some(p.serial) != self.reader.selected() {
                            // A chained stream: not followed.
                            self.ended = true;
                            continue;
                        }
                        if !self.take_packet(&p, src.position()) || p.len == 0 {
                            continue;
                        }
                        self.pkt_len = p.len.min(self.pkt.len());
                        self.bits = Bits::new(self.pkt_len);
                        self.loaded = true;
                    }
                    Next::NeedData(n) => return Ok(Step::NeedData(n)),
                    Next::End => {
                        self.ended = true;
                        return Ok(Step::EndOfStream);
                    }
                    Next::Corrupt => return Err(DecodeError::Corrupt),
                }
            }
            let ret = self.decode_frame();
            if ret <= -2 {
                self.loaded = false;
                return Err(DecodeError::Corrupt);
            }
            if ret != 0 {
                self.loaded = false;
                continue;
            }
            let n = self.emit_pcm();
            let ch = usize::from(self.head.channels);
            let frames = n / ch;
            let d = self.discard.min(frames as u64) as usize;
            self.discard -= d as u64;
            if d == frames {
                continue;
            }
            let h: &mut [i16] = bytemuck::cast_slice_mut(&mut self.out[..]);
            return Ok(Step::Pcm(PcmBlock::I16(&mut h[d * ch..n])));
        }
    }

    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError> {
        let mut st = match self.seek {
            Some(s) if s.target == frame => s,
            _ => SeekSt { target: frame, phase: SeekPhase::Start },
        };
        loop {
            match st.phase {
                SeekPhase::Start => {
                    if self.offset.is_some() && self.audio_start.is_some() {
                        st.phase = SeekPhase::Begin;
                    } else {
                        src.seek(self.stream_start)?;
                        self.reader.restart(false);
                        self.headers_left = 2 + self.head.extra_headers;
                        self.packets = 0;
                        self.loaded = false;
                        st.phase = SeekPhase::FindOffset;
                    }
                }
                SeekPhase::FindOffset => match self.reader.next_packet(src, self.pkt) {
                    Next::Packet(p) => {
                        self.take_packet(&p, src.position());
                        if self.offset.is_some() {
                            st.phase = SeekPhase::Begin;
                        }
                    }
                    Next::NeedData(n) => {
                        self.seek = Some(st);
                        return Ok(SeekStep::NeedData(n));
                    }
                    Next::Corrupt => {}
                    Next::End => {
                        self.offset.get_or_insert(0);
                        self.audio_start.get_or_insert(self.stream_start);
                        st.phase = SeekPhase::Begin;
                    }
                },
                SeekPhase::Begin => {
                    let offset = self.offset.unwrap_or(0);
                    let audio = self.audio_start.unwrap_or(self.stream_start);
                    let pre = st.target.saturating_sub(self.preroll);
                    let tg = i64::try_from(pre).unwrap_or(i64::MAX).saturating_add(offset);
                    let small = src.len().is_some_and(|l| l.saturating_sub(audio) <= self.skim_limit);
                    if tg < 0 || small {
                        // Skim from the first audio packet: the noise
                        // seeds come out exact.
                        src.seek(audio)?;
                        self.reader.restart(false);
                        self.reset_codec();
                        st.phase = SeekPhase::Skim { at: 0 };
                    } else {
                        let end = src.len().unwrap_or(audio).max(audio);
                        // `open` selected the stream.
                        let serial = self.reader.selected().unwrap_or(0);
                        st.phase = SeekPhase::Bisect(Bisect::new(audio, end, tg as u64, serial));
                    }
                }
                SeekPhase::Bisect(mut b) => {
                    let r = b.step(src)?;
                    st.phase = SeekPhase::Bisect(b);
                    match r {
                        orpheus_ogg::SeekStep::NeedData(n) => {
                            self.seek = Some(st);
                            return Ok(SeekStep::NeedData(n));
                        }
                        orpheus_ogg::SeekStep::Done(found) => {
                            let offset = self.offset.unwrap_or(0);
                            let audio = self.audio_start.unwrap_or(self.stream_start);
                            let at = found.map(|(pos, g)| (pos, (g as i64).saturating_sub(offset)));
                            match at {
                                Some((pos, at)) if at >= 0 && at as u64 <= st.target => {
                                    src.seek(pos)?;
                                    self.reader.restart(true);
                                    st.phase = SeekPhase::Skim { at: at as u64 };
                                }
                                _ => {
                                    src.seek(audio)?;
                                    self.reader.restart(false);
                                    st.phase = SeekPhase::Skim { at: 0 };
                                }
                            }
                            self.reset_codec();
                        }
                    }
                }
                SeekPhase::Skim { at } => {
                    let pre = st.target.saturating_sub(self.preroll);
                    if at.saturating_add(self.head.packet_samples()) > pre {
                        self.landed(at, st.target);
                        return Ok(SeekStep::Done(st.target));
                    }
                    match self.reader.next_packet(src, self.pkt) {
                        Next::Packet(p) => {
                            if p.bos && Some(p.serial) != self.reader.selected() {
                                self.landed(at, st.target);
                                self.ended = true;
                                return Ok(SeekStep::Done(st.target));
                            }
                            let frames = self.skim_packet(p.len);
                            st.phase =
                                SeekPhase::Skim { at: at + frames * frame_size(self.head.mode) as u64 };
                        }
                        Next::NeedData(n) => {
                            self.seek = Some(st);
                            return Ok(SeekStep::NeedData(n));
                        }
                        Next::Corrupt => {}
                        Next::End => {
                            self.landed(at, st.target);
                            self.ended = true;
                            return Ok(SeekStep::Done(st.target));
                        }
                    }
                }
            }
        }
    }

    fn arena_used(&self) -> usize {
        self.used
    }
}

#[cfg(test)]
mod tests;
