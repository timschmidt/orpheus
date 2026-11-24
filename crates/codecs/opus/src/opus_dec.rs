//! The Opus decoder proper: libopus 1.5.2 `src/opus_decoder.c`
//! (`opus_decode_frame`, `opus_decode_native`) and `src/opus.c`
//! (`opus_pcm_soft_clip`), float build, 48 kHz output, 1–2 channels
//! (BSD-3-Clause, see LICENSE-XIPH).
//!
//! Differences from libopus, all outside the normative decoder:
//! * Output is produced one Opus frame at a time (the caller hands each
//!   frame on), so the soft clipper runs per frame instead of per packet:
//!   only a clipping excursion that spans a frame boundary inside a
//!   multi-frame packet is shaped differently.
//! * The packet-loss concealment's inner sums run in natural order.
//! * No FEC decoding (Ogg streams have no losses to recover with it).
//!
//! Memory, all from the arena (stereo): CELT history 17 KiB, SILK state,
//! one region of `2880 * channels` `i16` (11.25 KiB) holding either a SILK
//! frame of up to 60 ms or a ≤ 20 ms float frame plus its SILK part, two
//! 5 ms float buffers (transition, redundancy) and a shared scratch area
//! used by CELT (12.4 KiB of `f32`) and SILK in turn.

use bytemuck::{Pod, Zeroable};

use crate::celt::{self, CeltState};
use crate::entdec::EcDec;
use crate::packet::{Bandwidth, Frames, Mode, Toc};
use crate::pcm::{float2int16, soft_clip};
use crate::silk::{DecControl, SilkDecoder, SilkScratch};

pub const MODE_SILK: i32 = 1000;
pub const MODE_HYBRID: i32 = 1001;
pub const MODE_CELT: i32 = 1002;

const F20: usize = 960;
const F10: usize = 480;
const F5: usize = 240;
const F2_5: usize = 120;

/// Words (u32) of the output region per channel.
pub const REGION_WORDS_PER_CH: usize = 1440;

/// Decoder state besides CELT and SILK.
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
#[repr(C)]
pub struct OpusState {
    pub channels: i32,
    pub decode_gain: i32,
    pub stream_channels: i32,
    pub bandwidth: i32,
    pub mode: i32,
    pub prev_mode: i32,
    pub frame_size: i32,
    pub prev_redundancy: i32,
    pub last_packet_duration: i32,
    pub softclip_mem: [f32; 2],
    pub range_final: u32,
    pub silk_internal_rate: i32,
    pub silk_payload_ms: i32,
    pub silk_channels_internal: i32,
    pub silk_prev_pitch_lag: i32,
}

/// The arena buffers of one decoder.
pub struct OpusDec<'a> {
    pub st: &'a mut OpusState,
    pub celt: &'a mut CeltState,
    pub celt_mem: &'a mut [f32],
    pub silk: &'a mut SilkDecoder,
    /// Shared CELT/SILK scratch (`u64` for alignment).
    pub scratch: &'a mut [u64],
    /// The frame region, `REGION_WORDS_PER_CH * channels` words.
    pub region: &'a mut [u32],
    pub transition: &'a mut [f32],
    pub redundant: &'a mut [f32],
}

/// Bytes of shared scratch needed.
#[must_use]
pub fn scratch_words() -> usize {
    let celt = celt::SCRATCH_LEN * 4;
    let silk = core::mem::size_of::<SilkScratch>();
    celt.max(silk).div_ceil(8)
}

fn bw_code(b: Bandwidth) -> i32 {
    match b {
        Bandwidth::Narrow => 1101,
        Bandwidth::Medium => 1102,
        Bandwidth::Wide => 1103,
        Bandwidth::SuperWide => 1104,
        Bandwidth::Full => 1105,
    }
}

/// `smooth_fade` with the output in place of `in1`.
fn fade_over_in1(io1: &mut [f32], in2: &[f32], overlap: usize, channels: usize) {
    let w = &celt::modes::WINDOW;
    for c in 0..channels {
        for i in 0..overlap {
            let wi = w[i] * w[i];
            let k = i * channels + c;
            io1[k] = wi * in2[k] + (1.0 - wi) * io1[k];
        }
    }
}

/// `smooth_fade` with the output in place of `in2`.
fn fade_over_in2(in1: &[f32], io2: &mut [f32], overlap: usize, channels: usize) {
    let w = &celt::modes::WINDOW;
    for c in 0..channels {
        for i in 0..overlap {
            let wi = w[i] * w[i];
            let k = i * channels + c;
            io2[k] = wi * io2[k] + (1.0 - wi) * in1[k];
        }
    }
}

