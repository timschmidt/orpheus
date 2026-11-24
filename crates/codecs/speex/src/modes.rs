//! Mode and sub-mode descriptions (libspeex `modes.c`, `modes_wb.c`),
//! decoder fields only.

use crate::tables;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LspKind {
    Nb,
    Lbr,
    High,
}

#[derive(Debug, Clone, Copy)]
pub struct SplitCb {
    pub subvect_size: usize,
    pub nb_subvect: usize,
    pub shape_cb: &'static [i8],
    pub shape_bits: u32,
    pub have_sign: bool,
}

#[derive(Debug, Clone, Copy)]
pub enum Ltp {
    /// No pitch (wideband high band).
    None,
    /// `forced_pitch_unquant`.
    Forced,
    /// `pitch_unquant_3tap`: gain codebook, gain bits, pitch bits.
    ThreeTap(&'static [i8], u32, u32),
}

#[derive(Debug, Clone, Copy)]
pub enum Innov {
    /// No innovation (folding in the high band).
    None,
    Noise,
    Split(SplitCb),
}

#[derive(Debug, Clone, Copy)]
pub struct Submode {
    /// −1 for normal pitch coding, else the margin around the open-loop
    /// pitch.
    pub lbr_pitch: i32,
    pub forced_pitch_gain: bool,
    pub have_subframe_gain: u8,
    pub double_codebook: bool,
    pub lsp: LspKind,
    pub ltp: Ltp,
    pub innov: Innov,
    /// Enhancer comb gain (≤ 0: enhancer off for this sub-mode) and the
    /// two constants `multicomb` derives from it, rounded as libspeex
    /// rounds them (`c1 = .4·g + .07`, `c2 = .5 + 1.72·(c1 − .07)`, double
    /// precision, stored in `float`).
    pub comb_gain: f32,
    pub comb_c1: f32,
    pub comb_c2: f32,
}

const fn split(
    subvect_size: usize,
    nb_subvect: usize,
    shape_cb: &'static [i8],
    shape_bits: u32,
    have_sign: bool,
) -> SplitCb {
    SplitCb { subvect_size, nb_subvect, shape_cb, shape_bits, have_sign }
}

const CB_NB_VLBR: SplitCb = split(10, 4, &tables::EXC_10_16_TABLE, 4, false);
const CB_NB_ULBR: SplitCb = split(20, 2, &tables::EXC_20_32_TABLE, 5, false);
const CB_NB_LBR: SplitCb = split(10, 4, &tables::EXC_10_32_TABLE, 5, false);
const CB_NB: SplitCb = split(5, 8, &tables::EXC_5_64_TABLE, 6, false);
const CB_NB_MED: SplitCb = split(8, 5, &tables::EXC_8_128_TABLE, 7, false);
const CB_SB: SplitCb = split(5, 8, &tables::EXC_5_256_TABLE, 8, false);
const CB_HIGH: SplitCb = split(8, 5, &tables::HEXC_TABLE, 7, true);
const CB_HIGH_LBR: SplitCb = split(10, 4, &tables::HEXC_10_32_TABLE, 5, false);

const LTP_NB: Ltp = Ltp::ThreeTap(&tables::GAIN_CDBK_NB, 7, 7);
const LTP_VLBR: Ltp = Ltp::ThreeTap(&tables::GAIN_CDBK_LBR, 5, 0);
const LTP_LBR: Ltp = Ltp::ThreeTap(&tables::GAIN_CDBK_LBR, 5, 7);

#[allow(clippy::too_many_arguments)]
const fn sm(
    lbr_pitch: i32,
    forced_pitch_gain: bool,
    have_subframe_gain: u8,
    double_codebook: bool,
    lsp: LspKind,
    ltp: Ltp,
    innov: Innov,
    comb: (f32, f32, f32),
) -> Submode {
    Submode {
        lbr_pitch,
        forced_pitch_gain,
        have_subframe_gain,
        double_codebook,
        lsp,
        ltp,
        innov,
        comb_gain: comb.0,
        comb_c1: comb.1,
        comb_c2: comb.2,
    }
}

const NO_COMB: (f32, f32, f32) = (-1.0, 0.0, 0.0);

/// Narrowband sub-modes 0–15 (`None`: null mode 0 and the invalid ones).
pub static NB_SUBMODES: [Option<Submode>; 16] = [
    None,
    // 1: 2150 bps vocoder-like comfort noise.
    Some(sm(0, true, 0, false, LspKind::Lbr, Ltp::Forced, Innov::Noise, NO_COMB)),
    // 2: 5.95 kbps.
    Some(sm(0, false, 0, false, LspKind::Lbr, LTP_VLBR, Innov::Split(CB_NB_VLBR), (0.6, 3.1e-1, 9.128e-1))),
    // 3: 8 kbps.
    Some(sm(
        -1,
        false,
        1,
        false,
        LspKind::Lbr,
        LTP_LBR,
        Innov::Split(CB_NB_LBR),
        (0.55, 2.9e-1, 8.783_999_7e-1),
    )),
    // 4: 11 kbps.
    Some(sm(-1, false, 1, false, LspKind::Lbr, LTP_LBR, Innov::Split(CB_NB_MED), (0.45, 2.5e-1, 8.096e-1))),
    // 5: 15 kbps.
    Some(sm(-1, false, 3, false, LspKind::Nb, LTP_NB, Innov::Split(CB_NB), (0.25, 1.7e-1, 6.72e-1))),
    // 6: 18.2 kbps.
    Some(sm(-1, false, 3, false, LspKind::Nb, LTP_NB, Innov::Split(CB_SB), (0.15, 1.3e-1, 6.032e-1))),
    // 7: 24.6 kbps.
    Some(sm(-1, false, 3, true, LspKind::Nb, LTP_NB, Innov::Split(CB_NB), (0.05, 9.0e-2, 5.344e-1))),
    // 8: 3.95 kbps.
    Some(sm(
        0,
        true,
        0,
        false,
        LspKind::Lbr,
        Ltp::Forced,
        Innov::Split(CB_NB_ULBR),
        (0.5, 2.7e-1, 8.440_000_4e-1),
    )),
    None,
    None,
    None,
    None,
    None,
    None,
    None,
];

/// The default narrowband sub-mode of a fresh decoder.
pub const NB_DEFAULT_SUBMODE: u32 = 5;

/// Wideband high-band sub-modes 0–7.
pub static WB_SUBMODES: [Option<Submode>; 8] = [
    None,
    Some(sm(0, false, 1, false, LspKind::High, Ltp::None, Innov::None, NO_COMB)),
    Some(sm(0, false, 1, false, LspKind::High, Ltp::None, Innov::Split(CB_HIGH_LBR), NO_COMB)),
    Some(sm(0, false, 1, false, LspKind::High, Ltp::None, Innov::Split(CB_HIGH), NO_COMB)),
    Some(sm(0, false, 1, true, LspKind::High, Ltp::None, Innov::Split(CB_HIGH), NO_COMB)),
    None,
    None,
    None,
];

/// Ultra-wideband (third layer) sub-modes 0–7: only the folding mode 1.
pub static UWB_SUBMODES: [Option<Submode>; 8] = [
    None,
    Some(sm(0, false, 1, false, LspKind::High, Ltp::None, Innov::None, NO_COMB)),
    None,
    None,
    None,
    None,
    None,
    None,
];

/// A sub-band layer (`SpeexSBMode`, decoder fields).
#[derive(Debug)]
pub struct SbMode {
    /// High-band frame size (= the low layer's full frame).
    pub frame_size: usize,
    pub subframe_size: usize,
    pub nb_subframes: usize,
    pub folding_gain: f32,
    pub submodes: &'static [Option<Submode>; 8],
    pub default_submode: u32,
}

pub static WB_MODE: SbMode = SbMode {
    frame_size: 160,
    subframe_size: 40,
    nb_subframes: 4,
    folding_gain: 0.9,
    submodes: &WB_SUBMODES,
    default_submode: 3,
};

pub static UWB_MODE: SbMode = SbMode {
    frame_size: 320,
    subframe_size: 80,
    nb_subframes: 4,
    folding_gain: 0.7,
    submodes: &UWB_SUBMODES,
    default_submode: 1,
};

/// Bits of a wideband layer by sub-mode, for skipping layers a decoder does
/// not use (`wb_skip_table` in `nb_celp.c`).
pub const WB_SKIP: [i32; 8] = [0, 36, 112, 192, 352, 0, 0, 0];
