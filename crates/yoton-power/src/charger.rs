//! Charger control (docs/plan/03-hal-drivers.md §10; system.md §3.1–3.2).
//!
//! The decision logic of the stock firmware's key driver, over raw 10-bit
//! battery ADC values:
//!
//! * cable plugged → battery-presence detect, then charge;
//! * battery < 640 → trickle at current code 1 (50 mA), re-evaluated after
//!   [`TRICKLE_PERIODS`] periods; otherwise code **4 (200 mA)** — never more
//!   (0.54 C for the 370 mAh cell, CLAUDE.md rule 3);
//! * while charging and battery ≥ 892: when the hardware reports the
//!   constant-voltage phase with the current down to ≤ 10 % of the set
//!   current, stop; if it still reads ≥ 892 three times, the cell is full;
//! * not charging and battery > 885 three times → full;
//! * full and battery ≤ 821 → charge again.
//!
//! The driver (`yoton-bsp::charger`) applies [`Command`]s with the vendor's
//! register sequence and feeds back [`Inputs`] once per period.

/// Raw battery thresholds [code] (system.md §3.2).
pub const TRICKLE_BELOW: u16 = 0x280;
pub const FULL_AT_OR_ABOVE: u16 = 0x37C;
pub const FULL_IDLE_ABOVE: u16 = 0x375;
pub const RECHARGE_AT_OR_BELOW: u16 = 0x335;
pub const BOOT_REFUSE_BELOW: u16 = 0x2AB;

/// Current codes (datasheet `CHG_CURRENT`): 1 = 50 mA, 4 = 200 mA.
pub const CODE_TRICKLE: u8 = 1;
pub const CODE_NORMAL: u8 = 4;
/// The highest code this firmware may ever select.
pub const CODE_MAX: u8 = 4;

/// Periods spent trickling before re-evaluating.
pub const TRICKLE_PERIODS: u16 = 240;
/// Consecutive confirmations for "full".
pub const CONFIRM: u8 = 3;

/// What the UI is told.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ChargeState {
    NotPresent,
    /// Cable in, battery detection running.
    Detecting,
    /// No battery found (or detection failed).
    NoBattery,
    Trickle,
    Charging,
    Full,
    Fault,
}

/// Hardware status read each period.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Inputs {
    pub cable: bool,
    pub battery_raw: u16,
    /// `CHG_DET[7:6]`: 1 trickle, 2 constant current, 3 constant voltage.
    pub phase: u8,
    /// `CHG_DET[5:3]`: charge current as a fraction of the set current
    /// (0: < 5 %, 1: 5–10 %, …, 7: > 87.5 %).
    pub current_step: u8,
}

/// What the driver must do now.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Command {
    /// Leave the hardware as it is.
    None,
    /// Stop charging (clear enable).
    Stop,
    /// Run the battery-presence detection; report with
    /// [`Charger::presence_result`].
    DetectPresence,
    /// (Re)program the current code and enable.
    Charge(u8),
}

#[derive(Clone, Copy, Debug)]
pub struct Charger {
    state: ChargeState,
    periods_in_state: u16,
    full_count: u8,
    idle_full_count: u8,
    current_code: u8,
}

impl Default for Charger {
    fn default() -> Self {
        Self::new()
    }
}

