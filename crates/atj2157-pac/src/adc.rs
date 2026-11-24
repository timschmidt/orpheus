//! Audio ADC and analogue input front end, `0xC0181000` (datasheet §8.2).
//!
//! Sources: datasheet §8.2 (authoritative layout); docs/hw/audio.md §3–4
//! (vendor `AUD_DEV.DRV` record and FM/line-in paths, written before the
//! datasheet was found: its register names differ, see the disagreement
//! notes). Nothing here has run on the player: the port and the payloads do
//! not use this block, so every vendor value is \[code\].
//!
//! Channels: CH0/CH1 feed FIFO "ADC01" ([`ADC01_DAT_FIFO`], `0xC0181018`, DMA
//! source code `0x0B`), CH2/CH3 feed "ADC23" (`0xC018101C`, code `0x0C`).
//! [`MAPPING`] picks the source of each channel (analogue L/R, DMIC, low-power
//! channel). Interrupts: NVIC 40 (ADC01), 41 (ADC23). Clocks: `cmu::ADC01CLK`
//! / `ADC23CLK`, gate CLKEN1 b0.
//!
//! Unknown: layouts of `BIAS0`/`BIAS1` (listed, not described), the vendor's
//! read of `+0x70`, AUX_CTL `[23:16]`, real gains on this board (U-AUD-4).

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC018_1000;

pub const DIGCTL_OFFSET: usize = 0x00;
pub const GRPCTL_OFFSET: usize = 0x04;
pub const ADC01_FIFOCTL_OFFSET: usize = 0x08;
pub const ADC23_FIFOCTL_OFFSET: usize = 0x0C;
pub const ADC01_FIFOSTA_OFFSET: usize = 0x10;
pub const ADC23_FIFOSTA_OFFSET: usize = 0x14;
pub const ADC01_DAT_FIFO_OFFSET: usize = 0x18;
pub const ADC23_DAT_FIFO_OFFSET: usize = 0x1C;
pub const AAL_CTL_OFFSET: usize = 0x20;
pub const CH0_DIGCTL_OFFSET: usize = 0x24;
pub const MAPPING_OFFSET: usize = 0x34;
pub const AUX_CTL_OFFSET: usize = 0x38;
pub const MIC_MIX_CTL_OFFSET: usize = 0x3C;
pub const AEC_CTL_OFFSET: usize = 0x40;
pub const CTL_OFFSET: usize = 0x44;
pub const BIAS0_OFFSET: usize = 0x48;
pub const BIAS1_OFFSET: usize = 0x4C;
pub const VMIC_CTL_OFFSET: usize = 0x54;

/// ADC_DIGCTL: digital channel enables. \[DS\] §8.2.6.1; vendor sets CH0/CH1
/// \[code\]. Reset 0.
pub const DIGCTL: Reg = Reg::at(BASE + DIGCTL_OFFSET);
pub mod digctl {
    use super::bit;
    pub const CH3_DIG_EN: u32 = bit(3);
    pub const CH2_DIG_EN: u32 = bit(2);
    pub const CH1_DIG_EN: u32 = bit(1);
    pub const CH0_DIG_EN: u32 = bit(0);
}

/// ADC_GRPCTL: oversampling and decimation-filter choice per pair. \[DS\]
/// §8.2.6.2. Vendor writes 0 (128fs, FIR mode A) \[code\]; audio.md §3.1 left it
/// unnamed. Reset 0.
pub const GRPCTL: Reg = Reg::at(BASE + GRPCTL_OFFSET);
pub mod grpctl {
    use super::Field;
    /// DMIC pre-gain 1×·2^n (n = 0..5), 6–7 = 63×. \[DS\]
    pub const DMIC_PRE_GAIN: Field = Field::bits(14, 12);
    /// 0 128fs, 1 64fs, 2 150fs, 3 50fs. \[DS\]
    pub const ADC23_OVFS_SEL: Field = Field::bits(10, 9);
    /// FIR mode 0 A, 1 B, 2 C (datasheet §8.2.3). \[DS\]
    pub const ADC23_FIR_MD_SEL: Field = Field::bits(8, 7);
    pub const ADC01_OVFS_SEL: Field = Field::bits(5, 4);
    pub const ADC01_FIR_MD_SEL: Field = Field::bits(3, 2);
}

