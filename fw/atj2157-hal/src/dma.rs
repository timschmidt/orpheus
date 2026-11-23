//! General DMA controller, `0xC0070000` (datasheet ch. 5; 03-hal-drivers §5).
//!
//! Ten channels; channel `n` registers at `0x100·(n+1)`. Completion pending
//! bits in `DMAIP` (bit n = complete, bit n+16 = half), enables in `DMAIE`;
//! NVIC line `26 + n` per channel. Channel 0 belongs to the pager and NOR
//! driver (their own resident code), not to this module's allocator.
//!
//! Register constants are local until WP-HAL-0's PAC module lands
//! (TODO(pac): switch to `atj2157_pac::dma`).

use core::future::poll_fn;
use core::task::Poll;

use atj2157_pac::{Reg, irq};
use embassy_sync::waitqueue::AtomicWaker;

use crate::interrupt::{self, Level};

const BASE: usize = 0xC007_0000;
const DMAIP: Reg = Reg::at(BASE);
const DMAIE: Reg = Reg::at(BASE + 0x04);

pub const CHANNELS: usize = 10;

/// Mode-word fields [DS] §5.3.1.3.
pub mod mode {
    /// Source select `[4:0]` / destination select `[12:8]` device codes.
    pub const MEM: u32 = 0x00;
    pub const UART0: u32 = 0x01;
    pub const UART1: u32 = 0x02;
    pub const SD: u32 = 0x06;
    pub const SPI0: u32 = 0x08;
    /// DAC FIFO0 (as destination) / ADC01 FIFO (as source).
    pub const DAC0_ADC01: u32 = 0x0B;
    pub const TWI1: u32 = 0x14;
    pub const PWM: u32 = 0x15;
    pub const LCD: u32 = 0x16;

    #[must_use]
    pub const fn src(dev: u32) -> u32 {
        dev & 0x1F
    }
    #[must_use]
    pub const fn dst(dev: u32) -> u32 {
        (dev & 0x1F) << 8
    }
    /// Source address constant (a FIFO).
    pub const SRC_FIXED: u32 = 1 << 7;
    /// Destination address constant (a FIFO).
    pub const DST_FIXED: u32 = 1 << 15;
    /// Planar (separated) audio layout.
    pub const PLANAR: u32 = 1 << 16;
    /// Single transfers instead of burst-8.
    pub const SINGLE: u32 = 1 << 17;
    /// Reload: restart automatically when the count runs out.
    pub const RELOAD: u32 = 1 << 18;
    pub const WIDTH_32: u32 = 0 << 20;
    pub const WIDTH_16: u32 = 1 << 20;
    pub const WIDTH_8: u32 = 2 << 20;
}

const fn ch_reg(n: usize, off: usize) -> Reg {
    Reg::at(BASE + 0x100 * (n + 1) + off)
}

/// Translate a CPU address to the address the DMA engines use
/// (SRAM2/3 live at `0x10000000` / `0x18000000` for DMA) [HW].
#[must_use]
pub const fn dma_addr(cpu: u32) -> u32 {
    match cpu {
        0x0020_0000..0x0030_0000 => 0x1000_0000 + (cpu & 0x7FFF),
        0x0030_0000..0x0040_0000 => 0x1800_0000 + (cpu & 0x7FFF),
        _ => cpu,
    }
}

static WAKERS: [AtomicWaker; CHANNELS] = [const { AtomicWaker::new() }; CHANNELS];

fn on_irq(n: usize) {
    let ip = DMAIP.read();
    let mine = (1 << n) | (1 << (n + 16));
    if ip & mine != 0 {
        DMAIP.write(ip & mine);
    }
    WAKERS[n].wake();
}

macro_rules! handlers {
    ($($n:literal),*) => {
        const HANDLERS: [fn(); CHANNELS] = [$(|| on_irq($n)),*];
    };
}
handlers!(0, 1, 2, 3, 4, 5, 6, 7, 8, 9);

/// Ownership token for one channel (handed out once by the board).
pub struct Channel {
    n: usize,
}

impl Channel {
    /// # Safety
    /// Exactly one `Channel` per number may exist; channel 0 is the pager's.
    #[must_use]
    pub unsafe fn steal(n: usize) -> Self {
        Self { n }
    }

    #[must_use]
    pub fn number(&self) -> usize {
        self.n
    }

    /// Enable the completion interrupt for this channel.
    pub fn enable_irq(&self) {
        DMAIE.set_bits(1 << self.n);
        interrupt::install(irq::DMA0 + self.n as u16, Level::Deadline, HANDLERS[self.n]);
    }

    /// Program and start a transfer of `len` bytes. Addresses are CPU
    /// addresses (translated here).
    pub fn start(&self, mode_word: u32, src: u32, dst: u32, len: u32) {
        let n = self.n;
        ch_reg(n, 0x04).write(0);
        DMAIP.write((1 << n) | (1 << (n + 16)));
        ch_reg(n, 0x00).write(mode_word);
        ch_reg(n, 0x08).write(dma_addr(src));
        ch_reg(n, 0x10).write(dma_addr(dst));
        ch_reg(n, 0x18).write(len);
        ch_reg(n, 0x04).write(1);
    }

    #[must_use]
    pub fn busy(&self) -> bool {
        ch_reg(self.n, 0x04).read() & 1 != 0
    }

    #[must_use]
    pub fn remaining(&self) -> u32 {
        ch_reg(self.n, 0x1C).read()
    }

    pub fn abort(&self) {
        ch_reg(self.n, 0x04).write(0);
    }

    /// Wait for the transfer to finish (interrupt driven; call
    /// [`Self::enable_irq`] once first).
    pub async fn wait(&self) {
        poll_fn(|cx| {
            WAKERS[self.n].register(cx.waker());
            if self.busy() { Poll::Pending } else { Poll::Ready(()) }
        })
        .await;
    }

    /// Busy-wait for completion with an iteration bound; false on timeout.
    pub fn wait_blocking(&self, max_iters: u32) -> bool {
        for _ in 0..max_iters {
            if !self.busy() {
                return true;
            }
        }
        false
    }
}

#[must_use]
pub fn pending() -> u32 {
    DMAIP.read()
}
