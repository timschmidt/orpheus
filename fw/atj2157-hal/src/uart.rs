//! UART0–3 (datasheet §7.4; 03-hal-drivers §16; 01-hardware §6).
//!
//! * [`Uart::open`]: reset pulse, clock gate, baud divisors, `CTL` (the
//!   caller's word: FIFO routing, DMA request levels), status flags
//!   cleared. Pins are the board's business (`yoton-bsp`).
//! * Receive: [`DmaRing`] — a DMA channel in single-transfer mode with
//!   **reload** fills a ring from `RXDAT` one byte per request (`CTL`
//!   `RDIC` = 1 byte, `RX_FIFO_SEL` = DMA, `RXDE`), and the reader finds
//!   the write position from the channel's remain counter. No interrupt:
//!   the owner polls (the Bluetooth task every 10 ms; a 256-byte ring
//!   holds 22 ms at 115200 baud). Datasheet §7.4.1 ("UART RX supports
//!   DMA single mode") and §7.4.5.1 (single mode wants `RDIC` = 0); the
//!   vendor instead ran burst-8 without reload and drained the last
//!   bytes by CPU (bluetooth.md §2.2). Not yet run on the hardware.
//! * Or by CPU ([`Uart::read_fifo`], `RX_FIFO_SEL` = CPU): a diagnostic
//!   fallback; the 16-byte FIFO fills in 1.4 ms at 115200 baud, so it
//!   loses bytes unless polled that often.
//! * Transmit: [`Uart::send_dma`] (the vendor's channel-9 word, single
//!   transfers, `TFER` cleared before and after, then wait for `UTBB` —
//!   bluetooth.md §2.3, VERBATIM) or [`Uart::send_fifo`].
//!
//! Interrupt lines 21–24 (datasheet table) are not used (U-IRQ-1).

use core::sync::atomic::{AtomicU32, Ordering};

use atj2157_pac::{cmu, rmu, uart as reg};
use embassy_time::{Duration, Timer, with_timeout};

use crate::dma::{self, Channel};

/// One UART block. Ownership token: one per block.
pub struct Uart {
    n: usize,
}

/// TX DMA transfers that did not finish within their time bound (aborted).
pub static TX_DMA_TIMEOUTS: AtomicU32 = AtomicU32::new(0);
/// TX DMA transfers that finished without their interrupt waking the
/// sender (the DMA line of the channel is unproven).
pub static TX_DMA_NO_IRQ: AtomicU32 = AtomicU32::new(0);

impl Uart {
    /// # Safety
    /// One `Uart` per block `n` (0..=3).
    #[must_use]
    pub unsafe fn steal(n: usize) -> Self {
        Self { n }
    }

    #[must_use]
    pub fn number(&self) -> usize {
        self.n
    }

    /// Reset, clock on, `BR` and `CTL`, pending flags cleared.
    pub fn open(&self, br: u32, ctl: u32) {
        let bit = rmu::rst0::UART0 << self.n;
        critical_section::with(|_| {
            rmu::RST0.clear_bits(bit);
        });
        crate::delay::spin_us(5);
        critical_section::with(|_| {
            rmu::RST0.set_bits(bit);
            cmu::CLKEN0.set_bits(cmu::clken0::UART0 << self.n);
        });
        reg::br(self.n).write(br);
        reg::ctl(self.n).write(ctl);
        reg::sta(self.n).write(reg::VENDOR_STA_CLEAR);
    }

    /// `CTL` = 0 (receiver, transmitter, FIFOs and requests off) and the
    /// clock gate off.
    pub fn close(&self) {
        reg::ctl(self.n).write(0);
        critical_section::with(|_| {
            cmu::CLKEN0.clear_bits(cmu::clken0::UART0 << self.n);
        });
    }

    #[must_use]
    pub fn status(&self) -> u32 {
        reg::sta(self.n).read()
    }

    #[must_use]
    pub fn control(&self) -> u32 {
        reg::ctl(self.n).read()
    }

    /// Clear the receive/stop/parity error flags; returns those that were
    /// set (`STA` bits).
    pub fn take_errors(&self) -> u32 {
        let m = reg::sta::RXST | reg::sta::RXER | reg::sta::STER | reg::sta::PAER;
        let e = reg::sta(self.n).read() & m;
        if e != 0 {
            reg::sta(self.n).write(e);
        }
        e
    }

    /// Bytes waiting in the RX FIFO.
    #[must_use]
    pub fn rx_level(&self) -> u32 {
        reg::sta::RXFL.get(self.status())
    }

    /// Read the RX FIFO by CPU (`RX_FIFO_SEL` = CPU only).
    pub fn read_fifo(&self, buf: &mut [u8]) -> usize {
        let mut n = 0;
        while n < buf.len() && reg::sta(self.n).read() & reg::sta::RFEM == 0 {
            buf[n] = reg::rxdat(self.n).read() as u8;
            n += 1;
        }
        n
    }

    /// Wait (bounded) until the transmitter is idle: FIFO empty, last bit
    /// out.
    async fn wait_tx_idle(&self) -> bool {
        for _ in 0..20 {
            if reg::sta(self.n).read() & reg::sta::UTBB == 0 {
                return true;
            }
            Timer::after_micros(500).await;
        }
        false
    }

