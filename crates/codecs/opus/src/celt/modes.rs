//! The one CELT mode Opus uses (48 kHz, 960-sample frames, 120-sample
//! overlap), after libopus 1.5.2 `celt/modes.[ch]`, `celt/static_modes_float.h`
//! and `celt/cwrs.c` (BSD-3-Clause, see LICENSE-XIPH).

pub use super::tables::*;

pub const NB_EBANDS: usize = 21;
pub const EFF_EBANDS: usize = 21;
pub const OVERLAP: usize = 120;
pub const SHORT_MDCT: usize = 120;
pub const MAX_LM: usize = 3;
pub const NB_ALLOC_VECTORS: usize = 11;
pub const PREEMPH: f32 = 0.850_006_1;
pub const MAX_PERIOD: usize = 1024;
pub const DECODE_BUFFER_SIZE: usize = 2048;
pub const LPC_ORDER: usize = 24;
/// MDCT size `N` of the mode (`mdct.n`).
pub const MDCT_N: usize = 1920;

/// Mean energy per band (Q4 values as float), `eMeans`.
pub static EMEANS: [f32; 25] = [
    6.437500, 6.250000, 5.750000, 5.312500, 5.062500, 4.812500, 4.500000, 4.375000, 4.875000, 4.687500,
    4.562500, 4.437500, 4.875000, 4.625000, 4.312500, 4.500000, 4.375000, 4.625000, 4.750000, 4.437500,
    3.750000, 3.750000, 3.750000, 3.750000, 3.750000,
];

/// One kiss FFT configuration of the mode (`fft_state48000_960_N`).
#[derive(Debug, Clone, Copy)]
pub struct FftState {
    pub nfft: usize,
    pub scale: f32,
    pub shift: i32,
    pub factors: [i16; 16],
    pub bitrev: &'static [i16],
    /// The twiddles of the inverse MDCT of this size (`4 * nfft` points).
    pub trig: &'static [f32],
}

pub static FFT_STATES: [FftState; 4] = [
    FftState {
        nfft: 480,
        scale: 0.002083333,
        shift: -1,
        factors: [5, 96, 3, 32, 4, 8, 2, 4, 4, 1, 0, 0, 0, 0, 0, 0],
        bitrev: &BITREV480,
        trig: &MDCT_TRIG_1920,
    },
    FftState {
        nfft: 240,
        scale: 0.004166667,
        shift: 1,
        factors: [5, 48, 3, 16, 4, 4, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0],
        bitrev: &BITREV240,
        trig: &MDCT_TRIG_960,
    },
    FftState {
        nfft: 120,
        scale: 0.008333333,
        shift: 2,
        factors: [5, 24, 3, 8, 2, 4, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0],
        bitrev: &BITREV120,
        trig: &MDCT_TRIG_480,
    },
    FftState {
        nfft: 60,
        scale: 0.016666667,
        shift: 3,
        factors: [5, 12, 3, 4, 4, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0],
        bitrev: &BITREV60,
        trig: &MDCT_TRIG_240,
    },
];

/// Columns of [`PVQ_U`].
pub const PVQ_COLS: usize = 178;

const fn pvq_u() -> [[u32; PVQ_COLS]; 15] {
    let mut t = [[0u32; PVQ_COLS]; 15];
    t[0][0] = 1;
    let mut n = 1;
    while n < 15 {
        let mut k = 1;
        while k < PVQ_COLS {
            t[n][k] = t[n - 1][k].wrapping_add(t[n][k - 1]).wrapping_add(t[n - 1][k - 1]);
            k += 1;
        }
        n += 1;
    }
    t
}

/// Row starts of libopus's packed `CELT_PVQ_U_DATA` (row `n` holds U(n, k)
/// for `n <= k <= ROW_OFF[n + 1] - ROW_OFF[n] + n`).
static ROW_OFF: [usize; 16] =
    [0, 176, 351, 525, 698, 870, 1041, 1131, 1178, 1207, 1226, 1240, 1248, 1254, 1257, 1258];
const PVQ_DATA_LEN: usize = 1272;

const fn pvq_data() -> [u32; PVQ_DATA_LEN] {
    let t = pvq_u();
    let mut d = [0u32; PVQ_DATA_LEN];
    let mut n = 0;
    while n < 15 {
        let mut k = n;
        while ROW_OFF[n] + k < PVQ_DATA_LEN && k < PVQ_COLS {
            d[ROW_OFF[n] + k] = t[n][k];
            k += 1;
        }
        n += 1;
    }
    d
}

/// U(N, K) of the PVQ codebook, packed as libopus's `CELT_PVQ_U_DATA`
/// (computed at build time from the recurrence; 5 KB).
static PVQ_DATA: [u32; PVQ_DATA_LEN] = pvq_data();

/// U(n, k) (`CELT_PVQ_U`); 0 outside the table (never reached by valid
/// allocations).
#[inline]
#[must_use]
pub fn pvq_u_at(n: usize, k: usize) -> u32 {
    let (a, b) = if n < k { (n, k) } else { (k, n) };
    if a >= 15 || b > ROW_OFF[a + 1] - ROW_OFF[a] + a {
        return 0;
    }
    PVQ_DATA.get(ROW_OFF[a] + b).copied().unwrap_or(0)
}

#[inline]
#[must_use]
pub fn ebands(i: usize) -> i32 {
    i32::from(EBANDS.get(i).copied().unwrap_or(100))
}

/// Pulse-cache row for `band` at `LM + 1` (`m->cache.bits + m->cache.index[...]`).
#[inline]
#[must_use]
pub fn cache_row(lm1: usize, band: usize) -> &'static [u8] {
    let idx = CACHE_INDEX.get(lm1 * NB_EBANDS + band).copied().unwrap_or(0);
    let idx = usize::try_from(idx).unwrap_or(0);
    CACHE_BITS.get(idx..).unwrap_or(&[0])
}
