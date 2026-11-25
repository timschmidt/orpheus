//! The microSD card as a `orpheus_fs::BlockDevice` (SD0, DMA channel 4).

use atj2157_hal::dma;
use atj2157_hal::sd::{self, Error, Sd};
pub use orpheus_fs::BlockDevice;

/// Card read and write operations (wrapping; only differences mean
/// anything: the word is not cleared at boot): the battery logger's
/// `card_io` column (PWR-5). The last word of SRAM3, outside SRAM0, whose
/// every byte comes off the stack (02 §1 memory map).
pub const IO_ADDR: usize = orpheus_proto::mem::SRAM3_CARD_IO as usize;

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

/// Card work with a deadline running now ([`Urgent`]): no rests.
static URGENT: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Marks card work with a deadline (the playback ring's refill) for its
/// lifetime: [`pace`] does not rest meanwhile. Found 2026-11-25: with rests
/// in the refill, a refill took 25–29 ms and, with the decoding, no longer
/// fitted a 32 ms DAC half — 27 underruns while browsing Pictures.
pub struct Urgent(());

impl Urgent {
    /// Start; the mark ends when the value is dropped.
    #[must_use]
    pub fn enter() -> Self {
        URGENT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        Self(())
    }
}

impl Drop for Urgent {
    fn drop(&mut self) {
        URGENT.fetch_sub(1, core::sync::atomic::Ordering::Relaxed);
    }
}

/// End of the card task's last rest ([`pace`]), µs counter.
static LAST_REST: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// On the real-time executor, rest [`orpheus_power::pace::REST_US`] after
/// every [`orpheus_power::pace::RUN_US`] of card work: the SD driver spins
/// for its transfers, so card loops never yield otherwise and starve thread
/// mode into the watchdog. Thread-mode users (the USB card reader) keep
/// full speed.
async fn pace() {
    use core::sync::atomic::Ordering::Relaxed;
    // Thread mode needs no pacing; inside a synchronous poller a timer
    // would never fire (`orpheus_fs::SyncPoll`).
    if orpheus_power::pace::should_rest(
        atj2157_hal::rt::in_handler_mode(),
        orpheus_fs::SyncPoll::active(),
        URGENT.load(Relaxed) != 0,
        atj2157_hal::time::now_us() as u32,
        LAST_REST.load(Relaxed),
    ) {
        embassy_time::Timer::after_micros(u64::from(orpheus_power::pace::REST_US)).await;
        LAST_REST.store(atj2157_hal::time::now_us() as u32, Relaxed);
    }
}

impl BlockDevice for Card {
    type Error = Error;

    async fn read(&mut self, lba: u32, buf: &mut [[u8; 512]]) -> Result<(), Error> {
        pace().await;
        self.sd.set_no_sleep(orpheus_fs::SyncPoll::active());
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
        pace().await;
        self.sd.set_no_sleep(orpheus_fs::SyncPoll::active());
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
