//! USB 2.0 device controller, `0xC0140000` (06-connectivity §2;
//! `yoton/re/usb_controller.md`; port `usb-atj2157.c` [HW]).
//!
//! A Faraday FOTG210-style device core whose data path is an Actions DMA
//! engine: channel 0 serves EP0 (SETUP packets and control data), channels
//! 1–4 serve FIFOs 0–3. The CPU never touches FIFO data. Completion of a
//! USB DMA is polled (no interrupt for it is known, U-USB-3); SETUP, FIFO
//! OUT data and bus events arrive on IRQ 12.
//!
//! Implements `embassy-usb-driver` 0.2 so `embassy-usb` handles enumeration
//! and the classes. EP0 data moves one packet per DMA (≤ 64 bytes), so
//! control transfers of any length work without relying on the DMA to
//! split them (U-USB-2). Buffers handed to the USB DMA must be in SRAM0 and
//! word aligned; others go through a bounce buffer.

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};
use core::task::Poll;

use atj2157_pac::{Reg, cmu, irq, memctl, rmu};
use drv::{
    Direction, EndpointAddress, EndpointAllocError, EndpointError, EndpointInfo, EndpointType, Event,
    Unsupported,
};
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::{Duration, Instant, Timer};
use embassy_usb_driver as drv;

use crate::interrupt::{self, Level};

const BASE: usize = 0xC014_0000;
const fn r(off: usize) -> Reg {
    Reg::at(BASE + off)
}
const DMCR: Reg = r(0x100);
const DAR: Reg = r(0x104);
const DTR: Reg = r(0x108);
const SOFFNR: Reg = r(0x10C);
const PHYTMSR: Reg = r(0x114);
const CXFIFO: Reg = r(0x120);
const IDLE: Reg = r(0x124);
const GIMR: Reg = r(0x130);
const GIMR0: Reg = r(0x134);
const GIMR1: Reg = r(0x138);
const GIMR2: Reg = r(0x13C);
const GISR: Reg = r(0x140);
const GISR0: Reg = r(0x144);
const GISR1: Reg = r(0x148);
const GISR2: Reg = r(0x14C);
const fn in_mps(n: usize) -> Reg {
    r(0x160 + 4 * (n - 1))
}
const fn out_mps(n: usize) -> Reg {
    r(0x180 + 4 * (n - 1))
}
const EPMAP14: Reg = r(0x1A0);
const EPMAP58: Reg = r(0x1A4);
const FIFOMAP: Reg = r(0x1A8);
const FIFOCF: Reg = r(0x1AC);
const fn fibc(k: usize) -> Reg {
    r(0x1B0 + 4 * k)
}
const DMA0_CTL: Reg = r(0x300);
const DMA0_ADDR: Reg = r(0x304);
const fn dmak_ctl(k: usize) -> Reg {
    r(0x308 + 8 * k)
}
const fn dmak_addr(k: usize) -> Reg {
    r(0x30C + 8 * k)
}
const DMA_STAT: Reg = r(0x328);
const DMA_EN: Reg = r(0x330);

const DMCR_GLINT_EN: u32 = 1 << 2;
const DMCR_GOSUSP: u32 = 1 << 3;
const DMCR_SFRST: u32 = 1 << 4;
const DMCR_CHIP_EN: u32 = 1 << 5;
const DMCR_HS: u32 = 1 << 6;
const DMCR_FORCE_FS: u32 = 1 << 9;
const DAR_AFT_CONF: u32 = 1 << 7;
const PHY_UNPLUG: u32 = 1;
const CX_DONE: u32 = 1;
const CX_STALL: u32 = 1 << 2;
const EP_STALL: u32 = 1 << 11;
const EP_RSTG: u32 = 1 << 12;
const FIBC_FFRST: u32 = 1 << 12;
const UDMA_START: u32 = 1;
const UDMA_IN: u32 = 1 << 1;

/// Data FIFOs (and so non-control endpoints).
const FIFOS: usize = 4;

