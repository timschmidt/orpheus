//! DAC, headphone amplifier (PA) and analogue mixer, `0xC0180000`
//! (datasheet §8.1).
//!
//! Sources: datasheet §8.1 (digital part, offsets `0x00..=0x24`); docs/hw/audio.md
//! §2 and §4 (vendor `AUD_DEV.DRV`); the Rockbox port `pcm-yotonym03.c` and the
//! payloads `audiotest*.c` / `dmabeep.c`, which play through this block \[HW\].
//!
//! The datasheet register list stops at `0x24`. The analogue registers the
//! vendor and the port use at `0x28` (PA control), `0x2C` (anti-pop switches,
//! FM/line-in bypass) and `0x30` (bias) are **not in the datasheet**; their
//! addresses are \[HW\] (the port's power-up writes them), most bit roles \[code\]
//! or unknown.
//!
//! FIFO data for DMA: [`DAT_FIFO0`] `0xC018000C` (DMA device code `0x0B`,
//! datasheet §5.3) and [`DAT_FIFO1`] `0xC0180010` (device code `0x0C`).
//! Interrupt: NVIC line 39 (datasheet IRQ table). Clock: `cmu::DACCLK`; gates
//! CLKEN1 b2 (DAC) and b11 (PA) — see the `cmu` module.
//!
//! Unknown: roles of most `PA_CTL`/`PA_ANTIPOP`/`BIAS` bits (U-FM-3), PA gain
//! step size (U-AUD-1), effect of `FIFOCTL.DMAWIDTH` with 16-bit DMA.

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC018_0000;

pub const DIGCTL_OFFSET: usize = 0x00;
pub const FIFOCTL_OFFSET: usize = 0x04;
pub const FIFOSTA_OFFSET: usize = 0x08;
pub const DAT_FIFO0_OFFSET: usize = 0x0C;
pub const DAT_FIFO1_OFFSET: usize = 0x10;
pub const VOL_LCH_OFFSET: usize = 0x14;
pub const VOL_RCH_OFFSET: usize = 0x18;
pub const PDMTX_CTL_OFFSET: usize = 0x1C;
pub const FIFO0_CNT_OFFSET: usize = 0x20;
pub const FIFO1_CNT_OFFSET: usize = 0x24;
pub const PA_CTL_OFFSET: usize = 0x28;
pub const PA_ANTIPOP_OFFSET: usize = 0x2C;
pub const BIAS_OFFSET: usize = 0x30;

