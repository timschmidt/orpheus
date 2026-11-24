//! Fuzz-target body of `orpheus-power` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`power_samples`] — the charger state machine and the battery gauge over
//! arbitrary sequences of charger status readings, battery ADC values,
//! cable changes and presence-detection results. Oracles: no panic; the
//! charge current code is never above the 200 mA code (CLAUDE.md rule 3)
//! and is one of the stock firmware's codes; without a cable the charger is
//! stopped and reports no current; a presence detection is asked for only
//! when a cable appears; "full" is confirmed only in the full state; the
//! gauge's percentage is monotonic in the voltage and within 0–100, and the
//! smoothed reading stays within the range of the raw readings.

#![forbid(unsafe_code)]

use orpheus_power::charger::{CODE_MAX, CODE_NORMAL, CODE_TRICKLE, ChargeState, Charger, Command, Inputs};
use orpheus_power::gauge::{Smoother, percent, raw_to_mv};

fn check_code(c: u8) {
    assert!(c <= CODE_MAX, "charge code {c} above the 200 mA code");
    assert!(c == CODE_TRICKLE || c == CODE_NORMAL, "charge code {c} not one the stock firmware uses");
}

/// Input: 4-byte records: flags (bit 0 cable, bits 1–2 presence result
/// none/absent/present/timeout, bits 3–4 phase), battery raw (10 bits of the
/// next two bytes), current step (low 3 bits of the fourth byte).
pub fn power_samples(data: &[u8]) {
    let mut c = Charger::new();
    let mut sm = Smoother::default();
    let (mut lo, mut hi) = (u16::MAX, 0u16);
    let mut asked = false;
    for r in data.chunks_exact(4).take(20_000) {
        let cable = r[0] & 1 != 0;
        let raw = u16::from_le_bytes([r[1], r[2]]) & 0x3FF;
        let i = Inputs { cable, battery_raw: raw, phase: (r[0] >> 3) & 3, current_step: r[3] & 7 };
        let before = c.state();
        let cmd = match (r[0] >> 1) & 3 {
            0 => c.step(i),
            k => {
                if !asked {
                    continue;
                }
                asked = false;
                let present = match k {
                    1 => Some(false),
                    2 => Some(true),
                    _ => None,
                };
                c.presence_result(present, raw)
            }
        };
        match cmd {
            Command::Charge(code) => {
                check_code(code);
                assert!(cable || (r[0] >> 1) & 3 != 0, "charging without a cable");
                assert_eq!(c.current_code(), code);
            }
            Command::DetectPresence => {
                assert_eq!(before, ChargeState::NotPresent, "detection asked for outside cable insertion");
                asked = true;
            }
            Command::Stop | Command::None => {}
        }
        if c.current_code() != 0 {
            check_code(c.current_code());
        }
        if (r[0] >> 1) & 3 == 0 && !cable {
            assert_eq!(c.state(), ChargeState::NotPresent);
            assert_eq!(c.current_code(), 0);
            assert!(matches!(cmd, Command::None | Command::Stop));
        }
        if matches!(
            c.state(),
            ChargeState::NotPresent | ChargeState::NoBattery | ChargeState::Fault | ChargeState::Full
        ) {
            assert_eq!(c.current_code(), 0, "current set in {:?}", c.state());
        }
        if c.full_confirmed() {
            assert_eq!(c.state(), ChargeState::Full);
        }

        // Gauge.
        let mv = raw_to_mv(raw);
        let p = percent(mv);
        assert!(p <= 100);
        assert!(percent(mv + 1) >= p, "percentage falls with the voltage at {mv} mV");
        lo = lo.min(raw);
        hi = hi.max(raw);
        let s = sm.feed(raw);
        assert!((lo..=hi).contains(&s), "smoothed {s} outside {lo}..={hi}");
    }
}