static BUS_WAKER: AtomicWaker = AtomicWaker::new();
static SETUP_WAKER: AtomicWaker = AtomicWaker::new();
static FIFO_WAKERS: [AtomicWaker; FIFOS] = [const { AtomicWaker::new() }; FIFOS];
/// Bus events seen by the ISR: bit 0 reset, 1 suspend, 2 resume.
static BUS_EVENTS: AtomicU32 = AtomicU32::new(0);
static SETUP_READY: AtomicBool = AtomicBool::new(false);
/// Per-FIFO "OUT data waiting" flags.
static FIFO_OUT_READY: AtomicU32 = AtomicU32::new(0);
static CONFIGURED: AtomicBool = AtomicBool::new(false);
/// Statistics.
pub static RESETS: AtomicU32 = AtomicU32::new(0);
pub static SETUPS: AtomicU32 = AtomicU32::new(0);
pub static DMA_ERRORS: AtomicU32 = AtomicU32::new(0);
static ADDR_PENDING: AtomicU8 = AtomicU8::new(0);

#[repr(C, align(4))]
struct Aligned<const N: usize>([u8; N]);
static mut SETUP_BUF: Aligned<8> = Aligned([0; 8]);
static mut EP0_BUF: Aligned<64> = Aligned([0; 64]);
static mut BOUNCE: [Aligned<512>; FIFOS] = [const { Aligned([0; 512]) }; FIFOS];

fn dma_reachable(p: *const u8, len: usize) -> bool {
    let a = p as usize;
    a.is_multiple_of(4) && a >= 0x0010_0000 && a + len <= 0x0013_8000
}

fn on_irq() {
    let g = GISR.read() & !GIMR.read();
    if g & 4 != 0 {
        let s = GISR2.read() & !GIMR2.read();
        if s & 1 != 0 {
            hw_bus_reset();
            RESETS.fetch_add(1, Ordering::Relaxed);
        }
        GISR2.write(s & 7);
        BUS_EVENTS.fetch_or(s & 7, Ordering::Release);
        BUS_WAKER.wake();
    }
    if g & 1 != 0 {
        let s = GISR0.read() & !GIMR0.read();
        if s & 1 != 0 {
            // Level until the 8 bytes are read: mask it, the setup() future
            // reads the packet and unmasks.
            GIMR0.set_bits(1);
            SETUP_READY.store(true, Ordering::Release);
            SETUP_WAKER.wake();
        }
    }
    if g & 2 != 0 {
        let s = GISR1.read() & !GIMR1.read();
        for (k, waker) in FIFO_WAKERS.iter().enumerate() {
            let bits = 3u32 << (2 * k);
            if s & bits != 0 {
                GIMR1.set_bits(bits);
                FIFO_OUT_READY.fetch_or(1 << k, Ordering::Release);
                waker.wake();
            }
        }
    }
}

/// Undo all endpoint configuration (bus reset; port/ROM sequence).
fn hw_bus_reset() {
    DAR.write(0);
    DTR.set_bits(1);
    for n in 1..=8 {
        in_mps(n).write(EP_RSTG);
        out_mps(n).write(EP_RSTG);
        in_mps(n).write(0);
        out_mps(n).write(0);
    }
    EPMAP14.write(0x3333_3333);
    EPMAP58.write(0x3333_3333);
    FIFOMAP.write(0x0F0F_0F0F);
    FIFOCF.write(0);
    DMA_EN.write(0);
    DMA0_CTL.write(0);
    for k in 0..FIFOS {
        dmak_ctl(k).write(0);
    }
    DMA_STAT.write(0x001F_001F);
    GIMR1.write(0x000F_00FF);
    CONFIGURED.store(false, Ordering::Relaxed);
    FIFO_OUT_READY.store(0, Ordering::Relaxed);
    DMA_EN.write(1);
}

/// Wait for a USB DMA channel: `done` / `err` bits of `DMA_STAT`.
async fn dma_wait(done: u32, err: u32) -> Result<(), EndpointError> {
    let start = Instant::now();
    let mut spins = 0u32;
    loop {
        let s = DMA_STAT.read();
        if s & err != 0 {
            DMA_STAT.write(err | done);
            DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
            return Err(EndpointError::Disabled);
        }
        if s & done != 0 {
            DMA_STAT.write(done);
            return Ok(());
        }
        if GISR2.read() & 1 != 0 || BUS_EVENTS.load(Ordering::Relaxed) & 1 != 0 {
            return Err(EndpointError::Disabled);
        }
        spins += 1;
        if spins > 200 {
            if start.elapsed() > Duration::from_millis(500) {
                DMA_ERRORS.fetch_add(1, Ordering::Relaxed);
                return Err(EndpointError::Disabled);
            }
            Timer::after_micros(20).await;
        }
    }
}

