//! SD card host (SD0, `0xC0160000`) with DMA (03-hal-drivers §9).
//!
//! Command engine as the Rockbox port drives it [HW]: argument, index,
//! control word with response type and `TS` (start), wait for `TS` to
//! clear, check `STATE` errors. Data phases go through a DMA channel (the
//! port uses channel 4, mode `0x85` read / `0x8500` write). After writes the
//! controller does not wait for the card's busy signal, so the driver polls
//! CMD13 until the card is back in `tran` and ready for data.
//!
//! The card protocol (initialisation, CSD parsing) uses `orpheus-sdcard`.

use atj2157_pac::{cmu, dma as pdma, gpio, rmu, sd};
use embassy_time::{Duration, Instant, Timer};

/// Wait `$us` µs in an `Sd` method: an Embassy timer sleep, or with
/// [`NO_SLEEP`] a spin on the timer counter. A macro, not an `async fn`:
/// an extra future in the driver's state machines grew two task pools by
/// 16 B each (one 256 B stack step).
macro_rules! nap {
    ($self:ident, $us:expr) => {
        if $self.flags & NO_SLEEP != 0 {
            crate::delay::wait_us_gently($us);
        } else {
            Timer::after_micros($us).await;
        }
    };
}
use orpheus_sdcard::{self as card, Cid, Csd, Ocr, R128, Status, cmd};

/// Delay chains of every command (`CTL` RDELAY/WDELAY): the port's by
/// default; [`Sd::set_timing`] changes them for the high-speed trials
/// (U-SD-1, PERF-SD). One word for the one SD host.
static TIMING: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(sd::VENDOR_CTL_TIMING);

/// The port's delay chains as [`Sd::set_timing`] takes them (RDELAY 8,
/// WDELAY 15; [`sd::VENDOR_CTL_TIMING`]): good at 24 MHz, not at 48 MHz.
pub const VENDOR_TIMING: u8 = (sd::VENDOR_CTL_TIMING >> 16) as u8;

fn timing() -> u32 {
    TIMING.load(core::sync::atomic::Ordering::Relaxed)
}

use crate::dma::Channel;

/// Driver errors.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    Crc,
    Controller(u32),
    Card(u32),
    NoCard,
    Unsupported,
    Dma,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Resp {
    None,
    R1,
    R1b,
    R2,
    R3,
}

/// Board wiring of the SD0 pads: (pin, drive) for CLK, CMD, D0..D3.
pub struct Pins<'a> {
    pub pads: &'a [(u8, u32)],
    /// Function word of the SD pads (`0x804` [HW]).
    pub func: u32,
}

/// Counters for diagnostics.
#[derive(Clone, Copy, Debug, Default)]
pub struct Stats {
    pub commands: u32,
    pub timeouts: u32,
    pub crc_errors: u32,
    pub blocks_read: u32,
    pub blocks_written: u32,
    pub busy_polls: u32,
    /// `STATE` and command index of the last failed command.
    pub last_err_state: u32,
    pub last_err_cmd: u32,
}

pub struct Sd {
    dma: Channel,
    rca: u32,
    pub csd: Option<Csd>,
    pub cid: Option<Cid>,
    pub ocr: Ocr,
    /// [`HIGH_CAPACITY`] and [`NO_SLEEP`] (one byte: the struct has no
    /// padding left, and a byte more cost a 256 B step of the stack).
    flags: u8,
    pub stats: Stats,
}

/// `Sd::flags`: the card addresses blocks, not bytes (SDHC/SDXC).
const HIGH_CAPACITY: u8 = 1;
/// `Sd::flags`: card work polled in a loop without an executor (a
/// synchronous session, `orpheus_index::card::spin`; set per call with
/// [`Sd::set_no_sleep`]): the waits spin on the timer counter instead of
/// sleeping on an Embassy timer, which never fires there. Found
/// 2026-11-25: a transfer that outlasted the short spin slept inside an
/// index session and the watchdog reset the player (last PC in
/// `Sd::wait_ts`).
const NO_SLEEP: u8 = 2;
/// `Sd::flags`: `STATE.DAT0S` reads the card's DAT0 line (high on an idle
/// card after `init_card`'s select), so busy waits poll it (U-SD-4).
const DAT0_OK: u8 = 4;