/// DAC_DIGCTL: digital path control. \[DS\] §8.1.4.1; port writes
/// `DDEN | ENDITH | DAF0M2DAEN | SDM_EN` \[HW\]. Reset `0x0008_0208`.
pub const DIGCTL: Reg = Reg::at(BASE + DIGCTL_OFFSET);
pub mod digctl {
    use super::{Field, bit};
    /// Auto-mute enable. \[DS\]
    pub const AMEN: u32 = bit(27);
    /// Auto-mute detected (read-only). \[DS\]
    pub const AMDP: u32 = bit(26);
    /// Auto-mute window: 0 512, 1 1024, 2 2048, 3 4096 samples. \[DS\]
    pub const AMCNT: Field = Field::bits(25, 24);
    /// Multi-device linkage: 0 off, 1 DAC with I2STX0. \[DS\]; vendor I2S
    /// loopback path sets 1 (audio.md §2.1 "b21 set, b22 cleared") \[code\].
    pub const MULT_DEVICE: Field = Field::bits(22, 21);
    pub const MULT_DEVICE_I2STX0: u32 = 1;
    /// SDM mute count: 0 256, 1 512. \[DS\]
    pub const SDMCNT: u32 = bit(20);
    /// SDM noise threshold: 0 −84, 1 −78, 2 −72 (reset), 3 −66 dBFS. \[DS\]
    pub const SDMNDTH: Field = Field::bits(19, 18);
    /// Reset the SDM after detected noise. \[DS\]
    pub const SDMREEN: u32 = bit(17);
    /// 1: a 128fs and a 256fs device (SPDIFTX with I2STX/DAC) run at once. \[DS\]
    pub const AUDIO_128FS_256FS: u32 = bit(16);
    /// ADC CH0/CH2 → DAC left digital mix. \[DS\]; vendor ADC-loopback path
    /// \[code\] (audio.md §4).
    pub const AD2DALPEN_L: u32 = bit(12);
    /// ADC CH1/CH3 → DAC right digital mix. \[DS\]
    pub const AD2DALPEN_R: u32 = bit(11);
    /// Mix ADC CH0/CH2 with CH1/CH3. \[DS\]
    pub const ADC_LRMIX: u32 = bit(10);
    /// ADC pair mixed into the DAC: 0 CH0/CH1, 1 CH2/CH3 (reset 1). \[DS\]
    pub const AD2DA_CHSEL: u32 = bit(9);
    /// Sigma-delta modulator enable. \[DS\]; set by port and vendor \[HW\].
    pub const SDM_EN: u32 = bit(8);
    /// Mix FIFO1 into the DAC (second stream, hardware-summed). \[DS\]
    pub const DAF1M2DAEN: u32 = bit(5);
    /// Mix FIFO0 into the DAC. \[DS\] \[HW\] (audio.md: "FIFO path enable").
    pub const DAF0M2DAEN: u32 = bit(4);
    /// Oversampling: 0 16×, 1 32×, 2 64× (reset), 3 128×. \[DS\]; vendor uses
    /// 2, and 1 at 96 kHz \[code\].
    pub const OSRDA: Field = Field::bits(3, 2);
    pub const OSRDA_32X: u32 = 1;
    pub const OSRDA_64X: u32 = 2;
    /// Dither enable. \[DS\]; set by the port \[HW\]. **Disagreement:** audio.md
    /// §2.1 calls b1 a "second enable"; the datasheet says dither.
    pub const ENDITH: u32 = bit(1);
    /// DAC digital enable. \[DS\] \[HW\]
    pub const DDEN: u32 = bit(0);
}

/// DAC_FIFOCTL. \[DS\] §8.1.4.2. Reset 0. Port: `0x01` CPU-fed during the
/// anti-pop ramp, then `|= 0x12` for DMA \[HW\]. **Disagreement:** audio.md §2.1
/// reads b2:1 as input select and b4 as DMA request; the datasheet has b1 DRQ
/// enable, b2 IRQ enable, b5:4 input select (`0x12` = DRQ + input DMA).
pub const FIFOCTL: Reg = Reg::at(BASE + FIFOCTL_OFFSET);
pub mod fifoctl {
    use super::{Field, bit};
    /// FIFO1 input: [`IS_CPU`] / [`IS_DMA`]. \[DS\]
    pub const DAF1_IS: Field = Field::bits(13, 12);
    /// FIFO1 half-empty IRQ enable. \[DS\]
    pub const DAF1_IEN: u32 = bit(10);
    /// FIFO1 half-empty DMA request enable. \[DS\]
    pub const DAF1_DEN: u32 = bit(9);
    /// FIFO1 run: 0 holds the FIFO in reset. \[DS\]
    pub const DAF1_RST: u32 = bit(8);
    /// DMA width of both FIFOs: 0 32-bit, 1 16-bit (match the DMA width).
    /// \[DS\]. Vendor sets it for 16-bit samples \[code\]; the port leaves 0 with
    /// 16-bit DMA mode `0x00148B00` and plays correctly \[HW\].
    pub const DMAWIDTH: u32 = bit(7);
    /// FIFO0 input: [`IS_CPU`] / [`IS_DMA`]. \[DS\] \[HW\]
    pub const DAF0_IS: Field = Field::bits(5, 4);
    pub const IS_CPU: u32 = 0;
    pub const IS_DMA: u32 = 1;
    /// FIFO0 half-empty IRQ enable (IRQ 39). \[DS\]
    pub const DAF0_IEN: u32 = bit(2);
    /// FIFO0 half-empty DMA request enable. \[DS\] \[HW\]
    pub const DAF0_DEN: u32 = bit(1);
    /// FIFO0 run: 0 holds the FIFO in reset. \[DS\] \[HW\]
    pub const DAF0_RST: u32 = bit(0);
}

