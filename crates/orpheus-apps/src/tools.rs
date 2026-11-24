//! Stopwatch, timer and alarms (05 §7.8, WP UI-5d) without hardware.
//!
//! The firmware's UI task owns one [`Tools`] (in `Fw`), so they keep
//! running while the app is left or the screen is off; the simulator owns
//! another. Times are milliseconds of the UI clock (`Instant`, the 1 MHz
//! timer on the player), taken when the key event arrives (the input task
//! samples the keys every 10 ms, so a press is timed to ± 10 ms).
//!
//! Alarms live in the settings store ([`alarm`], keys
//! [`id::ALARM0`]…), the last five timer durations too
//! ([`id::TIMER_PRESET0`]…); [`Tools::poll`] (at least once a second)
//! turns a finished timer or an alarm minute into an [`Alert`].

use crate::services::{DateTime, SettingsService};
use crate::settings::{SettingId, id};

/// Laps kept (the newest; older ones are counted but not listed).
pub const MAX_LAPS: usize = 20;
/// Alarms (05 §7.8).
pub const ALARMS: usize = 4;
/// Timer presets kept (the last five durations).
pub const PRESETS: usize = 5;
/// An alert rings this long unless dismissed (ms).
pub const RING_MS: u64 = 60_000;
/// Snooze (ms).
pub const SNOOZE_MS: u64 = 9 * 60_000;
/// Longest timer (9 h: the setting is an `i16` of seconds).
pub const TIMER_MAX_S: u32 = 9 * 3600;

/// A stopwatch with laps.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Stopwatch {
    /// Start of the running stretch (ms), if running.
    since: Option<u64>,
    /// Time of earlier stretches (ms).
    acc: u64,
    /// Splits at each kept lap (ms), oldest first, and the lap times.
    splits: [u32; MAX_LAPS],
    laps: [u32; MAX_LAPS],
    kept: u8,
    /// Laps taken since the reset.
    total: u16,
}

/// One lap as listed: its number (1 = first), its time and the split.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lap {
    pub number: u16,
    pub time_ms: u32,
    pub split_ms: u32,
}

impl Stopwatch {
    #[must_use]
    pub fn elapsed(&self, now: u64) -> u64 {
        self.acc + self.since.map_or(0, |s| now.saturating_sub(s))
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        self.since.is_some()
    }

    /// Something to show (running, or stopped at a time).
    #[must_use]
    pub fn started(&self) -> bool {
        self.since.is_some() || self.acc > 0
    }

    /// OK: start or stop.
    pub fn toggle(&mut self, now: u64) {
        match self.since.take() {
            Some(s) => self.acc += now.saturating_sub(s),
            None => self.since = Some(now),
        }
    }

    /// Next while running: a lap.
    pub fn lap(&mut self, now: u64) {
        if !self.running() {
            return;
        }
        let split = u32::try_from(self.elapsed(now)).unwrap_or(u32::MAX);
        let prev = self.last_split();
        if usize::from(self.kept) == MAX_LAPS {
            self.splits.copy_within(1.., 0);
            self.laps.copy_within(1.., 0);
            self.kept -= 1;
        }
        let i = usize::from(self.kept);
        if let (Some(s), Some(l)) = (self.splits.get_mut(i), self.laps.get_mut(i)) {
            *s = split;
            *l = split.saturating_sub(prev);
        }
        self.kept += 1;
        self.total = self.total.saturating_add(1);
    }

    fn last_split(&self) -> u32 {
        usize::from(self.kept).checked_sub(1).and_then(|i| self.splits.get(i)).copied().unwrap_or(0)
    }

    /// Next while stopped: back to zero.
    pub fn reset(&mut self) {
        *self = Stopwatch::default();
    }

    /// Laps taken since the reset.
    #[must_use]
    pub const fn laps(&self) -> u16 {
        self.total
    }

    /// Laps listed (at most [`MAX_LAPS`]).
    #[must_use]
    pub const fn kept(&self) -> usize {
        self.kept as usize
    }

    /// The `k`-th newest lap (0 = the last one taken).
    #[must_use]
    pub fn lap_at(&self, k: usize) -> Option<Lap> {
        let i = usize::from(self.kept).checked_sub(1 + k)?;
        let number = self.total.saturating_sub(u16::try_from(k).unwrap_or(u16::MAX));
        Some(Lap { number, time_ms: *self.laps.get(i)?, split_ms: *self.splits.get(i)? })
    }
}

/// The countdown timer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Countdown {
    /// The duration set (ms).
    duration: u32,
    /// Running: when it reaches zero (ms).
    end: Option<u64>,
    /// Paused (or stopped at zero): what is left (ms).
    left: u32,
    /// Started since the last reset (running or paused).
    started: bool,
}