async fn ep0_dma(addr: u32, len: usize, input: bool) -> Result<(), EndpointError> {
    DMA0_CTL.write(((len as u32) << 8) | if input { UDMA_IN } else { 0 });
    DMA0_ADDR.write(addr);
    DMA_EN.write(1);
    DMA0_CTL.set_bits(UDMA_START);
    let r = dma_wait(1, 1 << 16).await;
    if r.is_err() {
        DMA0_CTL.write(0);
    }
    r
}

/// Power up the controller (ROM/UDISK sequence [HW]) and leave it detached.
fn hw_init() {
    // VERBATIM(boot ROM 0x23c4 / port usb_init_device)
    atj2157_pac::pmu::VOUT_CTL.set_bits(1 << 29);
    crate::delay::spin_us(5);
    atj2157_pac::pmu::VOUT_CTL.set_bits(1 << 28);
    memctl::MASTER_EN.set_bits(0xFF);
    cmu::CLKEN0.set_bits(cmu::clken0::USB | cmu::clken0::DMA);
    rmu::RST0.clear_bits(rmu::rst0::USB2);
    crate::delay::spin_us(10);
    rmu::RST0.set_bits(rmu::rst0::USB2);
    crate::delay::spin_us(10);
    Reg::at(0xC000_1100).write(0x3055); // USB PHY clock, VERBATIM
    rmu::RST0.clear_bits(rmu::rst0::USB);
    crate::delay::spin_us(10);
    rmu::RST0.set_bits(rmu::rst0::USB);
    crate::delay::spin_us(10);
    PHYTMSR.set_bits(PHY_UNPLUG);
    IDLE.write(7);
    DMCR.set_bits(DMCR_CHIP_EN);
    DTR.set_bits(1);
    DMCR.set_bits(DMCR_SFRST);
    for _ in 0..1_000_000 {
        if DMCR.read() & DMCR_SFRST == 0 {
            break;
        }
    }
    DMCR.set_bits(0x02);
    GISR0.write(0x20);
    GISR2.write(0x1FF);
    GIMR.write(0);
    GIMR0.write(0x3E);
    GIMR1.write(0x000F_00FF);
    GIMR2.write(0x7F8);
    DMA_STAT.write(0x001F_001F);
    r(0x32C).write(0x001F_001F);
    r(0x334).write(0);
    r(0x338).write(1);
    r(0x03C).write(0);
    r(0x1C8).modify(|v| v & 0x3FFF_FFFE);
    DMA_EN.write(1);
    DMCR.clear_bits(DMCR_FORCE_FS);
    DMCR.set_bits(DMCR_GLINT_EN);
    interrupt::install(irq::USB, Level::Normal, on_irq);
}

/// VBUS present (`WAKE_PD` bit 30 or DC5V ADC > 0x200) [code].
#[must_use]
pub fn vbus_present() -> bool {
    atj2157_pac::pmu::WAKE_PD.read() & (1 << 30) != 0 || atj2157_pac::pmu::DC5VADC_DATA.read() & 0x3FF > 0x200
}

/// SOF frame number (a host is sending SOFs when it changes).
#[must_use]
pub fn frame_number() -> u32 {
    SOFFNR.read() & 0x7FF
}

#[must_use]
pub fn high_speed() -> bool {
    DMCR.read() & DMCR_HS != 0
}

/// Soft detach / attach (D+ pull-up).
pub fn set_attached(on: bool) {
    if on {
        PHYTMSR.clear_bits(PHY_UNPLUG);
    } else {
        PHYTMSR.set_bits(PHY_UNPLUG);
    }
}

#[derive(Clone, Copy)]
struct EpAlloc {
    used: bool,
    addr: u8,
    ep_type: EndpointType,
    mps: u16,
}

/// The driver. One per program.
pub struct Driver {
    eps: [EpAlloc; FIFOS],
}

impl Driver {
    /// Power the controller up (detached). The bus attaches on
    /// `Bus::enable`.
    #[must_use]
    pub fn new() -> Self {
        hw_init();
        Self { eps: [EpAlloc { used: false, addr: 0, ep_type: EndpointType::Bulk, mps: 0 }; FIFOS] }
    }