/// ADC01_FIFOCTL. \[DS\] §8.2.6.3 (fields in [`fifoctl`]). Reset 0. Vendor
/// `(v & !0xB3) | 0x13 | fmt16 << 7` \[code\]. **Disagreement:** audio.md §3.1
/// reads b1 as DMA select, b4 as DMA request; the datasheet has b1 DRQ enable,
/// b5:4 output select (1 = DMA).
pub const ADC01_FIFOCTL: Reg = Reg::at(BASE + ADC01_FIFOCTL_OFFSET);
/// ADC23_FIFOCTL. \[DS\] §8.2.6.4 (fields in [`fifoctl`]).
pub const ADC23_FIFOCTL: Reg = Reg::at(BASE + ADC23_FIFOCTL_OFFSET);
pub mod fifoctl {
    use super::{Field, bit};
    /// DMA width: 0 32-bit, 1 16-bit. \[DS\]
    pub const FIFO_DMAWIDTH: u32 = bit(7);
    /// Output: [`OS_CPU`] / [`OS_DMA`]. \[DS\]
    pub const FIFO_OS: Field = Field::bits(5, 4);
    pub const OS_CPU: u32 = 0;
    pub const OS_DMA: u32 = 1;
    /// Half-full IRQ enable. \[DS\]
    pub const FIFO_IEN: u32 = bit(2);
    /// Half-full DMA request enable. \[DS\]
    pub const FIFO_DEN: u32 = bit(1);
    /// Run: 0 holds the FIFO in reset. \[DS\]
    pub const FIFO_RST: u32 = bit(0);
}

/// ADC01_FIFOSTA. \[DS\] §8.2.6.5 (fields in [`fifosta`]). Reset `0x40`.
pub const ADC01_FIFOSTA: Reg = Reg::at(BASE + ADC01_FIFOSTA_OFFSET);
/// ADC23_FIFOSTA. \[DS\] §8.2.6.6.
pub const ADC23_FIFOSTA: Reg = Reg::at(BASE + ADC23_FIFOSTA_OFFSET);
pub mod fifosta {
    use super::{Field, bit};
    /// Overrun error (written while full); W1C. \[DS\]
    pub const FIFO_ER: u32 = bit(8);
    /// Half-full pending; W1C. \[DS\]
    pub const FIFO_IP: u32 = bit(7);
    pub const FIFO_EMPTY: u32 = bit(6);
    /// Fill level. \[DS\]
    pub const FIFO_STATUS: Field = Field::bits(4, 0);
}

/// ADC01_DAT_FIFO (read-only), DMA source `0xC0181018` (audio.md §3.4,
/// 01-hardware §6). 24-bit samples in `[31:8]`, 32 levels. \[DS\] §8.2.6.7
pub const ADC01_DAT_FIFO: Reg = Reg::at(BASE + ADC01_DAT_FIFO_OFFSET);
/// ADC23_DAT_FIFO. \[DS\] §8.2.6.8
pub const ADC23_DAT_FIFO: Reg = Reg::at(BASE + ADC23_DAT_FIFO_OFFSET);
pub mod dat {
    use super::Field;
    pub const ADDAT: Field = Field::bits(31, 8);
}

/// AAL_CTL: automatic amplitude limiter. Unused by the vendor. \[DS\] §8.2.6.9.
/// Reset `0xC0F0`.
pub const AAL_CTL: Reg = Reg::at(BASE + AAL_CTL_OFFSET);
pub mod aal_ctl {
    use super::{Field, bit};
    pub const AAL_EN: u32 = bit(17);
    /// 0 recover one step, 1 restore at once when below VT0. \[DS\]
    pub const AAL_RECOVER_MODE: u32 = bit(16);
    /// Frame length ratio 1, 1/2, 1/3, 1/6 (10.9 ms base at 48 kHz). \[DS\]
    pub const AAL_FL: Field = Field::bits(15, 14);
    /// High threshold: −0.4, −1.8, −5.3, −11.3 dBFS. \[DS\]
    pub const AAL_VT1: Field = Field::bits(13, 12);
    /// Recovery threshold: off, −49.3, −39.3, −29.3, −19.3, −9.3, −5.3, −2.3 dBFS. \[DS\]
    pub const AAL_VT0: Field = Field::bits(10, 8);
    pub const AAL_MAX: Field = Field::bits(7, 4);
    /// Current gain reduction (read-only). \[DS\]
    pub const AAL_CNT: Field = Field::bits(3, 0);
}

