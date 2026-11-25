//! Audio ADC, `0xC0181000`: capture from the microphone, the line input or
//! the FM tuner (03-hal-drivers §13, WP AU-13).
//!
//! The sequences are the vendor's (`AUD_DEV.DRV` API 18 "start capture",
//! API 20 "set gain", the stop routine; docs/hw/audio.md §3), register
//! names from the datasheet (`atj2157_pac::adc`). First run on the player
//! 2026-11-24: the capture and its DMA run, the microphone recorded
//! silence (the front end below, fixed since; U-AUD-4/12/13 open).
//!
//! The capture runs by DMA in reload mode from the ADC01 FIFO into a ring
//! of two halves (the caller's; `atj2157_hal::dma::Channel::
//! enable_ring_irq` counts the halves), interleaved when both converters
//! run (FM, line-in), one sample per frame for the microphone. The FIFO
//! holds 24-bit samples in bits 31:8 of a word; [`Width`] picks how they
//! reach memory: narrowed to 16 bits by the FIFO and the DMA (the vendor's
//! setting; which 16 bits is U-AUD-12) or as whole words that the caller
//! narrows (`orpheus_rec::pcm::unpack_24in32`).
//!
//! The microphone front end (2026-11-24, after the first device run
//! recorded digital silence): the vendor's mono microphone enables the
//! **left** mic input (`MIC_MIX_CTL[3:2]`) and the left FD buffer
//! (`CTL` `0x70080`) besides the left converter switches; the first
//! version enabled the right mic input (`[1:0]`, audio.md §3.1 has the two
//! swapped) and no buffer, so nothing reached the left converter (U-AUD-13).
//!
//! It must coexist with the DAC (monitoring, the FM bypass playing):
//!
//! * nothing in the DAC block is written; the shared clock gate
//!   `CLKEN1[0]` stays set at stop (the DAC power-up sets it, and whether
//!   the DAC needs it is unknown);
//! * the vendor's stop writes `CMU+0x80 = 0`, which also removes the
//!   `0x33000000` bits analogue output needs (audio.md §3.1): here the stop
//!   puts the value the DAC power-up wrote back instead
//!   ([`adcclk::PORT_VALUE`]);
//! * the rate function clears `CMU+0x80` bit 4 as well as the vendor's
//!   mask `0x3C7`, so a rate with the extra ÷2 followed by one without it
//!   is right (the vendor's mask leaves bit 4 set).
//!
//! Gains (audio.md §3.2): analogue `CTL[11:8]` index 0–15 (14–33 dB at
//! 6 kΩ, `PREAMP_GAIN_DB_6K`), digital `CHn_DIGCTL[6:0]` 0–59 (0.526 dB
//! a step). Vendor defaults: microphone 13 / 40, line-in 6 / 25, FM 10 and
//! (not in the vendor's table, [inferred]) the line-in's 25.

use atj2157_pac::adc::{self, chdigctl, ctl, digctl, mapping};
use atj2157_pac::cmu::{self, adcclk};
use atj2157_pac::rmu;

use crate::delay::spin_us;

/// ADC01 FIFO data register (the DMA source).
pub const FIFO_ADDR: u32 = 0xC018_1018;

/// How samples reach memory (U-AUD-12).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Width {
    /// `FIFO_DMAWIDTH` = 16-bit and DMA width 16 (the vendor's): one
    /// `i16` per sample in memory.
    Bits16,
    /// `FIFO_DMAWIDTH` = 32-bit and DMA width 32: the FIFO words as they
    /// are (24-bit sample in bits 31:8); the caller keeps the top 16 bits.
    Bits32,
}

/// DMA mode word: ADC01 FIFO (source code `0x0B`) → memory, fixed source
/// (b7), reload (b18), interleaved, width b21:20 (datasheet §5.3.1.3 TWS:
/// 0 32-bit, 1 16-bit). `0x0014008B` is the vendor's (B1:0x0D82062C:
/// `0x40000 | fmt << 20 | planar << 16 | 0x8B`, with the same `fmt` as
/// `FIFO_DMAWIDTH`) [code].
#[must_use]
pub const fn dma_mode(width: Width) -> u32 {
    match width {
        Width::Bits16 => 0x0014_008B,
        Width::Bits32 => 0x0004_008B,
    }
}

/// The vendor waits this long after the front end is on before the first
/// DMA block (API 18).
pub const SETTLE_MS: u64 = 100;

/// What is recorded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Input {
    /// The built-in microphone on the left converter (input code 4);
    /// `differential` as the vendor's `RECORD_MIC_INPUT_MODE = 1`.
    Mic { differential: bool },
    /// AUX0 (line-in), both converters.
    Line,
    /// AUX1 (the FM tuner's output), both converters.
    Fm,
}

