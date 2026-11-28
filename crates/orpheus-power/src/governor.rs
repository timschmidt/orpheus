//! The clock governor's decision (PWR-2, PWR-5; decision D-4 as revised
//! 2026-11-26 and the owner's Power mode rules of the same day): a pure
//! function of the main executor's idle share over the last step, the
//! current level, the clock floor the audio task asks for, the peripherals
//! in use, and Settings › Power › Power mode. The firmware runs it once a
//! second and carries out the level it returns.
//!
//! * **Battery saver**: "the lowest clocked performance level which is
//!   functional for required peripherals" — pinned at the highest of the
//!   minimum levels ([`PERIPHERAL_MIN`]) of the peripherals in use,
//!   whatever the load; audio floors do not raise it (playback may stutter,
//!   the synth may shed).
//! * **High performance**: pinned at `Max` (288 MHz, 1.3 V).
//! * **Automatic**: "all functionality at minimum battery cost … given the
//!   performance required to keep up". Never below the peripherals'
//!   minimum or the audio floor (which goes up to `Max`: nothing sheds in
//!   Automatic while a higher level would keep up). A saturated level
//!   (idle below [`UP_BELOW`] %: the work is not keeping up and its
//!   demand is not known) climbs ([`climb`]): to at least twice its clock
//!   each step (Low → Base → Mid → Turbo), and to `Max` only from a
//!   saturated `Turbo` — a sustained demand above what 192 MHz carries;
//!   otherwise the level is the one the power model ([`COST`], [`steady`])
//!   says carries the measured demand for the least energy; with nothing
//!   to do that is the peripherals' minimum. Down only after
//!   [`CALM_STEPS`] steps in a row asking for less (to the highest level
//!   asked in that run), so a load near a boundary does not flap.
//!
//! Why steady loads do not race to sleep (02 §5.1): the governor steps
//! once a second, far coarser than the audio blocks, so "racing" a steady
//! stream means sitting at `Max` with the core mostly in WFI. In the model
//! a cycle costs ∝ V² (1.0 V up to 144 MHz, 1.3 V at 288 MHz: 1.69× per
//! cycle) and a level's standing power (leakage, PLL, clock tree) grows
//! with its voltage and clock, so a demand that a 1.0 V level carries is
//! cheapest at the lowest such level. Racing would pay only where
//! finishing sooner lets the player sit in a much cheaper state afterwards
//! for long enough, which the model's figures do not show. The figures are
//! estimates: the datasheet gives no current per level (U-PWR-6) and
//! M13's measured drain replaces them; the decision is this one function.

pub use crate::clock::Level;

/// Settings › Power › Power mode (02 §5.4, owner 2026-11-26).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PowerMode {
    /// The cheapest level that keeps up ([`Governor::step`]).
    #[default]
    Auto,
    /// Battery saver: pinned at the peripherals' minimum.
    Saver,
    /// High performance: pinned at `Max`.
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

    /// The mode the governor follows: Automatic (the default) while a host
    /// test runs (the test look, a test session or the test mailbox), so a
    /// suite gives the same result on a player the owner left in another
    /// mode; the stored one otherwise.
    #[must_use]
    pub const fn for_governor(stored: i16, under_test: bool) -> PowerMode {
        if under_test { PowerMode::Auto } else { PowerMode::from_setting(stored) }
    }
}

/// Peripherals with a minimum core clock, as bits of [`Input::active`].
pub mod periph {
    /// USB while connected (VBUS present).
    pub const USB: u8 = 1 << 0;
    /// The card (SD0).
    pub const CARD: u8 = 1 << 1;
    /// The display while it is on.
    pub const DISPLAY: u8 = 1 << 2;
    /// The DAC and the audio path while something plays.
    pub const AUDIO: u8 = 1 << 3;
    /// The NOR flash pager.
    pub const PAGER: u8 = 1 << 4;
    /// The keys.
    pub const KEYS: u8 = 1 << 5;
    /// Always in use.
    pub const ALWAYS: u8 = CARD | PAGER | KEYS;
}

