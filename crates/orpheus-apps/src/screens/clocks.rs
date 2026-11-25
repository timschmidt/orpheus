//! Stopwatch · Timer · Alarm (05 §7.8, WP UI-5d), the repeat-days list
//! of an alarm, and the alert that rings over any screen.
//!
//! Keys: Prev goes to the previous tab everywhere. Next is the tab's
//! second action where 05 §7.8 gives it one (Stopwatch: lap while
//! running, reset while stopped; Timer: reset once started) and the next
//! tab otherwise. OK starts and stops; Up/Down scroll the laps, choose a
//! timer preset or an alarm. Menu: Set duration (Timer), Time and Repeat
//! (Alarm).
//!
//! The state is the service's ([`crate::tools::Tools`]): the screens only
//! show it and change it.

use orpheus_ui::event::{Event, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::list::{Accessory, Item, ListAction, ListSource, ListView, Row};
use orpheus_ui::widgets::misc::{draw_progress, draw_tabs};

use super::rows::Rows;
use super::{ALARM_DAYS, PICKER};
use crate::icons;
use crate::services::{Services, other, tools_event};
use crate::strings::{Lang, Str};
use crate::tools::{self, ALARMS, Alert, PRESETS};

/// `PickerScreen` argument: the timer's duration.
pub const PICK_TIMER: u32 = 2;
/// `PickerScreen` argument: alarm `i`'s time is `PICK_ALARM + i`.
pub const PICK_ALARM: u32 = 0x10;

const TABS_H: i16 = 16;
const TAB_SW: u8 = 0;
const TAB_TIMER: u8 = 1;
const TAB_ALARM: u8 = 2;

const M_DURATION: u16 = 1;
const M_ALARM_TIME: u16 = 2;
const M_ALARM_DAYS: u16 = 3;
const DLG_RESET: u16 = 0x310;
/// Alarm list row tags: the alarm's index; the note row.
/// Lap rows shown.
const LAP_ROWS: usize = 3;
/// Alarm rows visible under the tabs.
const ALARM_ROWS: u32 = 4;

/// `MM:SS.cc` below an hour, `H:MM:SS` from there (05 §7.8).
pub fn push_stopwatch<const N: usize>(b: &mut StrBuf<N>, ms: u64) {
    let cs = ms / 10;
    let s = cs / 100;
    if s >= 3600 {
        b.push_u32(u32::try_from(s / 3600).unwrap_or(u32::MAX));
        b.push(':');
        b.push_u32_padded(u32::try_from(s / 60 % 60).unwrap_or(0), 2);
        b.push(':');
        b.push_u32_padded(u32::try_from(s % 60).unwrap_or(0), 2);
    } else {
        b.push_u32_padded(u32::try_from(s / 60).unwrap_or(0), 2);
        b.push(':');
        b.push_u32_padded(u32::try_from(s % 60).unwrap_or(0), 2);
        b.push('.');
        b.push_u32_padded(u32::try_from(cs % 100).unwrap_or(0), 2);
    }
}

/// A lap: `M:SS.cc` (minutes not padded), `H:MM:SS` from an hour.
pub fn push_lap<const N: usize>(b: &mut StrBuf<N>, ms: u32) {
    let cs = ms / 10;
    let s = cs / 100;
    if s >= 3600 {
        push_hms(b, s);
    } else {
        b.push_u32(s / 60);
        b.push(':');
        b.push_u32_padded(s % 60, 2);
        b.push('.');
        b.push_u32_padded(cs % 100, 2);
    }
}

/// `M:SS` below an hour, `H:MM:SS` from there.
pub fn push_hms<const N: usize>(b: &mut StrBuf<N>, secs: u32) {
    if secs >= 3600 {
        b.push_u32(secs / 3600);
        b.push(':');
        b.push_u32_padded(secs / 60 % 60, 2);
    } else {
        b.push_u32(secs / 60);
    }
    b.push(':');
    b.push_u32_padded(secs % 60, 2);
}

/// A time split for the large display: the part in 28 px numerals and the
/// rest in 16 px ("00:22" + ".37", "1:01" + ":05").
#[must_use]
pub fn split_time(s: &str) -> (&str, &str) {
    let at = s.find('.').or_else(|| (s.matches(':').count() == 2).then(|| s.rfind(':')).flatten());
    match at {
        Some(i) => s.split_at(i),
        None => (s, ""),
    }
}

/// `HH:MM` of minutes after midnight.
pub fn push_hm<const N: usize>(b: &mut StrBuf<N>, minute: u16) {
    b.push_u32_padded(u32::from(minute / 60), 2);
    b.push(':');
    b.push_u32_padded(u32::from(minute % 60), 2);
}

/// The repeat days in words: Once, Every day, Weekdays, Weekend, or the
/// short names ("Mo We Fr").
pub fn push_days<const N: usize>(b: &mut StrBuf<N>, days: u8, lang: Lang) {
    let named = match days & 0x7F {
        0 => Some(Str::Once),
        0x7F => Some(Str::EveryDay),
        0x1F => Some(Str::Weekdays),
        0x60 => Some(Str::Weekend),
        _ => None,
    };
    if let Some(n) = named {
        b.push_str(lang.get(n));
        return;
    }
    let mut first = true;
    for (i, d) in super::steps::DAY_SHORT.iter().enumerate() {
        if days & (1 << i) != 0 {
            if !first {
                b.push(' ');
            }
            b.push_str(lang.get(*d));
            first = false;
        }
    }
}

/// The alarm list: four alarms ("07:30 Mo–Fr" and an on/off switch) and,
/// while alarms cannot wake the player, a note. Smaller than the general
/// `Rows` (this screen is one of `AppScreen`'s largest).
#[derive(Clone, Copy, Debug, Default)]
struct AlarmRows {
    text: [StrBuf<24>; ALARMS],
    on: [bool; ALARMS],
    note: Option<&'static str>,
}

impl ListSource for AlarmRows {
    fn len(&self) -> u32 {
        ALARMS as u32 + u32::from(self.note.is_some())
    }

    fn row(&self, i: u32) -> Row<'_> {
        let i = usize::try_from(i).unwrap_or(usize::MAX);
        match (self.text.get(i), self.on.get(i)) {
            (Some(t), Some(on)) => Row::Item(Item::new(t.as_str()).accessory(Accessory::Toggle(*on))),
            _ => self.note.map_or(Row::Pending, |n| Row::Item(Item::new(n).dim())),
        }
    }
}

