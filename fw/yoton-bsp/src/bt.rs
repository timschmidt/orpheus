//! The Bluetooth transmitter module's wiring (01-hardware §5, 06 §4.1,
//! bluetooth.md §1–2): UART1 on GPIO38/39 (MFP function 10), enable line
//! GPIO20 (high = on), host-busy line GPIO31 (high = "do not send").
//!
//! Pin direction (U-BT-1): the datasheet's pin list gives GPIO38
//! function 10 = UART1_TX and GPIO39 = UART1_RX (01-hardware §9); the
//! vendor programs both pads with function 10, so on this board the
//! module's RX is on GPIO38 and its TX on GPIO39 (the vendor's 10 kΩ
//! pull-up sits on 39, the receiving side). The function select fixes the
//! direction in silicon: there is no "swapped" UART set-up to try, and
//! the other UART1 pad pairs (GPIO4/5, 22/23, 28/29) belong to the
//! panel, backlight and card. [`listen`] confirms the board side without
//! driving either pin.
//!
//! Sequences (VERBATIM from the vendor, bluetooth.md §2.1, §1.1):
//! up = UART reset/clock/baud/`CTL`, pads `0x302A` / `0x312A`, receive
//! path armed, GPIO31 output low, then GPIO20 high; down = GPIO20 low,
//! UART off, GPIO38/39 GPIO outputs driven low (no back-feeding of the
//! unpowered module).

use atj2157_hal::dma::Channel;
use atj2157_hal::gpio::{Input, Output};
use atj2157_hal::pac::{gpio, uart as ureg};
use atj2157_hal::uart::{self, DmaRing, Rx, Uart};

use crate::pins::{BT_ENABLE, BT_HOST_BUSY, BT_UART_RX, BT_UART_TX};

/// UART block of the module.
pub const UART: usize = 1;
/// DMA channels (01-hardware §6 plan).
pub const RX_DMA: usize = 8;
pub const TX_DMA: usize = 9;
/// Pad words: function 10, Schmitt, drive 3; the RX pad adds the 10 kΩ
/// pull-up (bluetooth.md §1.1, VERBATIM).
pub const PAD_TX: u32 = 0x302A;
pub const PAD_RX: u32 = 0x312A;
/// Receive ring size (22 ms of 115200 baud).
pub const RING: usize = 256;

/// Boot state: the module off (enable line driven low), nothing else
/// touched (the UART pads stay at their reset value, high impedance).
pub fn init() {
    let _ = Output::new(BT_ENABLE, false);
}

/// How the UART's FIFOs are served.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Mode {
    /// Receive by DMA ring, or by CPU polling of the 16-byte FIFO (the
    /// owner must then poll at least every 1.4 ms).
    pub rx: Rx,
    /// Transmit by DMA (else by CPU through the FIFO).
    pub tx_dma: bool,
}

impl Mode {
    pub const DEFAULT: Mode = Mode { rx: Rx::Dma, tx_dma: true };
}

enum RxPath {
    Dma(DmaRing),
    Cpu,
}

/// The module powered and its UART running.
pub struct Link {
    uart: Uart,
    rx: RxPath,
    tx: Channel,
    mode: Mode,
    pub rx_errors: u32,
}

/// Power the module: UART and receive path first, then the enable line.
///
/// # Safety
/// Takes UART1 and DMA channels 8 and 9 (one `Link` at a time; give them
/// back with [`down`]). `ring` is the receive ring.
pub unsafe fn up(mode: Mode, ring: &'static mut [u8; RING]) -> Link {
    // SAFETY: the caller hands over UART1 and channels 8/9.
    let (u, rx_ch, tx) = unsafe { (Uart::steal(UART), Channel::steal(RX_DMA), Channel::steal(TX_DMA)) };
    u.open(ureg::VENDOR_BR_115200, uart::ctl_8n1(mode.rx, mode.tx_dma));
    gpio::mfp(usize::from(BT_UART_TX)).write(PAD_TX);
    gpio::mfp(usize::from(BT_UART_RX)).write(PAD_RX);
    let rx = match mode.rx {
        Rx::Dma => RxPath::Dma(DmaRing::start(rx_ch, &u, ring)),
        Rx::Cpu => RxPath::Cpu,
    };
    tx.enable_irq();
    let _ = Output::new(BT_HOST_BUSY, false);
    let _ = Output::new(BT_ENABLE, true);
    Link { uart: u, rx, tx, mode, rx_errors: 0 }
}