impl Input {
    /// Channels captured.
    #[must_use]
    pub const fn channels(self) -> u8 {
        match self {
            Input::Mic { .. } => 1,
            Input::Line | Input::Fm => 2,
        }
    }

    /// Vendor default gains: (analogue index, digital step).
    #[must_use]
    pub const fn default_gain(self) -> (u8, u8) {
        match self {
            Input::Mic { .. } => (adc::VENDOR_PG_MIC as u8, adc::VENDOR_FGAIN_MIC as u8),
            Input::Line => (adc::VENDOR_PG_LINEIN as u8, adc::VENDOR_FGAIN_LINEIN as u8),
            Input::Fm => (adc::VENDOR_PG_FM as u8, adc::VENDOR_FGAIN_LINEIN as u8),
        }
    }
}

/// The digital gain for an offset in dB from the input's default
/// (0.526 dB a step, 0–59).
#[must_use]
pub fn digital_for(input: Input, offset_db: i8) -> u8 {
    let (_, d) = input.default_gain();
    let steps = i32::from(offset_db) * 1000 / 526;
    (i32::from(d) + steps).clamp(0, adc::chdigctl::FGAIN_MAX as i32) as u8
}

/// Rates the ADC takes (both families, 8–48 kHz; datasheet §8.2.1).
#[must_use]
pub fn rate_supported(rate: u32) -> bool {
    [8000, 11025, 12000, 16000, 22050, 24000, 32000, 44100, 48000].contains(&rate)
}

/// `CMU+0x80` for `rate` (vendor rate function, audio.md §1.3, ADC form):
/// divider index, the extra ÷2, the 44.1 kHz family's PLL, the FIR/CIC
/// clock bits.
fn set_rate(rate: u32) {
    const DIVS: [u32; 7] = [1, 2, 3, 4, 6, 8, 12];
    let fam44 = rate.is_multiple_of(11025);
    let mut div = if fam44 { 176_400 / rate.max(1) } else { 192_000 / rate.max(1) };
    let mut half = 0;
    if div == 16 || div == 24 {
        div /= 2;
        half = 1;
    }
    let idx = DIVS.iter().position(|&d| d == div).unwrap_or(3) as u32;
    // VERBATIM(vendor AUD_DEV B1:0x0D820528) but for bit 4 in the mask.
    cmu::ADC01CLK
        .modify(|v| (v & !0x3D7) | (u32::from(fam44) << 6) | (half << 4) | idx | adcclk::VENDOR_FIR_CIC);
}

