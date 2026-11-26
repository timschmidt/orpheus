//! The software DSP chain (docs/plan/04-audio.md §5.1, WP AU-6).
//!
//! It sits between the decoder and the PCM ring and works in place on the
//! decoder's block ([`orpheus_audio::PcmBlock`]: interleaved `i16`, or `i32`
//! left-justified 24-bit), at most two channels:
//!
//! ```text
//! decoder block ─► load to Q4.27 ─► decimator ─► gain ─► EQ ─► bass/treble ─► crossfeed
//!   ─► channel mode + balance ─► limiter ─► fade ─► round/dither to 16 (or 24) bits ─► block ─► meter
//! ```
//!
//! ```ignore
//! let mut chain = Chain::new(OutWidth::Bits16, false);
//! let dac_rate = chain.set_format(Format { rate, channels, kind, bits })?;
//! chain.set_eq_preset(Preset::Rock);
//! chain.set_crossfeed(Some(CrossfeedParams::DEFAULT));
//! // per decoder block:
//! let n = chain.process(&mut block);   // valid samples now at the start
//! ```
//!
//! # Principles
//!
//! * **Bit-transparent when flat** (04 §1.2). Every stage reports whether it
//!   is active; when none is, [`Chain::process`] returns without touching
//!   the block. A stage that returns to flat ramps to an exact identity and
//!   then drops out.
//! * **Click-free changes.** Gains, the channel matrix and the crossfeed
//!   level ramp per frame (30–40 ms); filters move their parameters
//!   (log-frequency, Q, dB) over 30 ms, with one design per 64-frame chunk
//!   and per-frame linear interpolation of the coefficients in between.
//! * **Fixed point on the sample path** (02 §10): Q4.27 samples (24 dB of
//!   headroom), Q3.28 coefficients, 64-bit products (`smull`/`smlal`). `f32`
//!   only for coefficient design and the limiter's gain computer above its
//!   knee; `f64` only in host tests.
//! * **Fixed memory**: all state lives in [`Chain`] ([`chain::ram_bytes`]);
//!   no allocation, no `unsafe`.
//!
//! # Order, and where it differs from the list in the plan
//!
//! * The **decimator runs first**, not after the limiter: every later stage
//!   then runs at the output rate (2–4× cheaper), the filter designs keep
//!   their precision (60 Hz at 192 kHz is much harder in fixed point than at
//!   48 kHz), and the limiter sees the decimator's overshoot.
//! * **Channel mode before balance** (inside one matrix): with mono, a
//!   balance applied first would be summed away.
//!
//! # Headroom
//!
//! A full-scale input stays below 16 × full scale everywhere if the gains
//! and the peak filter boost together stay below 24 dB. The gain stage
//! takes ReplayGain, its pre-amp and the EQ pre-amp (automatic by default:
//! minus the peak of the EQ + tone response) and is lowered further so that
//! gain + peak filter boost ≤ 18 dB ([`chain::HEADROOM_MB`]), leaving 6 dB
//! for filter overshoot. The limiter then brings anything above −2 dBFS
//! softly under −0.1 dBFS.
//!
//! # Hardware engine
//!
//! The equaliser runs its sections through [`BiquadCascade`]; the hardware
//! FFT/IIR engine (WP DSP-1) implements that trait and is passed to
//! [`Chain::with_cascade`]; [`SoftCascade`] stays as the reference.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod biquad;
pub mod chain;
pub mod click;
pub mod cost;
pub mod crossfeed;
pub mod decimate;
pub mod dither;
pub mod eq;
pub mod fade;
pub mod fixed;
pub mod gain;
pub mod limiter;
pub mod math;
pub mod matrix;
pub mod meter;

pub use biquad::{BiquadCascade, Coeffs, Shape, SoftCascade};
pub use chain::{CHUNK, Chain, EqPreamp, Format, OutWidth, PAUSE_FADE_MS};
pub use cost::{Cost, StageId};
pub use crossfeed::CrossfeedParams;
pub use dither::DitherMode;
pub use eq::{Band, MAX_BANDS, Preset};
pub use gain::{RgMode, RgSettings, RgTags};
pub use matrix::ChannelMode;
pub use meter::{Levels, MeterMode};
