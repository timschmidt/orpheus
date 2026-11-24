//! Narrowband CELP decoder (libspeex `nb_celp.c`, `nb_decode`, float
//! build). Packet loss is never signalled to this decoder (every frame
//! comes from a packet), so the loss-concealment paths (`nb_decode_lost`,
//! `count_lost`) are left out; the DTX status is kept for the wideband
//! layer.

use bytemuck::{Pod, Zeroable};

use crate::bits::Bits;
use crate::dsp;
use crate::math;
use crate::modes::{Innov, Ltp, NB_DEFAULT_SUBMODE, NB_SUBMODES, WB_SKIP};
use crate::stereo::Stereo;
use crate::tables;

pub const FRAME: usize = 160;
pub const SUBFRAME: usize = 40;
pub const NB_SUBFRAMES: usize = 4;
pub const ORDER: usize = 10;
pub const PITCH_START: i32 = 17;
pub const PITCH_END: i32 = 144;
/// `NB_DEC_BUFFER`: excitation history plus the frame plus look-around for
/// the enhancer's interpolation.
const DEC_BUFFER: usize = FRAME + 2 * PITCH_END as usize + SUBFRAME + 12;
/// `st->exc` within the buffer.
const EXC: usize = 2 * PITCH_END as usize + SUBFRAME + 6;
/// Moved forward by one frame each frame.
const MOVE: usize = 2 * PITCH_END as usize + SUBFRAME + 12;

const LSP_MARGIN: f32 = 0.002;
/// `π − LSP_MARGIN` in double precision, stored as `float`.
const LSP_TOP: f32 = 3.139_592_6;

static EXC_GAIN_SCAL3: [f32; 8] =
    [0.061130, 0.163546, 0.310413, 0.428220, 0.555887, 0.719055, 0.938694, 1.326874];
static EXC_GAIN_SCAL1: [f32; 2] = [0.70469, 1.05127];

/// Decoder state (`DecState`), all of it in the arena.
#[derive(Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct NbState {
    exc_buf: [f32; DEC_BUFFER],
    old_qlsp: [f32; ORDER],
    interp_qlpc: [f32; ORDER],
    mem_sp: [f32; ORDER],
    mem_hp: [f32; 2],
    /// Analysis filter gain at π per sub-frame (read by the wideband layer).
    pub pi_gain: [f32; NB_SUBFRAMES],
    voc_m1: f32,
    voc_m2: f32,
    voc_mean: f32,
    voc_offset: i32,
    seed: u32,
    first: u32,
    submode: u32,
    /// DTX flag of the last frame (read by the wideband layer).
    pub dtx: u32,
    /// Perceptual enhancer on.
    pub enh: u32,
    /// Part of a wideband decoder (selects the output high-pass).
    pub wideband: u32,
}

impl NbState {
    /// `nb_decoder_init` (and `SPEEX_RESET_STATE` plus everything else: a
    /// fresh decoder).
    pub fn init(&mut self, wideband: bool) {
        *self = Self::zeroed();
        self.first = 1;
        self.submode = NB_DEFAULT_SUBMODE;
        self.enh = 1;
        self.seed = 1000;
        self.wideband = u32::from(wideband);
    }

    /// RMS of the excitation of sub-frame `i` (`SPEEX_GET_EXC`).
    #[must_use]
    pub fn exc_rms(&self, i: usize) -> f32 {
        let s = EXC + i * SUBFRAME;
        dsp::compute_rms(&self.exc_buf[s..s + SUBFRAME])
    }
}

/// Result of decoding one layer: 0 ok, −1 end of packet (terminator or
/// too few bits), −2 corrupt.
pub type Ret = i32;

/// The in-band request handler (`speex_inband_handler`): only the stereo
/// request is acted on (when the stream is stereo), as in ffmpeg.
fn inband(buf: &[u8], bits: &mut Bits, stereo: Option<&mut Stereo>) {
    let id = bits.unpack(buf, 4);
    if id == 9
        && let Some(s) = stereo
    {
        s.request(buf, bits);
        return;
    }
    let adv = match id {
        0..2 => 1,
        2..8 => 4,
        8..10 => 8,
        10..12 => 16,
        12..14 => 32,
        _ => 64,
    };
    bits.advance(adv);
}

/// `speex_default_user_handler`.
fn user_inband(buf: &[u8], bits: &mut Bits) {
    let req = bits.unpack(buf, 4) as i32;
    bits.advance(5 + 8 * req);
}