/// DAC_FIFOSTA. \[DS\] §8.1.4.3. Reset 0. CPU feeding (port, vendor): poll
/// `DAF0_IP`, write two words, write `DAF0_IP` back to clear \[HW\].
/// **Disagreement:** audio.md §2.1 reads b7 as "FIFO can take a word"; the
/// datasheet: half-empty pending, i.e. at least 16 of 32 levels free.
pub const FIFOSTA: Reg = Reg::at(BASE + FIFOSTA_OFFSET);
pub mod fifosta {
    use super::{Field, bit};
    /// FIFO1 error (read while empty); W1C. \[DS\]
    pub const DAF1_ER: u32 = bit(24);
    /// FIFO1 half-empty pending; W1C. \[DS\]
    pub const DAF1_IP: u32 = bit(23);
    pub const DAF1_FULL: u32 = bit(22);
    /// FIFO1 free sample pairs (0 without DAC clock). \[DS\]
    pub const DAF1_STATUS: Field = Field::bits(20, 16);
    /// FIFO0 error; W1C. \[DS\]
    pub const DAF0_ER: u32 = bit(8);
    /// FIFO0 half-empty pending; W1C. \[DS\] \[HW\]
    pub const DAF0_IP: u32 = bit(7);
    pub const DAF0_FULL: u32 = bit(6);
    /// FIFO0 free sample pairs (6 = room for 12 words). \[DS\]
    pub const DAF0_STATUS: Field = Field::bits(4, 0);
}

/// DAC_DAT_FIFO0 (write-only), DMA destination `0xC018000C` \[HW\]. 24-bit
/// samples left-justified in `[31:8]`, 32 levels, written L then R. \[DS\] §8.1.4.4
pub const DAT_FIFO0: Reg = Reg::at(BASE + DAT_FIFO0_OFFSET);
/// DAC_DAT_FIFO1 (write-only): second stream, summed with FIFO0 when
/// `DIGCTL.DAF1M2DAEN` is set. Unused by vendor and port. \[DS\] §8.1.4.5
pub const DAT_FIFO1: Reg = Reg::at(BASE + DAT_FIFO1_OFFSET);
pub mod dat {
    use super::Field;
    /// Sample bits. \[DS\]
    pub const DAT: Field = Field::bits(31, 8);
}

/// VOL_LCH: left digital volume. \[DS\] §8.1.4.6 (fields in [`vol`]). Reset
/// `0x0003_08BF`.
pub const VOL_LCH: Reg = Reg::at(BASE + VOL_LCH_OFFSET);
/// VOL_RCH: right digital volume. \[DS\] §8.1.4.7 (fields in [`vol`]).
pub const VOL_RCH: Reg = Reg::at(BASE + VOL_RCH_OFFSET);
/// Fields of [`VOL_LCH`] and [`VOL_RCH`] (identical layout).
pub mod vol {
    use super::{Field, bit};
    /// Soft-step period unit: 0 8·ADJ_CNT, 1 128·ADJ_CNT. \[DS\]
    pub const TO_CNT: u32 = bit(20);
    /// Soft-step timing, "set consistent with the sample rate" (reset 0x30 =
    /// 48 kHz). \[DS\]; vendor and port write 0x33 \[HW\].
    pub const ADJ_CNT: Field = Field::bits(19, 12);
    /// Soft step reached target (read-only, reset 1). \[DS\]
    pub const SOFT_STEP_DONE: u32 = bit(11);
    /// Soft-stepping enable (datasheet suggests ZCEN with it). \[DS\] \[HW\]
    pub const SOFT_STEP_EN: u32 = bit(10);
    /// Zero-cross timeout (apply after 100 ms at 48 kHz). \[DS\] \[HW\]
    pub const ZCTOEN: u32 = bit(9);
    /// Apply volume changes at a zero crossing. \[DS\] \[HW\]
    pub const ZCEN: u32 = bit(8);
    /// Volume, 0.375 dB/step: `0xFF` +24 dB, [`VOL_0DB`] 0 dB, `0x00` mute.
    /// \[DS\] (answers U-AUD-1 on paper).
    pub const VOL: Field = Field::bits(7, 0);
    pub const VOL_0DB: u32 = 0xBF;
    pub const VOL_MUTE: u32 = 0;
    /// Step size in millidecibels. \[DS\]
    pub const VOL_STEP_MDB: u32 = 375;
}

