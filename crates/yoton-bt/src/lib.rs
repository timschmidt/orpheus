//! Protocol of the Bluetooth transmitter module ("ATS2851") on UART1
//! (06-connectivity §4, docs/hw/bluetooth.md §3–4). Hardware-free: the
//! UART, GPIO20 (enable) and GPIO31 (host busy) live in `yoton-bsp::bt`.
//!
//! * [`frame`]: `AA`-framed codec with checksum, length limit and
//!   resynchronisation.
//! * [`proto`]: typed commands and events, volume scaling.
//! * [`link`]: one-command-at-a-time layer with acknowledge timeout and
//!   resend.
//! * [`transmitter`]: the transmitter-role state machine.
//! * [`restricted`]: upgrade, shutdown, baud and settings commands, which
//!   nothing in this crate sends on its own (CLAUDE.md rule 6).

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod frame;
pub mod link;
pub mod proto;
pub mod restricted;
pub mod transmitter;

pub use link::Transport;
pub use proto::{Addr, Command, Event, Key};
pub use transmitter::{Config, Output, State, TrackInfo, Transmitter};
