//! The clock governor's decision (PWR-2 first cut, PWR-5 / D-4): a pure
//! function of the main executor's idle share over the last step, the
//! current level, the clock floor the audio task asks for, and the
//! Performance setting. The firmware runs it once a second and carries out
//! the level it returns.
//!
//! * Idle below [`UP_BELOW`] %: one level up (Base → Mid → High). With
//!   Performance = Maximum, the step above High goes to `Max` (288 MHz);
//!   with Normal there is nothing above High.
//! * Idle above [`DOWN_ABOVE`] % for [`CALM_STEPS`] steps in a row: one
//!   level down (Turbo/Max → High → Mid → Base), never below the floor.
//! * With Normal the governor never runs at `Max`: a `Max` level found
//!   there (pinned by a test, or the setting just turned off) goes to High
//!   at the next step.

/// Clock levels the governor knows, in rising order (the HAL's
/// `clock::Level`).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Level {
    Low,
    Base,
    Mid,
    High,
    Turbo,
    Max,
}

impl Level {
    pub const ALL: [Level; 6] = [Level::Low, Level::Base, Level::Mid, Level::High, Level::Turbo, Level::Max];

    /// Core clock in MHz.
    #[must_use]
    pub const fn mhz(self) -> u16 {
        match self {
            Level::Low => 24,
            Level::Base => 48,
            Level::Mid => 96,
            Level::High => 144,
            Level::Turbo => 192,
            Level::Max => 288,
        }
    }

    /// From its index in [`Level::ALL`] (anything larger is `Max`).
    #[must_use]
    pub const fn from_index(i: u8) -> Level {
        match i {
            0 => Level::Low,
            1 => Level::Base,
            2 => Level::Mid,
            3 => Level::High,
            4 => Level::Turbo,
            _ => Level::Max,
        }
    }
}

/// Settings › System › Performance (decision D-4).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Performance {
    /// Base … High (48–144 MHz).
    #[default]
    Normal,
    /// The step above High is `Max` (288 MHz, 1.3 V core supply).
    Maximum,
}

impl Performance {
    /// From the stored setting value (0 Normal, 1 Maximum; anything else
    /// is Normal).
    #[must_use]
    pub const fn from_setting(v: i16) -> Performance {
        if v == 1 { Performance::Maximum } else { Performance::Normal }
    }
}

/// Settings › Power › Power mode (02 §5.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PowerMode {
    /// The governor as it is ([`Governor::step`]).
    #[default]
    Auto,
    /// Battery saver: the clock stays at Base, or at the floor the audio
    /// task asks for when that is higher (playback keeps what it needs; the
    /// UI never raises the clock).
    Saver,
    /// High performance: at least High (144 MHz) while the screen is on.
    High,
}

impl PowerMode {
    /// From the stored setting value (0 Automatic, 1 Battery saver, 2 High
    /// performance; anything else is Automatic).
    #[must_use]
    pub const fn from_setting(v: i16) -> PowerMode {
        match v {
            1 => PowerMode::Saver,
            2 => PowerMode::High,
            _ => PowerMode::Auto,
        }
    }
}

/// One level up when the idle share is below this (percent).
pub const UP_BELOW: u32 = 25;
/// One level down after [`CALM_STEPS`] steps above this (percent).
pub const DOWN_ABOVE: u32 = 70;
/// Steps in a row above [`DOWN_ABOVE`] before a step down.
pub const CALM_STEPS: u8 = 3;

/// What one step looks at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Input {
    /// Idle share of the main executor since the last step (percent).
    pub idle_pct: u32,
    pub current: Level,
    /// Lowest level the audio task allows: 0 Base, 1 Mid, 2 High.
    pub floor: u8,
    pub perf: Performance,
}

/// The idle share in percent from the idle time and the wall time of a
/// step (microseconds); a zero-length step counts as 1 µs.
#[must_use]
pub fn idle_percent(idle_us: u64, dt_us: u64) -> u32 {
    (idle_us.saturating_mul(100) / dt_us.max(1)).min(u64::from(u32::MAX)) as u32
}

/// The governor's memory between steps: the run of calm steps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Governor {
    calm: u8,
}

impl Governor {
    #[must_use]
    pub const fn new() -> Governor {
        Governor { calm: 0 }
    }