/// The lowest level at which each peripheral works (bit order of
/// [`periph`]), with its source:
///
/// * **USB `Base`** (48 MHz) — the lowest level Orpheus's USB has run at
///   [HW] (and the ROM's USB clock); the controller has its own UTMI+ PHY
///   clock (`0xC0001100`) but the datasheet states no minimum core/AHB
///   clock for it. **Unknown: 24 MHz is to be tested (U-PWR-5).** Assumed
///   `Base` meanwhile because a controller that stops answering trips the
///   USB liveness check, which holds the watchdog: the player would restart
///   over and over on the cable. `Low` once the player passes it.
/// * **Card `Low`** — SD0's clock is CK24M or DEVPLL, not the core ([DS]
///   §4.3 `CMU_SD0CLK`; 01 §7); the card runs at 24 MHz from CK24M [HW].
///   The core minimum for its DMA itself: assumed, to be tested (U-PWR-5).
/// * **Display `Low`** — LCD clock CK24M, DMA blit with no HOSC detour
///   (01 §9 U-LCM-1/2 [HW]); the stock firmware drives the panel with the
///   core on HOSC, 24 MHz (system.md §1.5) [code].
/// * **Audio `Low`** — the DAC runs from the audio PLLs ([DS] §4.3; 01 §7),
///   its DMA moves 176 KB/s. The decoders' own needs are the audio
///   floor, not this. Assumed, to be tested (U-PWR-5).
/// * **Pager `Low`** — SPI0 on CK24M at `Low` (24 Mbit/s, half the rate of
///   the PLL levels: page loads take twice as long); the clock self-test
///   passed its NOR DMA CRC at 24 MHz (2026-11-24) [HW].
/// * **Keys `Low`** — LRADC and the key timer run from HOSC (system.md
///   §1.5); the stock UI scans keys at 24 MHz with the screen off [code].
pub const PERIPHERAL_MIN: [Level; 6] =
    [Level::Base, Level::Low, Level::Low, Level::Low, Level::Low, Level::Low];

/// The highest minimum level of the peripherals in `active` ([`periph`]
/// bits); `Low` when none needs more.
#[must_use]
pub const fn peripheral_min(active: u8) -> Level {
    let mut l = Level::Low;
    let mut i = 0;
    while i < PERIPHERAL_MIN.len() {
        if active & (1 << i) != 0 && PERIPHERAL_MIN[i] as u8 > l as u8 {
            l = PERIPHERAL_MIN[i];
        }
        i += 1;
    }
    l
}

/// Below this idle share (percent) the level is saturated ([`climb`]).
pub const UP_BELOW: u32 = 25;
/// A level other than the current one is chosen only if the demand fills
/// at most this share of it (percent), so after a step down the idle share
/// is at least 40 %, well above [`UP_BELOW`].
pub const FIT_PCT: u32 = 60;
/// The current level carries the demand while it fills at most this share
/// (percent; above it the level is saturated).
pub const STAY_PCT: u32 = 100 - UP_BELOW;
/// Steps in a row asking for a lower level before the step down.
pub const CALM_STEPS: u8 = 2;

/// Modelled power of one level (µW): `on` while the level is selected
/// (leakage at its voltage, the PLL and clock tree; the core in WFI), and
/// `per_mhz` for each MHz of busy core on top (dynamic, ∝ V²).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cost {
    pub on_uw: u32,
    pub per_mhz_uw: u32,
}

/// The power model, by level ([`Level::ALL`] order). **Estimates, not
/// measurements** (U-PWR-6): 100 µW/MHz dynamic at 1.0 V scaled by V²
/// (1.1 V ×1.21, 1.3 V ×1.69); standing 1,000 µW leakage at 1.0 V scaled
/// by V², plus 4 µW/MHz for COREPLL and the clock tree (none at `Low`,
/// PLL off). Only their order matters to the decision; M13 replaces them.
pub const COST: [Cost; 6] = [
    Cost { on_uw: 1_000, per_mhz_uw: 100 },
    Cost { on_uw: 1_000 + 4 * 48, per_mhz_uw: 100 },
    Cost { on_uw: 1_000 + 4 * 96, per_mhz_uw: 100 },
    Cost { on_uw: 1_000 + 4 * 144, per_mhz_uw: 100 },
    Cost { on_uw: 1_210 + 4 * 192, per_mhz_uw: 121 },
    Cost { on_uw: 1_690 + 4 * 288, per_mhz_uw: 169 },
];

/// The idle part of one sleep of the main executor: `slept` µs from its
/// WFE to its wake-up, less `rt` µs the RT executor ran meanwhile (the
/// audio task decodes in an interrupt while thread mode sleeps; that is
/// work, not idle).
#[must_use]
pub const fn sleep_idle_us(slept: u32, rt: u32) -> u32 {
    slept.saturating_sub(rt)
}

/// The idle share in percent from the idle time and the wall time of a
/// step (microseconds); a zero-length step counts as 1 µs.
#[must_use]
pub fn idle_percent(idle_us: u64, dt_us: u64) -> u32 {
    (idle_us.saturating_mul(100) / dt_us.max(1)).min(u64::from(u32::MAX)) as u32
}