/// What the stopwatch and timer tabs showed.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Shown {
    tab: u8,
    /// The big time.
    time: StrBuf<12>,
    laps: u16,
    lap_top: u8,
    sel: u8,
    /// Stopwatch: running, or stopped at a time.
    running: bool,
    started: bool,
    timer_started: bool,
    timer_running: bool,
    duration: u32,
    progress: u16,
}

pub struct ToolsScreen {
    tab: u8,
    /// First lap row shown (0 = newest).
    lap_top: u8,
    /// Timer preset chosen.
    sel: u8,
    list: ListView,
    rows: AlarmRows,
    lang: Lang,
    wake: bool,
    drawn: Option<Shown>,
}

impl Default for ToolsScreen {
    fn default() -> Self {
        ToolsScreen {
            tab: 0,
            lap_top: 0,
            sel: 0,
            list: ListView::new(),
            rows: AlarmRows::default(),
            lang: Lang::ENGLISH,
            wake: false,
            drawn: None,
        }
    }
}

impl ToolsScreen {
    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        self.wake = svc.alarm_wakes_from_off();
        let l = self.lang;
        let mut rows = AlarmRows::default();
        for (i, (t, on)) in rows.text.iter_mut().zip(rows.on.iter_mut()).enumerate() {
            let a = tools::alarm(svc, i);
            push_hm(t, a.minute);
            t.push(' ');
            push_days(t, a.days, l);
            *on = a.on;
        }
        rows.note = (!self.wake).then(|| l.get(Str::AlarmOnlyOn));
        self.rows = rows;
        self.list.invalidate();
    }

    fn reload<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let f = self.list.focus;
        self.refresh(cx.svc);
        let mut l = self.list;
        l.set_focus(&self.rows, f, ALARM_ROWS, cx.now);
        self.list = l;
    }

    fn preset<S: Services + ?Sized>(&self, svc: &S, i: u8) -> u32 {
        tools::preset(svc, usize::from(i))
    }

    fn shown<S: Services + ?Sized>(&self, cx: &Cx<'_, S>) -> Shown {
        let t = cx.svc.tools();
        let now = cx.now.0;
        let mut s = Shown { tab: self.tab, lap_top: self.lap_top, sel: self.sel, ..Shown::default() };
        match self.tab {
            TAB_SW => {
                push_stopwatch(&mut s.time, t.stopwatch.elapsed(now));
                s.laps = t.stopwatch.laps();
                s.running = t.stopwatch.running();
                s.started = t.stopwatch.started();
            }
            TAB_TIMER => {
                let left = t.timer.left(now);
                // Whole seconds, rounded up while it counts.
                push_hms(&mut s.time, left.div_ceil(1000));
                s.timer_started = t.timer.started();
                s.timer_running = t.timer.running();
                s.duration = t.timer.duration();
                if s.timer_started && s.duration > 0 {
                    s.progress =
                        u16::try_from(u64::from(left) * 1000 / u64::from(s.duration)).unwrap_or(1000);
                }
            }
            _ => {}
        }
        s
    }

    fn draw_time(g: &mut Gfx<'_>, time: &str) {
        let (w, _) = g.size();
        g.fill_role(Rect::new(0, TABS_H, w, 38), Role::Bg);
        draw_big_time(g, 50, time);
    }

    fn draw_laps<S: Services + ?Sized>(&self, cx: &Cx<'_, S>, g: &mut Gfx<'_>) {
        let (w, h) = g.size();
        g.fill_role(Rect::new(0, 54, w, h - 54), Role::Bg);
        let sw = &cx.svc.tools().stopwatch;
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let rule = g.pal.mix(Role::Bg, Role::TextDim, 60);
        if sw.kept() > 0 {
            g.hline(4, 55, w - 8, rule);
        }
        for r in 0..LAP_ROWS {
            let Some(lap) = sw.lap_at(usize::from(self.lap_top) + r) else { break };
            let y = 69 + 13 * i16::try_from(r).unwrap_or(0);
            // Number, lap time, split.
            let mut t = StrBuf::<12>::new();
            t.push_u32(u32::from(lap.number));
            g.text_in(2, y, 18, t.as_str(), FontId::Ui, dim, Align::Right);
            t.clear();
            push_lap(&mut t, lap.time_ms);
            g.text_in(20, y, 52, t.as_str(), FontId::Ui, text, Align::Right);
            t.clear();
            push_lap(&mut t, lap.split_ms);
            g.text_in(72, y, w - 76, t.as_str(), FontId::Ui, dim, Align::Right);
        }
    }

    fn draw_timer<S: Services + ?Sized>(&self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, s: &Shown) {
        let (w, h) = g.size();
        g.fill_role(Rect::new(0, 54, w, h - 54), Role::Bg);
        if s.timer_started {
            draw_progress(g, Rect::new(10, 57, w - 20, 4), s.progress, None, Role::Bg);
        }
        // The presets as chips in up to two centred rows; the chosen one
        // (while not started) filled.
        let mut labels = [StrBuf::<10>::new(); PRESETS];
        let mut widths = [0i16; PRESETS];
        let mut n = 0;
        for (i, (lb, wd)) in labels.iter_mut().zip(widths.iter_mut()).enumerate() {
            let p = self.preset(cx.svc, u8::try_from(i).unwrap_or(0));
            if p == 0 {
                break;
            }
            push_hms(lb, p);
            *wd = g.measure(lb.as_str(), FontId::Ui) + 8;
            n += 1;
        }
        let text = g.c(Role::Text);
        let acc = g.c(Role::Accent);
        let on_acc = g.c(Role::OnAccent);
        let chip = g.pal.mix(Role::Bg, Role::TextDim, 50);
        const GAP: i16 = 3;
        let mut first = 0;
        for row in 0..2i16 {
            // As many chips as fit in this row.
            let mut end = first;
            let mut total = 0;
            while let Some(&cw) = widths.get(end).filter(|_| end < n)
                && total + cw <= w - 4
            {
                total += cw + GAP;
                end += 1;
            }
            let mut x = (w - total + GAP) / 2;
            for (i, (lb, &cw)) in labels.iter().zip(widths.iter()).enumerate().take(end).skip(first) {
                let r = Rect::new(x, 63 + row * 17, cw, 15);
                let chosen = !s.timer_started && usize::from(self.sel) == i;
                let (bg, fg) = if chosen { (acc, on_acc) } else { (chip, text) };
                g.fill_round(r, 5, bg);
                g.text_in(r.x, r.y + 12, r.w, lb.as_str(), FontId::Ui, fg, Align::Center);
                x += cw + GAP;
            }
            first = end;
        }
    }

    fn switch_tab(&mut self, to: u8) {
        self.tab = to % 3;
        self.drawn = None;
        self.list.invalidate();
    }

    fn next_key<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let now = cx.now.0;
        let l = self.lang;
        match self.tab {
            TAB_SW => {
                let sw = &mut cx.svc.tools_mut().stopwatch;
                if sw.running() {
                    sw.lap(now);
                    self.lap_top = 0;
                } else if sw.started() {
                    if sw.laps() > 0 {
                        cx.ui.dialog(DialogSpec::destructive(
                            DLG_RESET,
                            l.get(Str::ResetStopwatchQ),
                            "",
                            l.get(Str::Cancel),
                            l.get(Str::ResetBtn),
                        ));
                    } else {
                        sw.reset();
                    }
                } else {
                    self.switch_tab(TAB_TIMER);
                }
            }
            TAB_TIMER => {
                let t = &mut cx.svc.tools_mut().timer;
                if t.started() {
                    t.reset();
                } else {
                    self.switch_tab(TAB_ALARM);
                }
            }
            _ => self.switch_tab(TAB_SW),
        }
    }

    fn ok_key<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let now = cx.now.0;
        match self.tab {
            TAB_SW => cx.svc.tools_mut().stopwatch.toggle(now),
            TAB_TIMER => {
                if !cx.svc.tools().timer.started() {
                    let secs = cx.svc.tools().timer.duration() / 1000;
                    if secs == 0 {
                        return;
                    }
                    tools::remember_preset(cx.svc, secs);
                    self.sel = 0;
                }
                cx.svc.tools_mut().timer.toggle(now);
            }
            _ => {}
        }
    }

    /// Up/Down on the timer tab: choose a preset (it becomes the duration).
    fn choose_preset<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, up: bool) {
        if cx.svc.tools().timer.started() {
            return;
        }
        let n = (0..PRESETS).take_while(|&i| tools::preset(cx.svc, i) > 0).count();
        let n = u8::try_from(n).unwrap_or(0);
        if n == 0 {
            return;
        }
        let next = if up { self.sel + n - 1 } else { self.sel + 1 };
        self.sel = next.checked_rem(n).unwrap_or(0);
        let p = self.preset(cx.svc, self.sel);
        cx.svc.tools_mut().timer.set(p * 1000);
    }

    fn focused_alarm(&self) -> Option<usize> {
        usize::try_from(self.list.focus).ok().filter(|&i| i < ALARMS)
    }
}

