//! 2× and 4× decimation for 88.2–192 kHz sources (04 §3.2) with half-band
//! FIR filters.
//!
//! # Coefficients
//!
//! Both filters are Kaiser-windowed half-band low-passes,
//! `h[n] = ½·sinc(n/2)·w_kaiser(n; β)` for `n = −(N−1)/2 … (N−1)/2`, so every
//! even tap except the centre (½) is zero. The tables hold the odd taps
//! `h[2k+1]` (symmetric) in Q2.30, rounded; `tests/decimate.rs` recomputes
//! them from this definition and checks the tables, and measures the
//! response of the quantised filters.
//!
//! | Filter | N | β | Pass band (of input rate) | Ripple | Stop band | Attenuation | Products per output |
//! |---|---|---|---|---|---|---|---|
//! | [`HB81`] | 81 | 9.4 | 0 … 0.2125 | ±0.0002 dB | 0.2875 … 0.5 | ≥ 93.9 dB | 20 + centre |
//! | [`HB27`] | 27 | 10.0 | 0 … 0.1125 | ±0.0001 dB | 0.3875 … 0.5 | ≥ 100 dB | 7 + centre |
//!
//! 2× (88.2/96 kHz → 44.1/48 kHz) uses HB81: flat to 0.425 × the output
//! rate (18.7 kHz at 44.1 kHz, 20.4 kHz at 48 kHz) and alias-free to 94 dB
//! there; only the band 0.425–0.5 × output rate receives aliases (from the
//! transition band, above 18.7 kHz). 94 dB is below the 16-bit dither floor
//! relative to the ultrasonic content of real hi-res recordings (≤ −40 dBFS).
//! The length is a cost decision: the 129-tap design for 0.45 × output rate
//! and 100 dB needed 32 products per output (≈ 60 % more). 4× (176.4/192
//! kHz) runs HB27 first (to 88.2/96 kHz; its narrow pass band suffices
//! because HB81 removes everything above 0.2125 of its rate afterwards),
//! then HB81.
//!
//! Delay: 39 input samples for HB81 (0.41 ms at 96 kHz); HB27 adds 13.
//!
//! The filters have unity gain at DC (sum of taps 1 − 3.5·10⁻⁶) but
//! overshoot on full-scale steps (Gibbs, ≈ 9 %): the chain counts the stage
//! as one that may boost, so the limiter engages.

/// Odd taps `h[2k+1]`, k = 0 … 19, of the 81-tap half-band, Q2.30.
pub const HB81: [i32; 20] = [
    340_834_836,
    -111_112_206,
    63_756_194,
    -42_572_979,
    30_244_268,
    -22_067_787,
    16_243_792,
    -11_932_130,
    8_681_270,
    -6_218_921,
    4_363_555,
    -2_983_419,
    1_976_472,
    -1_260_285,
    766_890,
    -440_101,
    234_003,
    -111_901,
    45_384,
    -13_331,
];

/// Odd taps `h[2k+1]`, k = 0 … 6, of the 27-tap half-band, Q2.30.
pub const HB27: [i32; 7] = [332_310_768, -88_209_806, 33_006_381, -11_056_462, 2_777_812, -403_919, 9_337];

/// Kaiser β and tap counts of the two filters (for the design check).
pub const HB81_DESIGN: (usize, f32) = (81, 9.4);
pub const HB27_DESIGN: (usize, f32) = (27, 10.0);

const CENTRE: i64 = 1 << 29; // ½ in Q2.30

/// One half-band decimating stage with `P` pairs of odd taps (an
/// `N = 4P + 1`-tap design whose outermost taps are even and therefore
/// zero, or an `N = 4P − 1`-tap one).
///
/// Polyphase storage: an output frame is formed after every second input
/// frame `x[n]`. The odd taps touch only samples of `x[n]`'s parity — the
/// last `2P` of them are kept in `even` — and the centre tap one sample of
/// the other parity, `2P − 1` frames back, kept in `odd`. With `even`'s
/// window oldest … newest, pair `k` is `w[P−1−k] + w[P+k]`: two contiguous
/// runs read with post-indexed loads. Each ring is written twice (at `i`
/// and `i + len`) so a contiguous window always exists. `E2 = 4P`,
/// `O2 = 2P`.
#[derive(Debug, Clone)]
pub struct HalfBand<const P: usize, const E2: usize, const O2: usize> {
    taps: &'static [i32; P],
    even: [[i32; E2]; 2],
    odd: [[i32; O2]; 2],
    /// Positions of the newest sample in the first half of each ring.
    pe: usize,
    po: usize,
    /// The next input frame completes a pair.
    second: bool,
}

impl<const P: usize, const E2: usize, const O2: usize> HalfBand<P, E2, O2> {
    const CHECK: () = assert!(E2 == 4 * P && O2 == 2 * P && P > 0);

