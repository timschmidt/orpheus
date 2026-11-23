//! Yoton YM03 board support (docs/plan/01-hardware.md §2, §5).
//!
//! Knows the board: which pin does what, the panel and its init sequence,
//! the key ladders, the backlight channel. The SoC lives in `atj2157-hal`.

#![no_std]

pub mod display;
pub mod pins;
