//! Charger and battery hardware (03-hal-drivers §10). Decisions are made by
//! `orpheus_power::charger`; this module only reads status and applies
//! commands with the stock firmware's register sequence (system.md §3.1).
//!
//! Fields written: `CHGEN`, `ENTKLE`, `CHG_CURRENT`, `ENBATDT`, `ENCHGATDT`
//! — exactly the ones the stock firmware writes. The end-voltage fields
//! (`CHG_VLOT`, `STOPV`) are never touched (CLAUDE.md rule 3). The current
//! code is clamped to `MAX_ALLOWED_CHG_CURRENT` (code 4, 200 mA) here as
//! well, whatever the caller asks.

use atj2157_hal::delay::spin_us;
use atj2157_hal::pac::pmu::{self, chg_ctl, chg_det, wake_pd};
use embassy_time::{Duration, Instant, Timer};
pub use orpheus_power::charger::{ChargeState, Charger, Command, Inputs};

/// Enable the battery ADC (`PMUADC_CTL` bit 1). [DS]
pub fn init() {
    pmu::PMUADC_CTL.set_bits(pmu::pmuadc::BAT_EN);
}

#[must_use]
pub fn battery_raw() -> u16 {
    (pmu::BATADC_DATA.read() & 0x3FF) as u16
}

#[must_use]
pub fn dc5v_raw() -> u16 {
    (pmu::DC5VADC_DATA.read() & 0x3FF) as u16
}

/// Cable present: `WAKE_PD` bit 30 or DC5V ADC > 0x200 [code].
#[must_use]
pub fn cable_present() -> bool {
    pmu::WAKE_PD.read() & wake_pd::DC5VON != 0 || dc5v_raw() > 0x200
}

#[must_use]
pub fn inputs() -> Inputs {
    let det = pmu::CHG_DET.read();
    Inputs {
        cable: cable_present(),
        battery_raw: battery_raw(),
        phase: chg_det::CHGPHASE.get(det) as u8,
        current_step: chg_det::CHG_STA.get(det) as u8,
    }
}

fn stop() {
    pmu::CHG_CTL.clear_bits(chg_ctl::VENDOR_STOP_CLEAR);
}

/// VERBATIM(vendor KEY.DRV charge_set): clear, 500 µs, current, 500 µs,
/// enable, 500 µs.
fn charge(code: u8) {
    let code = u32::from(code).min(chg_ctl::MAX_ALLOWED_CHG_CURRENT);
    pmu::CHG_CTL.clear_bits(chg_ctl::ENTKLE | chg_ctl::ENBATDT | chg_ctl::ENCHGATDT);
    spin_us(500);
    pmu::CHG_CTL.modify(|v| chg_ctl::CHG_CURRENT.set(v, code));
    spin_us(500);
    pmu::CHG_CTL.set_bits(chg_ctl::CHGEN);
    spin_us(500);
}

/// Battery-presence detection (≤ 800 ms). `None` on timeout.
pub async fn detect_presence() -> Option<bool> {
    stop();
    pmu::CHG_CTL.set_bits(chg_ctl::ENBATDT);
    let t0 = Instant::now();
    let r = loop {
        let det = pmu::CHG_DET.read();
        if det & chg_det::DTOVER != 0 {
            break Some(det & chg_det::BATEXT != 0);
        }
        if t0.elapsed() > Duration::from_millis(800) {
            break None;
        }
        Timer::after_millis(10).await;
    };
    pmu::CHG_CTL.clear_bits(chg_ctl::ENBATDT);
    r
}

/// Apply a command; presence results are fed back into `c`.
pub async fn apply(c: &mut Charger, cmd: Command) {
    match cmd {
        Command::None => {}
        Command::Stop => stop(),
        Command::Charge(code) => charge(code),
        Command::DetectPresence => {
            let p = detect_presence().await;
            let next = c.presence_result(p, battery_raw());
            apply_simple(next);
        }
    }
}

/// Second step after presence detection (never another detection).
fn apply_simple(cmd: Command) {
    match cmd {
        Command::Stop => stop(),
        Command::Charge(code) => charge(code),
        Command::None | Command::DetectPresence => {}
    }
}

/// Stop charging (end of a test image, power-off path).
pub fn stop_now() {
    stop();
}

/// Raw register words for diagnostics: (CHG_CTL, CHG_DET).
#[must_use]
pub fn regs() -> (u32, u32) {
    (pmu::CHG_CTL.read(), pmu::CHG_DET.read())
}
