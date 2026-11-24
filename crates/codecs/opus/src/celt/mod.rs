//! CELT (RFC 6716 §4.3), float build of libopus 1.5.2.

pub mod bands;
pub mod decoder;
pub mod math;
pub mod mdct;
pub mod modes;
pub mod plc;
pub mod rate;
pub mod tables;
pub mod vq;

pub use decoder::{CeltError, CeltState, MEM_PER_CH, SCRATCH_LEN, decode};

#[cfg(test)]
pub(crate) mod tests;
