//! PMU and the low-rate ADCs, `0xC0010000` (datasheet §4.1, §9.3).
//!
//! **Regulator and charger fields are dangerous** (a lithium cell is
//! attached): only values the stock firmware uses may be written
//! (CLAUDE.md rule 3; docs/hw/system.md §3; 01-hardware §7). This module
//! defines addresses, fields and the vendor's values as `VENDOR_*`
//! constants with their provenance; it contains no driver code. Writers live
//! in the HAL's `pmu` driver.
//!
//! Registers `+0x0C` and `+0x18` are used by vendor code but absent from the
//! datasheet (marked `VERBATIM` where values are kept).
//!
//! Unknown: end-voltage values the stock firmware leaves (U-PMU-1), S3
//! standby (U-PMU-2), wake behaviour on this board (U-PMU-3).

use crate::{Field, Reg, bit};

pub const BASE: usize = 0xC001_0000;

/// VOUT_CTL — regulator voltages. \[DS\] §4.1.4.1. The ROM-state recreation
/// sets USBVCC_EN then USBVDD_EN \[HW\].
pub const VOUT_CTL: Reg = Reg::at(BASE);
pub mod vout {
    use super::{Field, bit};
    /// USB 3.3 V supply enable (reset 1).
    pub const USBVCC_EN: u32 = bit(29);
    /// USB core supply enable (reset 1).
    pub const USBVDD_EN: u32 = bit(28);
    /// AVDD LDO enable (reset 1).
    pub const AVDD_EN: u32 = bit(22);
    /// AVDD: 1.0 V + 0.1 V × n (reset 2 = 1.2 V).
    pub const AVDD_VOL: Field = Field::bits(21, 20);
    /// AVCC: 2.8 V + 0.05 V × n (reset 3 = 2.95 V).
    pub const AVCC_SET: Field = Field::bits(19, 17);
    /// "Should be 0b1101" (reset 0xD). The vendor eFuse path writes 0xA here
    /// as part of [`VENDOR_EFUSE_READ`](super::VENDOR_EFUSE_READ) \[code\].
    pub const VCCB: Field = Field::bits(16, 13);
    /// VCC: 2.7 V + 0.1 V × n (reset 4 = 3.1 V; the vendor MANAGER writes 4,
    /// i.e. the default \[code\]).
    pub const VCC_SET: Field = Field::bits(12, 10);
    /// VDD in S3 standby: 0.7 V + 25 mV × n (reset 8 = 0.9 V).
    pub const VDD_SET_S3: Field = Field::bits(9, 5);
    /// Core VDD in S1 (run): 0.7 V + 25 mV × n up to 0x1A = 1.35 V (reset 0x14
    /// = 1.2 V). Only the vendor codes below may be written (01-hardware §7).
    pub const VDD_SET: Field = Field::bits(4, 0);
    /// Vendor VDD codes per level (system.md §1.4): ≤ 144 MHz.
    pub const VENDOR_VDD_1V000: u32 = 0x0C;
    /// 156–192 MHz.
    pub const VENDOR_VDD_1V100: u32 = 0x10;
    /// 204–228 MHz; also the reset default.
    pub const VENDOR_VDD_1V200: u32 = 0x14;
    /// 240–276 MHz.
    pub const VENDOR_VDD_1V250: u32 = 0x16;
    /// 288–300 MHz; UDISK forces it in USB mode.
    pub const VENDOR_VDD_1V300: u32 = 0x18;
    /// Wait after raising the code before raising the clock, in µs. \[code\]
    pub const VENDOR_RAISE_SETTLE_US: u32 = 150;
}

/// Undocumented regulator control; the vendor kernel writes
/// [`VENDOR_REG0C`] once at start, then waits 100 µs. Field meaning unknown;
/// the port never writes it. \[code\] system.md §3.
pub const REG0C: Reg = Reg::at(BASE + 0x0C);
/// VERBATIM vendor value for [`REG0C`] (03-hal-drivers §10 experiment).
pub const VENDOR_REG0C: u32 = 0x6860_2990;