impl Countdown {
    /// Set the duration (stops a running timer).
    pub fn set(&mut self, ms: u32) {
        *self = Countdown { duration: ms, left: ms, ..Countdown::default() };
    }

    #[must_use]
    pub const fn duration(&self) -> u32 {
        self.duration
    }

    #[must_use]
    pub const fn running(&self) -> bool {
        self.end.is_some()
    }

    /// When a running timer reaches zero (ms).
    #[must_use]
    pub const fn ends_at(&self) -> Option<u64> {
        self.end
    }

    /// Started and not reset (running or paused).
    #[must_use]
    pub const fn started(&self) -> bool {
        self.started
    }

    /// Time left (ms).
    #[must_use]
    pub fn left(&self, now: u64) -> u32 {
        match self.end {
            Some(e) => u32::try_from(e.saturating_sub(now)).unwrap_or(u32::MAX),
            None => self.left,
        }
    }

    /// OK: start, pause or resume.
    pub fn toggle(&mut self, now: u64) {
        match self.end.take() {
            Some(e) => self.left = u32::try_from(e.saturating_sub(now)).unwrap_or(0),
            None => {
                if self.left == 0 {
                    self.left = self.duration;
                }
                if self.left > 0 {
                    self.end = Some(now + u64::from(self.left));
                    self.started = true;
                }
            }
        }
    }

    /// Next: back to the full duration, stopped.
    pub fn reset(&mut self) {
        self.set(self.duration);
    }

    /// Reached zero now (once).
    pub fn poll(&mut self, now: u64) -> bool {
        match self.end {
            Some(e) if now >= e => {
                // Done: ready to run the same duration again.
                self.end = None;
                self.left = self.duration;
                self.started = false;
                true
            }
            _ => false,
        }
    }
}

/// One alarm (from the settings store).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Alarm {
    /// Minutes after midnight (local time).
    pub minute: u16,
    /// Repeat days: bit 0 = Monday … bit 6 = Sunday; 0 = once.
    pub days: u8,
    pub on: bool,
}

impl Alarm {
    #[must_use]
    pub const fn hour(&self) -> u8 {
        (self.minute / 60) as u8
    }

    #[must_use]
    pub const fn min(&self) -> u8 {
        (self.minute % 60) as u8
    }

    /// Rings at local time `t` (its minute, on a selected day).
    #[must_use]
    pub fn rings_at(&self, t: &DateTime) -> bool {
        self.on
            && u16::from(t.hour) * 60 + u16::from(t.minute) == self.minute
            && (self.days == 0 || self.days & (1 << (t.weekday % 7)) != 0)
    }
}

/// Settings keys of alarm `i`: time, repeat days, on.
#[must_use]
pub const fn alarm_keys(i: usize) -> (SettingId, SettingId, SettingId) {
    let base = id::ALARM0.0 + 3 * (i as u16);
    (SettingId(base), SettingId(base + 1), SettingId(base + 2))
}

/// Alarm `i` from the settings.
pub fn alarm<S: SettingsService + ?Sized>(s: &S, i: usize) -> Alarm {
    let (t, d, o) = alarm_keys(i);
    Alarm {
        minute: u16::try_from(s.setting(t)).unwrap_or(0).min(1439),
        days: u8::try_from(s.setting(d)).unwrap_or(0) & 0x7F,
        on: s.setting(o) != 0,
    }
}

/// Every alarm.
pub fn alarms<S: SettingsService + ?Sized>(s: &S) -> [Alarm; ALARMS] {
    core::array::from_fn(|i| alarm(s, i))
}

/// Timer preset `i` (seconds; 0 = unused).
pub fn preset<S: SettingsService + ?Sized>(s: &S, i: usize) -> u32 {
    let k = SettingId(id::TIMER_PRESET0.0 + u16::try_from(i).unwrap_or(0));
    u32::try_from(s.setting(k)).unwrap_or(0)
}

/// A duration was started: it becomes the first preset (an equal one
/// moves to the front, the oldest drops out).
pub fn remember_preset<S: SettingsService + ?Sized>(s: &mut S, secs: u32) {
    let secs = secs.min(TIMER_MAX_S);
    let mut list = [0u32; PRESETS];
    for (i, v) in list.iter_mut().enumerate() {
        *v = preset(s, i);
    }
    let pos = list.iter().position(|&v| v == secs).unwrap_or(PRESETS - 1);
    list.copy_within(..pos, 1);
    list[0] = secs;
    for (i, v) in list.iter().enumerate() {
        let k = SettingId(id::TIMER_PRESET0.0 + u16::try_from(i).unwrap_or(0));
        s.set_setting(k, i16::try_from(*v).unwrap_or(0));
    }
}

