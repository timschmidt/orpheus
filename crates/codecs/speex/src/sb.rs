//! Sub-band CELP high band (libspeex `sb_celp.c`, `sb_decode`, float
//! build): one layer on top of a decoded low band — wideband on
//! narrowband, ultra-wideband on wideband — and the QMF synthesis that
//! joins them. The low layer is decoded first by the caller (the C code
//! nests the decoders; here the layers run one after the other, without
//! recursion).

use bytemuck::{Pod, Zeroable};

use crate::bits::Bits;
use crate::dsp::{self, QMF_M2};
use crate::math;
use crate::modes::{Innov, SbMode};
use crate::nb::Ret;
use crate::tables;

pub const ORDER: usize = 8;
const MAX_SUBFRAME: usize = 80;
const MAX_SUBFRAMES: usize = 4;
const LSP_MARGIN: f32 = 0.05;
/// `π − 0.05` in double precision, stored as `float`.
const LSP_TOP: f32 = 3.091_592_6;

/// `0.87360 · gc_quant_bound[q]` (double product stored as `float`).
static GC_QUANT: [f32; 16] = [
    8.559445e-1,
    1.1215626e0,
    1.4695961e0,
    1.9256417e0,
    2.52321e0,
    3.306209e0,
    4.3321824e0,
    5.6765394e0,
    7.438075e0,
    9.746248e0,
    1.2770686e1,
    1.673367e1,
    2.1926443e1,
    2.8730618e1,
    3.7646263e1,
    4.9328594e1,
];

/// High-band decoder state (`SBDecState`). The QMF memories keep only the
/// odd entries of libspeex's 64-entry arrays (the even ones are unused).
#[derive(Clone, Copy, Zeroable, Pod)]
#[repr(C)]
pub struct SbState {
    g0_mem: [f32; QMF_M2],
    g1_mem: [f32; QMF_M2],
    exc_buf: [f32; MAX_SUBFRAME],
    old_qlsp: [f32; ORDER],
    interp_qlpc: [f32; ORDER],
    mem_sp: [f32; ORDER],
    /// Per sub-frame, read by the layer above.
    pub pi_gain: [f32; MAX_SUBFRAMES],
    pub exc_rms: [f32; MAX_SUBFRAMES],
    last_ener: f32,
    seed: u32,
    first: u32,
    submode: u32,
}

/// What a high-band layer reads from the layer below it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Low {
    pub pi_gain: [f32; MAX_SUBFRAMES],
    pub exc_rms: [f32; MAX_SUBFRAMES],
    pub dtx: bool,
}

impl SbState {
    /// `sb_decoder_init`.
    pub fn init(&mut self, mode: &SbMode) {
        *self = Self::zeroed();
        self.submode = mode.default_submode;
        self.first = 1;
        self.seed = 1000;
    }

    /// The low-layer view of this layer for the layer above.
    #[must_use]
    pub fn as_low(&self, dtx: bool) -> Low {
        Low { pi_gain: self.pi_gain, exc_rms: self.exc_rms, dtx }
    }
}

/// Step over this layer's part of a frame without decoding it, keeping the
/// noise seed (drawn from only in DTX comfort noise). See `nb::skim`.
pub fn skim(st: &mut SbState, mode: &SbMode, dtx: bool, buf: &[u8], bits: &mut Bits) -> Ret {
    let wideband = if bits.remaining() > 0 { bits.peek(buf) } else { 0 };
    st.submode = if wideband != 0 {
        bits.unpack(buf, 1);
        bits.unpack(buf, 3)
    } else {
        0
    };
    if st.submode == 0 {
        if dtx {
            math::skip_rand(&mut st.seed, mode.frame_size);
        }
        return 0;
    }
    if mode.submodes[st.submode as usize & 7].is_none() {
        return -2;
    }
    bits.advance(crate::modes::WB_SKIP[st.submode as usize & 7] - 4);
    0
}