/// PDMTX_CTL: PDM output (pins). Unused on this board. \[DS\] §8.1.4.8
pub const PDMTX_CTL: Reg = Reg::at(BASE + PDMTX_CTL_OFFSET);
pub mod pdmtx_ctl {
    use super::{Field, bit};
    pub const DMIC_BYPASS: u32 = bit(14);
    /// 0 off (hi-Z), 1 ch0, 2 ch1, 3 both. \[DS\]
    pub const PDMTX_CH_SET: Field = Field::bits(13, 12);
    /// 0 DAC, 1 LPADC bypass, 2 DMIC01, 3 DMIC23. \[DS\]
    pub const PDMTX_SRC: Field = Field::bits(9, 8);
    pub const PDM1_ARST_EN: u32 = bit(3);
    pub const PDM0_ARST_EN: u32 = bit(2);
    pub const PDM1_EN: u32 = bit(1);
    pub const PDM0_EN: u32 = bit(0);
}

/// DAC_FIFO0_CNT: samples (pairs) played from FIFO0. \[DS\] §8.1.4.9
pub const FIFO0_CNT: Reg = Reg::at(BASE + FIFO0_CNT_OFFSET);
/// DAC_FIFO1_CNT. \[DS\] §8.1.4.10
pub const FIFO1_CNT: Reg = Reg::at(BASE + FIFO1_CNT_OFFSET);
/// Fields of [`FIFO0_CNT`] / [`FIFO1_CNT`].
pub mod cnt {
    use super::{Field, bit};
    /// Overflow pending; W1C. \[DS\]
    pub const IP: u32 = bit(18);
    /// Overflow interrupt enable. \[DS\]
    pub const IE: u32 = bit(17);
    /// Counter enable; 0 resets the count. \[DS\]
    pub const EN: u32 = bit(16);
    pub const CNT: Field = Field::bits(15, 0);
}

/// PA_CTL: headphone amplifier control. **Not in the datasheet**; address
/// \[HW\] (port power-up, `dacgate.c` read-back probe), bits \[code\] from
/// audio.md §2.4 unless noted.
pub const PA_CTL: Reg = Reg::at(BASE + PA_CTL_OFFSET);
pub mod pa_ctl {
    use super::{Field, bit};
    /// Set at enable; meaning unknown. \[code\]
    pub const UNK_B27: u32 = bit(27);
    /// Set after the ramp with `UNK_B17_16 = 1`. \[code\] \[HW\]
    pub const UNK_B18: u32 = bit(18);
    /// Set to 1 after the ramp; cleared at enable. \[code\] \[HW\]
    pub const UNK_B17_16: Field = Field::bits(17, 16);
    /// PA gain 0–7; vendor music value 7. Step size unknown (U-AUD-1). \[code\]
    pub const PA_VOL: Field = Field::bits(14, 12);
    /// Set from the enable call's second argument (0 → 1). \[code\]
    pub const UNK_B10: u32 = bit(10);
    /// Output stage on (value 3): cleared first, set last in power-up; likely a
    /// click-free mute \[inferred\]. \[code\] \[HW\]
    pub const OUT_EN: Field = Field::bits(9, 8);
    pub const OUT_EN_ON: u32 = 3;
    /// Analogue bias / master enable: first set, last cleared. \[code\] \[HW\]
    pub const BIAS_EN: u32 = bit(7);
    /// VRO (virtual-ground amplifier) for direct drive; pin VRO = GPIO73
    /// \[DS\] pin list. \[code\] \[HW\]
    pub const VRO_EN: u32 = bit(6);
    /// Stage B / output connect (b3, b5), set late in the ramp. \[code\] \[HW\]
    pub const STAGE_B: u32 = bit(3) | bit(5);
    /// Stage A enables (b2, b4), set with `CH_EN`. \[code\] \[HW\]
    pub const STAGE_A: u32 = bit(2) | bit(4);
    /// Channel enables (value 3). \[code\] \[HW\]
    pub const CH_EN: Field = Field::bits(1, 0);
}

