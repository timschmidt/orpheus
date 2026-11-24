//! HID keyboard without hardware (06 §4.3.1 "built in every case", §5.4;
//! WP USB-5, reused by BT-4):
//!
//! * [`descriptor`]: report descriptors (the boot keyboard);
//! * [`report`]: the 8-byte boot keyboard report and the LED output report;
//! * [`usage`]: keyboard usage ids and modifier bits;
//! * [`layout`]: which keys produce a character on the computer's side
//!   (US, UK, German, French);
//! * [`typist`]: text to a paced press/release report sequence that always
//!   ends with every key released, and the [`typist::Transport`] trait the
//!   USB and Bluetooth back ends implement;
//! * [`host`]: a fake HID host that turns reports back into text (the
//!   simulator and the tests use it to see what a computer would type).
//!
//! The device cannot see the computer's keyboard layout: the user chooses
//! it (Settings › USB › Keyboard layout). Characters the layout cannot
//! produce without dead keys are refused before anything is sent, so a
//! code is typed completely or not at all.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod descriptor;
pub mod host;
pub mod layout;
pub mod report;
pub mod typist;
pub mod usage;

pub use layout::{Layout, Stroke};
pub use report::{KeyboardReport, Leds};
pub use typist::{Transport, Typist, TypistConfig};