/// Identification clock and fast clock words of `CMU_SD0CLK` [HW].
pub const CLK_IDENT: u32 = 0x41;
pub const CLK_FAST: u32 = 0x00;

/// Largest blocks per multi-block transfer this driver issues.
pub const MAX_BLOCKS: u32 = 128;

impl Sd {
    /// Power the controller and send the initialisation clocks.
    pub fn new(pins: &Pins<'_>, dma: Channel) -> Self {
        cmu::CLKEN0.set_bits(cmu::clken0::SD0 | cmu::clken0::DMA);
        rmu::RST0.clear_bits(rmu::rst0::SD0);
        crate::delay::spin_us(10);
        rmu::RST0.set_bits(rmu::rst0::SD0);
        for &(p, drive) in pins.pads {
            gpio::mfp(usize::from(p)).write(pins.func | (drive << 12));
        }
        cmu::SD0CLK.write(CLK_IDENT);
        TIMING.store(sd::VENDOR_CTL_TIMING, core::sync::atomic::Ordering::Relaxed);
        sd::EN.write(sd::VENDOR_EN_INIT);
        dma.enable_irq();
        let s = Self { dma, rca: 0, csd: None, cid: None, ocr: Ocr(0), flags: 0, stats: Stats::default() };
        sd::CTL.write(sd::VENDOR_CTL_INIT);
        s.wait_ts_blocking();
        s
    }

    fn wait_ts_blocking(&self) -> bool {
        for _ in 0..2_000_000 {
            if sd::CTL.read() & sd::ctl::TS == 0 {
                return true;
            }
        }
        false
    }

    async fn wait_ts(&self, timeout: Duration) -> bool {
        let end = Instant::now() + timeout;
        // Commands finish in tens of µs: poll for ≈ 200 µs, then sleep
        // between polls (or, spinning, keep polling). Every poll leaves
        // the bus to the DMAs for ≈ 1 µs (`delay::pause`, PERF-SD round 2:
        // a tight loop at 288 MHz starved the SD write DMA).
        for _ in 0..200 {
            if sd::CTL.read() & sd::ctl::TS == 0 {
                return true;
            }
            crate::delay::pause();
        }
        while Instant::now() < end {
            if sd::CTL.read() & sd::ctl::TS == 0 {
                return true;
            }
            if self.flags & NO_SLEEP != 0 {
                crate::delay::pause();
            } else {
                Timer::after_micros(50).await;
            }
        }
        false
    }

    fn check_state(&mut self, resp: Resp) -> Result<(), Error> {
        let st = sd::STATE.read();
        let mut mask = sd::state::ERR_MASK;
        if resp == Resp::R3 {
            mask &= !sd::state::NOCRC_IGNORE;
        }
        if st & mask == 0 {
            return Ok(());
        }
        self.stats.last_err_state = st;
        self.stats.last_err_cmd = sd::CMD.read();
        // CLNR without a CRC7 error: the card did not answer at all.
        if st & sd::state::TOUTE != 0 || (st & sd::state::CLNR != 0 && st & sd::state::CRC7ER == 0) {
            self.stats.timeouts += 1;
            return Err(Error::Timeout);
        }
        if st & 0x13 != 0 {
            self.stats.crc_errors += 1;
            return Err(Error::Crc);
        }
        Err(Error::Controller(st))
    }

    fn short_resp() -> u32 {
        ((sd::rspbuf(0, 1).read() & 0xFF) << 24) | (sd::rspbuf(0, 0).read() >> 8)
    }

    fn long_resp() -> R128 {
        R128([
            sd::rspbuf(0, 3).read(),
            sd::rspbuf(0, 2).read(),
            sd::rspbuf(0, 1).read(),
            sd::rspbuf(0, 0).read(),
        ])
    }

