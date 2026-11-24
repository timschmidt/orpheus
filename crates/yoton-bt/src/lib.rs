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
//! * [`restricted`]: upgrade, shutdown, baud and settings commands
//!   (CLAUDE.md rule 6); the only one sent is the shutdown (0x24), by
//!   [`Transmitter::shutdown`] on the player's power-off path.
//! * [`driver`]: glue for the firmware task: a UART/enable-line [`driver::Port`],
//!   the TX queue, the bounded power-off shutdown, diagnostics.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod driver;
pub mod frame;
pub mod link;
pub mod proto;
pub mod restricted;
pub mod transmitter;

pub use link::Transport;
pub use proto::{Addr, Command, Event, Key};
pub use transmitter::{Config, Output, State, TrackInfo, Transmitter};