/// CHG_CTL — charger control (VDD domain). \[DS\] §4.1.4.2.
pub const CHG_CTL: Reg = Reg::at(BASE + 0x10);
pub mod chg_ctl {
    use super::{Field, bit};
    /// DC5V over-voltage detection enable (reset 1).
    pub const DC5VOV_EN: u32 = bit(31);
    /// Over-voltage threshold: 5.5 V + 0.5 V × n.
    pub const DC5VOV_SET: Field = Field::bits(30, 29);
    /// Battery pull-down current: 0 off, 1 2.5 mA, 2 5 mA, 3 15 mA.
    pub const BAT_PD: Field = Field::bits(21, 20);
    /// Charger enable.
    pub const CHGEN: u32 = bit(18);
    /// Trickle enable: below 3.0 V charge at 1/10 of `CHG_CURRENT`.
    pub const ENTKLE: u32 = bit(17);
    /// Charge current code: 25, 50, 75, 100, 200, 300, 400, 450 mA.
    /// **Never above [`VENDOR_CHG_CURRENT_200MA`]** (CLAUDE.md rule 3).
    pub const CHG_CURRENT: Field = Field::bits(16, 14);
    /// Battery-presence detection start (result in `CHG_DET` bits 1:0).
    pub const ENBATDT: u32 = bit(13);
    /// Constant-voltage target: 4.1 V + 6.25 mV × n (0x10 = 4.2 V). Value the
    /// stock firmware leaves: U-PMU-1.
    pub const CHG_VLOT: Field = Field::bits(12, 7);
    /// End-of-charge voltage: 0 4.16 V, 1 4.18 V (reset), 2 4.32 V, 3 4.34 V.
    pub const STOPV: Field = Field::bits(6, 5);
    /// Automatic charge-termination detection (reset 1).
    pub const ENCHGATDT: u32 = bit(1);
    /// Termination detection interval: 0 12 min, 1 20 s.
    pub const DTSEL: u32 = bit(0);

    /// Vendor charge current on this product: code 4 = 200 mA (0.54 C). \[code\]
    /// system.md §3.1; mA per code \[DS\].
    pub const VENDOR_CHG_CURRENT_200MA: u32 = 4;
    /// Vendor current while BATADC < 0x280: code 1 = 50 mA. \[code\]
    pub const VENDOR_CHG_CURRENT_50MA: u32 = 1;
    /// Highest code orpheus may ever select.
    pub const MAX_ALLOWED_CHG_CURRENT: u32 = VENDOR_CHG_CURRENT_200MA;
    /// Bits the vendor clears on every charger stop/reprogram. \[code\]
    pub const VENDOR_STOP_CLEAR: u32 = CHGEN | ENTKLE | ENBATDT | ENCHGATDT;
    /// Vendor delay between the steps of a charger reprogram, in µs. \[code\]
    pub const VENDOR_STEP_US: u32 = 500;
}

/// CHG_DET — charger status (read-only). \[DS\] §4.1.4.3.
pub const CHG_DET: Reg = Reg::at(BASE + 0x14);
pub mod chg_det {
    use super::{Field, bit};
    pub const DC5VOV_DET: u32 = bit(8);
    /// Phase: 1 trickle, 2 constant current, 3 constant voltage.
    pub const CHGPHASE: Field = Field::bits(7, 6);
    pub const PHASE_TRICKLE: u32 = 1;
    pub const PHASE_CC: u32 = 2;
    pub const PHASE_CV: u32 = 3;
    /// Present current as a fraction of the set current: 0 < 5 %, 1 5–10 %,
    /// 2 10–20 %, 3 20–30 %, 4 30–50 %, 5 50–75 %, 6 75–87.5 %, 7 above.
    /// The vendor terminates at phase 3 with `CHG_STA` ≤ 1 \[code\].
    pub const CHG_STA: Field = Field::bits(5, 3);
    /// End of charge (hardware termination).
    pub const CHGEND: u32 = bit(2);
    /// Battery present (valid once `DTOVER`).
    pub const BATEXT: u32 = bit(1);
    /// Presence detection finished.
    pub const DTOVER: u32 = bit(0);
}

/// Undocumented analogue control shared by audio and the eFuse reader.
/// `[4:0]`: a reference code the eFuse reader sets to 0xD (ROM) / 0xE
/// (kernel) if 0 \[code\]; bit 6 set and bit 5 cleared = audio analogue power
/// \[HW\] (port `dac_power_up`). system.md §3, audio.md §1.2.
pub const REG18: Reg = Reg::at(BASE + 0x18);
pub mod reg18 {
    use super::{Field, bit};
    pub const UNK_4_0: Field = Field::bits(4, 0);
    /// Cleared for audio power-up. \[HW\]
    pub const AUDIO_CLEAR: u32 = bit(5);
    /// Set for audio power-up. \[HW\]
    pub const AUDIO_SET: u32 = bit(6);
}