    /// A command without data. Returns the short response (0 for none).
    async fn command(&mut self, index: u8, arg: u32, resp: Resp) -> Result<u32, Error> {
        self.stats.commands += 1;
        let tm = match resp {
            Resp::None => sd::ctl::TM_NONE,
            Resp::R1 => sd::ctl::TM_R1,
            Resp::R2 => sd::ctl::TM_R2,
            Resp::R1b | Resp::R3 => sd::ctl::TM_R3,
        };
        sd::ARG.write(arg);
        sd::CMD.write(u32::from(index));
        sd::CTL.write(timing() | tm | sd::ctl::TS);
        if !self.wait_ts(Duration::from_millis(500)).await {
            self.stats.timeouts += 1;
            return Err(Error::Timeout);
        }
        self.check_state(resp)?;
        if resp == Resp::R1b {
            self.wait_dat0().await;
        }
        Ok(if resp == Resp::None { 0 } else { Self::short_resp() })
    }

    /// The card's busy signal after an R1b command or a write: wait until
    /// `STATE.DAT0S` reads high (the card released DAT0), at most 1 s; the
    /// caller confirms with CMD13 as before. With [`DAT0_OK`] clear (DAT0
    /// read low on an idle card at `init_card`) a fixed 500 µs as before
    /// PERF-SD round 2: a CMD12 after every 32 KiB read cost 0.5 ms of
    /// 3 ms (U-SD-4).
    async fn wait_dat0(&mut self) {
        if self.flags & DAT0_OK == 0 {
            nap!(self, 500);
            return;
        }
        let end = Instant::now() + Duration::from_secs(1);
        let mut spins = 0u32;
        while sd::STATE.read() & sd::state::DAT0S == 0 {
            crate::delay::pause();
            spins += 1;
            if spins > 64 {
                if Instant::now() > end {
                    return;
                }
                nap!(self, 20);
            }
        }
    }

    async fn app_command(&mut self, index: u8, arg: u32, resp: Resp) -> Result<u32, Error> {
        self.command(cmd::APP_CMD, self.rca, Resp::R1).await?;
        self.command(index, arg, resp).await
    }

    /// Card initialisation: identification, addressing, 4-bit bus, fast
    /// clock.
    pub async fn init_card(&mut self) -> Result<(), Error> {
        self.rca = 0;
        // A card left active by the previous firmware sometimes misses the
        // first CMD0/CMD8 pair: retry the pair a few times.
        let mut if_cond = Err(Error::Timeout);
        for _ in 0..4 {
            self.command(cmd::GO_IDLE, 0, Resp::None).await?;
            nap!(self, 2_000);
            if_cond = self.command(cmd::SEND_IF_COND, card::IF_COND_ARG, Resp::R1).await;
            if if_cond.is_ok() {
                break;
            }
            nap!(self, 10_000);
        }
        let v2 = match if_cond {
            Ok(r) if r & 0xFFF == card::IF_COND_ARG => true,
            Ok(_) => return Err(Error::Unsupported),
            Err(Error::Timeout) => false,
            Err(e) => return Err(e),
        };
        let arg = if v2 { card::OP_COND_ARG } else { card::OP_COND_ARG & !(1 << 30) };
        let end = Instant::now() + Duration::from_secs(1);
        loop {
            let ocr = match self.app_command(cmd::ACMD_SEND_OP_COND, arg, Resp::R3).await {
                Ok(o) => Ocr(o),
                Err(Error::Timeout) if !v2 => return Err(Error::NoCard),
                Err(e) => return Err(e),
            };
            if ocr.ready() {
                self.ocr = ocr;
                self.set_flag(HIGH_CAPACITY, ocr.high_capacity());
                break;
            }
            if Instant::now() > end {
                return Err(Error::Timeout);
            }
            nap!(self, 10_000);
        }
        self.command(cmd::ALL_SEND_CID, 0, Resp::R2).await?;
        self.cid = Some(Cid::parse(&Self::long_resp()));
        let r6 = self.command(cmd::SEND_RELATIVE_ADDR, 0, Resp::R1).await?;
        self.rca = r6 & 0xFFFF_0000;
        self.command(cmd::SEND_CSD, self.rca, Resp::R2).await?;
        self.csd = Csd::parse(&Self::long_resp());
        self.command(cmd::SELECT_CARD, self.rca, Resp::R1b).await?;
        self.wait_ready().await?;
        // The card is idle in `tran`: DAT0 must read high if `DAT0S` is
        // the line (U-SD-4).
        self.set_flag(DAT0_OK, sd::STATE.read() & sd::state::DAT0S != 0);
        self.app_command(cmd::ACMD_SET_BUS_WIDTH, 2, Resp::R1).await?;
        sd::EN.modify(|v| sd::en::DATAWID.set(v, sd::en::DATAWID_4BIT));
        if self.flags & HIGH_CAPACITY == 0 {
            self.command(cmd::SET_BLOCKLEN, 512, Resp::R1).await?;
        }
        cmu::SD0CLK.write(CLK_FAST);
        Ok(())
    }

