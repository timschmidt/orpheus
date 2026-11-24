//! Steps (05 §7.7, WP UI-5c): today's count against the daily goal with
//! distance and energy, and the last seven days as bars. Counting runs in
//! the firmware's sensor task, not here (SENS-1).
//!
//! Tabs Today · Week (Prev/Next). Week: Up/Down choose a day, its numbers
//! show under the bars. Menu: Daily goal, Stride length, Weight, Units,
//! Count in background, Sensitivity, Reset today. The 30-day Month tab of
//! 05 §7.7 waits for a longer history (the file keeps seven days).

use orpheus_ui::event::{Event, Key, KeyPhase, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::misc::{draw_progress, draw_tabs};

use super::{CHOICE, SLIDER};
use crate::services::{Services, StepsInfo};
use crate::settings::{self, id};
use crate::strings::{Lang, Str};

const M_GOAL: u16 = 1;
const M_STRIDE: u16 = 2;
const M_WEIGHT: u16 = 3;
const M_UNITS: u16 = 4;
const M_BACKGROUND: u16 = 5;
const M_SENSITIVITY: u16 = 6;
const M_RESET: u16 = 7;
const DLG_RESET: u16 = 0x300;

/// Days shown (today and six before).
const DAYS: u8 = 7;
/// Tab row height.
const TABS_H: i16 = 16;

/// Short weekday names, Monday first.
pub const DAY_SHORT: [Str; 7] =
    [Str::DayMo, Str::DayTu, Str::DayWe, Str::DayTh, Str::DayFr, Str::DaySa, Str::DaySu];

/// `n` with a space between thousands ("10 000").
pub fn push_grouped<const N: usize>(b: &mut StrBuf<N>, n: u32) {
    let mut groups = [0u32; 4];
    let mut k = 0;
    let mut v = n;
    loop {
        if let Some(g) = groups.get_mut(k) {
            *g = v % 1000;
        }
        k += 1;
        v /= 1000;
        if v == 0 || k == groups.len() {
            break;
        }
    }
    for i in (0..k).rev() {
        let g = groups.get(i).copied().unwrap_or(0);
        if i + 1 == k {
            b.push_u32(g + v * 1000);
        } else {
            b.push(' ');
            b.push_u32_padded(g, 3);
        }
    }
}

/// Distance of `steps` in the chosen unit with one decimal ("4.7 km").
pub fn push_distance<S: Services + ?Sized, const N: usize>(
    b: &mut StrBuf<N>,
    svc: &S,
    steps: u32,
    lang: Lang,
) {
    let stride = u16::try_from(svc.setting(id::STRIDE)).unwrap_or(70);
    let m = orpheus_steps::distance_m(steps, stride);
    let miles = svc.setting(id::STEP_UNITS) == 1;
    // Tenths of a km or a mile, rounded.
    let tenths = if miles { (u64::from(m) * 10 + 804) / 1609 } else { (u64::from(m) + 50) / 100 };
    let tenths = u32::try_from(tenths).unwrap_or(u32::MAX);
    push_grouped(b, tenths / 10);
    b.push('.');
    b.push_u32(tenths % 10);
    b.push(' ');
    b.push_str(lang.get(if miles { Str::Mi } else { Str::Km }));
}

/// Energy of `steps` ("212 kcal").
pub fn push_energy<S: Services + ?Sized, const N: usize>(b: &mut StrBuf<N>, svc: &S, steps: u32, lang: Lang) {
    let stride = u16::try_from(svc.setting(id::STRIDE)).unwrap_or(70);
    let weight = u16::try_from(svc.setting(id::WEIGHT)).unwrap_or(70);
    push_grouped(b, orpheus_steps::kcal(orpheus_steps::distance_m(steps, stride), weight));
    b.push(' ');
    b.push_str(lang.get(Str::Kcal));
}

/// What was drawn (a change redraws the content).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Shown {
    tab: u8,
    sel: u8,
    days: [u32; DAYS as usize],
    info: StepsInfo,
    goal: i16,
    stride: i16,
    weight: i16,
    units: i16,
    weekday: u8,
}

#[derive(Default)]
pub struct StepsScreen {
    /// 0 Today, 1 Week.
    tab: u8,
    /// Week: the day chosen, days ago.
    sel: u8,
    lang: Lang,
    /// The strip hint (counting off in the background, no history).
    hint: Option<Str>,
    drawn: Option<Shown>,
}