/// SYSTEM_SET (SVCC domain). \[DS\] §4.1.4.4. Not written by the vendor
/// code analysed \[code\].
pub const SYSTEM_SET: Reg = Reg::at(BASE + 0x1C);
pub mod system_set {
    use super::{Field, bit};
    /// Enter standby automatically on low battery.
    pub const LB_EN: u32 = bit(9);
    /// Low-battery threshold: 0 2.7 V, 1 2.8 V, 2 3.0 V, 3 3.3 V.
    pub const LB_VOL: Field = Field::bits(8, 7);
    /// VCC over-current protection enable (reset 1).
    pub const OC_EN: u32 = bit(6);
    /// VCC/VDD low-voltage protection enable (reset 1).
    pub const LVPRO_EN: u32 = bit(5);
    /// DC5V-on debounce: 0 4 ms, 1 16 ms (reset 1).
    pub const DC5VON_T: u32 = bit(1);
    /// Reset on DC5V.
    pub const DC5VRST_EN: u32 = bit(0);
}

/// POWER_CTL (SVCC domain). \[DS\] §4.1.4.5. Power-off = clear `EN_S3`, then
/// `EN_S1`, 500 µs apart (vendor order) \[HW\]+\[code\].
pub const POWER_CTL: Reg = Reg::at(BASE + 0x20);
pub mod power_ctl {
    use super::bit;
    /// Run state (reset 1).
    pub const EN_S1: u32 = bit(0);
    /// S3 standby (U-PMU-2).
    pub const EN_S3: u32 = bit(1);
}

/// WKEN_CTL — wake sources (SVCC domain). \[DS\] §4.1.4.6. Reset `0x2402_0005`.
pub const WKEN_CTL: Reg = Reg::at(BASE + 0x24);
pub mod wken {
    use super::{Field, bit};
    /// Battery low-voltage wake threshold: 3.4 V + 0.1 V × n.
    pub const BATLV_VOL: Field = Field::bits(30, 28);
    /// DC5V low-voltage wake threshold: 0.2, 0.5, 1.0 … 3.5 V.
    pub const DC5VLV_VOL: Field = Field::bits(27, 25);
    /// Remote key on LRADC1 (voltage < 0.9 × SVCC).
    pub const REMOTE: u32 = bit(23);
    pub const BATLV: u32 = bit(22);
    pub const DC5VLV: u32 = bit(21);
    pub const WIO1LV: u32 = bit(20);
    /// WIO1 low-voltage threshold: < 0.66, 1.0, 1.1, 1.17 V.
    pub const WIO1LV_VOL: Field = Field::bits(19, 18);
    /// DC5V pull-down enable and current (2.5, 5, 10, 20 mA).
    pub const DC5VPD_EN: u32 = bit(17);
    pub const DC5VPD_SET: Field = Field::bits(16, 15);
    pub const WIO1: u32 = bit(11);
    pub const CPUIRQ: u32 = bit(9);
    /// RTC alarm (U-PMU-3).
    pub const ALARM: u32 = bit(4);
    pub const DC5VOFF: u32 = bit(3);
    /// DC5V plugged in (reset 1).
    pub const DC5VON: u32 = bit(2);
    pub const SHORT: u32 = bit(1);
    /// Power key long press (reset 1).
    pub const LONG: u32 = bit(0);
    /// Vendor value before power-off: long press + DC5V in. \[code\] system.md §3.4.
    pub const VENDOR_POWEROFF: u32 = LONG | DC5VON;
}

