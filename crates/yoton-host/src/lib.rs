//! Host-side access to the player.
//!
//! * [`bot`]: USB Bulk-Only Transport framing over `nusb`.
//! * [`adfu`]: the boot ROM's ADFU mode and the `ymon` monitor.
//! * [`sg`]: SCSI pass-through, to reboot Rockbox/stock into ADFU.
//! * [`nor`]: NOR read/program through RAM payloads.
//! * [`image`]: RGB565 ↔ PNG.
//! * [`device`]: what is attached and how to get it into ADFU.

pub mod adfu;
pub mod bot;
pub mod device;
pub mod devchan;
pub mod image;
pub mod nor;
pub mod sg;
pub use nusb;