impl<S: Services + ?Sized> Screen<S> for ToolsScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.tab = u8::try_from(arg.0 % 3).unwrap_or(0);
        self.refresh(cx.svc);
        let mut l = self.list;
        l.set_focus(&self.rows, 0, ALARM_ROWS, cx.now);
        self.list = l;
        // A timer never set starts from the newest preset.
        if cx.svc.tools().timer.duration() == 0 {
            let p = tools::preset(cx.svc, 0);
            cx.svc.tools_mut().timer.set(p * 1000);
        }
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(Key::Prev, KeyPhase::Down | KeyPhase::Repeat) => {
                self.switch_tab(self.tab + 2);
                return Nav::Stay;
            }
            Event::Key(Key::Next, KeyPhase::Down) => {
                self.next_key(cx);
                return Nav::Stay;
            }
            // Prev/Next belong to the tabs: the rest of the press (its
            // release) must not page the alarm list, which would roll it
            // round (UI-8).
            Event::Key(Key::Prev | Key::Next, _) => return Nav::Stay,
            Event::Ui(UiEvent::Dialog { id: DLG_RESET, button }) => {
                if button == 1 {
                    cx.svc.tools_mut().stopwatch.reset();
                    self.lap_top = 0;
                }
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id, .. }) => {
                return match id {
                    M_DURATION => Nav::Push(PICKER, Arg(PICK_TIMER)),
                    M_ALARM_TIME | M_ALARM_DAYS => match self.focused_alarm() {
                        Some(i) if id == M_ALARM_TIME => {
                            Nav::Push(PICKER, Arg(PICK_ALARM + u32::try_from(i).unwrap_or(0)))
                        }
                        Some(i) => Nav::Push(ALARM_DAYS, Arg(u32::try_from(i).unwrap_or(0))),
                        None => Nav::Stay,
                    },
                    _ => Nav::Stay,
                };
            }
            Event::Service(_) => {
                self.reload(cx);
                return Nav::Stay;
            }
            _ => {}
        }
        match self.tab {
            TAB_ALARM => {
                let mut l = self.list;
                let act = l.handle(&self.rows, ev, ALARM_ROWS, cx.list());
                self.list = l;
                if let ListAction::Activated(_) = act
                    && let Some(i) = self.focused_alarm()
                {
                    let key = tools::alarm_keys(i).2;
                    let on = cx.svc.setting(key) == 0;
                    cx.svc.set_setting(key, i16::from(on));
                    if !on {
                        cx.svc.tools_mut().cancel_snooze(u8::try_from(i).unwrap_or(0));
                    }
                    self.reload(cx);
                }
            }
            _ => match *ev {
                Event::Key(Key::Ok, KeyPhase::Down) => self.ok_key(cx),
                Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                    if self.tab == TAB_SW {
                        // The laps scroll by whole rows with the toolkit's
                        // list rules (roll-around, repeat stops at the end).
                        let max = cx.svc.tools().stopwatch.kept().saturating_sub(LAP_ROWS);
                        let n = u32::try_from(max).map_or(1, |m| m + 1);
                        if let Some((t, _)) =
                            ListView::step_plain(u32::from(self.lap_top), 0, n, 1, ev, cx.list())
                        {
                            self.lap_top = u8::try_from(t).unwrap_or(0);
                        }
                    } else {
                        self.choose_preset(cx, k == Key::Up);
                    }
                }
                _ => {}
            },
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let (w, h) = g.size();
        let l = self.lang;
        let s = self.shown(cx);
        let full = dirty == Dirty::All || self.drawn.is_none_or(|d| d.tab != s.tab);
        if full {
            g.clear(Role::Bg);
            draw_tabs(
                g,
                Rect::new(0, 0, w, TABS_H),
                &[l.get(Str::AppStopwatch), l.get(Str::TabTimer), l.get(Str::TabAlarm)],
                usize::from(self.tab),
                Role::Bg,
            );
        }
        if self.tab == TAB_ALARM {
            let mut list = self.list;
            g.with_area(Rect::new(0, TABS_H, w, h - TABS_H), |g| {
                list.draw(&self.rows, g, if full { Dirty::All } else { dirty }, cx.now, Role::Bg);
            });
            self.list = list;
            self.drawn = Some(s);
            return;
        }
        let before = if full { None } else { self.drawn };
        if before.is_none_or(|d| d.time != s.time) {
            Self::draw_time(g, s.time.as_str());
        }
        let rest = |d: &Shown| (d.laps, d.lap_top, d.sel, d.timer_started, d.duration, d.progress);
        if before.is_none_or(|d| rest(&d) != rest(&s)) {
            if self.tab == TAB_SW {
                self.draw_laps(cx, g);
            } else {
                self.draw_timer(cx, g, &s);
            }
        }
        self.drawn = Some(s);
    }

    fn menu(&self, _cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
        match self.tab {
            TAB_TIMER => {
                out.action(M_DURATION, l.get(Str::SetDuration));
            }
            TAB_ALARM if self.focused_alarm().is_some() => {
                out.action(M_ALARM_TIME, l.get(Str::AlarmTime));
                out.action(M_ALARM_DAYS, l.get(Str::RepeatDays));
            }
            _ => {}
        }
    }

    fn wants(&self) -> Wants {
        let hint = match (self.tab, self.drawn) {
            (TAB_SW, Some(d)) if d.running => Some(Str::HintSwRunning),
            (TAB_SW, Some(d)) if d.started => Some(Str::HintSwStopped),
            (TAB_SW, _) => Some(Str::HintSwIdle),
            (TAB_TIMER, Some(d)) if d.timer_started => Some(Str::HintTimerRunning),
            (TAB_TIMER, _) => Some(Str::HintTimer),
            _ => None,
        };
        // The time moves while the stopwatch or the timer runs (seen in
        // the last frame: the first frame after a start asks for them).
        let moving = self.drawn.is_some_and(|d| d.running || d.timer_running);
        Wants {
            ticks: (self.tab != TAB_ALARM && moving) || (self.tab == TAB_ALARM && self.list.wants_ticks()),
            title: Some(self.lang.get(Str::AppStopwatch)),
            strip: hint.map_or(StripUse::Auto, |h| StripUse::Hint(self.lang.get(h))),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume {
            focus: self.list.focus,
            top: self.list.top,
            handle: u32::from(self.tab) | u32::from(self.sel) << 8 | u32::from(self.lap_top) << 16,
            ..Resume::default()
        }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.tab = u8::try_from(r.handle & 0xFF).unwrap_or(0) % 3;
        self.sel = u8::try_from(r.handle >> 8 & 0xFF).unwrap_or(0);
        self.lap_top = u8::try_from(r.handle >> 16 & 0xFF).unwrap_or(0);
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(&self.rows, r.focus, ALARM_ROWS, cx.now);
        self.list = l;
    }
}

