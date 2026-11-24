//! The microSD card as a `orpheus_fs::BlockDevice` (SD0, DMA channel 4).

use atj2157_hal::dma;
use atj2157_hal::sd::{self, Error, Sd};
pub use orpheus_fs::BlockDevice;

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

    #[must_use]
    pub fn stats(&self) -> sd::Stats {
        self.sd.stats
    }
}

impl BlockDevice for Card {
    type Error = Error;

    async fn read(&mut self, lba: u32, buf: &mut [[u8; 512]]) -> Result<(), Error> {
        let flat = buf.as_flattened_mut();
        if dma_ok(flat.as_ptr(), flat.len()) {
            return self.sd.read(lba, flat).await;
        }
        for (i, s) in buf.iter_mut().enumerate() {
            self.bounced += 1;
            self.sd.read(lba + i as u32, &mut self.bounce.0).await?;
            s.copy_from_slice(&self.bounce.0);
        }
        Ok(())
    }

    async fn write(&mut self, lba: u32, buf: &[[u8; 512]]) -> Result<(), Error> {
        let flat = buf.as_flattened();
        if dma_ok(flat.as_ptr(), flat.len()) {
            return self.sd.write(lba, flat).await;
        }
        for (i, s) in buf.iter().enumerate() {
            self.bounced += 1;
            self.bounce.0.copy_from_slice(s);
            self.sd.write(lba + i as u32, &self.bounce.0).await?;
        }
        Ok(())
    }

    fn sector_count(&self) -> u32 {
        self.blocks
    }
}
