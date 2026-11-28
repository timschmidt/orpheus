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
        // The clock is back on CK24M (`Sd::new`): DEVPLL can stop.
        atj2157_hal::clock::set_devpll(0);
        self.sd.init_card().await?;
        self.blocks = self.sd.blocks().min(u64::from(u32::MAX)) as u32;
        Ok(())
    }

    #[must_use]
    pub fn stats(&self) -> sd::Stats {
        self.sd.stats
    }

    /// The card's high-speed clock (PERF-SD, U-SD-1): DEVPLL at
    /// [`DEVPLL_N`] × 6 MHz, CMD6 to high speed, the delay chains
    /// [`HS_TIMING`], then the fastest divider at most `max_khz` (≤ 50 MHz).
    /// Blocks read before the switch are read again after it and compared;
    /// a difference or a failed read re-initialises the card at the
    /// default clock. The clock it runs at (kHz): 24,000 when the card
    /// stays at default speed (it cannot switch, failed the check, or
    /// `max_khz` ≤ 25 MHz needs no switch). [`Self::reinit`] goes back (the
    /// card leaves high speed only by CMD0) and stops DEVPLL.
    pub async fn high_speed(&mut self, max_khz: u32) -> Result<u32, Error> {
        const DEFAULT: u32 = 24_000;
        let Some((word, khz)) = orpheus_sdcard::clock_word(2, DEVPLL_N * 6_000, max_khz.min(50_000)) else {
            return Ok(DEFAULT);
        };
        if khz <= 25_000 || self.blocks < 4 {
            return Ok(DEFAULT);
        }
        let lbas = [0, self.blocks / 3, self.blocks / 3 * 2];
        let mut want = [0u32; 3];
        for (w, &lba) in want.iter_mut().zip(&lbas) {
            self.sd.read(lba, &mut self.bounce.0).await?;
            *w = hash(&self.bounce.0);
        }
        atj2157_hal::clock::set_devpll(DEVPLL_N);
        let Some(status) = self.bounce.0.first_chunk_mut::<64>() else { return Ok(DEFAULT) };
        Sd::set_timing(HS_TIMING);
        match self.sd.high_speed(status, word).await {
            Ok(true) => {}
            r => {
                Sd::set_timing(sd::VENDOR_TIMING);
                atj2157_hal::clock::set_devpll(0);
                return r.map(|_| DEFAULT);
            }
        }
        for (&w, &lba) in want.iter().zip(&lbas) {
            if self.sd.read(lba, &mut self.bounce.0).await.is_err() || hash(&self.bounce.0) != w {
                HS_FALLBACKS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
                self.reinit().await?;
                return Ok(DEFAULT);
            }
        }
        Ok(khz)
    }

    /// The command delay chains (`SD_CTL` bits 23:16, `0x8F` the port's),
    /// for trials of the high-speed timing (U-SD-1).
    pub fn set_timing(delays: u8) {
        Sd::set_timing(delays);
    }

    /// `SD_EN.CLKS` for the high-speed trials (U-SD-1).
    pub fn set_clks(on: bool) {
        Sd::set_clks(on);
    }
}

/// DEVPLL for the card's high-speed clock: 24 × 6 = 144 MHz, ÷3 = 48 MHz.
pub const DEVPLL_N: u32 = 24;

/// The delay chains at 48 MHz high speed (`SD_CTL` bits 23:16: RDELAY 7,
/// WDELAY 4): the middle of the window `card timing` found on the test
/// card (2026-11-27, U-SD-1): WDELAY 0, 4 and 8 read correctly with
/// RDELAY 0–13 (14 failed, 15 passed); the port's WDELAY 15 failed every
/// command but one (RDELAY 15), the first file create of round 1's `card
/// bench hs` ("no response", `STATE` `0xCB8`).
pub const HS_TIMING: u8 = 0x74;

/// High-speed switches whose read-back differed from the default clock's
/// (the card went back to 24 MHz).
pub static HS_FALLBACKS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// A word hash (FNV-1a) of a block, for [`Card::high_speed`]'s check.
fn hash(b: &[u8; 512]) -> u32 {
    b.chunks_exact(4)
        .fold(0x811C_9DC5u32, |h, w| (h ^ u32::from_le_bytes([w[0], w[1], w[2], w[3]])).wrapping_mul(0x0100_0193))
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

/// The card's waits spin instead of sleeping (PERF-SD round 2): set by
/// the USB card reader for its session. Its transport starts the USB DMA
/// of one piece and then reads or writes the card: a card wait that slept
/// sent the USB task back through the executor every 20–50 µs, and with
/// it the select of the device, the transport and the mode signal, whose
/// code lines evicted each other (≈ 1,300 line loads per MiB moved,
/// 2026-11-27). Spinning, the card transfer runs to its end and the USB
/// DMA beside it, one return to the executor per piece.
static SPIN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// See [`SPIN`].
pub fn set_spin(on: bool) {
    SPIN.store(on, core::sync::atomic::Ordering::Relaxed);
}

fn no_sleep() -> bool {
    orpheus_fs::SyncPoll::active() || SPIN.load(core::sync::atomic::Ordering::Relaxed)
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
        self.sd.set_no_sleep(no_sleep());
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
        self.sd.set_no_sleep(no_sleep());
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