/// Repeat days of an alarm: seven check rows.
pub struct AlarmDaysScreen {
    alarm: usize,
    list: ListView,
    rows: Rows,
    lang: Lang,
}

impl Default for AlarmDaysScreen {
    fn default() -> Self {
        AlarmDaysScreen { alarm: 0, list: ListView::new(), rows: Rows::default(), lang: Lang::ENGLISH }
    }
}

const DAY_NAMES: [Str; 7] =
    [Str::Monday, Str::Tuesday, Str::Wednesday, Str::Thursday, Str::Friday, Str::Saturday, Str::Sunday];

impl AlarmDaysScreen {
    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        let days = tools::alarm(svc, self.alarm).days;
        let mut rows = Rows::default();
        if let Some(h) = rows.push(self.lang.get(Str::Repeat), 0x100) {
            h.header = true;
        }
        for (i, d) in DAY_NAMES.iter().enumerate() {
            if let Some(r) = rows.push(self.lang.get(*d), u16::try_from(i).unwrap_or(0)) {
                r.acc = Accessory::Check(days & (1 << i) != 0);
            }
        }
        self.rows = rows;
        self.list.invalidate();
    }
}

impl<S: Services + ?Sized> Screen<S> for AlarmDaysScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.alarm = usize::try_from(arg.0).unwrap_or(0).min(ALARMS - 1);
        self.refresh(cx.svc);
        let mut l = self.list;
        l.set_focus(&self.rows, 1, 5, cx.now);
        self.list = l;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) {
            return Nav::Pop;
        }
        let mut l = self.list;
        let act = l.handle(&self.rows, ev, 5, cx.list());
        self.list = l;
        if let ListAction::Activated(i) = act
            && let Some(r) = self.rows.get(i)
            && r.tag < 7
        {
            let key = tools::alarm_keys(self.alarm).1;
            let days = cx.svc.setting(key) ^ (1 << r.tag);
            cx.svc.set_setting(key, days);
            let f = self.list.focus;
            self.refresh(cx.svc);
            let mut l = self.list;
            l.set_focus(&self.rows, f, 5, cx.now);
            self.list = l;
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut l = self.list;
        l.draw(&self.rows, g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: self.list.wants_ticks(),
            title: Some(self.lang.get(Str::TabAlarm)),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        let mut l = self.list;
        l.top = r.top;
        l.set_focus(&self.rows, r.focus, 5, cx.now);
        self.list = l;
    }
}

