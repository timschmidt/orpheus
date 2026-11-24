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

/// Core performance levels (03-hal-drivers §2.1). Rows of the vendor's own
/// table (system.md §1.4); never interpolated.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Level {
    /// 24 MHz from HOSC, COREPLL off.
    Low,
    /// 48 MHz, COREPLL N = 8.
    Base,
    /// 96 MHz, N = 16.
    Mid,
    /// 144 MHz, N = 24.
    High,
    /// 192 MHz, N = 32, core supply 1.1 V.
    Turbo,
    /// 288 MHz, N = 48, core supply 1.3 V, `SYSCLK` bit 8 — the vendor's
    /// level 29. An option the user chooses (decision D-4, WP PWR-5); the
    /// governor never selects it on its own.
    Max,
}

struct Row {
    hz: u32,
    /// COREPLL N (6 MHz steps); 0 = PLL off, core from HOSC.
    n: u32,
    /// Core voltage code `VOUT_CTL[4:0]` (0x0C = 1.0 V, 0x10 = 1.1 V).
    vdd: u32,
    /// SPI0 clock word keeping the NOR at ≤ 48 MHz from COREPLL, or CK24M
    /// when the PLL is off (divider code n ≥ 2 divides by 2n).
    spi: u32,
    /// Extra `CMU_SYSCLK` bits with the PLL selected: bit 8 for the
    /// vendor's levels ≥ 204 MHz (meaning unknown, U-PWR-2).
    sys: u32,
}

const fn row(l: Level) -> Row {
    match l {
        Level::Low => Row { hz: 24_000_000, n: 0, vdd: 0x0C, spi: 0x100, sys: 0 },
        Level::Base => Row { hz: 48_000_000, n: 8, vdd: 0x0C, spi: 0x200, sys: 0 },
        Level::Mid => Row { hz: 96_000_000, n: 16, vdd: 0x0C, spi: 0x201, sys: 0 },
        Level::High => Row { hz: 144_000_000, n: 24, vdd: 0x0C, spi: 0x202, sys: 0 },
        Level::Turbo => Row { hz: 192_000_000, n: 32, vdd: 0x10, spi: 0x202, sys: 0 },
        // VERBATIM(vendor KERNEL.DRV level table, level 29: N 0x30, VDD
        // 0x18, CMU_SYSCLK 0x0102; system.md §1.4). NOR ÷6 = 48 MHz.
        Level::Max => Row { hz: 288_000_000, n: 48, vdd: 0x18, spi: 0x203, sys: 0x100 },
    }
}

/// Voltage codes this firmware may write (vendor table rows only).
const ALLOWED_VDD: [u32; 3] = [0x0C, 0x10, 0x18];

static CURRENT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(1);
static SWITCHES: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

impl Level {
    #[must_use]
    pub const fn hz(self) -> u32 {
        row(self).hz
    }
    const fn from_u8(v: u8) -> Self {
        match v {
            0 => Level::Low,
            2 => Level::Mid,
            3 => Level::High,
            4 => Level::Turbo,
            5 => Level::Max,
            _ => Level::Base,
        }
    }
}

#[must_use]
pub fn current() -> Level {
    Level::from_u8(CURRENT.load(core::sync::atomic::Ordering::Relaxed))
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

fn set_vdd(code: u32) {
    if !ALLOWED_VDD.contains(&code) {
        return;
    }
    pmu::VOUT_CTL.modify(|v| (v & !0x1F) | code);
}

/// Switch the core clock (and supply) to `to`. Runs with interrupts masked
/// (BASEPRI); detours through HOSC while the PLL is reprogrammed.
pub fn set_level(to: Level) {
    critical_section::with(|_| {
        let from = current();
        let r = row(to);
        let up_v = r.vdd > vdd_code();
        if up_v {
            // Voltage first when going up, then wait for it to settle.
            set_vdd(r.vdd);
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
        crate::delay::set_core_hz(r.hz);
        crate::nor::set_clock(r.spi);
        if !up_v && r.vdd != vdd_code() {
            set_vdd(r.vdd);
        }
        CURRENT.store(to as u8, core::sync::atomic::Ordering::Relaxed);
        if from != to {
            SWITCHES.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        }
    });
}