/// The next alarm to arm the RTC with: (index, hour, minute) of the
/// enabled alarm that rings soonest after `now` (within a week).
#[must_use]
pub fn next_alarm(alarms: &[Alarm; ALARMS], now: &DateTime) -> Option<(usize, u8, u8)> {
    let cur = u32::from(now.hour) * 60 + u32::from(now.minute);
    let mut best: Option<(u32, usize)> = None;
    for (i, a) in alarms.iter().enumerate().filter(|(_, a)| a.on) {
        for day in 0..8u32 {
            let wd = (u32::from(now.weekday) + day) % 7;
            if a.days != 0 && a.days & (1 << wd) == 0 {
                continue;
            }
            let at = day * 1440 + u32::from(a.minute);
            if at <= cur {
                continue;
            }
            let wait = at - cur;
            if best.is_none_or(|(b, _)| wait < b) {
                best = Some((wait, i));
            }
            break;
        }
    }
    best.and_then(|(_, i)| alarms.get(i).map(|a| (i, a.hour(), a.min())))
}

/// What rings.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Alert {
    Timer,
    Alarm(u8),
}

/// The three tools and the alert state.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Tools {
    pub stopwatch: Stopwatch,
    pub timer: Countdown,
    ringing: Option<Alert>,
    ring_since: u64,
    /// Snoozed alarm and when it rings again.
    snooze: Option<(u8, u64)>,
    /// Minute (minutes since 2000) an alarm last rang, so it rings once.
    rang_minute: u32,
}

/// Minutes since 2000-01-01 of a local time.
fn minute_number(t: &DateTime) -> u32 {
    orpheus_minutes(t.year, t.month, t.day) * 1440 + u32::from(t.hour) * 60 + u32::from(t.minute)
}

const fn orpheus_minutes(year: u16, month: u8, day: u8) -> u32 {
    // Days since 2000-01-01 (2000–2099 is enough for the comparison).
    const CUM: [u16; 12] = [0, 31, 59, 90, 120, 151, 181, 212, 243, 273, 304, 334];
    let y = if year < 2000 { 0 } else { (year - 2000) as u32 };
    let m = if month < 1 || month > 12 { 1 } else { month };
    let mut d =
        y * 365 + y.div_ceil(4) + CUM[(m - 1) as usize] as u32 + if day == 0 { 0 } else { day as u32 - 1 };
    if m > 2 && y % 4 == 0 {
        d += 1;
    }
    d
}

impl Tools {
    /// What rings now.
    #[must_use]
    pub const fn ringing(&self) -> Option<Alert> {
        self.ringing
    }

    /// When the ringing started (ms).
    #[must_use]
    pub const fn ring_since(&self) -> u64 {
        self.ring_since
    }

    /// An alarm is snoozed (status bar).
    #[must_use]
    pub const fn snoozed(&self) -> bool {
        self.snooze.is_some()
    }

    /// At least once a second: the timer reaching zero, an alarm's minute
    /// (`clock` is `None` while the clock is not valid: alarms wait), a
    /// snooze ending. Returns an alert that starts now; a ringing alert
    /// ends by itself after [`RING_MS`] (then [`Tools::ringing`] is
    /// `None` again).
    pub fn poll(&mut self, now: u64, clock: Option<&DateTime>, alarms: &[Alarm; ALARMS]) -> Option<Alert> {
        if self.ringing.is_some() && now.saturating_sub(self.ring_since) >= RING_MS {
            self.ringing = None;
        }
        let mut new = None;
        if self.timer.poll(now) {
            new = Some(Alert::Timer);
        }
        if let Some((i, at)) = self.snooze
            && now >= at
        {
            self.snooze = None;
            new = new.or(Some(Alert::Alarm(i)));
        }
        if let Some(t) = clock {
            let m = minute_number(t);
            if m != self.rang_minute
                && let Some(i) = alarms.iter().position(|a| a.rings_at(t))
            {
                self.rang_minute = m;
                new = new.or(Some(Alert::Alarm(u8::try_from(i).unwrap_or(0))));
            }
        }
        if let Some(a) = new {
            self.ringing = Some(a);
            self.ring_since = now;
        }
        new
    }

    /// OK on the alert: stop it.
    pub fn dismiss(&mut self) {
        self.ringing = None;
    }

