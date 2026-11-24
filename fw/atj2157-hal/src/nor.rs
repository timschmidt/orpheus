//! SPI NOR on SPI0 (03-hal-drivers §6). **Resident**: the pager reads code
//! lines through [`read_dma`] from inside the MemManage handler, so nothing
//! here may touch paged code or data (no `core::fmt`, no panics, no calls
//! into other modules except register access).
//!
//! * Reads: READ (0x03) byte-polled, or FAST READ (0x0B) with the RX FIFO
//!   drained by DMA channel 0 (mode `0x00200088`) as the mask ROM does [HW].
//! * Program/erase: WREN, PP (0x02, ≤ 256 bytes within a page), SE (0x20,
//!   4 KiB), status polling. Every write path checks
//!   `yoton_proto::layout::dual_writable` — nothing below `0x762000` or in
//!   the boot sector region outside the free area is ever written.

use atj2157_pac::spi::spi0;
use atj2157_pac::{Reg, cmu, rmu};
use yoton_proto::layout::{dual_writable, full_writable};

/// Set once the firmware runs from a full-layout slot (or a payload works
/// on one): widens the write guard from the dual-boot free region to the
/// full layout's (sector 0 and the vendor data at the top stay excluded).
static FULL: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Allow writes anywhere the full layout permits.
pub fn allow_full_layout() {
    FULL.store(true, core::sync::atomic::Ordering::Relaxed);
}

#[unsafe(link_section = ".text.resident.nor")]
#[inline(never)]
fn writable(addr: u32, len: u32) -> bool {
    if FULL.load(core::sync::atomic::Ordering::Relaxed) {
        full_writable(addr, len)
    } else {
        dual_writable(addr, len)
    }
}

const CTL: Reg = spi0::CTL;
const STA: Reg = spi0::STA;
const TX: Reg = spi0::TXDAT;
const RX: Reg = spi0::RXDAT;
const BC: Reg = spi0::BC;
/// RX data register address for the DMA source.
const RX_ADDR: u32 = 0xC00A_000C;

const DMA0_CTL: Reg = Reg::at(0xC007_0100);
const DMA0_START: Reg = Reg::at(0xC007_0104);
const DMA0_SRC: Reg = Reg::at(0xC007_0108);
const DMA0_DST: Reg = Reg::at(0xC007_0110);
const DMA0_LEN: Reg = Reg::at(0xC007_0118);
/// SPI0 RX FIFO → memory, byte-wide, fixed source [HW] (mask ROM, port).
const DMA_MODE_SPI_RX: u32 = 0x0020_0088;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    Timeout,
    /// Address outside the writable region.
    Refused,
    Verify,
}

#[unsafe(link_section = ".text.resident.nor")]
#[inline(never)]
fn wait(mask: u32, val: u32) -> bool {
    for _ in 0..200_000 {
        if STA.read() & mask == val {
            return true;
        }
    }
    false
}

#[inline(always)]
fn mode(m: u32) {
    CTL.modify(|v| (v & !3) | m);
}
#[inline(always)]
fn cs_low() {
    CTL.set_bits(8);
    CTL.clear_bits(8);
}
#[inline(always)]
fn cs_high() {
    CTL.set_bits(8);
}

/// Send command bytes (CS stays low afterwards).
#[unsafe(link_section = ".text.resident.nor")]
#[inline(never)]
fn send(bytes: &[u8]) -> bool {
    cs_low();
    mode(0);
    mode(2);
    for &b in bytes {
        TX.write(u32::from(b));
    }
    let ok = wait(0x10, 0x10) & wait(0x40, 0);
    mode(0);
    ok
}

/// Receive into `dst` (byte-polled), CS stays low.
#[unsafe(link_section = ".text.resident.nor")]
#[inline(never)]
fn recv(dst: &mut [u8]) -> bool {
    let mut ok = true;
    for chunk in dst.chunks_mut(0x1000) {
        CTL.clear_bits(0x30);
        CTL.set_bits(0x30);
        BC.write(chunk.len() as u32);
        mode(1);
        for b in chunk.iter_mut() {
            ok &= wait(4, 0);
            *b = RX.read() as u8;
        }
        mode(0);
    }
    ok
}

/// SPI0 set-up as the boot ROM does it (port `nor_init`) [HW].
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn init() {
    cmu::SPI0CLK.modify(|v| (v & !0x3FF) | 0x101);
    cmu::CLKEN0.set_bits(cmu::clken0::SPI0);
    rmu::RST0.clear_bits(rmu::rst0::SPI0);
    rmu::RST0.set_bits(rmu::rst0::SPI0);
    CTL.write(spi0::VENDOR_INIT_CTL);
    for pin in 12..=15 {
        atj2157_pac::gpio::mfp(pin).write(0x3027);
    }
    command(0xAB); // release from power-down
    crate::delay::spin_us(100);
    command(0xFF); // leave continuous-read mode
    // The first transaction after this sequence is dropped [HW]: burn one.
    let mut id = [0u8; 4];
    let _ = read(0, &mut id);
}

/// SPI0 clock word (`CMU_SPI0CLK[9:0]`): `0x101` ROM default, `0x200` the
/// port's fast setting (48 MHz) [HW].
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn set_clock(word: u32) {
    cmu::SPI0CLK.modify(|v| (v & !0x3FF) | (word & 0x3FF));
}

#[must_use]
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn clock() -> u32 {
    cmu::SPI0CLK.read() & 0x3FF
}

// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
fn command(c: u8) -> bool {
    let ok = send(&[c]);
    cs_high();
    ok
}

