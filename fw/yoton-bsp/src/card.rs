//! The microSD card as a `orpheus_fs::BlockDevice` (SD0, DMA channel 4).

use atj2157_hal::dma;
use atj2157_hal::sd::{self, Error, Sd};
pub use orpheus_fs::BlockDevice;

/// Card read and write operations (wrapping; only differences mean
/// anything: the word is not cleared at boot): the battery logger's
/// `card_io` column (PWR-5). The last word of SRAM3, outside SRAM0, whose
/// every byte comes off the stack (02 §1 memory map).
pub const IO_ADDR: usize = 0x0030_7FFC;

fn io() -> &'static core::sync::atomic::AtomicU32 {
    // SAFETY: the last word of SRAM3 (enabled in crt0, inside the MPU's
    // RAM sub-region), aligned, used for nothing else; any bit pattern is
    // a valid `AtomicU32`.
    unsafe { &*(IO_ADDR as *const core::sync::atomic::AtomicU32) }
}

fn count_io() {
    io().fetch_add(1, core::sync::atomic::Ordering::Relaxed);
}

/// The operation count (see [`IO_ADDR`]).
#[must_use]
pub fn io_count() -> u32 {
    io().load(core::sync::atomic::Ordering::Relaxed)
}

/// Card read and write operations that failed (VER-12 soak oracle:
/// developer-channel STATS group 0 word 22).
static ERRORS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Failed card operations since boot.
#[must_use]
pub fn error_count() -> u32 {
    ERRORS.load(core::sync::atomic::Ordering::Relaxed)
}

fn counted<T>(r: Result<T, Error>) -> Result<T, Error> {
    if r.is_err() {
        ERRORS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    }
    r
}

use crate::pins;

#[repr(C, align(4))]
struct Bounce([u8; 512]);

pub struct Card {
    sd: Sd,
    blocks: u32,
    bounce: Bounce,
    pub bounced: u32,
}

fn dma_ok(p: *const u8, len: usize) -> bool {
    let a = p as usize;
    a.is_multiple_of(4) && a >= 0x0010_0000 && a + len <= 0x0013_8000
}

impl Card {
    /// Power the host up and initialise the card.
    pub async fn open() -> Result<Self, Error> {
        // SAFETY: channel 4 is the SD channel (01-hardware §6).
        let mut sd = Sd::new(&sd::Pins { pads: &pins::SD_PADS, func: pins::SD_FUNC }, unsafe {
            dma::Channel::steal(4)
        });
        sd.init_card().await?;
        let blocks = sd.blocks().min(u64::from(u32::MAX)) as u32;
        Ok(Self { sd, blocks, bounce: Bounce([0; 512]), bounced: 0 })
    }

    /// Reset the host controller and initialise the card again (after the
    /// USB card reader: a host may stop in the middle of a transfer and
    /// leave the card or the controller in a state the next command does
    /// not expect).
    pub async fn reinit(&mut self) -> Result<(), Error> {
        // SAFETY: channel 4 is the SD channel (01-hardware §6); the `Sd`
        // that held it is replaced below.
        let ch = unsafe { dma::Channel::steal(4) };
        ch.abort();
        self.sd = Sd::new(&sd::Pins { pads: &pins::SD_PADS, func: pins::SD_FUNC }, ch);
        self.sd.init_card().await?;
        self.blocks = self.sd.blocks().min(u64::from(u32::MAX)) as u32;
        Ok(())
    }

    #[must_use]
    pub fn stats(&self) -> sd::Stats {
        self.sd.stats
    }
}

impl BlockDevice for Card {
    type Error = Error;

    async fn read(&mut self, lba: u32, buf: &mut [[u8; 512]]) -> Result<(), Error> {
        count_io();
        let flat = buf.as_flattened_mut();
        if dma_ok(flat.as_ptr(), flat.len()) {
            return counted(self.sd.read(lba, flat).await);
        }
        for (i, s) in buf.iter_mut().enumerate() {
            self.bounced += 1;
            counted(self.sd.read(lba + i as u32, &mut self.bounce.0).await)?;
            s.copy_from_slice(&self.bounce.0);
        }
        Ok(())
    }

    async fn write(&mut self, lba: u32, buf: &[[u8; 512]]) -> Result<(), Error> {
        count_io();
        let flat = buf.as_flattened();
        if dma_ok(flat.as_ptr(), flat.len()) {
            return counted(self.sd.write(lba, flat).await);
        }
        for (i, s) in buf.iter().enumerate() {
            self.bounced += 1;
            self.bounce.0.copy_from_slice(s);
            counted(self.sd.write(lba + i as u32, &self.bounce.0).await)?;
        }
        Ok(())
    }

    fn sector_count(&self) -> u32 {
        self.blocks
    }
}