impl StepsScreen {
    fn shown<S: Services + ?Sized>(&self, svc: &S) -> Shown {
        Shown {
            tab: self.tab,
            sel: self.sel,
            days: core::array::from_fn(|k| svc.steps_day(u8::try_from(k).unwrap_or(0))),
            info: svc.steps_info(),
            goal: svc.setting(id::STEP_GOAL),
            stride: svc.setting(id::STRIDE),
            weight: svc.setting(id::WEIGHT),
            units: svc.setting(id::STEP_UNITS),
            weekday: if svc.clock_valid() { svc.now().weekday % 7 } else { 0 },
        }
    }

    fn draw_today<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>, s: &Shown) {
        let (w, _) = g.size();
        let l = self.lang;
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let today = s.days[0];
        if !s.info.sensor {
            g.text_in(4, 46, w - 8, l.get(Str::NoSensor), FontId::UiBold, dim, Align::Center);
            return;
        }
        let mut n = StrBuf::<16>::new();
        push_grouped(&mut n, today);
        g.text_in(0, 48, w, n.as_str(), FontId::Num28, text, Align::Center);
        let goal = u32::try_from(s.goal).unwrap_or(10_000).max(1);
        let mut cap = StrBuf::<40>::new();
        if today >= goal {
            cap.push_str(l.get(Str::GoalReached));
        } else {
            cap.push_str(l.get(Str::Goal));
            cap.push(' ');
            push_grouped(&mut cap, goal);
        }
        g.text_in(4, 62, w - 8, cap.as_str(), FontId::Ui, dim, Align::Center);
        let pct = u16::try_from((u64::from(today) * 1000 / u64::from(goal)).min(1000)).unwrap_or(1000);
        draw_progress(g, Rect::new(10, 67, w - 20, 5), pct, None, Role::Bg);
        let mut line = StrBuf::<48>::new();
        push_distance(&mut line, svc, today, l);
        line.push_str(" · ");
        push_energy(&mut line, svc, today, l);
        g.text_in(4, 89, w - 8, line.as_str(), FontId::Ui, text, Align::Center);
    }

    fn draw_week<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>, s: &Shown) {
        let (w, _) = g.size();
        let l = self.lang;
        let goal = u32::try_from(s.goal).unwrap_or(10_000).max(1);
        let top = s.days.iter().copied().max().unwrap_or(0).max(goal);
        let (base, max_h) = (63i16, 40i16);
        let slot = 18;
        let x0 = (w - slot * 7) / 2;
        let acc = g.c(Role::Accent);
        let bar = g.pal.mix(Role::Bg, Role::TextDim, 150);
        let rule = g.pal.mix(Role::Bg, Role::TextDim, 70);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        // The goal as a dotted line, the baseline.
        let gy = base - i16::try_from(u64::from(goal) * 40 / u64::from(top)).unwrap_or(max_h);
        let mut x = x0;
        while x < x0 + slot * 7 {
            g.hline(x, gy, 2, rule);
            x += 4;
        }
        g.hline(x0, base, slot * 7, rule);
        for i in 0..DAYS {
            // Oldest on the left.
            let ago = DAYS - 1 - i;
            let v = s.days.get(usize::from(ago)).copied().unwrap_or(0);
            let h = i16::try_from(u64::from(v) * 40 / u64::from(top)).unwrap_or(max_h);
            let x = x0 + slot * i16::from(i);
            let c = if ago == s.sel { acc } else { bar };
            if h > 0 {
                g.fill(Rect::new(x + 4, base - h, slot - 8, h), c);
            }
            let wd = (usize::from(s.weekday) + 7 - usize::from(ago)) % 7;
            let name = l.get(DAY_SHORT.get(wd).copied().unwrap_or(Str::DayMo));
            let (font, tc) = if ago == s.sel { (FontId::UiBold, text) } else { (FontId::Ui, dim) };
            g.text_in(x, 77, slot, name, font, tc, Align::Center);
        }
        let v = s.days.get(usize::from(s.sel)).copied().unwrap_or(0);
        let mut line = StrBuf::<48>::new();
        push_grouped(&mut line, v);
        line.push_str(" · ");
        push_distance(&mut line, svc, v, l);
        g.text_in(4, 93, w - 8, line.as_str(), FontId::Ui, text, Align::Center);
    }

    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        self.hint = steps_hint(svc.steps_info(), svc.setting(id::STEP_BACKGROUND) != 0);
    }

    fn open_setting(menu: u16) -> Nav {
        let (screen, key) = match menu {
            M_GOAL => (SLIDER, id::STEP_GOAL),
            M_STRIDE => (SLIDER, id::STRIDE),
            M_WEIGHT => (SLIDER, id::WEIGHT),
            M_UNITS => (CHOICE, id::STEP_UNITS),
            M_BACKGROUND => (CHOICE, id::STEP_BACKGROUND),
            _ => (CHOICE, id::STEP_SENSITIVITY),
        };
        Nav::Push(screen, Arg(u32::from(key.0)))
    }
}