/// WAKE_PD — wake pending flags (SVCC domain), write 1 to clear. \[DS\]
/// §4.1.4.7. Bit 30 is the live DC5V state \[HW\].
pub const WAKE_PD: Reg = Reg::at(BASE + 0x28);
pub mod wake_pd {
    use super::bit;
    /// Read-only: DC5V present (> BAT + 0.08 V). \[DS\]; \[HW\] (port USB/power).
    pub const DC5VON: u32 = bit(30);
    pub const OC_PD: u32 = bit(27);
    pub const LVPRO_PD: u32 = bit(26);
    pub const LB_PD: u32 = bit(25);
    pub const BATWK_PD: u32 = bit(24);
    pub const REMOTE_PD: u32 = bit(23);
    pub const BATLVWK_PD: u32 = bit(22);
    pub const DC5VLVWK_PD: u32 = bit(21);
    pub const WIO1LV_PD: u32 = bit(20);
    pub const WIO1_PD: u32 = bit(11);
    pub const CPUIRQ_PD: u32 = bit(9);
    pub const ALARM_PD: u32 = bit(4);
    pub const DC5VOFF_PD: u32 = bit(3);
    pub const DC5VON_PD: u32 = bit(2);
    pub const ONOFF_S_PD: u32 = bit(1);
    pub const ONOFF_L_PD: u32 = bit(0);
    /// Vendor "clear all pending" before power-off. \[code\] system.md §3.4.
    /// Covers bits 0–4, 9–11 and 20–24 (not 25–27).
    pub const VENDOR_CLEAR_ALL: u32 = 0x01F0_0E1F;
}

/// ONOFF_CTL — power key (SVCC domain). \[DS\] §4.1.4.8.
pub const ONOFF_CTL: Reg = Reg::at(BASE + 0x2C);
pub mod onoff {
    use super::{Field, bit};
    /// Short/long threshold: 0.125, 0.25, 0.5, 1, 1.5, 2, 3, 4 s (reset 1).
    pub const PRESS_TIME: Field = Field::bits(8, 6);
    /// Long-press action: 1 reset, 2 restart (reset 1).
    pub const RST_EN: Field = Field::bits(5, 4);
    pub const RST_EN_RESET: u32 = 1;
    pub const RST_EN_RESTART: u32 = 2;
    /// Hold time before the long-press action: 8, 12, 16, 24 s.
    pub const RST_TIME: Field = Field::bits(3, 2);
    /// Key pressed. The datasheet calls it a read-only live status; the
    /// vendor and the port treat it as a latch and write 1 back to clear it
    /// \[HW\] (works). **Disagreement** to settle in INPUT-1.
    pub const PRESS: u32 = bit(1);
    /// Same bit as [`PRESS`] (earlier name).
    pub const SHORT_PD: u32 = PRESS;
    /// Key polarity: 0 pressed = pulled low, 1 pressed = pulled high.
    pub const PULLR_SEL: u32 = bit(0);
    /// Vendor while running: `PRESS_TIME` 3 (1 s), restart after 8 s. \[code\]
    /// system.md §3 (`[8:6]` = 3, `[5:2]` = 8), decoded \[DS\].
    pub const VENDOR_RUN_PRESS_TIME: u32 = 3;
    pub const VENDOR_RUN_RST: u32 = RST_EN_RESTART << 2;
    /// Vendor just before power-off: `PRESS_TIME` 2 (0.5 s). \[code\]
    pub const VENDOR_POWEROFF_PRESS_TIME: u32 = 2;
}