/// CHn_DIGCTL, n = 0..3, at `+0x24 + 4·n`. \[DS\] §8.2.6.10–13 (fields in
/// [`chdigctl`]). Reset 0. Vendor CH0/CH1 \[code\].
#[must_use]
pub const fn ch_digctl(n: usize) -> Reg {
    Reg::at(BASE + CH0_DIGCTL_OFFSET + 4 * n)
}
pub const CH0_DIGCTL: Reg = ch_digctl(0);
pub const CH1_DIGCTL: Reg = ch_digctl(1);
pub const CH2_DIGCTL: Reg = ch_digctl(2);
pub const CH3_DIGCTL: Reg = ch_digctl(3);
pub mod chdigctl {
    use super::{Field, bit};
    /// Limiter on this channel (CH0/CH1 only). \[DS\]
    pub const AAL_CH_EN: u32 = bit(21);
    /// Channel writes into its FIFO slot. \[DS\]; vendor sets it on enable
    /// \[code\] (audio.md §3.1 "b20", meaning unknown there).
    pub const DATA_OUT_EN: u32 = bit(20);
    /// High-pass filter enable. \[DS\]; vendor sets it on enable \[code\].
    /// **Disagreement:** audio.md §3.2 says no high-pass is written anywhere.
    pub const HPF_EN: u32 = bit(19);
    /// HPF range: 0 low (1–60 Hz at 48 kHz), 1 high (30–1860 Hz). \[DS\]
    pub const HPF_S: u32 = bit(18);
    /// HPF cut-off `fc0 + N·s0` (§8.2.2). \[DS\]
    pub const HPF_N: Field = Field::bits(17, 12);
    /// Source: 0 analogue ADC, 1 digital microphone. \[DS\]
    pub const MIC_SEL: u32 = bit(11);
    /// Coarse gain 0/6/12/18 dB. \[DS\]
    pub const SGAIN: Field = Field::bits(9, 8);
    /// Apply `FGAIN` at a zero crossing. \[DS\]
    pub const ZERO_CROSS_EN: u32 = bit(7);
    /// Fine gain 0.526 dB/step, 0..=[`FGAIN_MAX`] (31.03 dB). \[DS\]; vendor
    /// clamps to 59 \[code\].
    pub const FGAIN: Field = Field::bits(6, 0);
    pub const FGAIN_MAX: u32 = 0x3B;
}

/// ADC_MAPPING: source of each digital channel. \[DS\] §8.2.6.14. Reset
/// `0x5410`. Vendor: CH0 = 4 (analogue L), CH1 = 5 (analogue R) \[code\].
/// **Disagreement:** audio.md §3.1 ("ADC_SRC") reads 4-bit codes at b3:0/b7:4;
/// the datasheet has 3-bit fields at `[2:0]`, `[6:4]`, `[10:8]`, `[14:12]`.
pub const MAPPING: Reg = Reg::at(BASE + MAPPING_OFFSET);
pub mod mapping {
    use super::Field;
    pub const CH3_MAP: Field = Field::bits(14, 12);
    pub const CH2_MAP: Field = Field::bits(10, 8);
    pub const CH1_MAP: Field = Field::bits(6, 4);
    pub const CH0_MAP: Field = Field::bits(2, 0);
    /// DMIC0..3 are 0..3. \[DS\]
    pub const MAP_DMIC0: u32 = 0;
    pub const MAP_ANALOG_L: u32 = 4;
    pub const MAP_ANALOG_R: u32 = 5;
    /// Low-power (AEC) analogue channel. \[DS\]; vendor "source code 6" \[code\].
    pub const MAP_LOW_POWER: u32 = 6;
}

