//! USB mass storage: Bulk-Only Transport and the SCSI transparent command
//! set (06-connectivity §2.4; disk-image export §3).
//!
//! * [`scsi`]: pure command interpretation: CDB in, a plan out (data to
//!   send, blocks to read or write, or an error with sense data).
//! * [`mmc`]: the CD/DVD-ROM commands of a type-5 unit (§3.3).
//! * [`bot`]: the transport pump over `embassy-usb-driver` endpoints and a
//!   [`BlockDevice`].
//! * [`extent`]: image files as block devices through an extent table
//!   (§3.1).

pub mod bot;
pub mod extent;
pub mod mmc;
pub mod scsi;

/// Storage behind a logical unit, in 512-byte (or `block_size`) blocks.
#[allow(async_fn_in_trait)]
pub trait BlockDevice {
    type Error;
    /// Read `buf.len() / block_size` blocks from `lba`.
    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), Self::Error>;
    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), Self::Error>;
    async fn flush(&mut self) -> Result<(), Self::Error> {
        Ok(())
    }
}
