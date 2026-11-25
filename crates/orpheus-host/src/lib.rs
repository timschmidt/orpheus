//! Host-side access to the player.
//!
//! The protocols are written once, `async`, over a [`transport::Transport`]
//! (WEB-3): the desktop runs them over `nusb` ([`native`], blocking front
//! ends for `xtask`), the browser over WebUSB (`orpheus-install-web`), the
//! tests over a simulated player ([`mock`]).
//!
//! * [`bot`]: USB Bulk-Only Transport framing.
//! * [`adfu`]: the boot ROM's ADFU mode and the `ymon` monitor.
//! * [`nor`]: NOR read/program through RAM payloads, with the write guards.
//! * [`devchan`]: the firmware's developer channel, and the firmware update
//!   through it (UPD-1).
//! * [`flows`]: backup, first install, restore, recovery (the browser
//!   installer's, 09 §5).
//! * `sg`: SCSI pass-through, to reboot Rockbox/stock into ADFU (Linux).
//! * `image`: RGB565 ↔ PNG.
//! * `device`: what is attached and how to get it into ADFU.

pub mod adfu;
pub mod bot;
pub mod devchan;
pub mod flows;
#[cfg(any(test, feature = "mock"))]
pub mod mock;
pub mod nor;
pub mod transport;

#[cfg(feature = "native")]
pub mod device;
#[cfg(feature = "native")]
pub mod image;
#[cfg(feature = "native")]
pub mod native;
#[cfg(feature = "native")]
pub mod sg;
#[cfg(feature = "native")]
pub use nusb;