/// The demand in MHz of busy core: the busy share of the step at the
/// current level's clock.
#[must_use]
pub const fn demand_mhz(idle_pct: u32, current: Level) -> u32 {
    let busy = 100 - if idle_pct > 100 { 100 } else { idle_pct };
    busy * current.mhz() as u32 / 100
}

/// Modelled power (µW) of carrying `demand` MHz at `l`.
#[must_use]
pub const fn power_uw(l: Level, demand: u32) -> u32 {
    let c = COST[l as usize];
    c.on_uw + demand * c.per_mhz_uw
}

/// The level that carries `demand` MHz for the least modelled power, at
/// or above `floor`: `current` if it fills at most [`STAY_PCT`] of it,
/// any other level if it fills at most [`FIT_PCT`]; `Max` when none does.
/// Ties go to the lower level.
#[must_use]
pub fn steady(demand: u32, floor: Level, current: Level) -> Level {
    let (mut best, mut best_uw) = (Level::Max, u32::MAX);
    for l in Level::ALL {
        let fill = if l == current { STAY_PCT } else { FIT_PCT };
        if l >= floor && demand * 100 <= fill * u32::from(l.mhz()) {
            let p = power_uw(l, demand);
            if p < best_uw {
                (best, best_uw) = (l, p);
            }
        }
    }
    best
}

/// The level after a saturated step at `l`: the lowest at least twice its
/// clock (the demand is at least what it carried), but `Max` (1.3 V, the
/// costliest) only from `Turbo`.
#[must_use]
pub const fn climb(l: Level) -> Level {
    match l {
        Level::Low => Level::Base,
        Level::Base => Level::Mid,
        Level::Mid | Level::High => Level::Turbo,
        Level::Turbo | Level::Max => Level::Max,
    }
}

/// The level the audio task raises the clock to at once for its floor
/// (`floor`, a level), if any: never down, and never in a pinned mode
/// (Battery saver stays at the peripherals' minimum whatever the audio
/// needs; High performance is at `Max` already).
#[must_use]
pub fn raise_for_floor(mode: PowerMode, current: Level, floor: Level) -> Option<Level> {
    (mode == PowerMode::Auto && current < floor).then_some(floor)
}

/// How heavy a codec's decoding is, for its clock floor ([`track_floor`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Decoding {
    /// MP3, WAV, AIFF, Speex and the like: Base carries them.
    Light,
    /// FLAC: one block (4,096 frames and more) is longer than the PCM
    /// ring, and at Base its decode took 57 ms of a 93 ms block on the
    /// player (AQ-0, 2026-11-28): ten underruns a second.
    Blocky,
    /// Vorbis, Opus: decoders that run from paged code.
    Paged,
}

/// A codec track's own floor (`track` of [`audio_floor`]): High for paged
/// decoders and for sources above 48 kHz, which the DSP chain decimates;
/// Mid for FLAC; else none (the governor follows the load).
#[must_use]
pub const fn track_floor(d: Decoding, rate: u32) -> u8 {
    match d {
        Decoding::Paged => 2,
        _ if rate > 48_000 => 2,
        Decoding::Blocky => 1,
        Decoding::Light => 0,
    }
}

/// The audio task's clock floor (0 Base, 1 Mid, 2 High, 3 Turbo, 4 Max:
/// [`Level::from_floor`]): the track's (`track`, the codec's or the
/// synth's), at least Mid while the DSP chain works — **none while the
/// track is paused** (2026-11-26: a track resumed paused at start-up held
/// Mid 96 MHz for nothing); `boost` (a picture decoding, the card task)
/// asks High whatever the track does.
#[must_use]
pub const fn audio_floor(track: u8, dsp_active: bool, paused: bool, boost: bool) -> u8 {
    let t = if paused {
        0
    } else if dsp_active && track == 0 {
        1
    } else {
        track
    };
    if boost && t < 2 { 2 } else { t }
}

/// The clock floor while speech sounds (04 §8 budgets: speech ≈ 11 % of
/// Mid at 48 kHz output, 22 % of Base): Mid (96 MHz).
pub const SPEECH_FLOOR: u8 = 1;

/// The audio floor `f` ([`audio_floor`]) with speech: at least
/// [`SPEECH_FLOOR`] while it sounds (TTS-3), whatever plays. Like every
/// floor it raises the level only in Automatic ([`raise_for_floor`],
/// [`Governor::step`]): Battery saver stays pinned and speech degrades
/// there (gaps, never a hang).
#[must_use]
pub const fn with_speech(f: u8, speaking: bool) -> u8 {
    if speaking && f < SPEECH_FLOOR { SPEECH_FLOOR } else { f }
}

