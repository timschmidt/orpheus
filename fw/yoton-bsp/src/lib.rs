//! Yoton YM03 board support (docs/plan/01-hardware.md §2, §5).
//!
//! Knows the board: which pin does what, the panel and its init sequence,
//! the key ladders, the backlight channel. The SoC lives in `atj2157-hal`.

#![no_std]

pub mod bt;
pub mod card;
pub mod charger;
pub mod display;
pub mod keys;
pub mod pins;
pub mod settings;
pub mod tiny;
