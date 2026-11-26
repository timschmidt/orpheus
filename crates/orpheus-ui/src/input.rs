//! Key input: raw key edges with timestamps in, key phases out (05 §3).
//! All hold and repeat thresholds live here and nowhere else.
//!
//! The firmware's input task calls [`KeyMapper::press`] / [`KeyMapper::release`]
//! on debounced ladder edges, then drains [`KeyMapper::poll`] and sleeps until
//! [`KeyMapper::next_deadline`]. [`KeyGate`] applies screen-off and lock rules.

use crate::event::{Instant, Key, KeyPhase};

/// Timing of the key grammar, in milliseconds.
#[derive(Clone, Copy, Debug)]
pub struct Timing {
    /// Hold threshold for all keys but Power.
    pub hold: u32,
    /// Hold threshold for Power (power menu).
    pub power_hold: u32,
    /// Power held this long: power off without the menu.
    pub power_long: u32,
    /// Repeat interval at the start (8 per second).
    pub repeat_slow: u32,
    /// Repeat interval at full speed (30 per second).
    pub repeat_fast: u32,
    /// Repeating at the slow rate lasts this long after the hold, then the
    /// interval ramps linearly to `repeat_fast` over `accel_ramp`.
    pub accel_after: u32,
    pub accel_ramp: u32,
}

impl Timing {
    pub const DEFAULT: Timing = Timing {
        hold: 500,
        power_hold: 1500,
        power_long: 4000,
        repeat_slow: 125,
        repeat_fast: 33,
        accel_after: 1000,
        accel_ramp: 1000,
    };
}

impl Default for Timing {
    fn default() -> Self {
        Timing::DEFAULT
    }
}

#[derive(Clone, Copy, Default)]
struct KeyState {
    down: bool,
    since: u64,
    held: bool,
    long: bool,
    next_repeat: u64,
}

const QUEUE: usize = 16;

/// Turns key edges into [`KeyPhase`] events.
pub struct KeyMapper {
    keys: [KeyState; 10],
    queue: [(Key, KeyPhase); QUEUE],
    head: u8,
    len: u8,
    /// Swap Up/Down and Prev/Next (display rotated by 180°, 05 §4).
    pub rotated: bool,
    pub timing: Timing,
    /// Events dropped because the queue was full (should stay 0).
    pub overflows: u32,
}

impl Default for KeyMapper {
    fn default() -> Self {
        Self::new()
    }
}

impl KeyMapper {
    #[must_use]
    pub const fn new() -> KeyMapper {
        KeyMapper {
            keys: [KeyState { down: false, since: 0, held: false, long: false, next_repeat: 0 }; 10],
            queue: [(Key::Up, KeyPhase::Down); QUEUE],
            head: 0,
            len: 0,
            rotated: false,
            timing: Timing::DEFAULT,
            overflows: 0,
        }
    }

    fn map(&self, k: Key) -> Key {
        if !self.rotated {
            return k;
        }
        match k {
            Key::Up => Key::Down,
            Key::Down => Key::Up,
            Key::Prev => Key::Next,
            Key::Next => Key::Prev,
            other => other,
        }
    }

    fn push(&mut self, k: Key, p: KeyPhase) {
        if usize::from(self.len) == QUEUE {
            self.overflows = self.overflows.wrapping_add(1);
            return;
        }
        let i = (usize::from(self.head) + usize::from(self.len)) % QUEUE;
        self.queue[i] = (k, p);
        self.len += 1;
    }

    fn hold_ms(&self, k: Key) -> u64 {
        u64::from(if k == Key::Power { self.timing.power_hold } else { self.timing.hold })
    }

    /// A key went down at `now`.
    pub fn press(&mut self, raw: Key, now: Instant) {
        let k = self.map(raw);
        let st = &mut self.keys[k.index()];
        if st.down {
            return;
        }
        *st = KeyState { down: true, since: now.0, held: false, long: false, next_repeat: 0 };
        self.push(k, KeyPhase::Down);
    }