/// PMUADC_CTL — channel enables of the low-rate ADC. \[DS\] §9.3.4.1.
/// Reset `0x0001_001B` (DC5V, BAT, AVCC, LRADC1 on).
pub const PMUADC_CTL: Reg = Reg::at(BASE + 0x38);
pub mod pmuadc {
    use super::{Field, bit};
    /// Single-channel fast mode: channel 0 DC5V, 1 BAT, 2 sensor, 3 AVCC,
    /// 4..13 LRADC1..10, 15 SVCC.
    pub const FASTMODE_SET: Field = Field::bits(23, 20);
    pub const FASTMODE_EN: u32 = bit(19);
    /// Comparator current: 0 0.5 µA, 1 1 µA.
    pub const I_COMP_SET: u32 = bit(17);
    pub const SVCC_EN: u32 = bit(15);
    /// LRADCn enable, n = 1..10: bit 3 + n. \[DS\] §9.3.4.1 (checked in
    /// WP-HAL-0; the port's `|= 0x210` = LRADC1 + LRADC6 \[HW\]).
    #[must_use]
    pub const fn lradc_en(n: u8) -> u32 {
        bit(3 + n)
    }
    pub const AVCC_EN: u32 = bit(3);
    /// Temperature sensor and its ADC.
    pub const SENSOR_EN: u32 = bit(2);
    /// Battery ADC (reset 1).
    pub const BAT_EN: u32 = bit(1);
    /// DC5V ADC (reset 1).
    pub const DC5V_EN: u32 = bit(0);
    /// Port key set-up: both key ladders. \[HW\]
    pub const PORT_KEYS: u32 = 0x210;
}
/// BATADC_DATA `[9:0]`: 1 LSB = 4.8 V / 1024 × Vref / 1.5 V. The vendor adds
/// a signed trim from eFuse word 2 `[15:11]` \[code\]. \[DS\] §9.3.4.2.
pub const BATADC_DATA: Reg = Reg::at(BASE + 0x3C);
/// DC5VADC_DATA `[9:0]`: 1 LSB = 6 V / 1024 × Vref / 1.5 V; the vendor counts
/// > 0x200 as cable present \[code\]. \[DS\] §9.3.4.3.
pub const DC5VADC_DATA: Reg = Reg::at(BASE + 0x40);
/// SENSADC_DATA `[9:0]`: temperature, T = (1.2/1024 × n × Vref/1.5 −
/// 0.7691) / −0.001707 °C. \[DS\] §9.3.4.4.
pub const SENSADC_DATA: Reg = Reg::at(BASE + 0x44);
/// AVCCADC_DATA `[9:0]`: 1 LSB = 3.6 V / 1024 × Vref / 1.5 V. \[DS\] §9.3.4.5.
pub const AVCCADC_DATA: Reg = Reg::at(BASE + 0x48);
/// LRADCn_DATA, n = 1..10, `[9:0]` (LRADC1 range 0..SVCC, others 0..AVCC).
/// Key ladder 1 = LRADC1 (`+0x50`), ladder 2 = LRADC6 (`+0x64`). \[DS\]
/// §9.3.4.6–15; ladders \[HW\].
#[must_use]
pub const fn lradc_data(n: usize) -> Reg {
    Reg::at(BASE + 0x50 + 4 * (n - 1))
}
/// The 10-bit result field of every ADC data register.
pub const ADC_DATA: Field = Field::bits(9, 0);
/// SVCCADC_DATA `[9:0]`. \[DS\] §9.3.4.16.
pub const SVCCADC_DATA: Reg = Reg::at(BASE + 0x7C);