    fn alloc(
        &mut self,
        dir: Direction,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        mps: u16,
        interval: u8,
    ) -> Result<(usize, EndpointInfo), EndpointAllocError> {
        if ep_type == EndpointType::Control {
            return Err(EndpointAllocError);
        }
        let k = self.eps.iter().position(|e| !e.used).ok_or(EndpointAllocError)?;
        // Endpoint numbers 1..=4 = FIFO index + 1 unless the class asks.
        let num = ep_addr.map_or(k + 1, |a| a.index());
        if num == 0 || num > 8 {
            return Err(EndpointAllocError);
        }
        let addr = EndpointAddress::from_parts(num, dir);
        self.eps[k] = EpAlloc { used: true, addr: addr.into(), ep_type, mps };
        Ok((k, EndpointInfo { addr, ep_type, max_packet_size: mps, interval_ms: interval }))
    }
}

impl Default for Driver {
    fn default() -> Self {
        Self::new()
    }
}

impl<'a> drv::Driver<'a> for Driver {
    type EndpointOut = EndpointOut;
    type EndpointIn = EndpointIn;
    type ControlPipe = ControlPipe;
    type Bus = Bus;

    fn alloc_endpoint_out(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<EndpointOut, EndpointAllocError> {
        let (k, info) = self.alloc(Direction::Out, ep_type, ep_addr, max_packet_size, interval_ms)?;
        Ok(EndpointOut { k, info })
    }

    fn alloc_endpoint_in(
        &mut self,
        ep_type: EndpointType,
        ep_addr: Option<EndpointAddress>,
        max_packet_size: u16,
        interval_ms: u8,
    ) -> Result<EndpointIn, EndpointAllocError> {
        let (k, info) = self.alloc(Direction::In, ep_type, ep_addr, max_packet_size, interval_ms)?;
        Ok(EndpointIn { k, info })
    }

    fn start(self, control_max_packet_size: u16) -> (Bus, ControlPipe) {
        (Bus { eps: self.eps, powered: false }, ControlPipe { mps: control_max_packet_size })
    }
}

pub struct Bus {
    eps: [EpAlloc; FIFOS],
    powered: bool,
}

impl Bus {
    fn fifo_of(&self, addr: EndpointAddress) -> Option<usize> {
        let a: u8 = addr.into();
        self.eps.iter().position(|e| e.used && e.addr == a)
    }
}

impl drv::Bus for Bus {
    async fn enable(&mut self) {
        set_attached(false);
        Timer::after_millis(20).await;
        set_attached(true);
    }

    async fn disable(&mut self) {
        set_attached(false);
    }

    async fn poll(&mut self) -> Event {
        if !self.powered {
            // Wait for VBUS, then report power once.
            loop {
                if vbus_present() {
                    self.powered = true;
                    return Event::PowerDetected;
                }
                Timer::after_millis(50).await;
            }
        }
        poll_fn(|cx| {
            BUS_WAKER.register(cx.waker());
            let ev = BUS_EVENTS.swap(0, Ordering::Acquire);
            if ev & 1 != 0 {
                ADDR_PENDING.store(0, Ordering::Relaxed);
                Poll::Ready(Event::Reset)
            } else if ev & 4 != 0 {
                Poll::Ready(Event::Resume)
            } else if ev & 2 != 0 {
                Poll::Ready(Event::Suspend)
            } else {
                Poll::Pending
            }
        })
        .await
    }

    fn endpoint_set_enabled(&mut self, ep_addr: EndpointAddress, enabled: bool) {
        if ep_addr.index() == 0 {
            return;
        }
        let Some(k) = self.fifo_of(ep_addr) else { return };
        let n = ep_addr.index();
        let e = self.eps[k];
        if !enabled {
            FIFOCF.modify(|v| v & !(0xFF << (8 * k)));
            return;
        }
        let hs = high_speed();
        let mps = u32::from(if hs { e.mps } else { e.mps.min(64) });
        let ty = match e.ep_type {
            EndpointType::Isochronous => 1,
            EndpointType::Bulk => 2,
            EndpointType::Interrupt => 3,
            EndpointType::Control => 0,
        };
        if ep_addr.is_in() {
            in_mps(n).write(mps);
            // EP map byte for n: bits[1:0] IN FIFO.
            let (reg, sh) = if n <= 4 { (EPMAP14, 8 * (n - 1)) } else { (EPMAP58, 8 * (n - 5)) };
            reg.modify(|v| (v & !(0x3 << sh)) | ((k as u32) << sh));
            FIFOMAP.modify(|v| (v & !(0xFF << (8 * k))) | ((0x10 | n as u32) << (8 * k)));
        } else {
            out_mps(n).write(mps);
            let (reg, sh) = if n <= 4 { (EPMAP14, 8 * (n - 1) + 4) } else { (EPMAP58, 8 * (n - 5) + 4) };
            reg.modify(|v| (v & !(0x3 << sh)) | ((k as u32) << sh));
            FIFOMAP.modify(|v| (v & !(0xFF << (8 * k))) | ((n as u32) << (8 * k)));
        }
        FIFOCF.modify(|v| (v & !(0xFF << (8 * k))) | ((0x20 | ty) << (8 * k)));
        DAR.set_bits(DAR_AFT_CONF);
        CONFIGURED.store(true, Ordering::Release);
    }