/// ADC_AUX_CTL: AUX0/AUX1 line inputs (pins AUX0L/R = GPIO66/67, AUX1L/R =
/// GPIO68/69). \[DS\] §8.2.6.15. Reset 0. Vendor: AUX0 byte for line-in,
/// AUX1 byte for FM (audio.md §3.1 "AIN_CTL") \[code\].
pub const AUX_CTL: Reg = Reg::at(BASE + AUX_CTL_OFFSET);
pub mod aux_ctl {
    use super::{Field, bit};
    /// Datasheet "Reserved" (RW); vendor writes 0xFB, the AUX byte layout
    /// (a third input pair? feature list says "3 pairs AUXIN") \[inferred\].
    pub const UNK_B23_16: Field = Field::bits(23, 16);
    /// AUX1L → ADC L: see [`IN_OFF`] … [`IN_ON`]. \[DS\]
    pub const AUX1L_EN: Field = Field::bits(15, 14);
    pub const AUX1R_EN: Field = Field::bits(13, 12);
    /// 1 single-ended, 0 differential (into ADC R). \[DS\]
    pub const AUX1_IN_MODE: u32 = bit(11);
    pub const AUX1LRMIX: u32 = bit(10);
    /// Input resistor: see [`IRS_120K`] …. \[DS\]
    pub const AUX1_IRS: Field = Field::bits(9, 8);
    pub const AUX0L_EN: Field = Field::bits(7, 6);
    pub const AUX0R_EN: Field = Field::bits(5, 4);
    /// 1 single-ended, 0 differential (into ADC L). \[DS\]
    pub const AUX0_IN_MODE: u32 = bit(3);
    pub const AUX0LRMIX: u32 = bit(2);
    pub const AUX0_IRS: Field = Field::bits(1, 0);
    /// Input enable codes: off, off with 15 kΩ pull-down, off at VR, on. \[DS\]
    pub const IN_OFF: u32 = 0;
    pub const IN_PULLDOWN: u32 = 1;
    pub const IN_VR: u32 = 2;
    pub const IN_ON: u32 = 3;
    pub const IRS_120K: u32 = 0;
    pub const IRS_60K: u32 = 1;
    pub const IRS_30K: u32 = 2;
    pub const IRS_6K: u32 = 3;
}

/// ADC_MIC_MIX_CTL: microphone input and PA-output → ADC mix. \[DS\] §8.2.6.16.
/// Reset 0. Vendor `(v & !0x13F) | 0x100 | MICL/MICR | MIC_IN_MODE` \[code\]
/// (audio.md "ADC_ANA"; the front-end function B1:0x0D82058C, see
/// [`VENDOR_MIC_LEFT`]).
pub const MIC_MIX_CTL: Reg = Reg::at(BASE + MIC_MIX_CTL_OFFSET);
pub mod mic_mix_ctl {
    use super::{Field, bit};
    /// PA OUTR → ADC L. With the three below: an analogue DAC → ADC loop for
    /// self-test (U-AUD-6). \[DS\]
    pub const PAR2ADL_EN: u32 = bit(14);
    pub const PAL2ADL_EN: u32 = bit(13);
    pub const PAR2ADR_EN: u32 = bit(12);
    pub const PAL2ADR_EN: u32 = bit(11);
    pub const PAR_PD_EN: u32 = bit(10);
    pub const PAL_PD_EN: u32 = bit(9);
    /// Mic input resistor: res mode 0 60 kΩ / 1 6 kΩ; GM mode 0 5 kΩ / 1 2 kΩ.
    /// \[DS\]. **Disagreement:** audio.md §3.3 guesses mic bias; vendor sets 1.
    pub const MICGRSEL: u32 = bit(8);
    /// GM-mode input resistor 10k/50k/250k/500k. \[DS\]
    pub const GMIRSEL: Field = Field::bits(7, 6);
    /// 0 resistor mode, 1 GM mode. \[DS\]
    pub const MIC_IRMS: u32 = bit(5);
    /// 1 single-ended, 0 differential. \[DS\]; audio.md agrees.
    pub const MIC_IN_MODE: u32 = bit(4);
    /// Left mic: codes as `aux_ctl::IN_*` (pull-down 5 kΩ). \[DS\]. The
    /// vendor's mono microphone (left converter, input code 4) sets this
    /// field, not b1:0 \[code\] (B1:0x0D82058C re-read 2026-11-24: its first
    /// argument, "left", ORs `0xC`). audio.md §3.1 has the two swapped.
    pub const MICL_EN: Field = Field::bits(3, 2);
    pub const MICR_EN: Field = Field::bits(1, 0);
}