/// The alert over any screen: the timer reached zero, or an alarm rings.
/// OK or Back stop it; Next snoozes an alarm. It closes by itself when the
/// ringing ends (after a minute) or another screen stopped it.
#[derive(Default)]
pub struct AlertScreen {
    lang: Lang,
    alert: Option<Alert>,
    /// Alarm time or timer duration shown.
    value: StrBuf<12>,
    blink: Option<bool>,
}

impl<S: Services + ?Sized> Screen<S> for AlertScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.alert = cx.svc.tools().ringing();
        self.value.clear();
        match self.alert {
            Some(Alert::Alarm(i)) => push_hm(&mut self.value, tools::alarm(cx.svc, usize::from(i)).minute),
            Some(Alert::Timer) => push_hms(&mut self.value, cx.svc.tools().timer.duration() / 1000),
            None => {}
        }
        self.blink = None;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if cx.svc.tools().ringing().is_none() {
            // Ended by itself, or this is an older alert under a newer one.
            return Nav::Pop;
        }
        match *ev {
            Event::Key(Key::Ok | Key::Back, KeyPhase::Up { .. }) => {
                cx.svc.tools_mut().dismiss();
                Nav::Pop
            }
            Event::Key(Key::Next, KeyPhase::Up { .. }) if matches!(self.alert, Some(Alert::Alarm(_))) => {
                cx.svc.tools_mut().snooze(cx.now.0);
                cx.ui.toast(self.lang.get(Str::Snooze));
                Nav::Pop
            }
            Event::Service(ServiceEvent::Other { kind: other::TOOLS, value: tools_event::ALERT_ENDED }) => {
                Nav::Pop
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let (w, _) = g.size();
        let on = (cx.now.0 / 500) % 2 == 0;
        if dirty == Dirty::Changed && self.blink == Some(on) {
            return;
        }
        let l = self.lang;
        if dirty == Dirty::All || self.blink.is_none() {
            g.clear(Role::Bg);
            let text = g.c(Role::Text);
            let title = match self.alert {
                Some(Alert::Timer) => l.get(Str::TimesUp),
                _ => l.get(Str::TabAlarm),
            };
            g.text_in(4, 50, w - 8, title, FontId::UiBold, text, Align::Center);
            draw_big_time(g, 84, self.value.as_str());
        }
        let c = if on { g.c(Role::Accent) } else { g.c(Role::TextDim) };
        g.fill_role(Rect::new((w - 24) / 2, 8, 24, 24), Role::Bg);
        g.icon2((w - 24) / 2, 8, &icons::STOPWATCH, c);
        self.blink = Some(on);
    }

    fn wants(&self) -> Wants {
        let hint = match self.alert {
            Some(Alert::Alarm(_)) => Str::HintAlert,
            _ => Str::HintAlertTimer,
        };
        Wants {
            ticks: true,
            title: Some(self.lang.get(Str::AppStopwatch)),
            strip: StripUse::Hint(self.lang.get(hint)),
            ..Wants::default()
        }
    }
}