    /// Poll CMD13 until the card is in `tran` and ready for data.
    async fn wait_ready(&mut self) -> Result<(), Error> {
        let end = Instant::now() + Duration::from_secs(1);
        loop {
            self.stats.busy_polls += 1;
            let st = Status(self.command(cmd::SEND_STATUS, self.rca, Resp::R1).await?);
            if st.has_error() {
                return Err(Error::Card(st.0));
            }
            if st.ready_for_data() && st.state() == card::State::Tran {
                return Ok(());
            }
            if Instant::now() > end {
                return Err(Error::Timeout);
            }
            nap!(self, 200);
        }
    }

    fn addr(&self, lba: u32) -> u32 {
        if self.flags & HIGH_CAPACITY != 0 { lba } else { lba.wrapping_mul(512) }
    }

    /// A command with a data phase of `blocks` blocks of `size` bytes
    /// (512, or 64 for CMD6's status) by DMA to or from `buf`.
    async fn data_command(
        &mut self,
        index: u8,
        arg: u32,
        buf: *mut u8,
        blocks: u32,
        size: u32,
        write: bool,
    ) -> Result<(), Error> {
        self.stats.commands += 1;
        let len = blocks * size;
        sd::ARG.write(arg);
        sd::CMD.write(u32::from(index));
        sd::EN.set_bits(sd::en::AHB_DMA_SEL);
        let fifo = sd::DAT.addr() as u32;
        if write {
            self.dma.start(pdma::VENDOR_MODE_SD_WRITE, buf as u32, fifo, len);
        } else {
            self.dma.start(pdma::VENDOR_MODE_SD_READ, fifo, buf as u32, len);
        }
        sd::BLK_SIZE.write(size);
        sd::BLK_NUM.write(blocks);
        let tm = if write { sd::ctl::TM_WRITE } else { sd::ctl::TM_READ };
        sd::CTL.write(sd::VENDOR_CTL_DATA | timing() | tm);
        sd::CTL.set_bits(sd::ctl::TS);
        if !self.wait_ts(Duration::from_millis(1000)).await {
            self.dma.abort();
            self.stats.timeouts += 1;
            return Err(Error::Timeout);
        }
        let r = self.check_state(Resp::R1);
        // The DMA finishes on its own after the controller's last word.
        let end = Instant::now() + Duration::from_millis(500);
        while self.dma.busy() {
            if Instant::now() > end {
                self.dma.abort();
                return Err(Error::Dma);
            }
            nap!(self, 20);
        }
        r
    }

    /// Read `buf.len() / 512` blocks starting at `lba`. `buf` must be word
    /// aligned and in DMA-reachable RAM.
    fn set_flag(&mut self, f: u8, on: bool) {
        self.flags = if on { self.flags | f } else { self.flags & !f };
    }

    /// Whether the next transfers run inside a synchronous poller
    /// ([`NO_SLEEP`]).
    pub fn set_no_sleep(&mut self, on: bool) {
        self.set_flag(NO_SLEEP, on);
    }

    pub async fn read(&mut self, lba: u32, buf: &mut [u8]) -> Result<(), Error> {
        let mut done = 0u32;
        let total = (buf.len() / 512) as u32;
        while done < total {
            let n = (total - done).min(MAX_BLOCKS);
            let p = buf[(done * 512) as usize..].as_mut_ptr();
            let a = self.addr(lba + done);
            if n == 1 {
                self.data_command(cmd::READ_SINGLE, a, p, 1, 512, false).await?;
            } else {
                let r = self.data_command(cmd::READ_MULTIPLE, a, p, n, 512, false).await;
                let stop = self.command(cmd::STOP_TRANSMISSION, 0, Resp::R1b).await;
                r?;
                stop?;
            }
            self.stats.blocks_read += n;
            done += n;
        }
        Ok(())
    }