/// PA_ANTIPOP: anti-pop switch pairs and the analogue FM/line-in → PA bypass.
/// **Not in the datasheet**; address \[HW\] (port), bits \[code\] (audio.md §2.4,
/// §4). Bypass bits were exercised by `audiotest.c` (variant bits 0/1).
pub const PA_ANTIPOP: Reg = Reg::at(BASE + PA_ANTIPOP_OFFSET);
pub mod pa_antipop {
    use super::{Field, bit};
    /// FM bypass field, vendor writes 2; meaning unknown (U-FM-3). \[code\]
    pub const UNK_B23_22: Field = Field::bits(23, 22);
    /// FM bypass pre-charge (set, 100 ms, clear). \[code\]
    pub const FM_PRECHARGE: u32 = bit(21);
    /// FM → PA analogue bypass enable. Which pins: likely AUX1 (vendor's FM
    /// capture preset is the AUX1 byte of `adc::AUX_CTL`) \[inferred\]. \[code\]
    pub const FM_BYPASS_EN: u32 = bit(20);
    /// Line-in bypass field, vendor writes 2; meaning unknown. \[code\]
    pub const UNK_B19_18: Field = Field::bits(19, 18);
    /// Line-in bypass pre-charge (set, 200 ms, clear). \[code\]
    pub const LINEIN_PRECHARGE: u32 = bit(17);
    /// Line-in → PA bypass enable (likely AUX0 \[inferred\]). \[code\]
    pub const LINEIN_BYPASS_EN: u32 = bit(16);
    /// Value written to the 2-bit bypass fields. \[code\]
    pub const BYPASS_FIELD_VENDOR: u32 = 2;
    /// Switch pair 5 (b5, b13); not used by the direct-drive sequence. \[code\]
    pub const SW5: u32 = bit(5) | bit(13);
    /// Switch pair 4 (b4, b12). \[code\] \[HW\]
    pub const SW4: u32 = bit(4) | bit(12);
    /// Switch pair 3 (b3, b11): stays set while the PA is on. \[code\] \[HW\]
    pub const SW3: u32 = bit(3) | bit(11);
    /// Switch pair 2 (b2, b10). \[code\] \[HW\]
    pub const SW2: u32 = bit(2) | bit(10);
    /// Switch pair 1 (b1, b9). \[code\] \[HW\]
    pub const SW1: u32 = bit(1) | bit(9);
    /// Switch pair 0 (b0, b8). \[code\] \[HW\]
    pub const SW0: u32 = bit(0) | bit(8);
}

/// DAC analogue bias (audio.md name "DAC_BIAS"). **Not in the datasheet.**
/// Vendor stream start sets `[14:12] = 4`, `[26:24] = 6`; the port does not
/// write it. \[code\]
pub const BIAS: Reg = Reg::at(BASE + BIAS_OFFSET);
pub mod bias {
    use super::Field;
    pub const UNK_B26_24: Field = Field::bits(26, 24);
    pub const UNK_B14_12: Field = Field::bits(14, 12);
    pub const UNK_B26_24_VENDOR: u32 = 6;
    pub const UNK_B14_12_VENDOR: u32 = 4;
}

// ---- Vendor / port values (provenance in each comment; no driver code) ----