    /// The level to switch to, if any.
    pub fn step(&mut self, i: Input) -> Option<Level> {
        if i.perf == Performance::Normal && i.current == Level::Max {
            self.calm = 0;
            return Some(Level::High);
        }
        if i.idle_pct < UP_BELOW {
            self.calm = 0;
            match (i.current, i.perf) {
                (Level::Low | Level::Base, _) => Some(Level::Mid),
                (Level::Mid, _) => Some(Level::High),
                (Level::High | Level::Turbo, Performance::Maximum) => Some(Level::Max),
                _ => None,
            }
        } else if i.idle_pct > DOWN_ABOVE {
            self.calm = self.calm.saturating_add(1);
            if self.calm < CALM_STEPS {
                return None;
            }
            self.calm = 0;
            match i.current {
                Level::Turbo | Level::Max => Some(Level::High),
                Level::High if i.floor < 2 => Some(Level::Mid),
                Level::Mid if i.floor < 1 => Some(Level::Base),
                _ => None,
            }
        } else {
            self.calm = 0;
            None
        }
    }

    /// [`Governor::step`] under a power mode (02 §5.4): Automatic is `step`
    /// itself; Battery saver caps the level at Base or the audio floor,
    /// whichever is higher (a higher level found goes there at once); High
    /// performance keeps at least High while the screen is on.
    pub fn step_in(&mut self, i: Input, mode: PowerMode, screen_on: bool) -> Option<Level> {
        match mode {
            PowerMode::Auto => self.step(i),
            PowerMode::Saver => {
                let cap = Level::from_index(1 + i.floor.min(2));
                if i.current > cap {
                    self.calm = 0;
                    return Some(cap);
                }
                self.step(i).filter(|l| *l <= cap)
            }
            PowerMode::High if screen_on => {
                if i.current < Level::High {
                    self.calm = 0;
                    return Some(Level::High);
                }
                self.step(i).filter(|l| *l >= Level::High)
            }
            PowerMode::High => self.step(i),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    fn input(idle_pct: u32, current: Level, perf: Performance) -> Input {
        Input { idle_pct, current, floor: 0, perf }
    }

    /// Run `loads` (idle percent per step) from `start`; the levels after
    /// each step.
    fn run(start: Level, perf: Performance, floor: u8, loads: &[u32]) -> Vec<Level> {
        let mut g = Governor::new();
        let mut cur = start;
        loads
            .iter()
            .map(|&idle_pct| {
                if let Some(l) = g.step(Input { idle_pct, current: cur, floor, perf }) {
                    cur = l;
                }
                cur
            })
            .collect()
    }

    #[test]
    fn normal_steps_up_to_high_and_stops() {
        use Level::*;
        assert_eq!(run(Base, Performance::Normal, 0, &[10, 10, 10, 10]), [Mid, High, High, High]);
        assert_eq!(run(Low, Performance::Normal, 0, &[0]), [Mid]);
    }

    /// VER-13: Settings › System › Performance (0x0C07) — the setting's
    /// values as the governor takes them, and its effect: under load,
    /// Maximum reaches 288 MHz, Normal stays at High; idle, both settle
    /// at the same levels (the setting changes nothing else).
    #[test]
    fn performance_setting_values() {
        use Level::*;
        assert_eq!(Performance::from_setting(0), Performance::Normal);
        assert_eq!(Performance::from_setting(1), Performance::Maximum);
        assert_eq!(Performance::from_setting(7), Performance::Normal, "an invalid value is Normal");
        let busy = [5; 6];
        let normal = run(Base, Performance::from_setting(0), 0, &busy);
        let max = run(Base, Performance::from_setting(1), 0, &busy);
        assert_ne!(normal, max);
        assert_eq!((normal[5], max[5]), (High, Max));
        let idle = [95; 12];
        for start in [Base, Mid, High] {
            assert_eq!(
                run(start, Performance::Normal, 0, &idle),
                run(start, Performance::Maximum, 0, &idle),
                "idle from {start:?}"
            );
        }
    }

    /// Run `loads` under a power mode with the screen on or off.
    fn run_mode(start: Level, mode: PowerMode, screen: bool, floor: u8, loads: &[u32]) -> Vec<Level> {
        let mut g = Governor::new();
        let mut cur = start;
        loads
            .iter()
            .map(|&idle_pct| {
                let i = Input { idle_pct, current: cur, floor, perf: Performance::Normal };
                if let Some(l) = g.step_in(i, mode, screen) {
                    cur = l;
                }
                cur
            })
            .collect()
    }

    /// VER-13: Settings › Power › Power mode (0x0401, 02 §5.4) — the
    /// values as the governor takes them and their effect: Automatic is the
    /// governor itself; Battery saver stays at Base under load (or at the
    /// audio floor) and leaves a higher level at once; High performance
    /// holds High while the screen is on and is Automatic with it off.
    #[test]
    fn power_mode_setting_values() {
        use Level::*;
        assert_eq!(PowerMode::from_setting(0), PowerMode::Auto);
        assert_eq!(PowerMode::from_setting(1), PowerMode::Saver);
        assert_eq!(PowerMode::from_setting(2), PowerMode::High);
        assert_eq!(PowerMode::from_setting(9), PowerMode::Auto);
        let busy = [5; 6];
        let idle = [95; 12];
        for screen in [false, true] {
            for (start, loads) in [(Base, &busy[..]), (High, &idle[..]), (Mid, &[50; 4][..])] {
                assert_eq!(
                    run_mode(start, PowerMode::Auto, screen, 0, loads),
                    run(start, Performance::Normal, 0, loads),
                    "Automatic is the governor"
                );
            }
        }
        assert_eq!(run_mode(Base, PowerMode::Auto, true, 0, &busy)[5], High);
        assert_eq!(run_mode(Base, PowerMode::Saver, true, 0, &busy), [Base; 6]);
        assert_eq!(run_mode(Base, PowerMode::Saver, true, 1, &busy), [Mid; 6], "the audio floor holds");
        assert_eq!(run_mode(High, PowerMode::Saver, true, 0, &[50]), [Base], "down at once");
        assert_eq!(run_mode(Base, PowerMode::High, true, 0, &idle), [High; 12]);
        assert_eq!(
            run_mode(Base, PowerMode::High, false, 0, &idle),
            run(Base, Performance::Normal, 0, &idle)
        );
        assert_eq!(run_mode(High, PowerMode::High, false, 0, &idle)[11], Base, "screen off: Automatic");
    }

    #[test]
    fn maximum_goes_from_high_to_max() {
        use Level::*;
        assert_eq!(run(Base, Performance::Maximum, 0, &[10, 10, 10, 10]), [Mid, High, Max, Max]);
        let mut g = Governor::new();
        assert_eq!(g.step(input(0, Turbo, Performance::Maximum)), Some(Max));
    }

    #[test]
    fn never_max_when_normal() {
        // Every load sequence, every start level, every floor: Normal never
        // returns Max.
        let mut seed = 0x1234_5678u32;
        for start in Level::ALL {
            for floor in 0..3 {
                let mut g = Governor::new();
                let mut cur = start;
                for _ in 0..2000 {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let idle_pct = seed % 101;
                    let r = g.step(Input { idle_pct, current: cur, floor, perf: Performance::Normal });
                    assert_ne!(r, Some(Level::Max), "from {start:?}, floor {floor}");
                    if let Some(l) = r {
                        cur = l;
                    }
                }
            }
        }
    }

    /// Regression: `clock 288` with the governor on stayed at 288 MHz under
    /// Normal (PWR-5, 2026-11-24): a Max level found under Normal now goes
    /// to High at the next step.
    #[test]
    fn regress_max_found_under_normal_leaves_at_once() {
        let mut g = Governor::new();
        for idle in [0, 50, 100] {
            assert_eq!(g.step(input(idle, Level::Max, Performance::Normal)), Some(Level::High));
        }
    }

    #[test]
    fn same_hysteresis_down_from_max() {
        use Level::*;
        // Three calm steps in a row for each step down; a busy or middling
        // step restarts the count.
        assert_eq!(run(Max, Performance::Maximum, 0, &[90, 90, 90]), [Max, Max, High]);
        assert_eq!(
            run(Max, Performance::Maximum, 0, &[90, 90, 50, 90, 90, 90]),
            [Max, Max, Max, Max, Max, High]
        );
        assert_eq!(
            run(Max, Performance::Maximum, 0, &[90; 9]),
            [Max, Max, High, High, High, Mid, Mid, Mid, Base]
        );
        assert_eq!(
            run(High, Performance::Normal, 0, &[90; 9]),
            [High, High, Mid, Mid, Mid, Base, Base, Base, Base]
        );
    }

    #[test]
    fn floor_holds() {
        use Level::*;
        assert_eq!(
            run(Max, Performance::Maximum, 2, &[90; 9]),
            [Max, Max, High, High, High, High, High, High, High]
        );
        assert_eq!(run(High, Performance::Normal, 1, &[90; 9])[8], Mid);
    }

    #[test]
    fn middling_load_stays() {
        let mut g = Governor::new();
        for l in Level::ALL {
            for perf in [Performance::Normal, Performance::Maximum] {
                if l == Level::Max && perf == Performance::Normal {
                    continue;
                }
                assert_eq!(g.step(input(25, l, perf)), None);
                assert_eq!(g.step(input(70, l, perf)), None);
            }
        }
    }

    #[test]
    fn helpers() {
        assert_eq!(idle_percent(500_000, 1_000_000), 50);
        assert_eq!(idle_percent(5, 0), 500);
        assert_eq!(Performance::from_setting(1), Performance::Maximum);
        assert_eq!(Performance::from_setting(0), Performance::Normal);
        assert_eq!(Performance::from_setting(7), Performance::Normal);
        for (i, l) in Level::ALL.iter().enumerate() {
            assert_eq!(Level::from_index(i as u8), *l);
        }
        assert_eq!(Level::Max.mhz(), 288);
    }
}