/// ADC_AEC_CTL: low-power (AEC/VAD) channel. \[DS\] §8.2.6.17. Reset 0. Vendor
/// sets b10:9 for source code 6 \[code\] (audio.md "AIN_SEL2").
pub const AEC_CTL: Reg = Reg::at(BASE + AEC_CTL_OFFSET);
pub mod aec_ctl {
    use super::{Field, bit};
    /// 0 /2 (256fs input), 1 /1. \[DS\]
    pub const AECCLK_DIV: u32 = bit(12);
    pub const AECADC_BINV: u32 = bit(11);
    pub const AECVRDA_EN: u32 = bit(10);
    pub const ADC_VAD_EN: u32 = bit(9);
    /// 0 AEC preamp, 1 ADC L preamp. \[DS\]
    pub const AEC_DSS: u32 = bit(8);
    pub const AECICCEN: u32 = bit(7);
    /// −6, −3, 0, +3, +6 dB (4–7). \[DS\]
    pub const AEC_GAIN: Field = Field::bits(6, 4);
    pub const AECL_PDEN: u32 = bit(3);
    pub const AECR_PDEN: u32 = bit(2);
    pub const AECL_EN: u32 = bit(1);
    pub const AECR_EN: u32 = bit(0);
}

/// ADC_CTL: analogue converters, preamps, buffers. \[DS\] §8.2.6.18. Reset 0.
/// audio.md §3.1 "ADC_INCTL" \[code\].
pub const CTL: Reg = Reg::at(BASE + CTL_OFFSET);
pub mod ctl {
    use super::{Field, bit};
    /// 0 /2 (256fs input), 1 /1. \[DS\]
    pub const ADCCLK_DIV: u32 = bit(28);
    /// Bias enable. \[DS\]; vendor sets it before input selection \[code\].
    pub const BIASEN: u32 = bit(26);
    pub const ADR_CAPFC_EN: u32 = bit(25);
    pub const ADL_CAPFC_EN: u32 = bit(24);
    pub const ADCL_BINV: u32 = bit(21);
    pub const ADCR_BINV: u32 = bit(20);
    /// FD buffer L input resistor: 0xx off, 4 120k, 5 60k, 6 30k, 7 6k. \[DS\]
    pub const FDBUFL_IRS: Field = Field::bits(18, 16);
    pub const FDBUFR_IRS: Field = Field::bits(14, 12);
    /// Preamp feedback resistor (gain index), see [`super::PREAMP_GAIN_DB_6K`].
    /// \[DS\]; vendor "analogue gain 0–15" \[code\].
    pub const PREAM_PG: Field = Field::bits(11, 8);
    /// **Disagreement** (b7/b6 and the IRS fields): audio.md §3.1 groups b6 +
    /// b14:12 as "left path" and b7 + b18:16 as "right"; datasheet: b6/b14:12
    /// are FDBUF R, b7/b18:16 FDBUF L.
    pub const FDBUFL_EN: u32 = bit(7);
    pub const FDBUFR_EN: u32 = bit(6);
    pub const PREOPL_EN: u32 = bit(5);
    pub const PREOPR_EN: u32 = bit(4);
    pub const VRDAL_EN: u32 = bit(3);
    pub const VRDAR_EN: u32 = bit(2);
    pub const ADCL_EN: u32 = bit(1);
    pub const ADCR_EN: u32 = bit(0);
}

/// ADC_BIAS0: listed in the datasheet register table, not described. Vendor
/// `|=` [`VENDOR_BIAS0_SET`] \[code\].
pub const BIAS0: Reg = Reg::at(BASE + BIAS0_OFFSET);
/// ADC_BIAS1: listed, not described. Vendor `|=` [`VENDOR_BIAS1_SET`] \[code\].
pub const BIAS1: Reg = Reg::at(BASE + BIAS1_OFFSET);

/// VMIC_CTL: microphone bias outputs (pins via MFP function "VMIC0/1").
/// \[DS\] §8.2.6.19. Reset 0. The vendor never writes it \[code\].
pub const VMIC_CTL: Reg = Reg::at(BASE + VMIC_CTL_OFFSET);
pub mod vmic_ctl {
    use super::bit;
    pub const VMIC1_EN: u32 = bit(1);
    pub const VMIC0_EN: u32 = bit(0);
}

/// Unknown register read once by the vendor (A3:0x1586029C). Not in the
/// datasheet. \[code\]
pub const UNK_70: Reg = Reg::at(BASE + 0x70);

// ---- Datasheet tables and vendor values ----