/// Enable line low, UART off, pads parked low.
pub fn down(link: Link) {
    let _ = Output::new(BT_ENABLE, false);
    match link.rx {
        RxPath::Dma(r) => {
            let _ = r.stop();
        }
        RxPath::Cpu => {}
    }
    link.tx.abort();
    link.uart.close();
    park_pads();
}

fn park_pads() {
    for p in [BT_UART_TX, BT_UART_RX] {
        // Function GPIO, no pull, output low (bluetooth.md §1.1 power-down).
        gpio::mfp(usize::from(p)).write(gpio::ctl::RESET);
        let _ = Output::new(p, false);
    }
}

impl Link {
    #[must_use]
    pub fn mode(&self) -> Mode {
        self.mode
    }

    /// Received bytes since the last call.
    pub fn read(&mut self, out: &mut [u8]) -> usize {
        if self.uart.take_errors() != 0 {
            self.rx_errors += 1;
        }
        match &mut self.rx {
            RxPath::Dma(r) => r.read(out),
            RxPath::Cpu => self.uart.read_fifo(out),
        }
    }

    /// Send a frame; `bytes` must stay put until this returns.
    pub async fn send(&mut self, bytes: &[u8]) -> bool {
        if self.mode.tx_dma {
            self.uart.send_dma(&self.tx, bytes).await
        } else {
            self.uart.send_fifo(bytes).await
        }
    }

    /// Diagnostics: `STA`, `CTL`, DMA ring remain counter, DMA ring
    /// running, most bytes found in one read.
    #[must_use]
    pub fn regs(&self) -> (u32, u32, u32, bool, u32) {
        let (rem, run, peak) = match &self.rx {
            RxPath::Dma(r) => (r.remaining(), r.running(), r.peak),
            RxPath::Cpu => (0, false, 0),
        };
        (self.uart.status(), self.uart.control(), rem, run, peak)
    }
}

/// Board-side check of U-BT-1 that drives neither UART pin: both pads as
/// GPIO inputs with the 10 kΩ pull-up (as the vendor's RX pad), the
/// module powered, and the falling edges on each pad counted while the
/// module starts (its ready event). The module's TX pad shows edges; its
/// RX pad stays high. Call [`Listen::sample`] in slices, then
/// [`Listen::end`].
pub struct Listen {
    tx_in: Input,
    rx_in: Input,
    last: (bool, bool),
    /// Falling edges seen on GPIO38 / GPIO39.
    pub falls: (u32, u32),
    /// Samples taken, and of those with each pad low.
    pub samples: u32,
    pub lows: (u32, u32),
}

impl Listen {
    /// Pads as inputs, then the enable line high. Only while no [`Link`]
    /// exists.
    #[must_use]
    pub fn begin() -> Self {
        let pad_in =
            gpio::ctl::GPIOINEN | gpio::ctl::GPIO10KPUEN | gpio::ctl::SMIT | gpio::ctl::PADDRV.val(3);
        gpio::mfp(usize::from(BT_UART_TX)).write(pad_in);
        gpio::mfp(usize::from(BT_UART_RX)).write(pad_in);
        let tx_in = Input::new(BT_UART_TX);
        let rx_in = Input::new(BT_UART_RX);
        let _ = Output::new(BT_HOST_BUSY, false);
        let _ = Output::new(BT_ENABLE, true);
        Self { tx_in, rx_in, last: (true, true), falls: (0, 0), samples: 0, lows: (0, 0) }
    }

    /// Busy-sample both pads for `us` microseconds (keep slices short: the
    /// executor is blocked meanwhile).
    pub fn sample(&mut self, us: u32) {
        let t0 = atj2157_hal::time::now_us_raw();
        while atj2157_hal::time::now_us_raw().wrapping_sub(t0) < us {
            let a = self.tx_in.is_high();
            let b = self.rx_in.is_high();
            self.samples += 1;
            if !a {
                self.lows.0 += 1;
                if self.last.0 {
                    self.falls.0 += 1;
                }
            }
            if !b {
                self.lows.1 += 1;
                if self.last.1 {
                    self.falls.1 += 1;
                }
            }
            self.last = (a, b);
        }
    }

    /// Enable line low, pads parked.
    pub fn end(self) {
        let _ = Output::new(BT_ENABLE, false);
        park_pads();
    }
}