/// `DIGCTL` bits the vendor direct-drive power-up sets (`DDEN`, `SDM_EN`,
/// `DAF0M2DAEN`). audio.md §2.5 \[code\]; port adds [`digctl::ENDITH`] \[HW\].
pub const VENDOR_DIGCTL_ON: u32 = digctl::DDEN | digctl::SDM_EN | digctl::DAF0M2DAEN;
/// Port power-up `DIGCTL` bits (`0x113`). `pcm-yotonym03.c` \[HW\]
pub const PORT_DIGCTL_ON: u32 = VENDOR_DIGCTL_ON | digctl::ENDITH;
/// `FIFOCTL` bits cleared for CPU feeding during the ramp (`0x36`). Port \[HW\]
pub const PORT_FIFOCTL_CPU_CLEAR: u32 = fifoctl::DAF0_DEN | fifoctl::DAF0_IEN | fifoctl::DAF0_IS.mask();
/// `FIFOCTL |= 0x12` after the ramp: DMA request + DMA input. Port, vendor \[HW\]
pub const VENDOR_FIFOCTL_DMA: u32 = fifoctl::DAF0_DEN | (fifoctl::IS_DMA << 4);
/// Volume control bits written with every volume change (`0x33700`): ADJ_CNT
/// 0x33 plus ZCEN, ZCTOEN, SOFT_STEP_EN. audio.md §2.3 \[code\], port \[HW\].
pub const VENDOR_VOL_RAMP: u32 = (0x33 << 12) | vol::SOFT_STEP_EN | vol::ZCTOEN | vol::ZCEN;
/// Bits the port masks off before writing a volume (`0xFF8FF`). \[HW\]
pub const PORT_VOL_CLEAR: u32 = vol::ADJ_CNT.mask() | vol::SOFT_STEP_DONE | vol::VOL.mask();
/// Port's fixed digital volume `0xC0` (= +0.375 dB per the datasheet; the
/// port comment calls it "analog level"). \[HW\]
pub const PORT_VOL: u32 = 0xC0;
/// Vendor 32-step user volume → `VOL` code (`MENGINE.AP` file 0x6E7A). Tops
/// out at 0xBF = 0 dB; step 1 = 0x2F = −54 dB. audio.md §2.3 \[code\]
pub const VENDOR_VOL_TABLE: [u8; 32] = [
    0x00, 0x2F, 0x37, 0x3F, 0x47, 0x4F, 0x57, 0x5F, 0x6F, 0x77, 0x7F, 0x84, 0x87, 0x8A, 0x8F, 0x92, 0x94,
    0x97, 0x9A, 0x9C, 0x9F, 0xA0, 0xA2, 0xA4, 0xA6, 0xA8, 0xAA, 0xAC, 0xAF, 0xB2, 0xB7, 0xBF,
];
/// Vendor FM volume: 32 steps of {tuner volume, `PA_VOL`} (`FMENGINE.AP`
/// file 0x2D04), split into two arrays. audio.md §4 \[code\]
pub const VENDOR_FM_TUNER_VOL: [u8; 32] = [
    0x0, 0x1, 0x1, 0x1, 0x1, 0x2, 0x3, 0x4, 0x4, 0x4, 0x4, 0x5, 0x5, 0x6, 0x6, 0x7, 0x7, 0x8, 0x8, 0xB, 0x9,
    0xB, 0xB, 0xC, 0xE, 0xC, 0xD, 0xD, 0xE, 0xE, 0xF, 0xF,
];
pub const VENDOR_FM_PA_VOL: [u8; 32] =
    [0, 1, 4, 6, 7, 6, 5, 4, 5, 6, 7, 6, 7, 6, 7, 6, 7, 6, 7, 5, 7, 6, 7, 6, 5, 7, 6, 7, 6, 7, 6, 7];
/// PA gain for music. audio.md §2.5 \[code\]
pub const VENDOR_PA_VOL_MUSIC: u32 = 7;
/// `PA_CTL` set at enable (`0x0800_0080`) / cleared (`0x37000`). audio.md
/// §2.5 \[code\]; `audiotest.c` replays it.
pub const VENDOR_PA_CTL_ENABLE_SET: u32 = pa_ctl::BIAS_EN | pa_ctl::UNK_B27;
pub const VENDOR_PA_CTL_ENABLE_CLEAR: u32 = pa_ctl::PA_VOL.mask() | pa_ctl::UNK_B17_16.mask();
/// Direct-drive power-up, early `PA_CTL` bits (`0x57`). VERBATIM vendor
/// (audio.md §2.5), replayed by the port \[HW\].
pub const VENDOR_PA_CTL_EARLY: u32 = pa_ctl::CH_EN.mask() | pa_ctl::STAGE_A | pa_ctl::VRO_EN;
/// `PA_CTL` without anti-pop (`0x3BF`, `| VRO_EN` for direct drive). \[code\]
pub const VENDOR_PA_CTL_NO_ANTIPOP: u32 =
    pa_ctl::OUT_EN.mask() | pa_ctl::BIAS_EN | pa_ctl::STAGE_A | pa_ctl::STAGE_B | pa_ctl::CH_EN.mask();