/// Preamp gain in dB per `PREAM_PG` index with 6 kΩ input (differential;
/// single-ended is 6 dB lower). \[DS\] §8.2.6.18. Equals the vendor
/// `CONFIG.TXT` analogue-gain list (audio.md §3.2), so the vendor runs 6 kΩ.
pub const PREAMP_GAIN_DB_6K: [u8; 16] = [14, 17, 20, 21, 22, 23, 24, 25, 26, 27, 28, 29, 30, 31, 32, 33];
/// FIFO control for DMA capture (`0x13`: run, DRQ, output DMA). \[code\]
pub const VENDOR_FIFOCTL_DMA: u32 = fifoctl::FIFO_RST | fifoctl::FIFO_DEN | (fifoctl::OS_DMA << 4);
/// Bits the vendor clears before writing FIFOCTL (`0xB3`). \[code\]
pub const VENDOR_FIFOCTL_CLEAR: u32 =
    fifoctl::FIFO_DMAWIDTH | fifoctl::FIFO_OS.mask() | fifoctl::FIFO_DEN | fifoctl::FIFO_RST;
/// `AUX_CTL` byte presets: line-in (AUX0) `0xFA` = both on, single-ended,
/// 30 kΩ; FM (AUX1) `0xFB` = both on, single-ended, 6 kΩ (`0xCB`, R off, on
/// IC type 0x86B); byte 2 `0xFB`. audio.md §3.1 \[code\]
pub const VENDOR_AUX_LINEIN: u32 = 0xFA;
pub const VENDOR_AUX_FM: u32 = 0xFB;
pub const VENDOR_AUX_FM_86B: u32 = 0xCB;
pub const VENDOR_AUX_BYTE2: u32 = 0xFB;
/// `CTL[5:0]` switch sets: left converter path (`0x2A` = ADCL, VRDAL, PREOPL),
/// right (`0x15`). \[code\], decoded with \[DS\].
pub const VENDOR_CTL_LEFT: u32 = ctl::ADCL_EN | ctl::VRDAL_EN | ctl::PREOPL_EN;
pub const VENDOR_CTL_RIGHT: u32 = ctl::ADCR_EN | ctl::VRDAR_EN | ctl::PREOPR_EN;
/// `CTL` presets: FDBUF R on, 6 kΩ (`0x7040`); FDBUF L on, 6 kΩ (`0x70080`);
/// FM `0x770C0`; line-in `0x66000` (both IRS 30 kΩ, buffers off). \[code\]
/// The microphone front end sets the FDBUF of its side too (B1:0x0D8205D4:
/// left `0x70080`, right `0x7040`).
pub const VENDOR_CTL_FDBUF_R: u32 = ctl::FDBUFR_EN | (7 << 12);
pub const VENDOR_CTL_FDBUF_L: u32 = ctl::FDBUFL_EN | (7 << 16);
/// The microphone front end's `CTL` mask (`0xFFF8803F`): it clears both
/// FDBUFs, their resistors and `PREAM_PG` before setting them. VERBATIM
/// vendor B1:0x0D8205C0 \[code\].
pub const VENDOR_CTL_MIC_KEEP: u32 = 0xFFF8_803F;
/// `MIC_MIX_CTL` for the vendor's mono microphone (left): bits it clears
/// (`0x13F`), then `MICGRSEL` and `MICL_EN` = on (`0x10C`); `MIC_IN_MODE`
/// (`0x10`) on top when single-ended. VERBATIM vendor B1:0x0D82058C–0x5BC,
/// called with (left = 1, right = 0) for one channel, (1, 1) for two,
/// (0, 1) for "right only" (B1:0x0D8207F2–0x866) \[code\].
pub const VENDOR_MIC_CLEAR: u32 = 0x13F;
pub const VENDOR_MIC_LEFT: u32 = mic_mix_ctl::MICGRSEL | (3 << 2);
pub const VENDOR_MIC_RIGHT: u32 = mic_mix_ctl::MICGRSEL | 3;
pub const VENDOR_CTL_FM: u32 = 0x770C0;
pub const VENDOR_CTL_LINEIN: u32 = 0x66000;
/// ADC bias words OR-ed in at capture start. VERBATIM vendor B1:0x0D8207CA
/// (audio.md §3.4) \[code\].
pub const VENDOR_BIAS0_SET: u32 = 0x8411_A293;
pub const VENDOR_BIAS1_SET: u32 = 0x5F5;
/// Vendor default analogue gain index (mic 13, line-in 6, FM 10) and digital
/// `FGAIN` (mic 40, line-in 25). audio.md §3.2 \[code\]
pub const VENDOR_PG_MIC: u32 = 13;
pub const VENDOR_PG_LINEIN: u32 = 6;
pub const VENDOR_PG_FM: u32 = 10;
pub const VENDOR_FGAIN_MIC: u32 = 40;
pub const VENDOR_FGAIN_LINEIN: u32 = 25;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn plan_addresses() {
        // 01-hardware §4 / §6 and audio.md §3.4 (the port does not use the ADC).
        assert_eq!(BASE, 0xC018_1000);
        assert_eq!(ADC01_DAT_FIFO.addr(), 0xC018_1018);
        assert_eq!(GRPCTL.addr(), 0xC018_1004);
        assert_eq!(ADC01_FIFOCTL.addr(), 0xC018_1008);
        assert_eq!(CH0_DIGCTL.addr(), 0xC018_1024);
        assert_eq!(CH1_DIGCTL.addr(), 0xC018_1028);
        assert_eq!(MAPPING.addr(), 0xC018_1034);
        assert_eq!(CTL.addr(), 0xC018_1044);
        assert_eq!(BIAS0.addr(), 0xC018_1048);
        assert_eq!(BIAS1.addr(), 0xC018_104C);
    }

    #[test]
    fn datasheet_list() {
        let regs = [
            DIGCTL,
            GRPCTL,
            ADC01_FIFOCTL,
            ADC23_FIFOCTL,
            ADC01_FIFOSTA,
            ADC23_FIFOSTA,
            ADC01_DAT_FIFO,
            ADC23_DAT_FIFO,
            AAL_CTL,
            CH0_DIGCTL,
            CH1_DIGCTL,
            CH2_DIGCTL,
            CH3_DIGCTL,
            MAPPING,
            AUX_CTL,
            MIC_MIX_CTL,
            AEC_CTL,
            CTL,
            BIAS0,
            BIAS1,
            VMIC_CTL,
            UNK_70,
        ];
        assert_increasing(&regs.map(Reg::addr));
        assert_eq!(ADC23_DAT_FIFO.addr(), 0xC018_101C);
        assert_eq!(AAL_CTL.addr(), 0xC018_1020);
        assert_eq!(CH3_DIGCTL.addr(), 0xC018_1030);
        assert_eq!(AUX_CTL.addr(), 0xC018_1038);
        assert_eq!(MIC_MIX_CTL.addr(), 0xC018_103C);
        assert_eq!(AEC_CTL.addr(), 0xC018_1040);
        assert_eq!(VMIC_CTL.addr(), 0xC018_1054);
    }

    #[test]
    fn vendor_values_decompose() {
        assert_eq!(VENDOR_FIFOCTL_DMA, 0x13);
        assert_eq!(VENDOR_FIFOCTL_CLEAR, 0xB3);
        assert_eq!(VENDOR_CTL_LEFT, 0x2A);
        assert_eq!(VENDOR_CTL_RIGHT, 0x15);
        assert_eq!(VENDOR_CTL_FDBUF_R, 0x7040);
        assert_eq!(VENDOR_CTL_FDBUF_L, 0x70080);
        assert_eq!(VENDOR_CTL_FM, VENDOR_CTL_FDBUF_R | VENDOR_CTL_FDBUF_L);
        // The mic front end's mask clears exactly the FDBUFs, their
        // resistors and the preamp gain.
        let fd = VENDOR_CTL_FDBUF_R | VENDOR_CTL_FDBUF_L | ctl::FDBUFL_IRS.mask() | ctl::FDBUFR_IRS.mask();
        assert_eq!(!VENDOR_CTL_MIC_KEEP, fd | ctl::PREAM_PG.mask());
        assert_eq!(VENDOR_MIC_LEFT, 0x10C);
        assert_eq!(VENDOR_MIC_LEFT & mic_mix_ctl::MICL_EN.mask(), mic_mix_ctl::MICL_EN.mask());
        assert_eq!(VENDOR_MIC_RIGHT, 0x103);
        assert_eq!(VENDOR_MIC_CLEAR, 0x100 | mic_mix_ctl::MIC_IN_MODE | 0xF | mic_mix_ctl::MIC_IRMS);
        assert_eq!(VENDOR_CTL_LINEIN, ctl::FDBUFL_IRS.val(6) | ctl::FDBUFR_IRS.val(6));
        let a = aux_ctl::AUX0L_EN.val(3) | aux_ctl::AUX0R_EN.val(3) | aux_ctl::AUX0_IN_MODE | 2;
        assert_eq!(a, VENDOR_AUX_LINEIN);
        let f = aux_ctl::AUX1L_EN.val(3) | aux_ctl::AUX1R_EN.val(3) | aux_ctl::AUX1_IN_MODE | (3 << 8);
        assert_eq!(f >> 8, VENDOR_AUX_FM);
        assert_eq!(mapping::CH0_MAP.val(4) | mapping::CH1_MAP.val(5), 0x54);
        assert_eq!(chdigctl::HPF_EN | chdigctl::DATA_OUT_EN, 0x18_0000);
        assert_eq!(chdigctl::FGAIN_MAX, 59);
    }

    #[test]
    fn fields_disjoint() {
        use crate::Field;
        let m = Field::mask;
        {
            use grpctl::*;
            assert_disjoint(&[
                m(DMIC_PRE_GAIN),
                m(ADC23_OVFS_SEL),
                m(ADC23_FIR_MD_SEL),
                m(ADC01_OVFS_SEL),
                m(ADC01_FIR_MD_SEL),
            ]);
        }
        {
            use fifoctl::*;
            assert_disjoint(&[FIFO_DMAWIDTH, m(FIFO_OS), FIFO_IEN, FIFO_DEN, FIFO_RST]);
        }
        {
            use fifosta::*;
            assert_disjoint(&[FIFO_ER, FIFO_IP, FIFO_EMPTY, m(FIFO_STATUS)]);
        }
        {
            use aal_ctl::*;
            assert_disjoint(&[
                AAL_EN,
                AAL_RECOVER_MODE,
                m(AAL_FL),
                m(AAL_VT1),
                m(AAL_VT0),
                m(AAL_MAX),
                m(AAL_CNT),
            ]);
        }
        {
            use chdigctl::*;
            assert_disjoint(&[
                AAL_CH_EN,
                DATA_OUT_EN,
                HPF_EN,
                HPF_S,
                m(HPF_N),
                MIC_SEL,
                m(SGAIN),
                ZERO_CROSS_EN,
                m(FGAIN),
            ]);
        }
        {
            use mapping::*;
            assert_disjoint(&[m(CH3_MAP), m(CH2_MAP), m(CH1_MAP), m(CH0_MAP)]);
        }
        {
            use aux_ctl::*;
            assert_disjoint(&[
                m(UNK_B23_16),
                m(AUX1L_EN),
                m(AUX1R_EN),
                AUX1_IN_MODE,
                AUX1LRMIX,
                m(AUX1_IRS),
                m(AUX0L_EN),
                m(AUX0R_EN),
                AUX0_IN_MODE,
                AUX0LRMIX,
                m(AUX0_IRS),
            ]);
        }
        {
            use mic_mix_ctl::*;
            assert_disjoint(&[
                PAR2ADL_EN,
                PAL2ADL_EN,
                PAR2ADR_EN,
                PAL2ADR_EN,
                PAR_PD_EN,
                PAL_PD_EN,
                MICGRSEL,
                m(GMIRSEL),
                MIC_IRMS,
                MIC_IN_MODE,
                m(MICL_EN),
                m(MICR_EN),
            ]);
        }
        {
            use aec_ctl::*;
            assert_disjoint(&[
                AECCLK_DIV,
                AECADC_BINV,
                AECVRDA_EN,
                ADC_VAD_EN,
                AEC_DSS,
                AECICCEN,
                m(AEC_GAIN),
                AECL_PDEN,
                AECR_PDEN,
                AECL_EN,
                AECR_EN,
            ]);
        }
        {
            use ctl::*;
            assert_disjoint(&[
                ADCCLK_DIV,
                BIASEN,
                ADR_CAPFC_EN,
                ADL_CAPFC_EN,
                ADCL_BINV,
                ADCR_BINV,
                m(FDBUFL_IRS),
                m(FDBUFR_IRS),
                m(PREAM_PG),
                FDBUFL_EN,
                FDBUFR_EN,
                PREOPL_EN,
                PREOPR_EN,
                VRDAL_EN,
                VRDAR_EN,
                ADCL_EN,
                ADCR_EN,
            ]);
        }
        assert_disjoint(&[digctl::CH3_DIG_EN, digctl::CH2_DIG_EN, digctl::CH1_DIG_EN, digctl::CH0_DIG_EN]);
        assert_disjoint(&[vmic_ctl::VMIC1_EN, vmic_ctl::VMIC0_EN]);
    }
}