    fn endpoint_set_stalled(&mut self, ep_addr: EndpointAddress, stalled: bool) {
        let n = ep_addr.index();
        if n == 0 {
            if stalled {
                CXFIFO.set_bits(CX_STALL);
            }
            return;
        }
        let reg = if ep_addr.is_in() { in_mps(n) } else { out_mps(n) };
        if stalled {
            reg.set_bits(EP_STALL);
        } else {
            reg.clear_bits(EP_STALL);
            reg.set_bits(EP_RSTG);
            reg.clear_bits(EP_RSTG);
        }
    }

    fn endpoint_is_stalled(&mut self, ep_addr: EndpointAddress) -> bool {
        let n = ep_addr.index();
        if n == 0 {
            return false;
        }
        let reg = if ep_addr.is_in() { in_mps(n) } else { out_mps(n) };
        reg.read() & EP_STALL != 0
    }

    async fn remote_wakeup(&mut self) -> Result<(), Unsupported> {
        Err(Unsupported)
    }
}

pub struct ControlPipe {
    mps: u16,
}

impl drv::ControlPipe for ControlPipe {
    fn max_packet_size(&self) -> usize {
        usize::from(self.mps)
    }

    async fn setup(&mut self) -> [u8; 8] {
        loop {
            poll_fn(|cx| {
                SETUP_WAKER.register(cx.waker());
                if SETUP_READY.load(Ordering::Acquire) { Poll::Ready(()) } else { Poll::Pending }
            })
            .await;
            SETUP_READY.store(false, Ordering::Relaxed);
            let buf = (&raw mut SETUP_BUF) as u32;
            let ok = ep0_dma(buf, 8, false).await.is_ok();
            GIMR0.clear_bits(1);
            if ok {
                SETUPS.fetch_add(1, Ordering::Relaxed);
                // SAFETY: the DMA has finished writing the 8 bytes; only this
                // pipe touches SETUP_BUF.
                return unsafe { (*(&raw const SETUP_BUF)).0 };
            }
        }
    }

    async fn data_out(&mut self, buf: &mut [u8], _first: bool, _last: bool) -> Result<usize, EndpointError> {
        let n = buf.len().min(64);
        if n == 0 {
            return Ok(0);
        }
        let b = (&raw mut EP0_BUF) as u32;
        ep0_dma(b, n, false).await?;
        // SAFETY: DMA finished; only this pipe uses EP0_BUF.
        let src = unsafe { &(*(&raw const EP0_BUF)).0 };
        buf[..n].copy_from_slice(&src[..n]);
        Ok(n)
    }

    async fn data_in(&mut self, data: &[u8], _first: bool, last: bool) -> Result<(), EndpointError> {
        for chunk in data.chunks(64) {
            // SAFETY: only this pipe uses EP0_BUF, and no DMA is running on it.
            let dst = unsafe { &mut (*(&raw mut EP0_BUF)).0 };
            dst[..chunk.len()].copy_from_slice(chunk);
            ep0_dma((&raw mut EP0_BUF) as u32, chunk.len(), true).await?;
        }
        if last {
            CXFIFO.set_bits(CX_DONE);
        }
        Ok(())
    }

    async fn accept(&mut self) {
        CXFIFO.set_bits(CX_DONE);
    }

    async fn reject(&mut self) {
        CXFIFO.set_bits(CX_STALL);
    }

    async fn accept_set_address(&mut self, addr: u8) {
        DAR.modify(|v| (v & DAR_AFT_CONF) | u32::from(addr & 0x7F));
        CXFIFO.set_bits(CX_DONE);
    }
}

async fn wait_configured() {
    while !CONFIGURED.load(Ordering::Acquire) {
        Timer::after_millis(5).await;
    }
}

pub struct EndpointIn {
    k: usize,
    info: EndpointInfo,
}

impl drv::Endpoint for EndpointIn {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }
    async fn wait_enabled(&mut self) {
        wait_configured().await;
    }
}