    #[must_use]
    pub const fn new(taps: &'static [i32; P]) -> Self {
        let () = Self::CHECK;
        Self { taps, even: [[0; E2]; 2], odd: [[0; O2]; 2], pe: 0, po: 0, second: false }
    }

    pub fn reset(&mut self) {
        self.even = [[0; E2]; 2];
        self.odd = [[0; O2]; 2];
        self.pe = 0;
        self.po = 0;
        self.second = false;
    }

    /// Filter and decimate `buf` (interleaved, `CH` channels) in place;
    /// returns the number of output frames, which are at the start of `buf`.
    pub fn process<const CH: usize>(&mut self, buf: &mut [i32]) -> usize {
        let frames = buf.len() / CH;
        let mut out = 0;
        for i in 0..frames {
            if !self.second {
                self.second = true;
                self.po = if self.po + 1 >= P { 0 } else { self.po + 1 };
                let po = self.po;
                for c in 0..CH {
                    let x = buf[i * CH + c];
                    self.odd[c][po] = x;
                    self.odd[c][po + P] = x;
                }
                continue;
            }
            self.second = false;
            self.pe = if self.pe + 1 >= 2 * P { 0 } else { self.pe + 1 };
            let pe = self.pe;
            // Window starts: the oldest of the last 2P (resp. P) samples.
            let we = pe + 1;
            let wo = self.po + 1;
            for c in 0..CH {
                let x = buf[i * CH + c];
                self.even[c][pe] = x;
                self.even[c][pe + 2 * P] = x;
                let centre = self.odd[c][wo];
                let w = &self.even[c][we..we + 2 * P];
                let (left, right) = w.split_at(P);
                let mut acc = CENTRE * i64::from(centre) + (1 << 29);
                // Four pairs per iteration (immediate-offset loads, one
                // loop branch per four `smlal`s), then the rest.
                let (lrem, lch) = left.as_rchunks::<4>();
                let (rch, rrem) = right.as_chunks::<4>();
                let (hch, hrem) = self.taps.as_chunks::<4>();
                for ((l, r), h) in lch.iter().rev().zip(rch).zip(hch) {
                    acc += i64::from(h[0]) * i64::from(l[3].wrapping_add(r[0]));
                    acc += i64::from(h[1]) * i64::from(l[2].wrapping_add(r[1]));
                    acc += i64::from(h[2]) * i64::from(l[1].wrapping_add(r[2]));
                    acc += i64::from(h[3]) * i64::from(l[0].wrapping_add(r[3]));
                }
                for ((&a, &b), &h) in lrem.iter().rev().zip(rrem).zip(hrem) {
                    acc += i64::from(h) * i64::from(a.wrapping_add(b));
                }
                buf[out * CH + c] = (acc >> 30) as i32;
            }
            out += 1;
        }
        out
    }
}

/// HB81 as a stage.
pub type Hb81 = HalfBand<20, 80, 40>;
/// HB27 as a stage.
pub type Hb27 = HalfBand<7, 28, 14>;

/// The decimator: factor 1 (off), 2 or 4.
///
/// RAM: 960 bytes of HB81 history + 336 bytes of HB27 history (two
/// channels of `i32`).
#[derive(Debug, Clone)]
pub struct Decimator {
    factor: u8,
    a: Hb27,
    b: Hb81,
}

impl Default for Decimator {
    fn default() -> Self {
        Self::new()
    }
}

impl Decimator {
    #[must_use]
    pub const fn new() -> Self {
        Self { factor: 1, a: Hb27::new(&HB27), b: Hb81::new(&HB81) }
    }

    /// 1, 2 or 4 (anything else is treated as 1). Clears the history.
    pub fn set_factor(&mut self, factor: u8) {
        self.factor = if matches!(factor, 2 | 4) { factor } else { 1 };
        self.reset();
    }

    #[must_use]
    pub fn factor(&self) -> u8 {
        self.factor
    }

    #[must_use]
    pub fn is_active(&self) -> bool {
        self.factor > 1
    }

    pub fn reset(&mut self) {
        self.a.reset();
        self.b.reset();
    }

    /// Decimate `buf` in place; returns the output frame count.
    pub fn process(&mut self, buf: &mut [i32], channels: usize) -> usize {
        match channels {
            1 => self.run::<1>(buf),
            2 => self.run::<2>(buf),
            _ => buf.len() / channels.max(1),
        }
    }

    fn run<const CH: usize>(&mut self, buf: &mut [i32]) -> usize {
        match self.factor {
            2 => self.b.process::<CH>(buf),
            4 => {
                let n = self.a.process::<CH>(buf);
                self.b.process::<CH>(&mut buf[..n * CH])
            }
            _ => buf.len() / CH,
        }
    }
}