/// Final `PA_CTL` bits after the ramp: `UNK_B17_16 = 1`, `UNK_B18`, `OUT_EN`.
/// VERBATIM vendor, port \[HW\].
pub const VENDOR_PA_CTL_LATE: u32 = (1 << 16) | pa_ctl::UNK_B18 | pa_ctl::OUT_EN.mask();
/// Anti-pop ramp: hold blocks of 192 FIFO writes, 4800-step ramps of the
/// FIFO value between `0x7FFF_FFFF` and ~0 with step `0xFFFF_FFFF / 4800`.
/// VERBATIM vendor B5:0x0D8A0400 (audio.md §2.5), port \[HW\].
pub const VENDOR_ANTIPOP_HOLD_WRITES: u32 = 192;
pub const VENDOR_ANTIPOP_RAMP_STEPS: u32 = 4800;
pub const VENDOR_ANTIPOP_START: u32 = 0x7FFF_FFFF;
pub const VENDOR_ANTIPOP_STEP: u32 = 0xFFFF_FFFF / 4800;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        // pcm-yotonym03.c, audiotest*.c, dmabeep.c, dacgate.c.
        assert_eq!(BASE, 0xC018_0000);
        assert_eq!(DIGCTL.addr(), 0xC018_0000);
        assert_eq!(FIFOCTL.addr(), 0xC018_0004);
        assert_eq!(FIFOSTA.addr(), 0xC018_0008);
        assert_eq!(DAT_FIFO0.addr(), 0xC018_000C); // also the DMA destination
        assert_eq!(DAT_FIFO1.addr(), 0xC018_0010); // read back by audiotest.c
        assert_eq!(VOL_LCH.addr(), 0xC018_0014);
        assert_eq!(VOL_RCH.addr(), 0xC018_0018);
        assert_eq!(PA_CTL.addr(), 0xC018_0028);
        assert_eq!(PA_ANTIPOP.addr(), 0xC018_002C);
        assert_eq!(BIAS.addr(), 0xC018_0030);
    }

    #[test]
    fn datasheet_list() {
        let regs = [
            DIGCTL, FIFOCTL, FIFOSTA, DAT_FIFO0, DAT_FIFO1, VOL_LCH, VOL_RCH, PDMTX_CTL, FIFO0_CNT,
            FIFO1_CNT, PA_CTL, PA_ANTIPOP, BIAS,
        ];
        assert_increasing(&regs.map(Reg::addr));
        assert_eq!(PDMTX_CTL.addr(), 0xC018_001C);
        assert_eq!(FIFO0_CNT.addr(), 0xC018_0020);
        assert_eq!(FIFO1_CNT.addr(), 0xC018_0024);
    }

    #[test]
    fn port_values_decompose() {
        assert_eq!(PORT_DIGCTL_ON, 0x113);
        assert_eq!(PORT_FIFOCTL_CPU_CLEAR, 0x36);
        assert_eq!(VENDOR_FIFOCTL_DMA, 0x12);
        assert_eq!(VENDOR_VOL_RAMP, 0x33700);
        assert_eq!(PORT_VOL_CLEAR, 0xFF8FF);
        assert_eq!(VENDOR_PA_CTL_ENABLE_SET, 0x0800_0080);
        assert_eq!(VENDOR_PA_CTL_ENABLE_CLEAR, 0x37000);
        assert_eq!(VENDOR_PA_CTL_EARLY, 0x03 | 0x14 | 0x40);
        assert_eq!(pa_ctl::STAGE_B, 0x28);
        assert_eq!(VENDOR_PA_CTL_NO_ANTIPOP, 0x3BF);
        assert_eq!(VENDOR_PA_CTL_LATE, 0x10000 | 0x40000 | 0x300);
        assert_eq!(pa_antipop::SW0 | pa_antipop::SW4, 0x101 | 0x1010);
        assert_eq!(pa_antipop::SW1 | pa_antipop::SW2, 0x202 | 0x404);
        assert_eq!(pa_antipop::SW3, 0x808);
        assert_eq!(pa_antipop::FM_PRECHARGE, 0x20_0000);
        assert_eq!(pa_antipop::UNK_B23_22.val(2) | pa_antipop::FM_BYPASS_EN, 0x90_0000);
        assert_eq!(pa_antipop::UNK_B19_18.val(2) | pa_antipop::LINEIN_BYPASS_EN, 0x9_0000);
        assert_eq!(vol::VOL_0DB, u32::from(VENDOR_VOL_TABLE[31]));
        assert_eq!(VENDOR_ANTIPOP_STEP, 894_784);
    }

    #[test]
    fn fields_disjoint() {
        {
            use digctl::*;
            assert_disjoint(&[
                AMEN,
                AMDP,
                AMCNT.mask(),
                MULT_DEVICE.mask(),
                SDMCNT,
                SDMNDTH.mask(),
                SDMREEN,
                AUDIO_128FS_256FS,
                AD2DALPEN_L,
                AD2DALPEN_R,
                ADC_LRMIX,
                AD2DA_CHSEL,
                SDM_EN,
                DAF1M2DAEN,
                DAF0M2DAEN,
                OSRDA.mask(),
                ENDITH,
                DDEN,
            ]);
        }
        {
            use fifoctl::*;
            assert_disjoint(&[
                DAF1_IS.mask(),
                DAF1_IEN,
                DAF1_DEN,
                DAF1_RST,
                DMAWIDTH,
                DAF0_IS.mask(),
                DAF0_IEN,
                DAF0_DEN,
                DAF0_RST,
            ]);
        }
        {
            use fifosta::*;
            assert_disjoint(&[
                DAF1_ER,
                DAF1_IP,
                DAF1_FULL,
                DAF1_STATUS.mask(),
                DAF0_ER,
                DAF0_IP,
                DAF0_FULL,
                DAF0_STATUS.mask(),
            ]);
        }
        assert_disjoint(&[
            vol::TO_CNT,
            vol::ADJ_CNT.mask(),
            vol::SOFT_STEP_DONE,
            vol::SOFT_STEP_EN,
            vol::ZCTOEN,
            vol::ZCEN,
            vol::VOL.mask(),
        ]);
        {
            use pdmtx_ctl::*;
            assert_disjoint(&[
                DMIC_BYPASS,
                PDMTX_CH_SET.mask(),
                PDMTX_SRC.mask(),
                PDM1_ARST_EN,
                PDM0_ARST_EN,
                PDM1_EN,
                PDM0_EN,
            ]);
        }
        assert_disjoint(&[cnt::IP, cnt::IE, cnt::EN, cnt::CNT.mask()]);
        {
            use pa_ctl::*;
            assert_disjoint(&[
                UNK_B27,
                UNK_B18,
                UNK_B17_16.mask(),
                PA_VOL.mask(),
                UNK_B10,
                OUT_EN.mask(),
                BIAS_EN,
                VRO_EN,
                STAGE_B,
                STAGE_A,
                CH_EN.mask(),
            ]);
        }
        {
            use pa_antipop::*;
            assert_disjoint(&[
                UNK_B23_22.mask(),
                FM_PRECHARGE,
                FM_BYPASS_EN,
                UNK_B19_18.mask(),
                LINEIN_PRECHARGE,
                LINEIN_BYPASS_EN,
                SW5,
                SW4,
                SW3,
                SW2,
                SW1,
                SW0,
            ]);
        }
        assert_disjoint(&[bias::UNK_B26_24.mask(), bias::UNK_B14_12.mask()]);
    }
}