    /// A key went up at `now`. Pending hold/repeat events up to `now` are
    /// generated first.
    pub fn release(&mut self, raw: Key, now: Instant) {
        let k = self.map(raw);
        self.advance(now);
        let st = &mut self.keys[k.index()];
        if !st.down {
            return;
        }
        st.down = false;
        let held = st.held;
        self.push(k, KeyPhase::Up { held });
    }

    /// Release everything (e.g. the ladder ADC was reset).
    pub fn release_all(&mut self, now: Instant) {
        for k in Key::ALL {
            if self.keys[k.index()].down {
                // Keys are stored after mapping; release by the mapped key.
                self.advance(now);
                let st = &mut self.keys[k.index()];
                st.down = false;
                let held = st.held;
                self.push(k, KeyPhase::Up { held });
            }
        }
    }

    fn interval(&self, held_for: u64) -> u64 {
        let t = &self.timing;
        let start = u64::from(t.hold) + u64::from(t.accel_after);
        let slow = u64::from(t.repeat_slow);
        let fast = u64::from(t.repeat_fast);
        if held_for <= start {
            return slow;
        }
        let into = (held_for - start).min(u64::from(t.accel_ramp.max(1)));
        slow - (slow - fast) * into / u64::from(t.accel_ramp.max(1))
    }

    /// Generate hold, long and repeat events that are due at `now`. Late
    /// polls produce at most one repeat per key (no bursts).
    pub fn advance(&mut self, now: Instant) {
        for k in Key::ALL {
            let st = self.keys[k.index()];
            if !st.down {
                continue;
            }
            let held_for = now.0.saturating_sub(st.since);
            if !st.held && held_for >= self.hold_ms(k) {
                let iv = self.interval(held_for);
                let s = &mut self.keys[k.index()];
                s.held = true;
                s.next_repeat = now.0 + iv;
                self.push(k, KeyPhase::Hold);
                if k.repeats() {
                    self.push(k, KeyPhase::Repeat);
                }
                continue;
            }
            if k == Key::Power && st.held && !st.long && held_for >= u64::from(self.timing.power_long) {
                self.keys[k.index()].long = true;
                self.push(k, KeyPhase::Long);
            }
            if st.held && k.repeats() && now.0 >= st.next_repeat {
                let iv = self.interval(held_for);
                self.keys[k.index()].next_repeat = now.0 + iv;
                self.push(k, KeyPhase::Repeat);
            }
        }
    }

    /// Next event, generating time-based ones due at `now`.
    pub fn poll(&mut self, now: Instant) -> Option<(Key, KeyPhase)> {
        if self.len == 0 {
            self.advance(now);
        }
        if self.len == 0 {
            return None;
        }
        let e = self.queue[usize::from(self.head)];
        self.head = u8::try_from((usize::from(self.head) + 1) % QUEUE).unwrap_or(0);
        self.len -= 1;
        Some(e)
    }

    /// When the input task should wake to generate the next timed event.
    #[must_use]
    pub fn next_deadline(&self) -> Option<Instant> {
        let mut best: Option<u64> = None;
        for k in Key::ALL {
            let st = &self.keys[k.index()];
            if !st.down {
                continue;
            }
            let t = if !st.held {
                Some(st.since + self.hold_ms(k))
            } else if k.repeats() {
                Some(st.next_repeat)
            } else if k == Key::Power && !st.long {
                Some(st.since + u64::from(self.timing.power_long))
            } else {
                None
            };
            if let Some(t) = t {
                best = Some(best.map_or(t, |b| b.min(t)));
            }
        }
        best.map(Instant)
    }

    /// True while any key is down.
    #[must_use]
    pub fn any_down(&self) -> bool {
        self.keys.iter().any(|k| k.down)
    }
}