/// Skip one wideband layer header and payload; `Err` on corruption.
fn skip_wb_layer(buf: &[u8], bits: &mut Bits) {
    let submode = bits.unpack(buf, 3) as usize;
    let advance = WB_SKIP[submode & 7] - 4;
    bits.advance(advance);
}

/// The start of `nb_decode`: find the next narrowband block, handling
/// in-band requests and skipping wideband layers. Returns the sub-mode.
fn read_mode(buf: &[u8], bits: &mut Bits, mut stereo: Option<&mut Stereo>) -> Result<u32, Ret> {
    loop {
        if bits.remaining() < 5 {
            return Err(-1);
        }
        if bits.unpack(buf, 1) != 0 {
            skip_wb_layer(buf, bits);
            if bits.remaining() < 5 {
                return Err(-1);
            }
            if bits.unpack(buf, 1) != 0 {
                skip_wb_layer(buf, bits);
                if bits.unpack(buf, 1) != 0 {
                    // More than two wideband layers.
                    return Err(-2);
                }
            }
        }
        if bits.remaining() < 4 {
            return Err(-1);
        }
        let m = bits.unpack(buf, 4);
        match m {
            15 => return Err(-1),
            14 => inband(buf, bits, stereo.as_deref_mut()),
            13 => user_inband(buf, bits),
            9..=12 => return Err(-2),
            _ => return Ok(m),
        }
    }
}

/// Bits of a narrowband frame by sub-mode, header included
/// (`bits_per_frame` in `modes.c`; null mode: the 5 header bits).
const BITS_PER_FRAME: [i32; 9] = [5, 43, 119, 160, 220, 300, 364, 492, 79];

/// Step over one frame without decoding it, keeping the state that never
/// converges by itself: the noise generator's seed, the vocoder's
/// memories (it runs the vocoder, which needs no history), the DTX flag (and in-band stereo parameters via `stereo`). Used
/// to position the decoder before a seek target cheaply.
pub fn skim(st: &mut NbState, buf: &[u8], bits: &mut Bits, stereo: Option<&mut Stereo>) -> Ret {
    let m = match read_mode(buf, bits, stereo) {
        Ok(m) => m,
        Err(r) => return r,
    };
    st.submode = m;
    match m {
        0 => {
            math::skip_rand(&mut st.seed, FRAME);
        }
        1 => {
            // The vocoder's excitation depends on the bit-stream, the seed
            // and its own memories only: run it.
            bits.advance(18);
            let ol_pitch = PITCH_START + bits.unpack(buf, 7) as i32;
            let coef = tables::PITCH_COEF[bits.unpack(buf, 4) as usize & 15];
            let ol_gain = tables::OL_GAIN[bits.unpack(buf, 5) as usize & 31];
            st.dtx = u32::from(bits.unpack(buf, 4) == 15);
            for _ in 0..NB_SUBFRAMES {
                let mut innov = [0.0f32; SUBFRAME];
                dsp::noise_unquant(&mut innov, &mut st.seed);
                for v in &mut innov {
                    *v *= ol_gain;
                }
                let mut exc = [0.0f32; SUBFRAME];
                vocoder(st, &mut exc, &innov, ol_pitch, coef, ol_gain);
            }
        }
        _ => {
            bits.advance(BITS_PER_FRAME[m as usize] - 5);
            st.dtx = 0;
        }
    }
    0
}

/// The vocoder of sub-mode 1 (one sub-frame): a pulse train at the
/// open-loop pitch mixed with the noise innovation, written to `exc`.
fn vocoder(
    st: &mut NbState,
    exc: &mut [f32; SUBFRAME],
    innov: &[f32; SUBFRAME],
    ol_pitch: i32,
    coef: f32,
    ol_gain: f32,
) {
    let g = (1.5 * (coef - 0.2)).clamp(0.0, 1.0);
    let amp = math::sqrt((2 * ol_pitch) as f32) * (g * ol_gain);
    let step = ol_pitch.max(1);
    while st.voc_offset < SUBFRAME as i32 {
        if st.voc_offset >= 0 {
            exc[st.voc_offset as usize] = amp;
        }
        st.voc_offset += step;
    }
    st.voc_offset -= SUBFRAME as i32;
    for i in 0..SUBFRAME {
        let exci = exc[i];
        let v = (0.7 * exci + 0.3 * st.voc_m1) + ((1.0 - 0.85 * g) * innov[i] - (0.15 * g) * st.voc_m2);
        st.voc_m1 = exci;
        st.voc_m2 = innov[i];
        st.voc_mean = 0.8 * st.voc_mean + 0.2 * v;
        exc[i] = v - st.voc_mean;
    }
}

