//! Clocks (03-hal-drivers §2): the ROM-state recreation (so a cold NOR boot
//! and a RAM load start alike) and the core performance levels with their
//! supply voltages. The request/hysteresis governor sits on top
//! (`firmware`).

use atj2157_pac::{cmu, memctl, pmu};

/// ROM-state recreation [HW]: USB supplies on, COREPLL 48 MHz, system clock
/// from COREPLL, DMA + USB clocks, bus-master memory enables.
pub fn rom_state() {
    // VERBATIM(vendor ROM state, via the Rockbox port's atj_rom_usb_clock_setup)
    pmu::VOUT_CTL.set_bits(pmu::vout::USBVCC_EN);
    crate::delay::spin_us(20);
    pmu::VOUT_CTL.set_bits(pmu::vout::USBVDD_EN);
    if cmu::COREPLL_CTL.read() != 0x88 {
        cmu::COREPLL_CTL.write(0x88);
        crate::delay::spin_us(1000);
    }
    cmu::SYSCLK.write(0x0300_0002);
    cmu::CLKEN0.set_bits(cmu::clken0::DMA | cmu::clken0::USB);
    memctl::MASTER_EN.set_bits(0xFF);
    crate::delay::set_core_hz(48_000_000);
}

/// DEVPLL at `n` × 6 MHz (`n` 6…31: 36…186 MHz, the vendor's accepted
/// range; system.md §1.1), 0 = off. The vendor runs it at 180 MHz always;
/// Orpheus only while the card has its high-speed clock (PERF-SD: the card
/// reader), and nothing else takes its clock from it.
pub fn set_devpll(n: u32) {
    if n == 0 {
        cmu::DEVPLL_CTL.write(0);
        return;
    }
    cmu::DEVPLL_CTL.write(cmu::corepll::EN | n.clamp(6, 31));
    crate::delay::spin_us(cmu::corepll::SETTLE_US);
}

/// Core performance levels (03-hal-drivers §2.1), their clocks and supply
/// codes, and the order of a switch: host-tested in `orpheus_power::clock`.
pub use orpheus_power::clock::Level;

struct Row {
    /// COREPLL N (6 MHz steps); 0 = PLL off, core from HOSC.
    n: u32,
    /// SPI0 clock word keeping the NOR at ≤ 48 MHz from COREPLL, or CK24M
    /// when the PLL is off (divider code n ≥ 2 divides by 2n).
    spi: u32,
    /// Extra `CMU_SYSCLK` bits with the PLL selected: bit 8 for the
    /// vendor's levels ≥ 204 MHz (meaning unknown, U-PWR-2).
    sys: u32,
}

/// Rows of the vendor's own table (system.md §1.4); never interpolated.
/// Clock and supply code: `Level::hz`, `Level::vdd_code`.
const fn row(l: Level) -> Row {
    match l {
        Level::Low => Row { n: 0, spi: 0x100, sys: 0 },
        Level::Base => Row { n: 8, spi: 0x200, sys: 0 },
        Level::Mid => Row { n: 16, spi: 0x201, sys: 0 },
        Level::High => Row { n: 24, spi: 0x202, sys: 0 },
        Level::Turbo => Row { n: 32, spi: 0x202, sys: 0 },
        // VERBATIM(vendor KERNEL.DRV level table, level 29: N 0x30, VDD
        // 0x18, CMU_SYSCLK 0x0102; system.md §1.4). NOR ÷6 = 48 MHz.
        Level::Max => Row { n: 48, spi: 0x203, sys: 0x100 },
    }
}

static CURRENT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(1);
static SWITCHES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

#[must_use]
pub fn current() -> Level {
    Level::from_index(CURRENT.load(core::sync::atomic::Ordering::Relaxed))
}

#[must_use]
pub fn switches() -> u32 {
    SWITCHES.load(core::sync::atomic::Ordering::Relaxed)
}

/// Current core voltage code.
#[must_use]
pub fn vdd_code() -> u32 {
    pmu::VOUT_CTL.read() & 0x1F
}

/// The one writer of the core supply code: clamped to
/// `orpheus_power::clock::VDD_MAX` (`0x18`, 1.3 V; [DS] §2.1 absolute
/// maximum 1.35 V, §2.2 recommended 0.9–1.35 V) and only a code of the
/// vendor table (`vdd_write`).
fn set_vdd(code: u8) {
    if let Some(c) = orpheus_power::clock::vdd_write(u32::from(code)) {
        pmu::VOUT_CTL.modify(|v| (v & !0x1F) | c);
    }
}

/// Switch the core clock (and supply) to `to`. Runs with interrupts masked
/// (BASEPRI); detours through HOSC while the PLL is reprogrammed. The
/// supply goes up before the clock and down after it
/// (`orpheus_power::clock::switch`).
pub fn set_level(to: Level) {
    critical_section::with(|_| {
        let from = current();
        let r = row(to);
        let s = orpheus_power::clock::switch(vdd_code() as u8, to);
        if let Some(v) = s.before {
            // Voltage first when going up, then wait for it to settle.
            set_vdd(v);
            crate::delay::spin_us(150);
        }
        // Core on HOSC (SPI0 on CK24M meanwhile so it never overclocks).
        crate::nor::set_clock(0x100);
        cmu::SYSCLK.modify(|v| (v & !0x133) | 1);
        crate::delay::set_core_hz(24_000_000);
        if r.n == 0 {
            cmu::COREPLL_CTL.write(0);
        } else {
            cmu::COREPLL_CTL.write(r.n | 0x80);
            crate::delay::spin_us(100);
            cmu::SYSCLK.modify(|v| (v & !0x133) | 2 | r.sys);
        }
        crate::delay::set_core_hz(to.hz());
        crate::nor::set_clock(r.spi);
        if let Some(v) = s.after {
            set_vdd(v);
        }
        CURRENT.store(to as u8, core::sync::atomic::Ordering::Relaxed);
        if from != to {
            SWITCHES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    });
}