    /// Next on an alarm's alert: ring again in [`SNOOZE_MS`].
    pub fn snooze(&mut self, now: u64) {
        if let Some(Alert::Alarm(i)) = self.ringing.take() {
            self.snooze = Some((i, now + SNOOZE_MS));
        }
    }

    /// Cancel a snooze (the alarm was switched off).
    pub fn cancel_snooze(&mut self, i: u8) {
        if self.snooze.is_some_and(|(s, _)| s == i) {
            self.snooze = None;
        }
    }
}

/// After a one-time alarm rang, it is switched off.
pub fn alarm_rang<S: SettingsService + ?Sized>(s: &mut S, i: usize) {
    let a = alarm(s, i);
    if a.days == 0 && a.on {
        s.set_setting(alarm_keys(i).2, 0);
    }
}

/// The services' side of [`Tools::poll`] (firmware UI loop, simulator):
/// alarms from the settings, the clock while it is valid; a one-time
/// alarm is switched off when it rings. Returns the
/// [`crate::services::tools_event`] to send, if any.
pub fn poll_service<S>(s: &mut S, now: u64) -> Option<u32>
where
    S: SettingsService + crate::services::ClockService + crate::services::ToolsService + ?Sized,
{
    use crate::services::tools_event;
    let was = s.tools().ringing();
    let clock = s.clock_valid().then(|| s.now());
    let list = alarms(s);
    let new = s.tools_mut().poll(now, clock.as_ref(), &list);
    if let Some(Alert::Alarm(i)) = new {
        alarm_rang(s, usize::from(i));
    }
    match (was, new, s.tools().ringing()) {
        (_, Some(_), _) => Some(tools_event::ALERT),
        (Some(_), None, None) => Some(tools_event::ALERT_ENDED),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stopwatch_laps() {
        let mut s = Stopwatch::default();
        assert!(!s.started());
        s.toggle(1_000);
        assert!(s.running());
        s.lap(13_370);
        s.lap(30_000);
        s.toggle(31_000);
        assert_eq!(s.elapsed(99_999), 30_000, "stopped: frozen");
        s.toggle(40_000);
        assert_eq!(s.elapsed(41_000), 31_000);
        s.lap(41_000);
        assert_eq!(s.laps(), 3);
        assert_eq!(s.lap_at(0), Some(Lap { number: 3, time_ms: 2_000, split_ms: 31_000 }));
        assert_eq!(s.lap_at(1), Some(Lap { number: 2, time_ms: 16_630, split_ms: 29_000 }));
        assert_eq!(s.lap_at(2), Some(Lap { number: 1, time_ms: 12_370, split_ms: 12_370 }));
        assert_eq!(s.lap_at(3), None);
        // Lap while stopped does nothing; reset clears.
        s.toggle(42_000);
        s.lap(43_000);
        assert_eq!(s.laps(), 3);
        s.reset();
        assert_eq!((s.elapsed(50_000), s.laps(), s.started()), (0, 0, false));
    }

    #[test]
    fn many_laps_keep_the_newest() {
        let mut s = Stopwatch::default();
        s.toggle(0);
        for k in 1..=25u64 {
            s.lap(k * 1000);
        }
        assert_eq!((s.laps(), s.kept()), (25, MAX_LAPS));
        assert_eq!(s.lap_at(0), Some(Lap { number: 25, time_ms: 1000, split_ms: 25_000 }));
        assert_eq!(s.lap_at(MAX_LAPS - 1).map(|l| (l.number, l.split_ms)), Some((6, 6000)));
        assert_eq!(s.lap_at(MAX_LAPS), None);
    }

    #[test]
    fn timer_counts_down_pauses_and_fires_once() {
        let mut t = Countdown::default();
        t.set(60_000);
        assert_eq!(t.left(5), 60_000);
        t.toggle(1_000);
        assert_eq!(t.left(31_000), 30_000);
        t.toggle(31_000);
        assert!(!t.running() && t.started());
        assert_eq!(t.left(99_000), 30_000, "paused");
        t.toggle(100_000);
        assert!(!t.poll(129_999));
        assert!(t.poll(130_000));
        assert!(!t.poll(130_001), "once");
        assert_eq!(t.left(130_002), 60_000, "ready again");
        assert!(!t.started());
        t.toggle(200_000);
        assert_eq!(t.left(200_000), 60_000);
        t.reset();
        assert!(!t.running() && !t.started());
        assert_eq!(t.left(0), 60_000);
    }

    fn at(weekday: u8, h: u8, m: u8) -> DateTime {
        DateTime { year: 2026, month: 11, day: 23 + weekday, hour: h, minute: m, second: 0, weekday }
    }

    #[test]
    fn alarms_ring_on_their_days_once_per_minute() {
        let weekdays = Alarm { minute: 7 * 60 + 30, days: 0x1F, on: true };
        let mut a = [Alarm::default(); ALARMS];
        a[1] = weekdays;
        let mut t = Tools::default();
        // Tuesday 07:29 → nothing; 07:30 → alarm 1, once.
        assert_eq!(t.poll(0, Some(&at(1, 7, 29)), &a), None);
        assert_eq!(t.poll(1000, Some(&at(1, 7, 30)), &a), Some(Alert::Alarm(1)));
        assert_eq!(t.ringing(), Some(Alert::Alarm(1)));
        assert_eq!(t.poll(2000, Some(&at(1, 7, 30)), &a), None);
        t.dismiss();
        assert_eq!(t.poll(3000, Some(&at(1, 7, 30)), &a), None, "not again in the same minute");
        // Saturday: not a selected day.
        assert_eq!(t.poll(4000, Some(&at(5, 7, 30)), &a), None);
        // No clock: alarms wait.
        assert_eq!(t.poll(5000, None, &a), None);
        // Off.
        a[1].on = false;
        assert_eq!(t.poll(6000, Some(&at(2, 7, 30)), &a), None);
    }

    #[test]
    fn ringing_times_out_and_snooze_rings_again() {
        let mut a = [Alarm::default(); ALARMS];
        a[0] = Alarm { minute: 6 * 60, days: 0, on: true };
        let mut t = Tools::default();
        assert_eq!(t.poll(10_000, Some(&at(0, 6, 0)), &a), Some(Alert::Alarm(0)));
        assert_eq!(t.poll(10_000 + RING_MS, Some(&at(0, 6, 1)), &a), None);
        assert_eq!(t.ringing(), None, "rang for a minute");
        assert_eq!(t.poll(100_000, Some(&at(0, 6, 1)), &a), None);
        // Again on the next day; snooze.
        assert_eq!(t.poll(200_000, Some(&at(1, 6, 0)), &a), Some(Alert::Alarm(0)));
        t.snooze(200_000);
        assert!(t.snoozed() && t.ringing().is_none());
        assert_eq!(t.poll(200_000 + SNOOZE_MS - 1, None, &a), None);
        assert_eq!(t.poll(200_000 + SNOOZE_MS, None, &a), Some(Alert::Alarm(0)));
        // The timer has priority when both happen in the same poll.
        let mut t = Tools::default();
        t.timer.set(1000);
        t.timer.toggle(0);
        assert_eq!(t.poll(1000, Some(&at(2, 6, 0)), &a), Some(Alert::Timer));
    }

    #[test]
    fn next_alarm_for_the_rtc() {
        let mut a = [Alarm::default(); ALARMS];
        assert_eq!(next_alarm(&a, &at(0, 12, 0)), None);
        a[0] = Alarm { minute: 7 * 60, days: 0x1F, on: true }; // weekdays 07:00
        a[2] = Alarm { minute: 9 * 60 + 15, days: 0x60, on: true }; // weekend 09:15
        a[3] = Alarm { minute: 13 * 60, days: 0, on: false };
        // Monday noon: Tuesday 07:00.
        assert_eq!(next_alarm(&a, &at(0, 12, 0)), Some((0, 7, 0)));
        // Friday noon: Saturday 09:15 before Monday 07:00.
        assert_eq!(next_alarm(&a, &at(4, 12, 0)), Some((2, 9, 15)));
        // Monday 06:59: today 07:00.
        assert_eq!(next_alarm(&a, &at(0, 6, 59)), Some((0, 7, 0)));
        // A one-time alarm later today.
        a[3].on = true;
        assert_eq!(next_alarm(&a, &at(0, 12, 0)), Some((3, 13, 0)));
    }

    #[test]
    fn minutes_increase() {
        let a = minute_number(&DateTime {
            year: 2026,
            month: 2,
            day: 28,
            hour: 23,
            minute: 59,
            ..DateTime::default()
        });
        let b = minute_number(&DateTime {
            year: 2026,
            month: 3,
            day: 1,
            hour: 0,
            minute: 0,
            ..DateTime::default()
        });
        assert_eq!(b, a + 1);
        let c = minute_number(&DateTime {
            year: 2028,
            month: 2,
            day: 29,
            hour: 0,
            minute: 0,
            ..DateTime::default()
        });
        let d = minute_number(&DateTime {
            year: 2028,
            month: 3,
            day: 1,
            hour: 0,
            minute: 0,
            ..DateTime::default()
        });
        assert_eq!(d, c + 1440);
    }
}