/// Whole-register `VOUT_CTL` value the vendor eFuse reader writes (the reset
/// defaults with `VCCB` = 0xA and reserved bits 25/23 set). VERBATIM; the
/// eFuse is not read by orpheus without a user decision. \[code\] system.md §5.5.
pub const VENDOR_EFUSE_READ: u32 = 0x32E7_5114;

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::*;

    #[test]
    fn port_addresses() {
        assert_eq!(VOUT_CTL.addr(), 0xC001_0000);
        assert_eq!(REG18.addr(), 0xC001_0018);
        assert_eq!(POWER_CTL.addr(), 0xC001_0020); // PMU_CTL0
        assert_eq!(WAKE_PD.addr(), 0xC001_0028); // PMU_STAT
        assert_eq!(ONOFF_CTL.addr(), 0xC001_002C);
        assert_eq!(PMUADC_CTL.addr(), 0xC001_0038);
        assert_eq!(BATADC_DATA.addr(), 0xC001_003C);
        assert_eq!(DC5VADC_DATA.addr(), 0xC001_0040);
        assert_eq!(lradc_data(1).addr(), 0xC001_0050);
        assert_eq!(lradc_data(6).addr(), 0xC001_0064);
    }

    #[test]
    fn datasheet_list() {
        assert_increasing(&[
            VOUT_CTL.addr(),
            CHG_CTL.addr(),
            CHG_DET.addr(),
            SYSTEM_SET.addr(),
            POWER_CTL.addr(),
            WKEN_CTL.addr(),
            WAKE_PD.addr(),
            ONOFF_CTL.addr(),
            PMUADC_CTL.addr(),
            BATADC_DATA.addr(),
            DC5VADC_DATA.addr(),
            SENSADC_DATA.addr(),
            AVCCADC_DATA.addr(),
            lradc_data(1).addr(),
            lradc_data(10).addr(),
            SVCCADC_DATA.addr(),
        ]);
        assert_eq!(lradc_data(10).addr(), 0xC001_0074);
        assert_eq!(SVCCADC_DATA.addr(), 0xC001_007C);
    }

    #[test]
    fn values() {
        assert_eq!(pmuadc::lradc_en(1) | pmuadc::lradc_en(6), pmuadc::PORT_KEYS);
        assert_eq!(pmuadc::lradc_en(10), bit(13));
        assert_eq!(onoff::VENDOR_RUN_RST, 8);
        assert_eq!(wken::VENDOR_POWEROFF, 5);
        assert_eq!(chg_ctl::VENDOR_STOP_CLEAR, bit(18) | bit(17) | bit(13) | bit(1));
        assert_eq!(chg_ctl::CHG_CURRENT.val(chg_ctl::VENDOR_CHG_CURRENT_200MA), 4 << 14);
        // The eFuse-read value is the reset defaults apart from VCCB and bits 25/23.
        use vout::*;
        let v = VENDOR_EFUSE_READ;
        assert_eq!(VDD_SET.get(v), VENDOR_VDD_1V200);
        assert_eq!(VDD_SET_S3.get(v), 8);
        assert_eq!(VCC_SET.get(v), 4);
        assert_eq!(VCCB.get(v), 0xA);
        assert_eq!(AVCC_SET.get(v), 3);
        assert_eq!(AVDD_VOL.get(v), 2);
        assert!(v & (AVDD_EN | USBVCC_EN | USBVDD_EN) == AVDD_EN | USBVCC_EN | USBVDD_EN);
        // WAKE_PD clear mask covers only documented pending bits.
        use wake_pd::*;
        let all = OC_PD
            | LVPRO_PD
            | LB_PD
            | BATWK_PD
            | REMOTE_PD
            | BATLVWK_PD
            | DC5VLVWK_PD
            | WIO1LV_PD
            | WIO1_PD
            | CPUIRQ_PD
            | ALARM_PD
            | DC5VOFF_PD
            | DC5VON_PD
            | ONOFF_S_PD
            | ONOFF_L_PD;
        assert_eq!(VENDOR_CLEAR_ALL & !all, 0x0000_0400, "only bit 10 (reserved) is extra");
    }

    #[test]
    fn disjoint() {
        {
            use vout::*;
            assert_disjoint(&[
                USBVCC_EN,
                USBVDD_EN,
                AVDD_EN,
                AVDD_VOL.mask(),
                AVCC_SET.mask(),
                VCCB.mask(),
                VCC_SET.mask(),
                VDD_SET_S3.mask(),
                VDD_SET.mask(),
            ]);
        }
        {
            use chg_ctl::*;
            assert_disjoint(&[
                DC5VOV_EN,
                DC5VOV_SET.mask(),
                BAT_PD.mask(),
                CHGEN,
                ENTKLE,
                CHG_CURRENT.mask(),
                ENBATDT,
                CHG_VLOT.mask(),
                STOPV.mask(),
                ENCHGATDT,
                DTSEL,
            ]);
        }
        {
            use chg_det::*;
            assert_disjoint(&[DC5VOV_DET, CHGPHASE.mask(), CHG_STA.mask(), CHGEND, BATEXT, DTOVER]);
        }
        {
            use system_set::*;
            assert_disjoint(&[LB_EN, LB_VOL.mask(), OC_EN, LVPRO_EN, DC5VON_T, DC5VRST_EN]);
        }
        {
            use wken::*;
            assert_disjoint(&[
                BATLV_VOL.mask(),
                DC5VLV_VOL.mask(),
                REMOTE,
                BATLV,
                DC5VLV,
                WIO1LV,
                WIO1LV_VOL.mask(),
                DC5VPD_EN,
                DC5VPD_SET.mask(),
                WIO1,
                CPUIRQ,
                ALARM,
                DC5VOFF,
                DC5VON,
                SHORT,
                LONG,
            ]);
        }
        assert_disjoint(&[
            onoff::PRESS_TIME.mask(),
            onoff::RST_EN.mask(),
            onoff::RST_TIME.mask(),
            onoff::PRESS,
            onoff::PULLR_SEL,
        ]);
        {
            use pmuadc::*;
            let mut masks = [0u32; 17];
            masks[0] = FASTMODE_SET.mask();
            masks[1] = FASTMODE_EN;
            masks[2] = I_COMP_SET;
            masks[3] = SVCC_EN;
            masks[4] = AVCC_EN;
            masks[5] = SENSOR_EN;
            masks[6] = BAT_EN;
            for n in 1..=10u8 {
                masks[6 + n as usize] = lradc_en(n);
            }
            assert_disjoint(&masks[..16]);
            assert_disjoint(&[masks[16], DC5V_EN]);
        }
        assert_disjoint(&[reg18::UNK_4_0.mask(), reg18::AUDIO_CLEAR, reg18::AUDIO_SET]);
    }
}
