//! CPU-cost model: Cortex-M4 cycles per output frame for each stage.
//!
//! The figures come from the inner loops of the `thumbv7em-none-eabihf`
//! release build (opt-level "s", fat LTO, as the firmware profile), counted
//! instruction by instruction in the disassembly (2026-11-23, rustc 1.98.1)
//! with the Cortex-M4 timings: 1 cycle for ALU operations, `mul`/`mla`,
//! `smull`/`smlal`, `ssat`/`qadd`; 2 for an isolated `ldr` (1 for each
//! further load pipelined behind it), 1–2 for `str`; 2–3 for a taken branch
//! (pipeline refill); FPU `vdiv` 14. Memory is assumed zero-wait (SRAM).
//! Per-chunk control code (once per 64 frames) and ramps (only while a
//! parameter moves; they roughly double a stage while they run) are left out
//! of the steady-state numbers.
//!
//! The on-device benchmark (`xtask dev hil audio --bench`) is to replace
//! these with measured numbers; the host benchmark (`cargo bench -p
//! yoton-dsp`) checks relative costs.
//!
//! Scale: 1 cycle per frame at 44.1 kHz is 44 100 cycles per second, 0.092 %
//! of the 48 MHz Base level. Examples (stereo, TPDF dither; printed by the
//! `cost_model_table` test, which builds each configuration and asks
//! [`crate::Chain::cost`]):
//!
//! | Configuration | Cycles/frame | % of 48 MHz | % of 192 MHz |
//! |---|---|---|---|
//! | Everything flat | 0 | 0 | 0 |
//! | ReplayGain −7 dB, 16/44.1 | 68 (gain 18, in/out + dither 50) | 6.2 | 1.6 |
//! | 5-band EQ (Rock) + auto pre-amp, 16/44.1 | 314 (EQ 220, limiter 26) | 28.8 | 7.2 |
//! | Crossfeed, 16/44.1 | 96 | 8.8 | 2.2 |
//! | 24/48 source, all flat (dither to 16 bits) | 50 | 5.0 | 1.3 |
//! | 24/96 → 48 kHz, otherwise flat | 578 (decimator 492) | 57.8 | 14.5 |
//! | 24/192 → 48 kHz, otherwise flat | 1062 (decimator 956) | 106 | 26.6 |
//! | Everything: 10 bands, tone, crossfeed, balance, meters, 16/44.1 | 686 | 63.0 | 15.8 |
//!
//! Against the targets of 04 §5.1 ("at Base"): the equaliser and the
//! decimator are over (28.8 % vs 6 %, 58 % vs 8 %); the plan's own design
//! consequence (04 §4.3: a higher clock level with effects) applies, and the
//! biquad loop is the first candidate for hand-tuning or the IIR engine
//! (DSP-1).

use crate::dither::DitherMode;
use crate::meter::MeterMode;

/// The stages, in chain order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum StageId {
    Decimator = 0,
    Gain = 1,
    Eq = 2,
    Tone = 3,
    Crossfeed = 4,
    Matrix = 5,
    Limiter = 6,
    Fade = 7,
    /// Load into the work buffer, round or dither, store.
    Output = 8,
    Meter = 9,
}

/// Cycles per output frame, per stage and in total.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Cost {
    pub per_stage: [u32; 10],
    pub total: u32,
}

impl Cost {
    pub fn add(&mut self, id: StageId, cycles: u32) {
        self.per_stage[id as usize] += cycles;
        self.total += cycles;
    }

    /// Share of a core clock at `rate` output frames per second, in
    /// hundredths of a percent.
    #[must_use]
    pub fn load_bp(&self, rate: u32, clock_hz: u32) -> u32 {
        (u64::from(self.total) * u64::from(rate) * 10_000 / u64::from(clock_hz.max(1))) as u32
    }
}

/// One direct-form-I section, one sample: `smull` + 4 `smlal`, rounding
/// (`adds`/`adc`), shift-or, sample load and store, one coefficient spilled
/// to the stack, history register moves, loop: 18 instructions, ≈ 22
/// cycles.
pub const BIQUAD_PER_SAMPLE: u32 = 22;
/// Gain or fade, one sample: `ldr`, `smull`, shift-or, `str`, loop
/// (7 instructions, ≈ 9 cycles).
pub const GAIN_PER_SAMPLE: u32 = 9;
/// Crossfeed, one stereo frame: `ldrd`, side difference, low-pass
/// (`smull` + `smlal`), cross gain (`smull`), add/sub, `strd`, loop
/// (16 instructions, ≈ 20 cycles).
pub const CROSSFEED_PER_FRAME: u32 = 20;
/// Channel matrix, one stereo frame: `ldrd`, 2 × (`smull` + `smlal` +
/// shift-or), `strd`, two spilled values, loop (≈ 22 cycles).
pub const MATRIX_PER_FRAME: u32 = 22;
/// Half-band: per tap pair three loads, an add and an `smlal` (unrolled by
/// four, with spills: ≈ 10 cycles); per output sample the centre product,
/// rounding, ring arithmetic and store (≈ 30); per input sample two stores
/// into the ring and its bookkeeping (≈ 8).
pub const HALFBAND_PER_PAIR: u32 = 10;
pub const HALFBAND_OVERHEAD: u32 = 30;
pub const HALFBAND_PER_INPUT: u32 = 8;

/// `n` busy sections over `ch` channels.
#[must_use]
pub const fn biquads(n: u32, ch: u32) -> u32 {
    n * ch * BIQUAD_PER_SAMPLE
}

/// Decimating by `factor` (cost per output frame).
#[must_use]
pub const fn decimator(factor: u32, ch: u32) -> u32 {
    let hb81 = 20 * HALFBAND_PER_PAIR + HALFBAND_OVERHEAD + 2 * HALFBAND_PER_INPUT;
    let hb27 = 7 * HALFBAND_PER_PAIR + HALFBAND_OVERHEAD + 2 * HALFBAND_PER_INPUT;
    match factor {
        2 => ch * hb81,
        4 => ch * (hb81 + 2 * hb27),
        _ => 0,
    }
}

/// Loading `factor` input frames into the work buffer (load, shift, store,
/// loop: ≈ 5 per sample).
#[must_use]
pub const fn convert(factor: u32, ch: u32) -> u32 {
    5 * factor * ch
}

/// Rounding/dithering and storing one output frame.
#[must_use]
pub const fn output(mode: DitherMode, ch: u32) -> u32 {
    ch * match mode {
        // `qadd`, `ssat`, store, loop (≈ 8) + store to the block (≈ 4).
        DitherMode::Off => 12,
        // xorshift (3), TPDF (4), `qadd` ×2, `ssat`, store, loop (≈ 16) + 4.
        DitherMode::Tpdf => 20,
        // Plus three error-feedback products, `qsub`, error clamp and
        // history, several spills (≈ 44) + 4.
        DitherMode::Shaped => 48,
    }
}

/// The limiter below its knee: per sample `abs`/max (≈ 9), per frame the
/// envelope decay (`smull`), compare and loop (≈ 8). A frame above the knee
/// costs ≈ 60 more (two FPU divisions, conversions, the gain products).
#[must_use]
pub const fn limiter(ch: u32) -> u32 {
    8 + 9 * ch
}

#[must_use]
pub const fn meter(mode: MeterMode, ch: u32) -> u32 {
    ch * match mode {
        MeterMode::Off => 0,
        // Load, abs, max, clip compare, loop.
        MeterMode::Peak => 8,
        // Plus `mul` and a 64-bit add.
        MeterMode::PeakRms => 11,
    }
}