/// A time in large numerals centred at baseline `y`: the part
/// [`split_time`] keeps large in 28 px, the rest in 16 px.
pub(crate) fn draw_big_time(g: &mut Gfx<'_>, y: i16, time: &str) {
    let (w, _) = g.size();
    let (big, small) = split_time(time);
    let bw = g.measure(big, FontId::Num28);
    let sw = if small.is_empty() { 0 } else { g.measure(small, FontId::Num16) + 1 };
    let x = (w - bw - sw) / 2;
    let text = g.c(Role::Text);
    let end = g.text(x, y, big, FontId::Num28, text);
    if !small.is_empty() {
        g.text(end + 1, y, small, FontId::Num16, text);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn time_formats() {
        assert_eq!(split_time("00:22.37"), ("00:22", ".37"));
        assert_eq!(split_time("1:01:05"), ("1:01", ":05"));
        assert_eq!(split_time("25:00"), ("25:00", ""));
        let mut b = StrBuf::<16>::new();
        push_stopwatch(&mut b, 0);
        assert_eq!(b.as_str(), "00:00.00");
        b.clear();
        push_stopwatch(&mut b, 59 * 60_000 + 59_999);
        assert_eq!(b.as_str(), "59:59.99");
        b.clear();
        push_stopwatch(&mut b, 3_600_000 + 61_000);
        assert_eq!(b.as_str(), "1:01:01");
        b.clear();
        push_lap(&mut b, 21_370);
        assert_eq!(b.as_str(), "0:21.37");
        b.clear();
        push_hms(&mut b, 1500);
        assert_eq!(b.as_str(), "25:00");
        b.clear();
        push_hms(&mut b, 5400);
        assert_eq!(b.as_str(), "1:30:00");
        b.clear();
        push_hm(&mut b, 7 * 60 + 5);
        assert_eq!(b.as_str(), "07:05");
        b.clear();
        push_days(&mut b, 0x15, Lang::ENGLISH);
        assert_eq!(b.as_str(), "Mo We Fr");
        b.clear();
        push_days(&mut b, 0x1F, Lang::ENGLISH);
        assert_eq!(b.as_str(), "Mo–Fr");
    }
}