/// Which keys work while the keys are locked or the screen is off
/// (Settings → Power → Keys when locked).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum LockedKeys {
    None,
    #[default]
    Volume,
    VolumeTransport,
}

/// Verdict of the gate on one key event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Gate {
    /// Deliver to the UI.
    Pass(Key, KeyPhase),
    /// Screen was off (not locked): turn it on; the rest of this press is
    /// swallowed.
    Wake,
    /// Keys are locked: show the lock screen for 2 s (05 §6.3), with the
    /// screen on for that time if it was off.
    LockNotice,
    /// Swallowed (rest of a waking press, or a locked key's later phases).
    Drop,
    /// A transport key (Prev, Next, OK) while the keys are locked or the
    /// screen is off, with Keys when locked = Volume and transport: it
    /// controls playback, never the screen under the lock.
    Transport(Key, KeyPhase),
}

/// Screen-off and key-lock rules (05 §3 "Rules", §6.3).
#[derive(Clone, Copy, Debug, Default)]
pub struct KeyGate {
    pub screen_on: bool,
    pub locked: bool,
    pub when_locked: LockedKeys,
    /// The screen is off for the keys (`screen_on` false) but the panel
    /// shows the screen saver (Settings › Display › Screen saver = Clock,
    /// after the backlight timeout; 05 §6.3). Cleared when the screen goes
    /// off with Power.
    pub saver: bool,
    swallowed: u16,
}

impl KeyGate {
    #[must_use]
    pub const fn new() -> KeyGate {
        KeyGate {
            screen_on: true,
            locked: false,
            when_locked: LockedKeys::Volume,
            saver: false,
            swallowed: 0,
        }
    }

    /// Keys when locked (Settings › Power, 0x0404): 0 None, 1 Volume, 2
    /// Volume and transport (anything else: Volume, the default).
    #[must_use]
    pub const fn locked_keys(v: i16) -> LockedKeys {
        match v {
            0 => LockedKeys::None,
            2 => LockedKeys::VolumeTransport,
            _ => LockedKeys::Volume,
        }
    }

    fn allowed_locked(&self, k: Key) -> bool {
        match k {
            Key::Power => true,
            Key::VolUp | Key::VolDown => self.when_locked != LockedKeys::None,
            Key::Prev | Key::Next | Key::Ok => self.when_locked == LockedKeys::VolumeTransport,
            _ => false,
        }
    }

    pub fn filter(&mut self, k: Key, p: KeyPhase) -> Gate {
        let bit = 1u16 << k.index();
        if self.swallowed & bit != 0 {
            if matches!(p, KeyPhase::Up { .. }) {
                self.swallowed &= !bit;
            }
            return Gate::Drop;
        }
        let restricted = !self.screen_on || self.locked;
        if !restricted {
            return Gate::Pass(k, p);
        }
        if self.allowed_locked(k) {
            return if matches!(k, Key::Prev | Key::Next | Key::Ok) {
                Gate::Transport(k, p)
            } else {
                Gate::Pass(k, p)
            };
        }
        if p != KeyPhase::Down {
            return Gate::Drop;
        }
        self.swallowed |= bit;
        if self.locked { Gate::LockNotice } else { Gate::Wake }
    }
}

/// The screen timers' settings (Settings › Display: Backlight timeout,
/// Lock delay, Screen saver; 05 §6.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Timeouts {
    /// The screen goes off this long after the last key (`None`: always on).
    pub screen_off_ms: Option<u32>,
    /// The keys lock this long after the timer turned the screen off
    /// (`None`: never; the Power key still locks them).
    pub lock_ms: Option<u32>,
    /// The panel shows the screen saver instead of going dark.
    pub saver: bool,
}

impl Timeouts {
    /// No timer runs (Always, Never): what a host test holds them at.
    pub const HELD: Timeouts = Timeouts { screen_off_ms: None, lock_ms: None, saver: false };
}

