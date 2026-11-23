//! The settings store on the NOR settings region (dual layout:
//! `0x7DA000..0x7DE000`, four sectors; 02 §2.4).
//!
//! Erase and program are blocking calls into the resident NOR driver
//! (milliseconds); the async front end yields between steps so other tasks
//! run. Program/erase suspend for concurrent page faults is U-NOR-2 work.

use atj2157_hal::nor;
use yoton_proto::layout::dual;
pub use yoton_settings::{Error, Store};

/// A NOR window `[base, base + len)` inside the writable region.
pub struct NorRegion {
    base: u32,
    len: u32,
}

impl NorRegion {
    /// The settings region.
    #[must_use]
    pub const fn settings() -> Self {
        Self { base: dual::SETTINGS, len: dual::SETTINGS_SIZE }
    }

    /// The scratch sector used by tests (unused half of the old page-store
    /// header page while Rockbox is installed).
    #[must_use]
    pub const fn scratch(len: u32) -> Self {
        Self { base: 0x0076_3000, len }
    }

    fn check(&self, off: u32, n: u32) -> Result<u32, nor::Error> {
        if off.checked_add(n).is_none_or(|e| e > self.len) {
            return Err(nor::Error::Refused);
        }
        Ok(self.base + off)
    }
}

impl yoton_settings::Nor for NorRegion {
    type Error = nor::Error;

    async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), nor::Error> {
        let a = self.check(off, buf.len() as u32)?;
        nor::read(a, buf)
    }

    async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), nor::Error> {
        let mut a = self.check(off, data.len() as u32)?;
        let mut rest = data;
        while !rest.is_empty() {
            // Never cross a 256-byte NOR page.
            let n = rest.len().min(256 - (a as usize & 0xFF));
            nor::program_page(a, &rest[..n])?;
            a += n as u32;
            rest = &rest[n..];
            embassy_futures::yield_now().await;
        }
        Ok(())
    }

    async fn erase(&mut self, sector_off: u32) -> Result<(), nor::Error> {
        let a = self.check(sector_off, 4096)?;
        nor::erase_sector(a)
    }
}
