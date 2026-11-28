//! Audio DAC and headphone amplifier, `0xC0180000` (03-hal-drivers §12).
//!
//! Power-up is the vendor's pop-free sequence as the Rockbox port runs it
//! [HW]: analogue power and audio PLL, clocks and resets, PA stages enabled
//! in steps while the FIFO is fed a mid-scale ramp, then DMA mode. Register
//! meanings beyond the datasheet are in `atj2157_pac::dac`; offsets used
//! here are the port's.

use atj2157_pac::{Reg, cmu, rmu};

use crate::delay::spin_us;

const BASE: usize = 0xC018_0000;
const fn r(off: usize) -> Reg {
    Reg::at(BASE + off)
}
const DIGCTL: Reg = r(0x00);
const FIFOCTL: Reg = r(0x04);
const FIFOSTA: Reg = r(0x08);
const FIFO0: Reg = r(0x0C);
const VOL_L: Reg = r(0x14);
const VOL_R: Reg = r(0x18);
const PA_CTL: Reg = r(0x28);
const PA_ANTIPOP: Reg = r(0x2C);
const DAC_BIAS: Reg = r(0x30);

/// FIFO0 data register address (DMA destination).
pub const FIFO0_ADDR: u32 = (BASE + 0x0C) as u32;

/// DMA mode word: memory → DAC FIFO0, fixed destination, 16-bit, single
/// transfers (`0x00148B00`) [HW].
pub const DMA_MODE_S16: u32 = 0x0014_8B00;

/// Digital volume code for 0 dB (0.375 dB/step; 0 = mute). [DS]/[code]
pub const VOL_0DB: u8 = 0xBF;

/// Amplifier gain field value the vendor uses for music (`PA_CTL[14:12]`).
pub const PA_GAIN_MUSIC: u32 = 7;

/// Native sample rates (Hz): the 44.1 kHz and 48 kHz families.
pub const RATES: [u32; 9] = [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000];

/// Program the DAC clock for `rate` (vendor routine via the port [HW]).
pub fn set_rate(rate: u32) {
    const DIVS: [u32; 8] = [1, 2, 3, 4, 6, 8, 12, 24];
    let fam44 = rate.is_multiple_of(11025);
    let mut div = if fam44 { 176_400 / rate } else { 192_000 / rate };
    let mut half = 0;
    if div == 16 || div == 24 {
        div /= 2;
        half = 1;
    }
    let idx = DIVS.iter().position(|&d| d == div).unwrap_or(3) as u32;
    // VERBATIM(vendor AUD_DEV 0x1864 via the port)
    cmu::DACCLK.modify(|v| (v & !0xD7) | 0x400 | if fam44 { 1 << 6 } else { 0 } | (half << 4) | idx);
}

fn fifo_put(v: u32, stereo: bool) {
    for _ in 0..20_000 {
        if FIFOSTA.read() & 0x80 != 0 {
            break;
        }
    }
    FIFO0.write(v);
    if stereo {
        FIFO0.write(v);
    }
    FIFOSTA.write(0x80);
}

fn fifo_fill(v: u32, n: u32, stereo: bool) {
    for _ in 0..n {
        fifo_put(v, stereo);
    }
}

/// Power the DAC and amplifier with the anti-pop ramp and switch the FIFO
/// to DMA. Takes ~0.3 s (blocking; start-up only). `volume` is the initial
/// digital volume code.
pub fn power_up(rate: u32, volume: u8) {
    // VERBATIM(vendor AUD_DEV power-up, via the port's dac_power_up) —
    // analogue power and audio PLLs.
    let pmu18 = Reg::at(0xC001_0018);
    pmu18.modify(|v| (v | 0x40) & !0x20);
    Reg::at(0xC000_0118).modify(|v| (v & !0x1F) | 0x1C);
    Reg::at(0xC000_011C).modify(|v| (v & !0x1F) | 0x14);
    spin_us(5000);
    cmu::CLKEN1.set_bits(cmu::clken1::ADC01 | cmu::clken1::DAC | cmu::clken1::DAC_ANALOG);
    cmu::ADC01CLK.write(0x3300_0000 | (1 << 6) | 3);
    set_rate(rate);
    rmu::RST1.clear_bits(rmu::rst1::ADC | rmu::rst1::DAC);
    spin_us(10);
    rmu::RST1.set_bits(rmu::rst1::ADC | rmu::rst1::DAC);
    // Pop-free PA power-up.
    let step = 0xFFFF_FFFFu32 / 4800;
    let mut v = 0x7FFF_FFFFu32;
    let mid = 0x7FFF_FFFFu32;
    PA_CTL.set_bits(0x80);
    PA_CTL.clear_bits(0x300);
    for b in [1, 0x100, 0x10, 2] {
        DIGCTL.set_bits(b);
    }
    for b in [3, 0x14, 0x40] {
        PA_CTL.set_bits(b);
    }
    FIFOCTL.write(0);
    FIFOCTL.modify(|x| (x | 1) & !0x36);
    fifo_fill(mid, 192, false);
    PA_ANTIPOP.set_bits(0x101);
    PA_ANTIPOP.set_bits(0x1010);
    fifo_fill(mid, 192, false);
    PA_ANTIPOP.set_bits(0x202);
    PA_ANTIPOP.set_bits(0x404);
    for _ in 0..4800 {
        fifo_put(v, true);
        v = v.wrapping_sub(step);
    }
    v = v.wrapping_add(step);
    fifo_fill(v, 192, false);
    PA_ANTIPOP.set_bits(0x808);
    fifo_fill(v, 192, false);
    PA_CTL.set_bits(0x28);
    fifo_fill(v, 192, false);
    PA_ANTIPOP.clear_bits(0x1010);
    for _ in 0..4800 {
        fifo_put(v, true);
        v = v.wrapping_add(step);
    }
    v = v.wrapping_sub(step);
    fifo_fill(v, 192, false);
    PA_ANTIPOP.clear_bits(0x202);
    PA_ANTIPOP.clear_bits(0x404);
    fifo_fill(0, 192, false);
    PA_ANTIPOP.clear_bits(0x101);
    PA_CTL.modify(|x| (x & !0x30000) | 0x10000);
    PA_CTL.set_bits(0x40000);
    PA_CTL.set_bits(0x300);
    // PA volume 7, as the vendor's enable wrapper sets after the sequence
    // (audio.md §2.5). Left at 0 the analogue output is effectively muted:
    // measured over the Bluetooth module's analogue tap, 0 gave silence and
    // 7 the file's level within 1 dB (01-hardware §9, 2026-11-24).
    set_pa_gain(PA_GAIN_MUSIC);
    set_volume(volume, volume);
    // FIFO fed by DMA from now on (vendor 0x195C).
    FIFOCTL.clear_bits(0x11);
    FIFOCTL.set_bits(1);
    FIFOCTL.set_bits(0x12);
}