/// JEDEC id (manufacturer, type, capacity) packed little-endian.
#[must_use]
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn jedec_id() -> u32 {
    let mut id = [0u8; 3];
    let ok = send(&[0x9F]) && recv(&mut id);
    cs_high();
    if ok { u32::from(id[0]) | u32::from(id[1]) << 8 | u32::from(id[2]) << 16 } else { 0 }
}

/// Byte-polled READ (0x03).
#[unsafe(link_section = ".text.resident.nor")]
#[inline(never)]
pub fn read(addr: u32, dst: &mut [u8]) -> Result<(), Error> {
    let ok = send(&[0x03, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]) && recv(dst);
    cs_high();
    if ok { Ok(()) } else { Err(Error::Timeout) }
}

/// FAST READ (0x0B) drained by DMA channel 0 into `dst` (a CPU address in
/// SRAM0–3; translated for SRAM2/3). `len` ≤ 64 KiB. Used by the pager with
/// the MPU disabled. `poll` runs while the transfer is in flight.
///
/// # Safety
/// `dst..dst+len` must be writable RAM that nothing else uses during the
/// call, and DMA channel 0 must not be in use.
#[unsafe(link_section = ".text.resident.nor")]
#[inline(never)]
pub unsafe fn read_dma(addr: u32, dst: u32, len: u32, poll: Option<fn()>) -> Result<(), Error> {
    let mut ok = true;
    cs_low();
    mode(0);
    mode(2);
    for b in [0x0B, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8, 0] {
        TX.write(u32::from(b));
    }
    ok &= wait(0x40, 0) & wait(0x10, 0x10);
    BC.write(len);
    DMA0_SRC.write(RX_ADDR);
    DMA0_DST.write(crate::dma::dma_addr(dst));
    DMA0_LEN.write(len);
    DMA0_CTL.write(DMA_MODE_SPI_RX);
    mode(0);
    CTL.clear_bits(0x30);
    CTL.set_bits(0x30);
    CTL.set_bits(0x8000_0040);
    DMA0_START.write(1);
    mode(1);
    let mut t = 0u32;
    while DMA0_START.read() & 1 != 0 {
        if let Some(p) = poll {
            p();
        }
        t += 1;
        if t > 2_000_000 {
            DMA0_START.write(0);
            ok = false;
            break;
        }
    }
    cs_high();
    CTL.modify(|v| v & 0x7FFF_EFBC);
    if ok { Ok(()) } else { Err(Error::Timeout) }
}

/// Status register 1.
#[must_use]
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn status() -> u8 {
    let mut s = [0xFFu8];
    let _ = send(&[0x05]) && recv(&mut s);
    cs_high();
    s[0]
}

// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
fn wait_ready(max_polls: u32) -> Result<(), Error> {
    for _ in 0..max_polls {
        if status() & 1 == 0 {
            return Ok(());
        }
    }
    Err(Error::Timeout)
}

/// Erase the 4 KiB sector at `addr` (aligned) inside the writable region.
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn erase_sector(addr: u32) -> Result<(), Error> {
    if !addr.is_multiple_of(4096) || !writable(addr, 4096) {
        return Err(Error::Refused);
    }
    // While the chip erases it cannot be read, so nothing may page-fault
    // meanwhile: interrupts are masked (BASEPRI) and everything this runs is
    // resident (02 §3.2 rule 5; suspend/resume is U-NOR-2).
    critical_section::with(|_| {
        command(0x06);
        let _ = send(&[0x20, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]);
        cs_high();
        wait_ready(2_000_000)
    })
}

/// Program up to 256 bytes within one NOR page.
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn program_page(addr: u32, data: &[u8]) -> Result<(), Error> {
    let len = data.len() as u32;
    if len == 0 || len > 256 || (addr & 0xFF) + len > 256 || !writable(addr, len) {
        return Err(Error::Refused);
    }
    // Copy first: `data` may live in paged rodata, which cannot be read
    // once the chip is busy.
    let mut buf = [0u8; 256];
    buf[..data.len()].copy_from_slice(data);
    let data = &buf[..data.len()];
    // As in `erase_sector`: no page fault while the chip programs.
    critical_section::with(|_| {
        command(0x06);
        let mut ok = send(&[0x02, (addr >> 16) as u8, (addr >> 8) as u8, addr as u8]);
        cs_low_continue();
        for &b in data {
            TX.write(u32::from(b));
            ok &= wait(0x10, 0x10);
        }
        ok &= wait(0x40, 0);
        mode(0);
        cs_high();
        if !ok {
            return Err(Error::Timeout);
        }
        wait_ready(200_000)
    })
}

/// Continue a transmit phase with CS still low.
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
fn cs_low_continue() {
    mode(0);
    mode(2);
}

/// Erase, program and verify a sector-aligned range.
// Never inlined into (paged) callers: the code that runs while the
// chip is busy must be this resident copy.
#[inline(never)]
pub fn write(addr: u32, data: &[u8], scratch: &mut [u8; 256]) -> Result<(), Error> {
    if !addr.is_multiple_of(4096) || !writable(addr, data.len().div_ceil(4096) as u32 * 4096) {
        return Err(Error::Refused);
    }
    for (si, sector) in data.chunks(4096).enumerate() {
        let sa = addr + (si as u32) * 4096;
        erase_sector(sa)?;
        for (pi, page) in sector.chunks(256).enumerate() {
            if page.iter().all(|&b| b == 0xFF) {
                continue;
            }
            let pa = sa + (pi as u32) * 256;
            program_page(pa, page)?;
            let back = &mut scratch[..page.len()];
            read(pa, back)?;
            if back != page {
                return Err(Error::Verify);
            }
        }
    }
    Ok(())
}