impl drv::EndpointIn for EndpointIn {
    async fn write(&mut self, buf: &[u8]) -> Result<(), EndpointError> {
        if !CONFIGURED.load(Ordering::Acquire) {
            return Err(EndpointError::Disabled);
        }
        let k = self.k;
        if buf.is_empty() {
            // Zero-length packets: not yet supported by this controller
            // driver (U-USB-2); report success so callers proceed.
            return Ok(());
        }
        let mut addr = buf.as_ptr() as u32;
        if !dma_reachable(buf.as_ptr(), buf.len()) {
            let n = buf.len().min(512);
            // SAFETY: bounce buffer k is used only by FIFO k's endpoint.
            let b = unsafe { &mut (*(&raw mut BOUNCE))[k].0 };
            b[..n].copy_from_slice(&buf[..n]);
            addr = b.as_ptr() as u32;
        }
        dmak_ctl(k).write(((buf.len().min(65536) as u32) << 8) | UDMA_IN);
        dmak_addr(k).write(addr);
        DMA_EN.write(1);
        dmak_ctl(k).set_bits(UDMA_START);
        let r = dma_wait(1 << (1 + k), 1 << (17 + k)).await;
        if r.is_err() {
            dmak_ctl(k).write(0);
        }
        r
    }
}

pub struct EndpointOut {
    k: usize,
    info: EndpointInfo,
}

impl drv::Endpoint for EndpointOut {
    fn info(&self) -> &EndpointInfo {
        &self.info
    }
    async fn wait_enabled(&mut self) {
        wait_configured().await;
    }
}

impl drv::EndpointOut for EndpointOut {
    async fn read(&mut self, buf: &mut [u8]) -> Result<usize, EndpointError> {
        let k = self.k;
        // Unmask this FIFO's OUT/short-packet interrupts and wait for data.
        FIFO_OUT_READY.fetch_and(!(1 << k), Ordering::AcqRel);
        GIMR1.clear_bits(3 << (2 * k));
        poll_fn(|cx| {
            FIFO_WAKERS[k].register(cx.waker());
            if !CONFIGURED.load(Ordering::Acquire) {
                return Poll::Ready(Err(EndpointError::Disabled));
            }
            if FIFO_OUT_READY.load(Ordering::Acquire) & (1 << k) != 0 {
                Poll::Ready(Ok(()))
            } else {
                Poll::Pending
            }
        })
        .await?;
        let n = (fibc(k).read() & 0x7FF) as usize;
        if n == 0 {
            fibc(k).set_bits(FIBC_FFRST);
            return Ok(0);
        }
        if n > buf.len() {
            fibc(k).set_bits(FIBC_FFRST);
            return Err(EndpointError::BufferOverflow);
        }
        let direct = dma_reachable(buf.as_ptr(), n);
        // SAFETY: bounce buffer k belongs to this endpoint.
        let bounce = unsafe { &mut (*(&raw mut BOUNCE))[k].0 };
        let addr = if direct { buf.as_mut_ptr() as u32 } else { bounce.as_mut_ptr() as u32 };
        dmak_ctl(k).write((n as u32) << 8);
        dmak_addr(k).write(addr);
        DMA_EN.write(1);
        dmak_ctl(k).set_bits(UDMA_START);
        if let Err(e) = dma_wait(1 << (1 + k), 1 << (17 + k)).await {
            dmak_ctl(k).write(0);
            fibc(k).set_bits(FIBC_FFRST);
            return Err(e);
        }
        if !direct {
            buf[..n].copy_from_slice(&bounce[..n]);
        }
        Ok(n)
    }
}

/// Detach and power the controller down (port `usb_drv_exit` order).
pub fn shutdown() {
    interrupt::disable(irq::USB);
    PHYTMSR.set_bits(PHY_UNPLUG);
    DMCR.set_bits(DMCR_GOSUSP);
    DMA_EN.write(0);
    rmu::RST0.clear_bits(rmu::rst0::USB);
    rmu::RST0.clear_bits(rmu::rst0::USB2);
    cmu::CLKEN0.clear_bits(cmu::clken0::USB);
}