/// The backlight timeout and the lock delay (05 §6.3): the screen goes off
/// [`Timeouts::screen_off_ms`] after the last key (the next key only wakes
/// it, [`Gate::Wake`]), then the keys lock [`Timeouts::lock_ms`] later
/// (the next key shows the lock notice). The caller reports keys with
/// [`ScreenTimer::key`] and calls [`ScreenTimer::step`] at least once a
/// second; a screen that comes on by any other way (Power, an alert, the
/// lock notice) counts from then.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ScreenTimer {
    /// The last key, or when the screen came on (ms).
    since: u64,
    /// When this timer turned the screen off (the lock delay runs from it).
    off_at: Option<u64>,
    /// The screen was on at the last step.
    was_on: bool,
}

impl ScreenTimer {
    #[must_use]
    pub const fn new(now: u64) -> ScreenTimer {
        ScreenTimer { since: now, off_at: None, was_on: true }
    }

    /// A key edge at `now` (ms).
    pub fn key(&mut self, now: u64) {
        self.since = now;
    }

    /// Apply the timers to `gate` at `now` (ms); true when it changed.
    pub fn step(&mut self, gate: &mut KeyGate, t: Timeouts, now: u64) -> bool {
        let mut changed = false;
        if gate.screen_on {
            if !self.was_on {
                self.since = now;
                self.off_at = None;
            }
            self.was_on = true;
            if let Some(ms) = t.screen_off_ms
                && now.saturating_sub(self.since) >= u64::from(ms)
            {
                gate.screen_on = false;
                gate.saver = t.saver;
                self.off_at = Some(now);
                self.was_on = false;
                changed = true;
            }
        } else {
            self.was_on = false;
        }
        if !gate.screen_on
            && !gate.locked
            && let (Some(off), Some(ms)) = (self.off_at, t.lock_ms)
            && now.saturating_sub(off) >= u64::from(ms)
        {
            gate.locked = true;
            changed = true;
        }
        changed
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn drain(m: &mut KeyMapper, now: u64) -> Vec<(Key, KeyPhase)> {
        let mut v = Vec::new();
        while let Some(e) = m.poll(Instant(now)) {
            v.push(e);
        }
        v
    }

    #[test]
    fn tap() {
        let mut m = KeyMapper::new();
        m.press(Key::Ok, Instant(0));
        m.release(Key::Ok, Instant(80));
        assert_eq!(drain(&mut m, 80), [(Key::Ok, KeyPhase::Down), (Key::Ok, KeyPhase::Up { held: false })]);
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn hold_and_repeat_accelerate() {
        let mut m = KeyMapper::new();
        m.press(Key::Down, Instant(0));
        assert_eq!(drain(&mut m, 0), [(Key::Down, KeyPhase::Down)]);
        assert_eq!(m.next_deadline(), Some(Instant(500)));
        assert!(drain(&mut m, 499).is_empty());
        assert_eq!(drain(&mut m, 500), [(Key::Down, KeyPhase::Hold), (Key::Down, KeyPhase::Repeat)]);
        // Simulate a 1 ms poll loop and count repeats per window.
        let mut count_slow = 0;
        for t in 501..1500 {
            count_slow += drain(&mut m, t).len();
        }
        let mut count_fast = 0;
        for t in 3000..4000 {
            count_fast += drain(&mut m, t).len();
        }
        assert!((7..=9).contains(&count_slow), "{count_slow}");
        assert!((29..=31).contains(&count_fast), "{count_fast}");
        m.release(Key::Down, Instant(4000));
        assert_eq!(drain(&mut m, 4000).last(), Some(&(Key::Down, KeyPhase::Up { held: true })));
    }

    #[test]
    fn power_phases() {
        let mut m = KeyMapper::new();
        m.press(Key::Power, Instant(0));
        drain(&mut m, 0);
        assert!(drain(&mut m, 1000).is_empty());
        assert_eq!(drain(&mut m, 1500), [(Key::Power, KeyPhase::Hold)]);
        assert_eq!(m.next_deadline(), Some(Instant(4000)));
        assert_eq!(drain(&mut m, 4000), [(Key::Power, KeyPhase::Long)]);
        assert_eq!(m.next_deadline(), None);
    }

    #[test]
    fn non_repeating_hold() {
        let mut m = KeyMapper::new();
        m.press(Key::Menu, Instant(0));
        drain(&mut m, 0);
        assert_eq!(drain(&mut m, 700), [(Key::Menu, KeyPhase::Hold)]);
        assert!(drain(&mut m, 3000).is_empty());
    }

    #[test]
    fn rotation_swaps() {
        let mut m = KeyMapper::new();
        m.rotated = true;
        m.press(Key::Up, Instant(0));
        m.press(Key::Prev, Instant(0));
        assert_eq!(drain(&mut m, 0), [(Key::Down, KeyPhase::Down), (Key::Next, KeyPhase::Down)]);
    }

    #[test]
    fn gate_wake_and_lock() {
        let mut g = KeyGate::new();
        g.screen_on = false;
        assert_eq!(g.filter(Key::Ok, KeyPhase::Down), Gate::Wake);
        assert_eq!(g.filter(Key::Ok, KeyPhase::Up { held: false }), Gate::Drop);
        g.locked = true;
        assert_eq!(g.filter(Key::Ok, KeyPhase::Down), Gate::LockNotice);
        assert_eq!(g.filter(Key::Ok, KeyPhase::Up { held: false }), Gate::Drop);
        g.locked = false;
        assert_eq!(g.filter(Key::VolUp, KeyPhase::Down), Gate::Pass(Key::VolUp, KeyPhase::Down));
        g.screen_on = true;
        g.locked = true;
        assert_eq!(g.filter(Key::Down, KeyPhase::Down), Gate::LockNotice);
        assert_eq!(g.filter(Key::Down, KeyPhase::Repeat), Gate::Drop);
        assert_eq!(g.filter(Key::Down, KeyPhase::Up { held: true }), Gate::Drop);
        assert_eq!(g.filter(Key::Power, KeyPhase::Down), Gate::Pass(Key::Power, KeyPhase::Down));
        g.locked = false;
        assert_eq!(g.filter(Key::Down, KeyPhase::Down), Gate::Pass(Key::Down, KeyPhase::Down));
    }

    /// Keys when locked (0x0404): None, Volume, Volume and transport; the
    /// transport keys reach playback (`Gate::Transport`), never the screen.
    #[test]
    fn keys_when_locked_setting() {
        assert_eq!(KeyGate::locked_keys(0), LockedKeys::None);
        assert_eq!(KeyGate::locked_keys(1), LockedKeys::Volume);
        assert_eq!(KeyGate::locked_keys(2), LockedKeys::VolumeTransport);
        assert_eq!(KeyGate::locked_keys(9), LockedKeys::Volume);
        for (v, vol, transport) in [(0, false, false), (1, true, false), (2, true, true)] {
            for off in [false, true] {
                let mut g = KeyGate::new();
                g.when_locked = KeyGate::locked_keys(v);
                if off {
                    g.screen_on = false;
                }
                g.locked = true;
                let got = g.filter(Key::VolUp, KeyPhase::Down);
                assert_eq!(got == Gate::Pass(Key::VolUp, KeyPhase::Down), vol, "{v} {off}: {got:?}");
                let _ = g.filter(Key::VolUp, KeyPhase::Up { held: false });
                for k in [Key::Prev, Key::Next, Key::Ok] {
                    let got = g.filter(k, KeyPhase::Down);
                    assert_eq!(got == Gate::Transport(k, KeyPhase::Down), transport, "{v} {k:?}: {got:?}");
                    let _ = g.filter(k, KeyPhase::Up { held: false });
                }
                // Up, Down, Back, Menu never pass a lock.
                for k in [Key::Up, Key::Down, Key::Back, Key::Menu] {
                    assert_eq!(g.filter(k, KeyPhase::Down), Gate::LockNotice);
                    let _ = g.filter(k, KeyPhase::Up { held: false });
                }
                // Power always does.
                assert_eq!(g.filter(Key::Power, KeyPhase::Down), Gate::Pass(Key::Power, KeyPhase::Down));
            }
        }
        // Unlocked with the screen on: every key reaches the screen.
        let mut g = KeyGate::new();
        g.when_locked = LockedKeys::VolumeTransport;
        assert_eq!(g.filter(Key::Ok, KeyPhase::Down), Gate::Pass(Key::Ok, KeyPhase::Down));
    }

    const T15: Timeouts = Timeouts { screen_off_ms: Some(15_000), lock_ms: Some(0), saver: false };

    /// Backlight timeout then lock delay (05 §6.3): off 15 s after the last
    /// key, locked at once ("Immediately"); a key in between restarts it.
    #[test]
    fn screen_timer_off_then_lock() {
        let mut g = KeyGate::new();
        let mut t = ScreenTimer::new(0);
        assert!(!t.step(&mut g, T15, 10_000));
        t.key(10_000);
        assert!(!t.step(&mut g, T15, 24_999));
        assert!(g.screen_on);
        assert!(t.step(&mut g, T15, 25_000));
        assert!(!g.screen_on && g.locked && !g.saver);
        // Locked: nothing more happens.
        assert!(!t.step(&mut g, T15, 90_000));
    }

    /// Lock delay 5 s: the screen is off but a key only wakes it during
    /// those 5 s; then the keys lock. Never: they never lock by the timer.
    #[test]
    fn screen_timer_lock_delay() {
        let five = Timeouts { lock_ms: Some(5_000), ..T15 };
        let mut g = KeyGate::new();
        let mut t = ScreenTimer::new(0);
        assert!(t.step(&mut g, five, 15_000));
        assert!(!g.screen_on && !g.locked);
        assert!(!t.step(&mut g, five, 19_999));
        assert!(t.step(&mut g, five, 20_000));
        assert!(g.locked);
        // Woken within the delay: no lock.
        let mut g = KeyGate::new();
        let mut t = ScreenTimer::new(0);
        assert!(t.step(&mut g, five, 15_000));
        assert_eq!(g.filter(Key::Ok, KeyPhase::Down), Gate::Wake);
        let _ = g.filter(Key::Ok, KeyPhase::Up { held: false });
        g.screen_on = true;
        t.key(16_000);
        assert!(!t.step(&mut g, five, 20_000));
        assert!(g.screen_on && !g.locked);
        let never = Timeouts { lock_ms: None, ..T15 };
        let mut g = KeyGate::new();
        let mut t = ScreenTimer::new(0);
        assert!(t.step(&mut g, never, 15_000));
        assert!(!t.step(&mut g, never, 3_600_000));
        assert!(!g.screen_on && !g.locked);
    }

    /// A screen woken (a key, Power, an alert) counts from then; Always
    /// keeps it on; the screen saver flag follows the setting.
    #[test]
    fn screen_timer_wake_always_and_saver() {
        let mut g = KeyGate::new();
        let mut t = ScreenTimer::new(0);
        let saver = Timeouts { saver: true, lock_ms: None, ..T15 };
        assert!(t.step(&mut g, saver, 15_000));
        assert!(!g.screen_on && g.saver);
        // Woken without a key (an alert) at 40 s.
        g.screen_on = true;
        assert!(!t.step(&mut g, saver, 40_000));
        assert!(!t.step(&mut g, saver, 54_999));
        assert!(t.step(&mut g, saver, 55_000));
        assert!(!g.screen_on);
        let mut g = KeyGate::new();
        let mut t = ScreenTimer::new(0);
        assert!(!t.step(&mut g, Timeouts::HELD, 86_400_000));
        assert!(g.screen_on && !g.locked);
    }
}