impl Charger {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            state: ChargeState::NotPresent,
            periods_in_state: 0,
            full_count: 0,
            idle_full_count: 0,
            current_code: 0,
        }
    }

    #[must_use]
    pub fn state(&self) -> ChargeState {
        self.state
    }

    #[must_use]
    pub fn current_code(&self) -> u8 {
        self.current_code
    }

    fn go(&mut self, s: ChargeState) {
        if self.state != s {
            self.state = s;
            self.periods_in_state = 0;
            self.full_count = 0;
            self.idle_full_count = 0;
        }
    }

    fn start_charge(&mut self, raw: u16) -> Command {
        let code = if raw < TRICKLE_BELOW { CODE_TRICKLE } else { CODE_NORMAL }.min(CODE_MAX);
        self.current_code = code;
        self.go(if code == CODE_TRICKLE { ChargeState::Trickle } else { ChargeState::Charging });
        Command::Charge(code)
    }

    /// Result of a presence detection requested with
    /// [`Command::DetectPresence`]; `None` = detection timed out.
    pub fn presence_result(&mut self, present: Option<bool>, raw: u16) -> Command {
        match present {
            Some(true) if raw > FULL_IDLE_ABOVE => {
                self.go(ChargeState::Full);
                self.current_code = 0;
                Command::Stop
            }
            Some(true) => self.start_charge(raw),
            Some(false) => {
                self.go(ChargeState::NoBattery);
                Command::Stop
            }
            None => {
                self.go(ChargeState::Fault);
                Command::Stop
            }
        }
    }

    /// One period (about one second).
    pub fn step(&mut self, i: Inputs) -> Command {
        self.periods_in_state = self.periods_in_state.saturating_add(1);
        if !i.cable {
            let was = self.state;
            self.go(ChargeState::NotPresent);
            self.current_code = 0;
            return if was == ChargeState::NotPresent { Command::None } else { Command::Stop };
        }
        match self.state {
            ChargeState::NotPresent => {
                self.go(ChargeState::Detecting);
                Command::DetectPresence
            }
            ChargeState::Detecting | ChargeState::NoBattery | ChargeState::Fault => Command::None,
            ChargeState::Trickle => {
                if self.periods_in_state >= TRICKLE_PERIODS {
                    self.periods_in_state = 0;
                    if i.battery_raw >= TRICKLE_BELOW {
                        return self.start_charge(i.battery_raw);
                    }
                }
                Command::None
            }
            ChargeState::Charging => {
                if i.battery_raw >= FULL_AT_OR_ABOVE {
                    if i.phase == 3 && i.current_step <= 1 {
                        // Terminate; confirm "full" with the charger off.
                        self.current_code = 0;
                        self.state = ChargeState::Full;
                        self.full_count = 0;
                        self.idle_full_count = 0;
                        self.periods_in_state = 0;
                        return Command::Stop;
                    }
                } else if i.battery_raw < TRICKLE_BELOW {
                    return self.start_charge(i.battery_raw);
                }
                Command::None
            }
            ChargeState::Full => {
                if i.battery_raw <= RECHARGE_AT_OR_BELOW {
                    return self.start_charge(i.battery_raw);
                }
                if i.battery_raw > FULL_IDLE_ABOVE {
                    self.idle_full_count = self.idle_full_count.saturating_add(1).min(CONFIRM);
                }
                Command::None
            }
        }
    }

    /// Full has been confirmed by repeated idle readings.
    #[must_use]
    pub fn full_confirmed(&self) -> bool {
        self.state == ChargeState::Full && self.idle_full_count >= CONFIRM
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inp(cable: bool, raw: u16) -> Inputs {
        Inputs { cable, battery_raw: raw, phase: 2, current_step: 7 }
    }

    #[test]
    fn plug_detect_charge() {
        let mut c = Charger::new();
        assert_eq!(c.step(inp(false, 800)), Command::None);
        assert_eq!(c.step(inp(true, 800)), Command::DetectPresence);
        assert_eq!(c.state(), ChargeState::Detecting);
        assert_eq!(c.presence_result(Some(true), 800), Command::Charge(4));
        assert_eq!(c.state(), ChargeState::Charging);
        assert_eq!(c.step(inp(true, 820)), Command::None);
    }

    #[test]
    fn low_battery_trickles_then_charges() {
        let mut c = Charger::new();
        c.step(inp(true, 600));
        assert_eq!(c.presence_result(Some(true), 600), Command::Charge(1));
        assert_eq!(c.state(), ChargeState::Trickle);
        for _ in 0..TRICKLE_PERIODS - 1 {
            assert_eq!(c.step(inp(true, 650)), Command::None);
        }
        assert_eq!(c.step(inp(true, 650)), Command::Charge(4));
        assert_eq!(c.state(), ChargeState::Charging);
    }

    #[test]
    fn never_above_200ma() {
        let mut c = Charger::new();
        for raw in [0u16, 500, 639, 640, 700, 891, 900, 1023] {
            c = Charger::new();
            c.step(inp(true, raw));
            if let Command::Charge(code) = c.presence_result(Some(true), raw) {
                assert!(code <= CODE_MAX);
            }
        }
        assert!(c.current_code() <= CODE_MAX);
    }

    #[test]
    fn termination_needs_cv_phase_and_low_current() {
        let mut c = Charger::new();
        c.step(inp(true, 850));
        c.presence_result(Some(true), 850);
        // High voltage but still constant current: keep charging.
        assert_eq!(
            c.step(Inputs { cable: true, battery_raw: 895, phase: 2, current_step: 7 }),
            Command::None
        );
        // CV phase but current still high: keep charging.
        assert_eq!(
            c.step(Inputs { cable: true, battery_raw: 895, phase: 3, current_step: 4 }),
            Command::None
        );
        // CV and current < 10 %: stop.
        assert_eq!(
            c.step(Inputs { cable: true, battery_raw: 895, phase: 3, current_step: 1 }),
            Command::Stop
        );
        assert_eq!(c.state(), ChargeState::Full);
        for _ in 0..3 {
            c.step(inp(true, 890));
        }
        assert!(c.full_confirmed());
        // Self-discharge to the re-charge threshold restarts charging.
        assert_eq!(c.step(inp(true, 821)), Command::Charge(4));
    }

    #[test]
    fn already_full_at_plug_in() {
        let mut c = Charger::new();
        c.step(inp(true, 900));
        assert_eq!(c.presence_result(Some(true), 900), Command::Stop);
        assert_eq!(c.state(), ChargeState::Full);
    }

    #[test]
    fn unplug_stops() {
        let mut c = Charger::new();
        c.step(inp(true, 800));
        c.presence_result(Some(true), 800);
        assert_eq!(c.step(inp(false, 800)), Command::Stop);
        assert_eq!(c.state(), ChargeState::NotPresent);
        assert_eq!(c.step(inp(false, 800)), Command::None);
    }

    #[test]
    fn no_battery_and_timeout() {
        let mut c = Charger::new();
        c.step(inp(true, 0));
        assert_eq!(c.presence_result(Some(false), 0), Command::Stop);
        assert_eq!(c.state(), ChargeState::NoBattery);
        let mut c = Charger::new();
        c.step(inp(true, 0));
        assert_eq!(c.presence_result(None, 0), Command::Stop);
        assert_eq!(c.state(), ChargeState::Fault);
    }
}