    /// Send by DMA (`TX_FIFO_SEL` = DMA, `TXDE`): memory → `TXDAT`, 8-bit
    /// single transfers. `bytes` must stay put until this returns (the
    /// transfer is aborted on timeout). Call `ch.enable_irq()` once before.
    /// Returns false on timeout.
    pub async fn send_dma(&self, ch: &Channel, bytes: &[u8]) -> bool {
        if bytes.is_empty() {
            return true;
        }
        // VERBATIM (bluetooth.md §2.3): TFER cleared before and after.
        reg::sta(self.n).write(reg::sta::TFER);
        let mode = dma::mode::src(dma::mode::MEM)
            | dma::mode::dst(dma::mode::UART0 + self.n as u32)
            | dma::mode::DST_FIXED
            | dma::mode::SINGLE
            | dma::mode::WIDTH_8;
        ch.start(mode, bytes.as_ptr() as u32, reg::txdat(self.n).addr() as u32, bytes.len() as u32);
        // 87 µs per byte at 115200 baud; generous bound.
        let bound_ms = 5 + bytes.len() as u64 / 8;
        let _ = with_timeout(Duration::from_millis(bound_ms), ch.wait()).await;
        let mut ok = true;
        if ch.busy() {
            ch.abort();
            TX_DMA_TIMEOUTS.fetch_add(1, Ordering::Relaxed);
            ok = false;
        } else if dma::pending() & (1 << ch.number()) != 0 {
            // Done, but the completion interrupt did not clear the bit.
            dma::clear(ch.number());
            TX_DMA_NO_IRQ.fetch_add(1, Ordering::Relaxed);
        }
        let idle = self.wait_tx_idle().await;
        reg::sta(self.n).write(reg::sta::TFER);
        ok && idle
    }

    /// Send through the TX FIFO by CPU (`TX_FIFO_SEL` = CPU), yielding
    /// while it is full. Returns false on timeout.
    pub async fn send_fifo(&self, bytes: &[u8]) -> bool {
        let mut i = 0;
        let mut waits = 0;
        while i < bytes.len() {
            while i < bytes.len() && reg::sta(self.n).read() & reg::sta::TFFU == 0 {
                reg::txdat(self.n).write(u32::from(bytes[i]));
                i += 1;
            }
            if i < bytes.len() {
                waits += 1;
                if waits > 200 {
                    return false;
                }
                Timer::after_micros(500).await;
            }
        }
        self.wait_tx_idle().await
    }
}

/// `CTL` fields for an 8N1 line with both FIFOs on; `rx_dma` / `tx_dma`
/// route the FIFOs to DMA (single-byte receive requests; transmit
/// requests at 8 free bytes as the vendor's Bluetooth word).
#[must_use]
pub const fn ctl_8n1(rx_dma: bool, tx_dma: bool) -> u32 {
    use reg::ctl::*;
    let mut v = RXENABLE | TXENABLE | TX_FIFO_EN | RX_FIFO_EN | EN | DWLS.val(DWLS_8);
    if rx_dma {
        v |= RX_FIFO_SEL | RXDE | RDIC.val(LEVEL_1);
    }
    if tx_dma {
        v |= TX_FIFO_SEL | TXDE | TDIC.val(LEVEL_8);
    }
    v
}

/// Receive ring filled by a DMA channel in reload mode (see the module
/// docs). The ring must be in SRAM0/1 or a DMA-visible alias.
pub struct DmaRing {
    ch: Channel,
    buf: *const u8,
    len: u32,
    rd: u32,
    /// Most bytes found in one [`Self::read`] (ring margin check).
    pub peak: u32,
}

impl DmaRing {
    /// Start receiving from UART `uart` into `ring` (at most 256 KiB − 1).
    pub fn start(ch: Channel, uart: &Uart, ring: &'static mut [u8]) -> Self {
        let len = ring.len() as u32;
        let mode = dma::mode::src(dma::mode::UART0 + uart.n as u32)
            | dma::mode::dst(dma::mode::MEM)
            | dma::mode::SRC_FIXED
            | dma::mode::SINGLE
            | dma::mode::RELOAD
            | dma::mode::WIDTH_8;
        ch.start(mode, reg::rxdat(uart.n).addr() as u32, ring.as_ptr() as u32, len);
        Self { ch, buf: ring.as_ptr(), len, rd: 0, peak: 0 }
    }

    /// Where the DMA writes next.
    fn write_pos(&self) -> u32 {
        let rem = self.ch.remaining().min(self.len);
        (self.len - rem) % self.len
    }

    /// The channel's remain counter (diagnostics).
    #[must_use]
    pub fn remaining(&self) -> u32 {
        self.ch.remaining()
    }

    #[must_use]
    pub fn running(&self) -> bool {
        self.ch.busy()
    }

    /// Copy what arrived since the last call (up to `out.len()`).
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        let wr = self.write_pos();
        let avail = (wr + self.len - self.rd) % self.len;
        self.peak = self.peak.max(avail);
        let n = (avail as usize).min(out.len());
        for o in &mut out[..n] {
            // SAFETY: `rd < len`, inside the ring handed to `start`; the
            // DMA writes other bytes (ahead of `rd`) concurrently, so
            // volatile.
            *o = unsafe { core::ptr::read_volatile(self.buf.add(self.rd as usize)) };
            self.rd = (self.rd + 1) % self.len;
        }
        n
    }

    /// Stop the channel and give it back.
    #[must_use]
    pub fn stop(self) -> Channel {
        self.ch.abort();
        dma::clear(self.ch.number());
        self.ch
    }
}
