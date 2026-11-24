//! orpheus-fm: the FM radio without hardware (WP FM-1, UI-5a; 03 §15,
//! 05 §7.3, docs/hw/fm-i2c-imu.md §2).
//!
//! The YM03 carries an RDA5807-family tuner at I2C `0x10` (sequential
//! access: writes start at register 02h, reads at 0Ah), chip id `0x5804`
//! (read on the player, 01-hardware §9), clocked from the SoC's 24 MHz
//! oscillator. Its audio goes analogue into the headphone amplifier's
//! mixer; the board code switches that route and the amplifier gain
//! through [`chip::Analog`].
//!
//! * [`chip`]: registers, the vendor's init tables (VERBATIM), the shadow
//!   of 02h–07h and every register sequence (reset, identify, load, tune,
//!   hardware seek, volume, mute, mono, standby, status read).
//! * [`region`]: bands, channel spacing, de-emphasis per region and the
//!   channel arithmetic.
//! * [`rds`]: RDS group decoding (programme service name, radio text).
//! * [`radio`]: the radio service as a time-driven state machine (start,
//!   tune, seek with wrap, auto-scan, volume ramps, stop), used by the
//!   firmware task and by the simulator.
//! * [`presets`]: up to 30 presets in their stored byte form.
//! * [`volume`]: the vendor's pairs of tuner volume and amplifier gain.
//! * `fake` (feature `fake`, and in tests): a register-level fake chip.
//!
//! `no_std`, no allocation, no `unsafe`, no panicking indexing in the
//! paths the firmware uses (every panic site would keep a file name in the
//! firmware's resident strings).

#![no_std]
#![forbid(unsafe_code)]

#[cfg(any(test, feature = "fake"))]
extern crate std;

pub mod chip;
#[cfg(any(test, feature = "fake"))]
pub mod fake;
pub mod presets;
pub mod radio;
pub mod rds;
pub mod region;
pub mod volume;

pub use chip::{Analog, Port};
pub use presets::{Preset, Presets};
pub use radio::{Info, Phase, Radio, Req};
pub use region::Region;