impl<S: Services + ?Sized> Screen<S> for StepsScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.refresh(cx.svc);
        self.tab = u8::from(arg.0 == 1);
        self.sel = 0;
        self.drawn = None;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => Nav::Pop,
            Event::Key(Key::Prev | Key::Next, KeyPhase::Down | KeyPhase::Repeat) => {
                self.tab ^= 1;
                Nav::Stay
            }
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) if self.tab == 1 => {
                self.sel =
                    if k == Key::Up { self.sel.saturating_sub(1) } else { (self.sel + 1).min(DAYS - 1) };
                Nav::Stay
            }
            Event::Ui(UiEvent::Menu { id: M_RESET, .. }) => {
                let l = self.lang;
                cx.ui.dialog(DialogSpec::destructive(
                    DLG_RESET,
                    l.get(Str::ResetTodayQ),
                    l.get(Str::ResetTodayText),
                    l.get(Str::Cancel),
                    l.get(Str::ResetBtn),
                ));
                Nav::Stay
            }
            Event::Ui(UiEvent::Menu { id, .. }) => Self::open_setting(id),
            Event::Ui(UiEvent::Dialog { id: DLG_RESET, button: 1 }) => {
                cx.svc.steps_reset_today();
                cx.ui.toast(self.lang.get(Str::Done));
                Nav::Stay
            }
            Event::Service(_) => {
                self.refresh(cx.svc);
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let s = self.shown(cx.svc);
        if dirty == Dirty::Changed && self.drawn == Some(s) {
            return;
        }
        let (w, _) = g.size();
        g.clear(Role::Bg);
        let l = self.lang;
        draw_tabs(
            g,
            Rect::new(0, 0, w, TABS_H),
            &[l.get(Str::TabToday), l.get(Str::TabWeek)],
            usize::from(self.tab),
            Role::Bg,
        );
        if self.tab == 0 {
            self.draw_today(cx.svc, g, &s);
        } else {
            self.draw_week(cx.svc, g, &s);
        }
        self.drawn = Some(s);
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
        let mut v = StrBuf::<24>::new();
        for (m, key) in [
            (M_GOAL, id::STEP_GOAL),
            (M_STRIDE, id::STRIDE),
            (M_WEIGHT, id::WEIGHT),
            (M_UNITS, id::STEP_UNITS),
            (M_BACKGROUND, id::STEP_BACKGROUND),
            (M_SENSITIVITY, id::STEP_SENSITIVITY),
        ] {
            let Some(d) = settings::def(key) else { continue };
            settings::format_value(d, cx.svc.setting(key), l, &mut v);
            out.value(m, l.get(d.label), v.as_str());
        }
        out.action(M_RESET, l.get(Str::ResetToday));
    }

    fn wants(&self) -> Wants {
        let strip = self.hint.map_or(StripUse::Auto, |h| StripUse::Hint(self.lang.get(h)));
        Wants { title: Some(self.lang.get(Str::AppSteps)), strip, ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: u32::from(self.tab), top: u32::from(self.sel), ..Resume::default() }
    }

    fn restore(&mut self, _cx: &mut Cx<'_, S>, r: &Resume) {
        self.tab = u8::from(r.focus == 1);
        self.sel = u8::try_from(r.top).unwrap_or(0).min(DAYS - 1);
    }
}

/// The strip hint for the Steps app, if one applies: counting is off in
/// the background, or no history is kept (clock not set).
#[must_use]
pub fn steps_hint(info: StepsInfo, background: bool) -> Option<Str> {
    if !info.dated {
        Some(Str::NoHistory)
    } else if !background {
        Some(Str::CountingOff)
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping() {
        let mut b = StrBuf::<24>::new();
        for (n, s) in [(0, "0"), (999, "999"), (1000, "1 000"), (6482, "6 482"), (10_000, "10 000")] {
            b.clear();
            push_grouped(&mut b, n);
            assert_eq!(b.as_str(), s);
        }
        b.clear();
        push_grouped(&mut b, 1_234_567);
        assert_eq!(b.as_str(), "1 234 567");
        b.clear();
        push_grouped(&mut b, u32::MAX);
        assert_eq!(b.as_str(), "4 294 967 295");
        assert_eq!(
            steps_hint(StepsInfo { sensor: true, counting: true, dated: false }, true),
            Some(Str::NoHistory)
        );
        assert_eq!(steps_hint(StepsInfo { sensor: true, counting: true, dated: true }, true), None);
    }
}