/// Where a frame's float output goes.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Dest {
    Region,
    Transition,
}

impl OpusDec<'_> {
    /// `opus_decoder_init` for `channels` at 48 kHz with `gain` (Q8 dB).
    pub fn init(&mut self, channels: usize, gain: i32) {
        *self.st = OpusState::zeroed();
        self.st.channels = channels as i32;
        self.st.stream_channels = channels as i32;
        self.st.decode_gain = gain;
        self.silk.init();
        self.celt.init(self.celt_mem, channels);
        self.reset();
    }

    /// `OPUS_RESET_STATE`.
    pub fn reset(&mut self) {
        let (ch, gain) = (self.st.channels, self.st.decode_gain);
        *self.st = OpusState::zeroed();
        self.st.channels = ch;
        self.st.decode_gain = gain;
        self.st.stream_channels = ch;
        self.st.frame_size = 960;
        self.celt.reset(self.celt_mem);
        self.silk.reset();
    }

    fn channels(&self) -> usize {
        self.st.channels as usize
    }

    fn silk_ctl(&self) -> DecControl {
        DecControl {
            n_channels_api: self.st.channels,
            n_channels_internal: self.st.silk_channels_internal,
            api_sample_rate: 48000,
            internal_sample_rate: self.st.silk_internal_rate,
            payload_size_ms: self.st.silk_payload_ms,
            prev_pitch_lag: self.st.silk_prev_pitch_lag,
        }
    }

    fn put_silk_ctl(&mut self, c: &DecControl) {
        self.st.silk_channels_internal = c.n_channels_internal;
        self.st.silk_internal_rate = c.internal_sample_rate;
        self.st.silk_payload_ms = c.payload_size_ms;
        self.st.silk_prev_pitch_lag = c.prev_pitch_lag;
    }

    /// Run SILK for `frame_size` samples into `out` (i16, interleaved).
    fn silk_run(&mut self, dec: &mut EcDec<'_>, lost: bool, frame_size: usize, out_words: (usize, usize)) {
        let ch = self.channels();
        let mut ctl = self.silk_ctl();
        let scratch: &mut SilkScratch = bytemuck::from_bytes_mut(
            &mut bytemuck::cast_slice_mut::<u64, u8>(self.scratch)[..core::mem::size_of::<SilkScratch>()],
        );
        let out: &mut [i16] = bytemuck::cast_slice_mut(&mut self.region[out_words.0..out_words.1]);
        let mut decoded = 0usize;
        let mut guard = 0;
        while decoded < frame_size && guard < 8 {
            guard += 1;
            let mut n = 0i32;
            let first = decoded == 0;
            let o = &mut out[decoded * ch..];
            let r = self.silk.decode(scratch, &mut ctl, i32::from(lost), first, dec, o, &mut n);
            if r.is_err() || n <= 0 {
                // PLC failure is not fatal; a decode failure leaves silence.
                let k = ((frame_size - decoded) * ch).min(o.len());
                for v in o[..k].iter_mut() {
                    *v = 0;
                }
                break;
            }
            decoded += n as usize;
        }
        self.put_silk_ctl(&ctl);
    }

    fn celt_scratch(scratch: &mut [u64]) -> &mut [f32] {
        &mut bytemuck::cast_slice_mut::<u64, f32>(scratch)[..celt::SCRATCH_LEN]
    }

    fn set_celt_end(&mut self, bandwidth: i32) {
        let end = match bandwidth {
            1101 => 13,
            1102 | 1103 => 17,
            1104 => 19,
            _ => 21,
        };
        self.celt.end = end;
    }

    /// `opus_decode_frame` with `data == NULL` for `frame_size` ≤ 20 ms:
    /// concealment into `dest` (float). Returns the samples produced.
    fn decode_lost(&mut self, frame_size: usize, dest: Dest) -> usize {
        let ch = self.channels();
        let mut audiosize = frame_size;
        let mode = if self.st.prev_redundancy != 0 { MODE_CELT } else { self.st.prev_mode };
        let dst_len = audiosize * ch;
        if mode == 0 {
            match dest {
                Dest::Region => {
                    for v in bytemuck::cast_slice_mut::<u32, f32>(&mut self.region[..dst_len]).iter_mut() {
                        *v = 0.0;
                    }
                }
                Dest::Transition => self.transition[..dst_len].fill(0.0),
            }
            return audiosize;
        }
        if audiosize < F20 {
            if audiosize > F10 {
                audiosize = F10;
            } else if mode != MODE_SILK && audiosize > F5 && audiosize < F10 {
                audiosize = F5;
            }
        }
        let frame_size = audiosize;
        // SILK part into the region's second half (as i16).
        let silk_words = (F20 * ch, F20 * ch + F10.max(frame_size) * ch / 2);
        if mode != MODE_CELT {
            self.st.silk_payload_ms = 10.max((1000 * audiosize / 48000) as i32);
            let mut dummy = EcDec::new(&[]);
            self.silk_run(&mut dummy, true, frame_size, silk_words);
        }
        self.celt.start = 0;
        if mode != MODE_CELT {
            self.celt.start = 17;
        }
        self.celt.stream_channels = self.st.stream_channels;
        {
            let pcm: &mut [f32] = match dest {
                Dest::Region => bytemuck::cast_slice_mut::<u32, f32>(&mut self.region[..frame_size * ch]),
                Dest::Transition => &mut self.transition[..frame_size * ch],
            };
            if mode != MODE_SILK {
                let cfs = F20.min(frame_size);
                let _ = celt::decode(
                    self.celt,
                    self.celt_mem,
                    None,
                    pcm,
                    cfs,
                    None,
                    Self::celt_scratch(self.scratch),
                );
            } else {
                pcm.fill(0.0);
            }
        }
        if mode != MODE_CELT {
            let (pf, ps) = self.region.split_at_mut(F20 * ch);
            let silk: &[i16] = bytemuck::cast_slice(&ps[..silk_words.1 - silk_words.0]);
            let pcm: &mut [f32] = match dest {
                Dest::Region => &mut bytemuck::cast_slice_mut::<u32, f32>(pf)[..frame_size * ch],
                Dest::Transition => &mut self.transition[..frame_size * ch],
            };
            for (p, &s) in pcm.iter_mut().zip(silk) {
                *p += (1.0 / 32768.0) * f32::from(s);
            }
        }
        {
            let gain_on = self.st.decode_gain != 0;
            if gain_on {
                let gain = celt::math::exp2(6.488_140_8e-4 * self.st.decode_gain as f32);
                let pcm: &mut [f32] = match dest {
                    Dest::Region => bytemuck::cast_slice_mut::<u32, f32>(&mut self.region[..frame_size * ch]),
                    Dest::Transition => &mut self.transition[..frame_size * ch],
                };
                for v in pcm.iter_mut() {
                    *v *= gain;
                }
            }
        }
        self.st.range_final = 0;
        self.st.prev_mode = mode;
        self.st.prev_redundancy = 0;
        audiosize
    }

    /// Decode one Opus frame (`data`, of a packet whose TOC is `toc`) into
    /// the region as `i16`; returns the sample count, or `None` on a
    /// decoding error (the region then holds silence of the frame length).
    fn decode_frame(&mut self, data: &[u8], toc: Toc) -> usize {
        let ch = self.channels();
        let audiosize = toc.frame_size;
        let mode = self.st.mode;
        let bandwidth = self.st.bandwidth;
        let frame_size = audiosize;
        let mut len = data.len();
        if len <= 1 {
            // Payloads of 0 or 1 bytes run the concealment (DTX).
            return self.conceal(audiosize.min(self.st.frame_size as usize));
        }
        let mut dec = EcDec::new(data);
        let long_silk = frame_size > F20;
        // Transition into CELT: conceal 5 ms with the previous mode first.
        let mut transition = false;
        if self.st.prev_mode > 0
            && ((mode == MODE_CELT && self.st.prev_mode != MODE_CELT && self.st.prev_redundancy == 0)
                || (mode != MODE_CELT && self.st.prev_mode == MODE_CELT))
        {
            transition = true;
            if mode == MODE_CELT {
                self.decode_lost(F5.min(audiosize), Dest::Transition);
            }
        }
        // Region layout: long SILK frames as i16 over the whole region;
        // else float pcm in words [0, F20*ch) and SILK i16 after it.
        let silk_words = if long_silk {
            (0, frame_size * ch / 2)
        } else {
            (F20 * ch, F20 * ch + frame_size.max(F10) * ch / 2)
        };
        if mode != MODE_CELT {
            if self.st.prev_mode == MODE_CELT {
                self.silk.reset();
            }
            self.st.silk_payload_ms = 10.max((1000 * audiosize / 48000) as i32);
            self.st.silk_channels_internal = self.st.stream_channels;
            self.st.silk_internal_rate = if mode == MODE_SILK {
                match bandwidth {
                    1101 => 8000,
                    1102 => 12000,
                    _ => 16000,
                }
            } else {
                16000
            };
            self.silk_run(&mut dec, false, frame_size, silk_words);
        }
        let mut start_band = 0;
        let mut redundancy = false;
        let mut celt_to_silk = false;
        let mut redundancy_bytes = 0usize;
        if mode != MODE_CELT && dec.tell() + 17 + 20 * i32::from(mode == MODE_HYBRID) <= 8 * len as i32 {
            redundancy = if mode == MODE_HYBRID { dec.bit_logp(12) } else { true };
            if redundancy {
                celt_to_silk = dec.bit_logp(1);
                redundancy_bytes = if mode == MODE_HYBRID {
                    dec.uint(256) as usize + 2
                } else {
                    len.saturating_sub(((dec.tell() + 7) >> 3) as usize)
                };
                if redundancy_bytes > len {
                    redundancy_bytes = len;
                }
                len -= redundancy_bytes;
                if (len * 8) < dec.tell() as usize {
                    len = 0;
                    redundancy_bytes = 0;
                    redundancy = false;
                }
                dec.storage = dec.storage.saturating_sub(redundancy_bytes as u32);
            }
        }
        if mode != MODE_CELT {
            start_band = 17;
        }
        if redundancy {
            transition = false;
        }
        if transition && mode != MODE_CELT {
            self.decode_lost(F5.min(audiosize), Dest::Transition);
        }
        if bandwidth != 0 {
            self.set_celt_end(bandwidth);
        }
        self.celt.stream_channels = self.st.stream_channels;
        let mut redundant_rng = 0u32;
        let red_data = &data[len.min(data.len())..(len + redundancy_bytes).min(data.len())];
        if redundancy && celt_to_silk {
            self.celt.start = 0;
            let _ = celt::decode(
                self.celt,
                self.celt_mem,
                Some(red_data),
                &mut self.redundant[..F5 * ch],
                F5,
                None,
                Self::celt_scratch(self.scratch),
            );
            redundant_rng = self.celt.rng;
        }
        self.celt.start = start_band;
        let mut ok = true;
        if long_silk {
            // SILK-only frame longer than 20 ms: no CELT frame; the i16
            // region is post-processed in float chunks.
            // The fade-out goes to the transition buffer: no transition is
            // pending when the previous frame was hybrid and this one SILK.
            let use_fade = self.st.prev_mode == MODE_HYBRID
                && !(redundancy && celt_to_silk && self.st.prev_redundancy != 0);
            if use_fade {
                // Hybrid -> SILK: the CELT MDCT fades out over 2.5 ms.
                self.celt.start = 0;
                let silence = [0xFFu8, 0xFF];
                let _ = celt::decode(
                    self.celt,
                    self.celt_mem,
                    Some(&silence),
                    &mut self.transition[..F2_5 * ch],
                    F2_5,
                    None,
                    Self::celt_scratch(self.scratch),
                );
            }
            if redundancy && !celt_to_silk {
                self.celt.reset(self.celt_mem);
                self.celt.start = 0;
                let _ = celt::decode(
                    self.celt,
                    self.celt_mem,
                    Some(red_data),
                    &mut self.redundant[..F5 * ch],
                    F5,
                    None,
                    Self::celt_scratch(self.scratch),
                );
                redundant_rng = self.celt.rng;
            }
            self.finish_long(frame_size, use_fade, redundancy, celt_to_silk, transition);
            self.st.range_final = dec.rng ^ redundant_rng;
            self.st.prev_mode = mode;
            self.st.prev_redundancy = i32::from(redundancy && !celt_to_silk);
            return frame_size;
        }
        {
            let (pf, _) = self.region.split_at_mut(F20 * ch);
            let pcm: &mut [f32] = &mut bytemuck::cast_slice_mut::<u32, f32>(pf)[..frame_size * ch];
            if mode != MODE_SILK {
                let cfs = F20.min(frame_size);
                if mode != self.st.prev_mode && self.st.prev_mode > 0 && self.st.prev_redundancy == 0 {
                    self.celt.reset(self.celt_mem);
                }
                let r = celt::decode(
                    self.celt,
                    self.celt_mem,
                    Some(&data[..len]),
                    pcm,
                    cfs,
                    Some(&mut dec),
                    Self::celt_scratch(self.scratch),
                );
                ok = r.is_ok();
            } else {
                pcm.fill(0.0);
                if self.st.prev_mode == MODE_HYBRID
                    && !(redundancy && celt_to_silk && self.st.prev_redundancy != 0)
                {
                    self.celt.start = 0;
                    let silence = [0xFFu8, 0xFF];
                    let _ = celt::decode(
                        self.celt,
                        self.celt_mem,
                        Some(&silence),
                        &mut pcm[..F2_5 * ch],
                        F2_5,
                        None,
                        Self::celt_scratch(self.scratch),
                    );
                }
            }
        }
        {
            let (pf, ps) = self.region.split_at_mut(F20 * ch);
            let pcm: &mut [f32] = &mut bytemuck::cast_slice_mut::<u32, f32>(pf)[..frame_size * ch];
            if mode != MODE_CELT {
                let silk: &[i16] = bytemuck::cast_slice(&ps[..silk_words.1 - silk_words.0]);
                for (p, &s) in pcm.iter_mut().zip(silk) {
                    *p += (1.0 / 32768.0) * f32::from(s);
                }
            }
            if redundancy && !celt_to_silk {
                self.celt.reset(self.celt_mem);
                self.celt.start = 0;
                let _ = celt::decode(
                    self.celt,
                    self.celt_mem,
                    Some(red_data),
                    &mut self.redundant[..F5 * ch],
                    F5,
                    None,
                    Self::celt_scratch(self.scratch),
                );
                redundant_rng = self.celt.rng;
                let o = ch * (frame_size - F2_5);
                fade_over_in1(&mut pcm[o..], &self.redundant[ch * F2_5..], F2_5, ch);
            }
            if redundancy && celt_to_silk && (self.st.prev_mode != MODE_SILK || self.st.prev_redundancy != 0)
            {
                pcm[..F2_5 * ch].copy_from_slice(&self.redundant[..F2_5 * ch]);
                let o = ch * F2_5;
                fade_over_in2(&self.redundant[ch * F2_5..], &mut pcm[o..], F2_5, ch);
            }
            if transition {
                if audiosize >= F5 {
                    pcm[..F2_5 * ch].copy_from_slice(&self.transition[..F2_5 * ch]);
                    let o = ch * F2_5;
                    fade_over_in2(&self.transition[ch * F2_5..], &mut pcm[o..], F2_5, ch);
                } else {
                    fade_over_in2(&self.transition[..], pcm, F2_5, ch);
                }
            }
        }
        {
            let pcm: &mut [f32] =
                &mut bytemuck::cast_slice_mut::<u32, f32>(&mut self.region[..frame_size * ch])[..];
            if self.st.decode_gain != 0 {
                let gain = celt::math::exp2(6.488_140_8e-4 * self.st.decode_gain as f32);
                for v in pcm.iter_mut() {
                    *v *= gain;
                }
            }
        }
        self.st.range_final = if len <= 1 { 0 } else { dec.rng ^ redundant_rng };
        self.st.prev_mode = mode;
        self.st.prev_redundancy = i32::from(redundancy && !celt_to_silk);
        if !ok {
            for v in bytemuck::cast_slice_mut::<u32, f32>(&mut self.region[..frame_size * ch]).iter_mut() {
                *v = 0.0;
            }
        }
        self.float_to_i16(frame_size);
        frame_size
    }

    /// Long SILK-only frames: the SILK i16 samples are in the whole region;
    /// apply the float post-processing (CELT fade-out, redundancy,
    /// transition, gain, soft clip per 2.5 ms chunk) in place.
    fn finish_long(
        &mut self,
        frame_size: usize,
        fade: bool,
        redundancy: bool,
        celt_to_silk: bool,
        transition: bool,
    ) {
        let ch = self.channels();
        let red_tail = redundancy && !celt_to_silk;
        let gain = if self.st.decode_gain != 0 {
            celt::math::exp2(6.488_140_8e-4 * self.st.decode_gain as f32)
        } else {
            1.0
        };
        let use_red_head =
            redundancy && celt_to_silk && (self.st.prev_mode != MODE_SILK || self.st.prev_redundancy != 0);
        let total = frame_size * ch;
        let mut pos = 0usize;
        // The shared scratch is idle here (no CELT or SILK call follows).
        let buf: &mut [f32] = &mut bytemuck::cast_slice_mut::<u64, f32>(self.scratch)[..F2_5 * 2];
        while pos < total {
            // Chunks of 2.5 ms: the head (5 ms) and tail (2.5 ms) fixes
            // align with chunk boundaries.
            let n = (F2_5 * ch).min(total - pos);
            let samp = pos / ch;
            {
                let r: &[i16] = bytemuck::cast_slice(&self.region[..total.div_ceil(2)]);
                for k in 0..n {
                    buf[k] = (1.0 / 32768.0) * f32::from(r[pos + k]);
                }
            }
            if samp == 0 && fade {
                for k in 0..n {
                    buf[k] += self.transition[k];
                }
            }
            if use_red_head {
                if samp == 0 {
                    buf[..n].copy_from_slice(&self.redundant[..n]);
                } else if samp == F2_5 {
                    fade_over_in2(&self.redundant[ch * F2_5..], buf, F2_5, ch);
                }
            }
            if transition && !redundancy {
                if samp == 0 {
                    buf[..n].copy_from_slice(&self.transition[..n]);
                } else if samp == F2_5 {
                    fade_over_in2(&self.transition[ch * F2_5..], buf, F2_5, ch);
                }
            }
            if red_tail && samp + F2_5 == frame_size {
                fade_over_in1(buf, &self.redundant[ch * F2_5..], F2_5, ch);
            }
            for v in &mut buf[..n] {
                *v *= gain;
            }
            let mut mem = self.st.softclip_mem;
            soft_clip(buf, n / ch, ch, &mut mem);
            self.st.softclip_mem = mem;
            let r: &mut [i16] = bytemuck::cast_slice_mut(&mut self.region[..total.div_ceil(2)]);
            for k in 0..n {
                r[pos + k] = float2int16(buf[k]);
            }
            pos += n;
        }
    }

    /// Soft clip and convert the float frame (region words `[0, n*ch)`)
    /// to `i16` at the start of the region, in place.
    fn float_to_i16(&mut self, n: usize) {
        let ch = self.channels();
        {
            let pcm: &mut [f32] = bytemuck::cast_slice_mut(&mut self.region[..n * ch]);
            let mut mem = self.st.softclip_mem;
            soft_clip(pcm, n, ch, &mut mem);
            self.st.softclip_mem = mem;
        }
        let total = n * ch;
        let mut k = 0;
        while k < total {
            let m = 32.min(total - k);
            let mut tmp = [0f32; 32];
            tmp[..m].copy_from_slice(bytemuck::cast_slice::<u32, f32>(&self.region[k..k + m]));
            let out: &mut [i16] = bytemuck::cast_slice_mut(&mut self.region[..total.div_ceil(2) + 1]);
            for j in 0..m {
                out[k + j] = float2int16(tmp[j]);
            }
            k += m;
        }
    }

    /// Concealment of `n` samples (up to 120 ms) into the region as `i16`,
    /// in steps of at most 20 ms; returns the count of the first step (call
    /// again for the rest).
    pub fn conceal(&mut self, n: usize) -> usize {
        let step = n.clamp(F2_5, F20);
        let got = self.decode_lost(step, Dest::Region);
        self.float_to_i16(got);
        got
    }

    /// Decode frame `i` of a parsed packet into the region (`i16`).
    pub fn decode_packet_frame(&mut self, packet: &[u8], frames: &Frames, i: usize) -> usize {
        if i == 0 {
            let toc = frames.toc;
            self.st.mode = match toc.mode {
                Mode::Silk => MODE_SILK,
                Mode::Hybrid => MODE_HYBRID,
                Mode::Celt => MODE_CELT,
            };
            self.st.bandwidth = bw_code(toc.bandwidth);
            self.st.frame_size = toc.frame_size as i32;
            self.st.stream_channels = if toc.stereo { 2 } else { 1 };
        }
        let o = usize::from(frames.offset[i]);
        let s = usize::from(frames.size[i]);
        let data = &packet[o.min(packet.len())..(o + s).min(packet.len())];
        self.decode_frame(data, frames.toc)
    }

    /// The decoded `i16` samples of the last frame.
    #[must_use]
    pub fn output(&mut self, samples: usize) -> &mut [i16] {
        let n = samples * self.channels();
        &mut bytemuck::cast_slice_mut::<u32, i16>(self.region)[..n]
    }
}
