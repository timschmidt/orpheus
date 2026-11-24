//! Opus decoder (04 §4.1 priority A, WP AU-8).
//!
//! A port of the libopus 1.5.2 decoder (BSD-3-Clause, Xiph.Org Foundation,
//! Skype Limited, Octasic, Jean-Marc Valin, Timothy B. Terriberry, CSIRO,
//! Gregory Maxwell, Mark Borgerding, Erik de Castro Lopo; licence text in
//! `LICENSE-XIPH`): SILK in integer arithmetic exactly as libopus (every
//! libopus build decodes SILK in fixed point), CELT in `f32` as libopus's
//! floating-point build. Ogg encapsulation per RFC 7845.

#![no_std]
#![forbid(unsafe_code)]
#![allow(
    clippy::too_many_arguments,
    clippy::too_many_lines,
    clippy::needless_range_loop,
    clippy::cast_lossless,
    clippy::manual_memcpy,
    clippy::excessive_precision,
    clippy::unreadable_literal,
    clippy::approx_constant,
    // NaN-aware comparisons, as in libopus (`!(a > b)` is true for NaN).
    clippy::neg_cmp_op_on_partial_ord
)]

#[cfg(test)]
extern crate std;

pub mod celt;
pub mod entdec;
pub mod ogg_opus;
pub mod opus_dec;
pub mod packet;
pub mod pcm;
pub mod silk;

pub use ogg_opus::{ARENA_MIN, OpusDecoder, OpusHead, arena_for};

#[cfg(test)]
mod diag_tests;
#[cfg(test)]
mod tests;