/// The floor one step takes (`Input::floor`) from the audio floor `f`
/// ([`audio_floor`]): none (`Low`) when nothing plays — or the track is
/// paused, which the governor treats as nothing playing (the audio
/// peripheral's minimum is `Low`; a key click while paused plays there) —
/// unless the floor asks for more (a picture's boost).
#[must_use]
pub const fn floor_level(f: u8, playing: bool, paused: bool) -> Level {
    if f == 0 && (!playing || paused) { Level::Low } else { Level::from_floor(f) }
}

/// What one step looks at.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Input {
    /// Idle share of the main executor since the last step (percent).
    pub idle_pct: u32,
    pub current: Level,
    /// Lowest level the audio task allows (`Low` when nothing plays).
    pub floor: Level,
    /// The peripherals in use ([`periph`] bits).
    pub active: u8,
    /// Highest level allowed, whatever the mode or the demand ([`Level::Max`]
    /// for none): the USB card reader with the card at 48 MHz asks
    /// [`CARD_HS_CAP`].
    pub cap: Level,
}

/// The core's ceiling while the card runs at its 48 MHz high-speed clock
/// and a USB card reader moves data (PERF-SD round 2, 2026-11-27): at Max
/// (288 MHz, `SYSCLK` bit 8) the card's writes failed whenever the USB
/// OUT DMA ran beside the SD write DMA (≈ every write op hung 500 ms, the
/// host saw I/O errors); at Turbo, High, Mid and Base they worked (16 MiB
/// each, integrity checked), Mid as fast as any (reads 14.3 MB/s, writes
/// 6.7 MB/s). Mid also spends the least on the transport's spinning waits.
pub const CARD_HS_CAP: Level = Level::Mid;

/// `want` limited to `cap`.
#[must_use]
pub fn capped(want: Level, cap: Level) -> Level {
    want.min(cap)
}

/// The governor's memory between steps: the run of steps asking for a
/// lower level, and the highest level asked in it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Governor {
    calm: u8,
    hold: Level,
}

impl Default for Governor {
    fn default() -> Self {
        Self::new()
    }
}

impl Governor {
    #[must_use]
    pub const fn new() -> Governor {
        Governor { calm: 0, hold: Level::Low }
    }

    /// The level to switch to under `mode`, if any.
    pub fn step(&mut self, i: Input, mode: PowerMode) -> Option<Level> {
        let want = match mode {
            PowerMode::Saver => peripheral_min(i.active),
            PowerMode::High => Level::Max,
            PowerMode::Auto => self.auto(i),
        };
        let want = capped(want, i.cap);
        if mode != PowerMode::Auto {
            self.calm = 0;
        }
        (want != i.current).then_some(want)
    }