/// `nb_decode`. `out` takes [`FRAME`] samples; `innov_save`, when given,
/// receives the innovation (the wideband layer folds it).
#[allow(clippy::too_many_lines)]
pub fn decode(
    st: &mut NbState,
    buf: &[u8],
    bits: &mut Bits,
    stereo: Option<&mut Stereo>,
    out: &mut [f32],
    mut innov_save: Option<&mut [f32]>,
) -> Ret {
    let m = match read_mode(buf, bits, stereo) {
        Ok(m) => m,
        Err(r) => return r,
    };
    st.submode = m;

    // Shift the excitation history by one frame.
    st.exc_buf.copy_within(FRAME..FRAME + MOVE, 0);

    let Some(sm) = NB_SUBMODES[m as usize] else {
        // Null mode: noise through the last filter, bandwidth-expanded.
        let mut lpc = [0.0f32; ORDER];
        dsp::bw_lpc(0.93, &st.interp_qlpc, &mut lpc);
        let innov_gain = dsp::compute_rms(&st.exc_buf[EXC..EXC + FRAME]);
        for i in 0..FRAME {
            st.exc_buf[EXC + i] = math::rand(innov_gain, &mut st.seed);
        }
        st.first = 1;
        dsp::iir_mem16_to(&st.exc_buf[EXC..EXC + FRAME], &lpc, &mut out[..FRAME], &mut st.mem_sp);
        return 0;
    };

    let mut qlsp = [0.0f32; ORDER];
    dsp::lsp_unquant(sm.lsp, &mut qlsp, buf, bits);
    if st.first != 0 {
        st.old_qlsp = qlsp;
    }

    let mut ol_pitch = 0i32;
    if sm.lbr_pitch != -1 {
        ol_pitch = PITCH_START + bits.unpack(buf, 7) as i32;
    }
    let mut ol_pitch_coef = 0.0f32;
    if sm.forced_pitch_gain {
        ol_pitch_coef = tables::PITCH_COEF[bits.unpack(buf, 4) as usize & 15];
    }
    let ol_gain = tables::OL_GAIN[bits.unpack(buf, 5) as usize & 31];

    if m == 1 {
        st.dtx = u32::from(bits.unpack(buf, 4) == 15);
    }
    if m > 1 {
        st.dtx = 0;
    }

    let mut best_pitch = 40i32;
    let mut best_pitch_gain = 0.0f32;
    for sub in 0..NB_SUBFRAMES {
        let offset = SUBFRAME * sub;
        let e = EXC + offset;
        st.exc_buf[e..e + SUBFRAME].fill(0.0);

        let (pit_min, pit_max) = if sm.lbr_pitch != -1 {
            let margin = sm.lbr_pitch;
            if margin != 0 {
                ((ol_pitch - margin + 1).max(PITCH_START), (ol_pitch + margin).min(PITCH_END))
            } else {
                (ol_pitch, ol_pitch)
            }
        } else {
            (PITCH_START, PITCH_END)
        };
        let _ = pit_max;
        let mut exc32 = [0.0f32; SUBFRAME];
        let (pitch, pitch_gain) = match sm.ltp {
            Ltp::Forced => dsp::forced_pitch_unquant(&mut st.exc_buf, e, &mut exc32, pit_min, ol_pitch_coef),
            Ltp::ThreeTap(cdbk, gain_bits, pitch_bits) => dsp::pitch_unquant_3tap(
                &st.exc_buf,
                e,
                &mut exc32,
                pit_min,
                cdbk,
                gain_bits,
                pitch_bits,
                buf,
                bits,
            ),
            Ltp::None => (pit_min, [0.0; 3]),
        };
        dsp::sanitize(&mut exc32, -32000.0, 32000.0);

        let g = pitch_gain;
        let tmp = g[1].abs()
            + (if g[0] > 0.0 { g[0] } else { -0.5 * g[0] })
            + (if g[2] > 0.0 { g[2] } else { -0.5 * g[2] });
        let near = |a: i32, b: i32| (a - b).abs();
        if (tmp > best_pitch_gain
            && near(2 * best_pitch, pitch) >= 3
            && near(3 * best_pitch, pitch) >= 4
            && near(4 * best_pitch, pitch) >= 5)
            || (tmp > 0.6 * best_pitch_gain
                && (near(best_pitch, 2 * pitch) < 3
                    || near(best_pitch, 3 * pitch) < 4
                    || near(best_pitch, 4 * pitch) < 5))
            || (0.67 * tmp > best_pitch_gain
                && (near(2 * best_pitch, pitch) < 3
                    || near(3 * best_pitch, pitch) < 4
                    || near(4 * best_pitch, pitch) < 5))
        {
            best_pitch = pitch;
            if tmp > best_pitch_gain {
                best_pitch_gain = tmp;
            }
        }

        // Innovation.
        let mut innov = [0.0f32; SUBFRAME];
        let ener = match sm.have_subframe_gain {
            3 => EXC_GAIN_SCAL3[bits.unpack(buf, 3) as usize & 7] * ol_gain,
            1 => EXC_GAIN_SCAL1[bits.unpack(buf, 1) as usize & 1] * ol_gain,
            _ => ol_gain,
        };
        match sm.innov {
            Innov::Split(p) => dsp::split_cb_unquant(&mut innov, &p, buf, bits),
            Innov::Noise => dsp::noise_unquant(&mut innov, &mut st.seed),
            Innov::None => {}
        }
        for v in &mut innov {
            *v *= ener;
        }
        if sm.double_codebook {
            let mut innov2 = [0.0f32; SUBFRAME];
            match sm.innov {
                Innov::Split(p) => dsp::split_cb_unquant(&mut innov2, &p, buf, bits),
                Innov::Noise => dsp::noise_unquant(&mut innov2, &mut st.seed),
                Innov::None => {}
            }
            let s = 0.454_545 * ener;
            for (a, b) in innov.iter_mut().zip(&innov2) {
                *a += s * b;
            }
        }
        for i in 0..SUBFRAME {
            st.exc_buf[e + i] = exc32[i] + innov[i];
        }
        if let Some(save) = innov_save.as_deref_mut() {
            save[offset..offset + SUBFRAME].copy_from_slice(&innov);
        }

        // Vocoder mode.
        if m == 1 {
            let mut exc = [0.0f32; SUBFRAME];
            vocoder(st, &mut exc, &innov, ol_pitch, ol_pitch_coef, ol_gain);
            st.exc_buf[e..e + SUBFRAME].copy_from_slice(&exc);
        }
    }

    // Enhancer (or plain copy): the output lags the excitation by one
    // sub-frame.
    if st.enh != 0 && sm.comb_gain > 0.0 {
        let (o1, o2) = out[..FRAME].split_at_mut(2 * SUBFRAME);
        dsp::multicomb(&st.exc_buf, EXC - SUBFRAME, o1, 2 * SUBFRAME, best_pitch, 40, sm.comb_c1, sm.comb_c2);
        dsp::multicomb(&st.exc_buf, EXC + SUBFRAME, o2, 2 * SUBFRAME, best_pitch, 40, sm.comb_c1, sm.comb_c2);
    } else {
        out[..FRAME].copy_from_slice(&st.exc_buf[EXC - SUBFRAME..EXC - SUBFRAME + FRAME]);
    }

    // Synthesis.
    let mut interp_qlsp = [0.0f32; ORDER];
    let mut ak = [0.0f32; ORDER];
    for sub in 0..NB_SUBFRAMES {
        let offset = SUBFRAME * sub;
        dsp::lsp_interpolate(&st.old_qlsp, &qlsp, &mut interp_qlsp, sub, NB_SUBFRAMES, LSP_MARGIN, LSP_TOP);
        dsp::lsp_to_lpc(&interp_qlsp, &mut ak);
        let mut pi_g = 1.0f32;
        for i in (0..ORDER).step_by(2) {
            pi_g += ak[i + 1] - ak[i];
        }
        st.pi_gain[sub] = pi_g;
        dsp::iir_mem16(&mut out[offset..offset + SUBFRAME], &st.interp_qlpc, &mut st.mem_sp);
        st.interp_qlpc = ak;
    }
    dsp::highpass(&mut out[..FRAME], if st.wideband != 0 { 3 } else { 1 }, &mut st.mem_hp);
    st.old_qlsp = qlsp;
    st.first = 0;
    0
}