    /// Write blocks. Raw writes are only for files the caller created
    /// (CLAUDE.md rule 5); the file system enforces where.
    pub async fn write(&mut self, lba: u32, buf: &[u8]) -> Result<(), Error> {
        let mut done = 0u32;
        let total = (buf.len() / 512) as u32;
        while done < total {
            let n = (total - done).min(MAX_BLOCKS);
            let p = buf[(done * 512) as usize..].as_ptr().cast_mut();
            let a = self.addr(lba + done);
            if n == 1 {
                self.data_command(cmd::WRITE_SINGLE, a, p, 1, 512, true).await?;
            } else {
                // Pre-erase (ACMD23, SET_WR_BLK_ERASE_COUNT): the card may
                // erase the blocks ahead of the data (SD Physical Layer
                // §4.3.4; every SD card has it). PERF-SD.
                // A hint only: a card that refuses it still takes the
                // write.
                // (CMD55 and ACMD23 written out: `app_command` would be a
                // new function on the games' and playback clusters' paths.)
                if self.command(cmd::APP_CMD, self.rca, Resp::R1).await.is_ok() {
                    let _ = self.command(cmd::ACMD_SET_WR_BLK_ERASE_COUNT, n, Resp::R1).await;
                }
                let r = self.data_command(cmd::WRITE_MULTIPLE, a, p, n, 512, true).await;
                let stop = self.command(cmd::STOP_TRANSMISSION, 0, Resp::R1b).await;
                r?;
                stop?;
            }
            self.wait_dat0().await;
            self.wait_ready().await?;
            self.stats.blocks_written += n;
            done += n;
        }
        Ok(())
    }

    /// Switch the card to high-speed timing (CMD6 function group 1 = 1,
    /// up to 50 MHz; SD Physical Layer §4.3.10) and then the clock to
    /// `clk` (a `CMU_SD0CLK` word). `Ok(false)`: the card has no switch
    /// command class (CSD CCC bit 10) or refused the function; the clock
    /// stays. `status`: a word-aligned, DMA-reachable 64-byte buffer for
    /// the switch status. U-SD-1, PERF-SD.
    pub async fn high_speed(&mut self, status: &mut [u8; 64], clk: u32) -> Result<bool, Error> {
        if self.csd.is_none_or(|c| c.ccc & card::CCC_SWITCH == 0) {
            return Ok(false);
        }
        self.data_command(cmd::SWITCH_FUNC, card::SWITCH_HIGH_SPEED, status.as_mut_ptr(), 1, 64, false)
            .await?;
        if !card::switched_high_speed(status) {
            return Ok(false);
        }
        // The card changes timing within 8 clocks of the status's end.
        nap!(self, 100);
        cmu::SD0CLK.write(clk);
        Ok(true)
    }

    /// The data clock word (`CMU_SD0CLK`); [`CLK_FAST`] after `init_card`.
    pub fn set_clock(&mut self, clk: u32) {
        cmu::SD0CLK.write(clk);
    }

    /// The command delay chains (`CTL` bits 16–23: WDELAY `[19:16]`,
    /// RDELAY `[23:20]`); the port's `0x8F` by default.
    pub fn set_timing(delays: u8) {
        TIMING.store(u32::from(delays) << 16, core::sync::atomic::Ordering::Relaxed);
    }

    /// `SD_EN.CLKS` (bit 8, "clock select" in the SDK header, meaning
    /// unknown): for the high-speed trials (U-SD-1). `Sd::new` clears it.
    pub fn set_clks(on: bool) {
        if on {
            sd::EN.set_bits(sd::en::CLKS);
        } else {
            sd::EN.clear_bits(sd::en::CLKS);
        }
    }

    /// Capacity in blocks (0 before init).
    #[must_use]
    pub fn blocks(&self) -> u64 {
        self.csd.map_or(0, |c| c.blocks)
    }
}