/// Digital volume per channel (0 = mute, 0xBF = 0 dB), with the vendor's
/// soft-step / zero-cross settings (`0x33700`).
pub fn set_volume(left: u8, right: u8) {
    VOL_L.modify(|v| (v & !0xFF8FF) | 0x33700 | u32::from(left));
    VOL_R.modify(|v| (v & !0xFF8FF) | 0x33700 | u32::from(right));
}

/// Amplifier gain field (`PA_CTL[14:12]`, vendor music value 7) [code].
pub fn set_pa_gain(g: u32) {
    PA_CTL.modify(|v| (v & !0x7000) | ((g & 7) << 12));
}

#[must_use]
pub fn regs() -> [u32; 4] {
    [DIGCTL.read(), FIFOCTL.read(), PA_CTL.read(), VOL_L.read()]
}

// FM bypass into the amplifier mixer (audio.md §4, fm-i2c-imu.md §2.6):
// the tuner's analogue output reaches the headphone amplifier without the
// ADC, the DAC or the CPU. The vendor's analogue-input enable (`AUD_DEV`
// API 15, type 1, A2:0x15840092) is three steps with 100 ms waits between
// them; the caller waits (the radio service's state machine).
//
// Differences from the vendor, on purpose: it cycles the amplifier (its
// pop-free power-down, the switch, its power-up) around the steps and
// writes DIGCTL[3:2] = 2; here the amplifier stays as `power_up` left it,
// and DIGCTL is not touched, so the DAC path (which plays with
// DIGCTL[3:2] = 0 [HW]) is unchanged. If the FM path stays silent, these
// are the first two things to try (U-FM-3). Bit meanings are [inferred].

/// Bypass step 1: the DAC block's clocks as the vendor's enable sets them
/// (already on after [`power_up`]) and `PA_ANTIPOP[21]` (FM switch
/// pre-charge / mute) set. Then wait 100 ms.
/// VERBATIM(vendor AUD_DEV A2:0x15840092, type 1)
pub fn fm_precharge() {
    cmu::CLKEN1.set_bits(cmu::clken1::DAC);
    cmu::DACCLK.set_bits(0x400);
    PA_ANTIPOP.set_bits(1 << 21);
}

/// Bypass step 2: `PA_ANTIPOP[23:22]` = 10b (gain field) and bit 20 (FM →
/// amplifier mixer) set. Then wait 100 ms.
/// VERBATIM(vendor AUD_DEV A2:0x15840092, type 1)
pub fn fm_connect() {
    PA_ANTIPOP.modify(|v| (v & !0x00C0_0000) | 0x0010_0000 | 0x0080_0000);
}

/// Bypass step 3: `PA_ANTIPOP[21]` cleared.
/// VERBATIM(vendor AUD_DEV A2:0x15840092, type 1)
pub fn fm_release() {
    PA_ANTIPOP.clear_bits(1 << 21);
}

/// Bypass off: `PA_ANTIPOP[20]` cleared (the FM half of the vendor's API
/// 16, A2:0x158402B8; the vendor then also switches the DAC's digital part
/// off, which the music path here keeps on).
pub fn fm_disconnect() {
    PA_ANTIPOP.clear_bits(1 << 20);
}

/// The two writes the vendor makes that Orpheus does not (AQ-0, owner's
/// A/B listening check; audio.md §2.1, §2.4, §2.6): `DAC_BIAS` (`+0x30`,
/// not in the datasheet) `[14:12]` = 4 and `[26:24]` = 6 at every stream
/// start (the player reads 2 and 3 there without them), and `PA_CTL`
/// bit 27 at the amplifier's enable. Meanings unknown; `vendor` false
/// puts back what the player has without them. Test command only until
/// the owner has listened.
/// VERBATIM(vendor AUD_DEV B0:0x0D800856 and B0:0x0D800606)
pub fn vendor_bias(vendor: bool) {
    let (b, c) = if vendor { (4, 6) } else { (2, 3) };
    DAC_BIAS.modify(|v| (v & !0x0700_7000) | (c << 24) | (b << 12));
    if vendor {
        PA_CTL.set_bits(1 << 27);
    } else {
        PA_CTL.clear_bits(1 << 27);
    }
}

/// `DAC_BIAS` (developer status).
#[must_use]
pub fn bias() -> u32 {
    DAC_BIAS.read()
}

/// `PA_ANTIPOP` (developer status).
#[must_use]
pub fn antipop() -> u32 {
    PA_ANTIPOP.read()
}