/// Power the converter path for `input` at `rate` and start the FIFO in
/// DMA mode (vendor API 18 up to the DMA). The caller then starts its DMA
/// channel with [`dma_mode`]`(width)` from [`FIFO_ADDR`] after
/// [`SETTLE_MS`]. `analog` 0–15, `digital` 0–59.
pub fn start(input: Input, rate: u32, analog: u8, digital: u8, width: Width) {
    // VERBATIM(vendor AUD_DEV B1:0x0D8206B2, API 18) from here.
    // Reset pulse, clock gate, rate.
    rmu::RST1.clear_bits(rmu::rst1::ADC);
    spin_us(10);
    rmu::RST1.set_bits(rmu::rst1::ADC);
    cmu::CLKEN1.set_bits(cmu::clken1::ADC01);
    set_rate(rate);
    // Input selection: bias first, then the converter switches.
    adc::CTL.set_bits(ctl::BIASEN);
    let stereo = input.channels() == 2;
    match input {
        Input::Mic { .. } => {
            // Left converter path (code 4): ADCL, VRDAL, PREOPL.
            adc::CTL.modify(|v| (v & !0x3F) | adc::VENDOR_CTL_LEFT);
        }
        Input::Line => {
            // AUX0 byte: both on, single-ended, 30 kΩ; the line preset.
            adc::AUX_CTL.modify(|v| (v & !0xFF) | adc::VENDOR_AUX_LINEIN);
            adc::CTL.set_bits(adc::VENDOR_CTL_LINEIN | adc::VENDOR_CTL_LEFT | adc::VENDOR_CTL_RIGHT);
        }
        Input::Fm => {
            // AUX1 byte: both on, single-ended, 6 kΩ; the FM preset.
            adc::AUX_CTL.modify(|v| (v & !0xFF00) | (adc::VENDOR_AUX_FM << 8));
            adc::CTL.set_bits(adc::VENDOR_CTL_FM | adc::VENDOR_CTL_LEFT | adc::VENDOR_CTL_RIGHT);
        }
    }
    // Channels: CH0 ← analogue L (code 4), CH1 ← analogue R (code 5);
    // gain, high-pass and FIFO slot on; the analogue clock for codes 4/5.
    let ch_word = |d: u8| (u32::from(d) & chdigctl::FGAIN.mask()) | chdigctl::HPF_EN | chdigctl::DATA_OUT_EN;
    adc::MAPPING.set(mapping::CH0_MAP, mapping::MAP_ANALOG_L);
    adc::CH0_DIGCTL.write(ch_word(digital));
    if stereo {
        adc::MAPPING.set(mapping::CH1_MAP, mapping::MAP_ANALOG_R);
        adc::CH1_DIGCTL.write(ch_word(digital));
    }
    adc::CTL.set(ctl::PREAM_PG, u32::from(analog.min(15)));
    cmu::ADC01CLK.set_bits(adcclk::ADCANACLK_EN);
    adc::DIGCTL.set_bits(if stereo { digctl::CH0_DIG_EN | digctl::CH1_DIG_EN } else { digctl::CH0_DIG_EN });
    // Bias words, FIFO set-up.
    adc::BIAS0.set_bits(adc::VENDOR_BIAS0_SET);
    adc::BIAS1.set_bits(adc::VENDOR_BIAS1_SET);
    cmu::ADC01CLK.clear_bits(adcclk::OVFSDIV.mask());
    adc::GRPCTL.write(0);
    let w16 = if width == Width::Bits16 { adc::fifoctl::FIFO_DMAWIDTH } else { 0 };
    adc::ADC01_FIFOCTL.modify(|v| (v & !adc::VENDOR_FIFOCTL_CLEAR) | adc::VENDOR_FIFOCTL_DMA | w16);
    // Analogue front end for the microphone, left side (B1:0x0D82058C,
    // called with left = 1, right = 0): the left mic input on, single-ended
    // or differential; then in CTL the gain and the left FD buffer (6 kΩ).
    // The values: `adc::mic_mix_for_mic`, `adc::ctl_for_mic` (host-tested).
    if let Input::Mic { differential } = input {
        adc::MIC_MIX_CTL.modify(|v| adc::mic_mix_for_mic(v, differential));
        adc::CTL.modify(|v| adc::ctl_for_mic(v, u32::from(analog)));
    }
    // VERBATIM end.
}

/// Change the gains while capturing (vendor API 20: both channels).
pub fn set_gain(analog: u8, digital: u8) {
    adc::CTL.set(ctl::PREAM_PG, u32::from(analog.min(15)));
    let d = u32::from(digital) & chdigctl::FGAIN.mask();
    adc::CH0_DIGCTL.modify(|v| (v & !chdigctl::FGAIN.mask()) | d);
    adc::CH1_DIGCTL.modify(|v| (v & !chdigctl::FGAIN.mask()) | d);
}

/// Stop capturing (the caller stops its DMA channel first). The vendor's
/// stop (B1:0x0D8208B8) less its `CLKEN1[0]` clear and `CMU+0x80 = 0`
/// (see the module notes).
pub fn stop() {
    adc::ADC01_FIFOCTL.write(0);
    adc::MIC_MIX_CTL.write(0);
    adc::AUX_CTL.write(0);
    adc::CTL.write(0);
    adc::CH0_DIGCTL.write(0);
    adc::CH1_DIGCTL.write(0);
    adc::DIGCTL.write(0);
    cmu::ADC01CLK.write(adcclk::PORT_VALUE);
}

/// The FIFO status (`ADC01_FIFOSTA`: b8 overrun, b7 half full, b6 empty,
/// b4:0 level) and two words read straight from the FIFO (developer
/// `rec raw`). Two, so the DMA's L/R order is kept: each read takes a word
/// the DMA would have had.
#[must_use]
pub fn fifo_peek() -> (u32, [u32; 2]) {
    let sta = adc::ADC01_FIFOSTA.read();
    let a = adc::ADC01_DAT_FIFO.read();
    let b = adc::ADC01_DAT_FIFO.read();
    (sta, [a, b])
}

/// Registers for the developer status: `CMU+0x80`, CTL, CH0, MIC_MIX,
/// FIFOCTL, FIFOSTA.
#[must_use]
pub fn regs() -> [u32; 6] {
    [
        cmu::ADC01CLK.read(),
        adc::CTL.read(),
        adc::CH0_DIGCTL.read(),
        adc::MIC_MIX_CTL.read(),
        adc::ADC01_FIFOCTL.read(),
        adc::ADC01_FIFOSTA.read(),
    ]
}