    fn auto(&mut self, i: Input) -> Level {
        if i.idle_pct < UP_BELOW {
            self.calm = 0;
            return climb(i.current).max(i.floor).max(peripheral_min(i.active));
        }
        let min = i.floor.max(peripheral_min(i.active));
        let t = steady(demand_mhz(i.idle_pct, i.current), min, i.current);
        if t >= i.current {
            self.calm = 0;
            return t;
        }
        self.hold = if self.calm == 0 { t } else { self.hold.max(t) };
        self.calm += 1;
        if self.calm < CALM_STEPS {
            return i.current;
        }
        self.calm = 0;
        self.hold
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Level::*;
    use std::vec::Vec;

    /// The idle share a steady `demand` (MHz) leaves at `l`.
    fn idle_at(demand: u32, l: Level) -> u32 {
        100 - (demand * 100 / u32::from(l.mhz())).min(100)
    }

    fn input(demand: u32, cur: Level, floor: Level, active: u8) -> Input {
        Input { idle_pct: idle_at(demand, cur), current: cur, floor, active, cap: Max }
    }

    /// PERF-SD round 2, 2026-11-27: with the card at its 48 MHz clock, the
    /// card reader's writes failed at Max (the governor climbed there: the
    /// transport's waits spin, the executor is never idle; Power mode High
    /// asks Max outright). The ceiling holds in every mode. (The audio
    /// floor, `raise_for_floor`, never applies then: the card is with the
    /// USB task, nothing plays.)
    #[test]
    fn regress_card_reader_writes_failed_at_max_with_the_card_at_48_mhz() {
        for mode in [PowerMode::Auto, PowerMode::High, PowerMode::Saver] {
            let mut g = Governor::new();
            let mut cur = Base;
            for _ in 0..10 {
                let i = Input { cap: CARD_HS_CAP, ..input(1_000, cur, Low, periph::ALWAYS | periph::USB) };
                if let Some(l) = g.step(i, mode) {
                    cur = l;
                }
                assert!(cur <= CARD_HS_CAP, "{mode:?}: {cur:?}");
            }
        }
    }

    /// Run a demand per step (MHz) from `start`; the level after each step.
    fn run_with(mode: PowerMode, start: Level, floor: Level, active: u8, demand: &[u32]) -> Vec<Level> {
        let mut g = Governor::new();
        let mut cur = start;
        demand
            .iter()
            .map(|&d| {
                if let Some(l) = g.step(input(d, cur, floor, active), mode) {
                    cur = l;
                }
                cur
            })
            .collect()
    }

    fn run(mode: PowerMode, start: Level, floor: Level, demand: &[u32]) -> Vec<Level> {
        run_with(mode, start, floor, periph::ALWAYS, demand)
    }

    fn auto(start: Level, demand: &[u32]) -> Vec<Level> {
        run(PowerMode::Auto, start, Low, demand)
    }

    /// Where a constant demand settles in Automatic, from `start`.
    fn settles(start: Level, demand: u32) -> Level {
        *auto(start, &[demand; 12]).last().unwrap()
    }

    /// PWR-5: a saturated level climbs (at least doubling its clock, Max
    /// only from Turbo), and a demand that a lower level carries comes back
    /// down after CALM_STEPS.
    #[test]
    fn saturated_work_climbs_and_comes_back() {
        // 30 MHz of work at 24 MHz: saturated → Base, which carries it.
        assert_eq!(auto(Low, &[30; 4]), [Base; 4]);
        // More than anything below Max carries: Max after four steps.
        assert_eq!(auto(Low, &[300; 5]), [Base, Mid, Turbo, Max, Max]);
        assert_eq!(auto(High, &[300; 2]), [Turbo, Max]);
        // 80 MHz: Mid saturates, Turbo carries it, then High is cheaper.
        assert_eq!(auto(Mid, &[80; 4]), [Turbo, Turbo, High, High]);
        for l in Level::ALL {
            assert!(climb(l) > l || l == Max, "{l:?}");
            assert!(climb(l) != Max || l >= Turbo, "{l:?}");
        }
        // The work ends: down to Low (nothing to do) after two steps.
        assert_eq!(auto(Max, &[0, 0, 0]), [Max, Low, Low]);
        assert_eq!(auto(Base, &[1, 1, 1]), [Base, Low, Low]);
    }

    /// PWR-5: Max is kept only for a sustained demand above what Turbo
    /// carries; below it the level steps down within CALM_STEPS and stays
    /// (no flapping), and wherever it settles the work keeps up.
    #[test]
    fn max_only_for_sustained_demand_above_turbo() {
        let turbo_cap = FIT_PCT * 192 / 100;
        for d in (0..=300).step_by(5) {
            for start in Level::ALL {
                let l = auto(start, &[d; 20]);
                let end = l[19];
                assert!(l[12..].iter().all(|x| *x == end), "{d} MHz from {start:?}: {l:?}");
                if d >= 150 {
                    assert_eq!(end, Max, "{d} MHz");
                } else if d <= turbo_cap {
                    assert!(end < Max, "{d} MHz from {start:?} stays at Max: {l:?}");
                }
                // From below, Max only through a saturated Turbo.
                if start < Max && d <= STAY_PCT * 192 / 100 {
                    assert!(!l.contains(&Max), "{d} MHz from {start:?}: {l:?}");
                }
                if d < 288 * STAY_PCT / 100 {
                    assert!(idle_at(d, end) >= UP_BELOW, "{d} MHz at {end:?} does not keep up");
                }
            }
        }
        // Down from Max after the demand drops: within CALM_STEPS.
        let l = auto(Max, &[250, 250, 100, 100, 100, 100]);
        assert_eq!(&l[..3], [Max, Max, Max]);
        assert_eq!(l[3], Turbo, "{l:?}");
    }

    /// The steady level is the cheapest one in the model: with the
    /// estimates, the lowest level carrying the demand at 1.0 V.
    #[test]
    fn steady_demand_runs_at_the_cheapest_level() {
        assert_eq!(settles(Max, 1), Low);
        assert_eq!(settles(Max, 14), Low);
        assert_eq!(settles(Max, 20), Base);
        assert_eq!(settles(Max, 50), Mid);
        assert_eq!(settles(Max, 80), High);
        assert_eq!(settles(Max, 110), Turbo);
        assert_eq!(settles(Max, 200), Max);
        for d in [0, 10, 20] {
            for w in Level::ALL.windows(2) {
                assert!(power_uw(w[0], d) < power_uw(w[1], d), "{d} MHz {w:?}");
            }
        }
        assert_eq!(steady(500, Low, Low), Max, "nothing carries it");
        assert_eq!(steady(0, High, Max), High, "never below the minimum");
    }

    /// The audio floor holds in Automatic, up to Max (nothing sheds while
    /// a higher level would keep up).
    #[test]
    fn floor_holds_up_to_max() {
        for floor in Level::ALL {
            let l = run(PowerMode::Auto, Max, floor, &[0; 6]);
            assert_eq!(l[5], floor, "floor {floor:?}");
            let l = run(PowerMode::Auto, Low, floor, &[0; 3]);
            assert_eq!(l[0], floor, "a floor above the level raises it at once");
        }
        assert_eq!(raise_for_floor(PowerMode::Auto, Base, Max), Some(Max));
        assert_eq!(raise_for_floor(PowerMode::Auto, High, Turbo), Some(Turbo));
        assert_eq!(raise_for_floor(PowerMode::Auto, Max, High), None, "never down");
        assert_eq!(raise_for_floor(PowerMode::Saver, Low, Max), None, "Battery saver");
        assert_eq!(raise_for_floor(PowerMode::High, Max, Max), None);
    }

    /// PWR-5: with the screen off the UI does not draw; whatever moderate
    /// background load comes and goes (up to what Turbo carries), the
    /// governor never chooses Max, and from Max it leaves.
    #[test]
    fn no_max_with_the_screen_off_and_nothing_demanding_it() {
        let mut seed = 0x1234_5678u32;
        for start in Level::ALL {
            for floor in [Low, Base, Mid, High] {
                let mut g = Governor::new();
                let mut cur = start;
                for n in 0..2000 {
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    // A demand up to 100 MHz (MP3 + DSP, the card task).
                    let d = seed % 101;
                    let r = g.step(input(d, cur, floor, periph::ALWAYS | periph::AUDIO), PowerMode::Auto);
                    assert_ne!(r, Some(Max), "from {start:?}, floor {floor:?}, step {n}, {d} MHz at {cur:?}");
                    if let Some(l) = r {
                        cur = l;
                    }
                    if n >= usize::from(CALM_STEPS) {
                        assert_ne!(cur, Max, "from {start:?}");
                    }
                }
            }
        }
        for start in Level::ALL {
            assert_eq!(settles(start, 0), Low, "an idle player from {start:?}");
        }
    }

    /// The peripherals' minimum levels: the highest of those in use, in
    /// Automatic (the lowest it goes) and Battery saver (where it stays).
    #[test]
    fn peripheral_minimum() {
        use periph::*;
        assert_eq!(peripheral_min(0), Low);
        assert_eq!(peripheral_min(ALWAYS | DISPLAY | AUDIO), Low);
        assert_eq!(peripheral_min(USB), PERIPHERAL_MIN[0]);
        assert_eq!(peripheral_min(ALWAYS | USB), PERIPHERAL_MIN[0]);
        assert_eq!(PERIPHERAL_MIN[0], Base, "U-PWR-5 unanswered: USB assumed to need 48 MHz");
        for (i, l) in PERIPHERAL_MIN.iter().enumerate() {
            assert_eq!(peripheral_min(1 << i), *l);
            assert!(*l < Max);
        }
        let usb = ALWAYS | USB;
        assert_eq!(run_with(PowerMode::Auto, Max, Low, usb, &[0; 4])[3], Base);
        assert_eq!(run_with(PowerMode::Saver, Max, Low, usb, &[200; 3]), [Base; 3]);
        assert_eq!(run_with(PowerMode::Saver, Max, Low, ALWAYS, &[200; 3]), [Low; 3]);
        // The cable comes and goes: Battery saver follows at once.
        let mut g = Governor::new();
        assert_eq!(g.step(input(0, Low, Low, usb), PowerMode::Saver), Some(Base));
        assert_eq!(g.step(input(0, Base, Low, ALWAYS), PowerMode::Saver), Some(Low));
    }

    /// VER-13: Settings › Power › Power mode (0x0401) — the values as the
    /// governor takes them and their effect on the same load: Battery
    /// saver pins the peripherals' minimum (24 MHz without USB) whatever the
    /// load or the audio floor; High performance pins Max (288 MHz) whatever
    /// happens, idle included; Automatic takes the work to Max while it
    /// does not keep up and idles at the minimum.
    #[test]
    fn power_mode_setting_values() {
        assert_eq!(PowerMode::from_setting(0), PowerMode::Auto);
        assert_eq!(PowerMode::from_setting(1), PowerMode::Saver);
        assert_eq!(PowerMode::from_setting(2), PowerMode::High);
        assert_eq!(PowerMode::from_setting(9), PowerMode::Auto);
        // Work, then nothing.
        let load = [300, 300, 300, 300, 300, 0, 0, 0, 0];
        for start in Level::ALL {
            for floor in Level::ALL {
                assert_eq!(run(PowerMode::Saver, start, floor, &load), [Low; 9], "{start:?} {floor:?}");
                assert_eq!(run(PowerMode::High, start, floor, &load), [Max; 9], "{start:?} {floor:?}");
            }
            let a = run(PowerMode::Auto, start, Low, &load);
            assert_eq!((a[4], a[5], a[8]), (Max, Max, Low), "{start:?}: {a:?}");
        }
        let [s, h, a] =
            [PowerMode::Saver, PowerMode::High, PowerMode::Auto].map(|m| run(m, Base, Low, &load));
        assert!(s != h && h != a && a != s);
        // The pinned modes leave a level found elsewhere at once (a test's
        // `clock` pin, a mode just changed).
        let mut g = Governor::new();
        for l in Level::ALL {
            let i = Input { idle_pct: 50, current: l, floor: Max, active: periph::ALWAYS, cap: Max };
            assert_eq!(g.step(i, PowerMode::Saver), (l != Low).then_some(Low));
            assert_eq!(g.step(i, PowerMode::High), (l != Max).then_some(Max));
        }
    }

    /// Regression (2026-11-26, owner): a suite run on a player the owner
    /// left in Battery saver would have run at 24 MHz (or with High
    /// performance at 288 MHz) and failed its timings; under a host test
    /// the governor follows Automatic, the default.
    #[test]
    fn regress_tests_follow_automatic_whatever_the_owner_chose() {
        for v in [0, 1, 2, 7] {
            assert_eq!(PowerMode::for_governor(v, true), PowerMode::Auto);
            assert_eq!(PowerMode::for_governor(v, false), PowerMode::from_setting(v));
        }
    }

    /// Regression (2026-11-26, the lead on the player, build `8240f128`):
    /// a track loaded but **paused** (resumed paused at start-up) kept its
    /// clock floor — Mid 96 MHz (an MP3 with the DSP chain on) — with
    /// nothing to play. Paused, the floor is released and the governor
    /// comes down to the peripherals' minimum (Low; Base on the cable);
    /// on play the floor is back at once (`raise_for_floor`). Fails with
    /// the old rule (the floor `max(codec, dsp)` whatever the pause, and a
    /// loaded track never below Base).
    #[test]
    fn regress_paused_track_keeps_clock_floor() {
        let playing = audio_floor(0, true, false, false);
        assert_eq!(floor_level(playing, true, false), Mid);
        let paused = audio_floor(0, true, true, false);
        assert_eq!(paused, 0);
        let fl = floor_level(paused, true, true);
        assert_eq!(fl, Low);
        let on_battery = run_with(PowerMode::Auto, Mid, fl, periph::ALWAYS | periph::DISPLAY, &[1; 6]);
        assert_eq!(*on_battery.last().unwrap(), Low);
        let on_cable = run_with(PowerMode::Auto, Mid, fl, periph::ALWAYS | periph::USB, &[1; 6]);
        assert_eq!(*on_cable.last().unwrap(), Base);
        // Play again: back to Mid at once, not a governor step later.
        assert_eq!(raise_for_floor(PowerMode::Auto, Low, floor_level(playing, true, false)), Some(Mid));
        // Every track floor (Vorbis High, the synth up to Max) likewise.
        for t in 1..=4 {
            assert_eq!(audio_floor(t, false, true, false), 0);
            assert_eq!(audio_floor(t, true, false, false), t);
            assert_eq!(floor_level(audio_floor(t, false, false, false), true, false), Level::from_floor(t));
        }
    }

    /// AQ-0, 2026-11-28, on the player: the main executor counted the whole
    /// of each WFE as idle, also the time the RT executor (the audio task,
    /// in an interrupt) decoded meanwhile. A FLAC of the demo card played
    /// at Base 48 MHz with ten underruns a second (audible as clicks) while
    /// `dev stats` showed 74 % idle, and the governor never raised the
    /// clock. A sleep's idle part is its time less the RT executor's.
    #[test]
    fn regress_rt_executor_counted_as_idle() {
        assert_eq!(sleep_idle_us(10_000, 0), 10_000);
        assert_eq!(sleep_idle_us(10_000, 7_400), 2_600);
        assert_eq!(sleep_idle_us(1_000, 1_200), 0);
        // A second of sleeps, 74 % of it the RT executor: 26 % idle, and
        // the governor wants more than Base.
        let idle = (0..100).map(|_| sleep_idle_us(10_000, 7_400)).sum::<u32>();
        let pct = idle_percent(u64::from(idle), 1_000_000);
        assert_eq!(pct, 26);
        assert!(demand_mhz(pct, Level::Base) > 30);
    }

    /// TTS-3b, 2026-11-28, on the player with the cable in (Base 48 MHz):
    /// the demo card's "05 Variatio 4 (complete, 96 kHz 24 bit).flac"
    /// started at floor 0 (only Vorbis and Opus had one), decoded at
    /// 46 ms a 42.7 ms block, underran at once and was given up ("decoder
    /// too slow for this player") before the governor's once-a-second
    /// step could raise the clock; started from the UI it lived only
    /// because its cover art's decode asked High meanwhile. A source the
    /// DSP chain decimates (above 48 kHz) now asks High from its first block.
    #[test]
    fn regress_hires_flac_given_up_at_base() {
        use Decoding::{Blocky, Light, Paged};
        assert_eq!(track_floor(Blocky, 96_000), 2);
        assert_eq!(track_floor(Light, 88_200), 2);
        assert_eq!(track_floor(Blocky, 192_000), 2);
        assert_eq!(track_floor(Light, 48_000), 0);
        assert_eq!(track_floor(Light, 44_100), 0);
        assert_eq!(track_floor(Paged, 44_100), 2);
        assert_eq!(audio_floor(track_floor(Blocky, 96_000), false, false, false), 2);
        assert_eq!(audio_floor(track_floor(Blocky, 96_000), false, true, false), 0);
    }

    /// AQ-0, 2026-11-28, on the player (the cable in, the governor fixed
    /// as in `regress_rt_executor_counted_as_idle`): the demo card's
    /// 44.1 kHz FLAC still underran 30 times in its first seconds at Base
    /// (`ub: dec=`), until the governor's once-a-second steps had raised
    /// the clock. FLAC asks Mid from its first block.
    #[test]
    fn regress_flac_underran_at_base_before_the_governor_rose() {
        assert_eq!(track_floor(Decoding::Blocky, 44_100), 1);
        assert_eq!(track_floor(Decoding::Blocky, 48_000), 1);
        assert_eq!(floor_level(track_floor(Decoding::Blocky, 44_100), true, false), Level::Mid);
    }

    /// The floor's parts: the track's, Mid for the DSP chain, the picture
    /// boost (High, also while a track is paused or none is loaded).
    #[test]
    fn audio_floor_parts() {
        assert_eq!(audio_floor(0, false, false, false), 0);
        assert_eq!(audio_floor(0, true, false, false), 1);
        assert_eq!(audio_floor(2, true, false, false), 2);
        assert_eq!(audio_floor(0, false, false, true), 2);
        assert_eq!(audio_floor(0, true, true, true), 2);
        assert_eq!(audio_floor(4, false, false, true), 4);
        assert_eq!(floor_level(0, false, false), Low);
        assert_eq!(floor_level(2, false, false), High);
        assert_eq!(floor_level(0, true, false), Base);
        assert_eq!(floor_level(2, true, true), High);
    }

    #[test]
    fn speech_floor_is_mid_and_only_in_automatic() {
        assert_eq!(with_speech(0, true), 1);
        assert_eq!(with_speech(0, false), 0);
        assert_eq!(with_speech(3, true), 3);
        // Nothing playing, speech alone: Mid, not Low.
        assert_eq!(floor_level(with_speech(0, true), false, false), Mid);
        assert_eq!(floor_level(with_speech(0, true), true, true), Mid);
        assert_eq!(
            raise_for_floor(PowerMode::Auto, Base, Level::from_floor(with_speech(0, true))),
            Some(Mid)
        );
        assert_eq!(raise_for_floor(PowerMode::Saver, Low, Level::from_floor(with_speech(0, true))), None);
    }

    #[test]
    fn helpers() {
        assert_eq!(idle_percent(500_000, 1_000_000), 50);
        assert_eq!(idle_percent(5, 0), 500);
        assert_eq!(demand_mhz(500, Max), 0);
        assert_eq!(demand_mhz(0, Max), 288);
        assert_eq!(demand_mhz(50, High), 72);
        assert_eq!(Governor::default(), Governor::new());
    }
}