/// Decode the high band of one frame into `out[fs..2fs]` (the low band is
/// in `out[..fs]`, and the low layer's innovation was saved there too),
/// then QMF-synthesise `out[..2fs]`. `innov_save` (`2fs` values) receives
/// this layer's excitation for the layer above, interleaved with zeros.
#[allow(clippy::too_many_arguments)]
pub fn decode(
    st: &mut SbState,
    mode: &SbMode,
    low: &Low,
    buf: &[u8],
    bits: &mut Bits,
    out: &mut [f32],
    mut innov_save: Option<&mut [f32]>,
    scratch: &mut [f32],
) -> Ret {
    let fs = mode.frame_size;
    let sfs = mode.subframe_size;
    let nsub = mode.nb_subframes;

    let wideband = if bits.remaining() > 0 { bits.peek(buf) } else { 0 };
    st.submode = if wideband != 0 {
        bits.unpack(buf, 1);
        bits.unpack(buf, 3)
    } else {
        0
    };
    let sm = mode.submodes[st.submode as usize & 7];
    if st.submode != 0 && sm.is_none() {
        return -2;
    }
    let Some(sm) = sm else {
        if low.dtx {
            // `sb_decode_lost` in DTX mode: comfort noise at the last energy.
            st.first = 1;
            for v in &mut out[fs..2 * fs] {
                *v = math::rand(st.last_ener, &mut st.seed);
            }
        } else {
            out[fs..2 * fs].fill(1e-15);
            st.first = 1;
        }
        dsp::iir_mem16(&mut out[fs..2 * fs], &st.interp_qlpc, &mut st.mem_sp);
        dsp::qmf_synth(out, fs, &mut st.g0_mem, &mut st.g1_mem, scratch);
        return 0;
    };

    let mut qlsp = [0.0f32; ORDER];
    let mut interp_qlsp = [0.0f32; ORDER];
    let mut ak = [0.0f32; ORDER];
    dsp::lsp_unquant(sm.lsp, &mut qlsp, buf, bits);
    if st.first != 0 {
        st.old_qlsp = qlsp;
    }
    let mut exc_ener_sum = 0.0f32;
    for sub in 0..nsub {
        let offset = sfs * sub;
        if let Some(s) = innov_save.as_deref_mut() {
            s[2 * offset..2 * offset + 2 * sfs].fill(0.0);
        }
        dsp::lsp_interpolate(&st.old_qlsp, &qlsp, &mut interp_qlsp, sub, nsub, LSP_MARGIN, LSP_TOP);
        dsp::lsp_to_lpc(&interp_qlsp, &mut ak);
        // Response ratio between the low and high filters at 4 kHz.
        let mut pi = 1.0f32;
        let mut rh = 1.0f32;
        for i in (0..ORDER).step_by(2) {
            rh += ak[i + 1] - ak[i];
            pi += ak[i] + ak[i + 1];
        }
        st.pi_gain[sub] = pi;
        let rl = low.pi_gain[sub];
        let filter_ratio = (rl + 0.01) / (rh + 0.01);

        let mut exc = [0.0f32; MAX_SUBFRAME];
        let exc = &mut exc[..sfs];
        match sm.innov {
            Innov::None => {
                // Spectral folding of the low band's innovation.
                let g = tables::FOLD_GAIN[bits.unpack(buf, 5) as usize & 31] / filter_ratio;
                let fg = mode.folding_gain;
                let mut i = 0;
                while i < sfs {
                    exc[i] = fg * out[fs + offset + i] * g;
                    exc[i + 1] = -(fg * out[fs + offset + i + 1] * g);
                    i += 2;
                }
            }
            Innov::Noise => {}
            Innov::Split(p) => {
                let qgc = bits.unpack(buf, 4) as usize & 15;
                let el = low.exc_rms[sub];
                let mut gc = GC_QUANT[qgc];
                if sfs == 80 {
                    gc *= 1.4142;
                }
                let scale = gc * el / filter_ratio;
                dsp::split_cb_unquant(exc, &p, buf, bits);
                for v in exc.iter_mut() {
                    *v *= scale;
                }
                if sm.double_codebook {
                    let mut innov2 = [0.0f32; MAX_SUBFRAME];
                    let innov2 = &mut innov2[..sfs];
                    dsp::split_cb_unquant(innov2, &p, buf, bits);
                    let s2 = 0.4 * scale;
                    for (a, b) in exc.iter_mut().zip(innov2.iter()) {
                        *a += s2 * b;
                    }
                }
            }
        }
        if let Some(s) = innov_save.as_deref_mut() {
            for (i, &v) in exc.iter().enumerate() {
                s[2 * offset + 2 * i] = v;
            }
        }
        // The previous sub-frame's excitation through the previous
        // sub-frame's filter (libspeex's one-sub-frame lag).
        dsp::iir_mem16_to(
            &st.exc_buf[..sfs],
            &st.interp_qlpc,
            &mut out[fs + offset..fs + offset + sfs],
            &mut st.mem_sp,
        );
        st.exc_buf[..sfs].copy_from_slice(exc);
        st.interp_qlpc = ak;
        let r = dsp::compute_rms(&st.exc_buf[..sfs]);
        st.exc_rms[sub] = r;
        exc_ener_sum += r * r / nsub as f32;
    }
    st.last_ener = math::sqrt(exc_ener_sum);
    dsp::qmf_synth(out, fs, &mut st.g0_mem, &mut st.g1_mem, scratch);
    st.old_qlsp = qlsp;
    st.first = 0;
    0
}
