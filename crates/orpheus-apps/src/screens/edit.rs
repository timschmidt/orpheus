//! Value editors: the slider for range settings (live apply) and the
//! time/date picker (05 §5.2 `Slider`, `Picker`).

use orpheus_ui::event::{Event, Key, KeyPhase};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::misc::{draw_big_value, draw_slider};

use super::clocks::{PICK_ALARM, PICK_TIMER};
use crate::services::{DateTime, Services};
use crate::settings::{self, Kind, SettingId, Unit, id};
use crate::strings::{Lang, Str};

/// Range setting editor: large value, slider, Up/Down or Prev/Next step
/// and apply immediately; OK or Back return.
#[derive(Default)]
pub struct SliderScreen {
    sid: u16,
    value: i16,
    lang: Lang,
    drawn: Option<i16>,
}

impl SliderScreen {
    fn range(&self) -> Option<(i16, i16, i16, Unit)> {
        match settings::def(SettingId(self.sid))?.kind {
            Kind::Range { min, max, step, unit } => Some((min, max, step, unit)),
            _ => None,
        }
    }
}

/// The numeral part and unit of a range value for the large display.
fn big_parts(v: i16, unit: Unit, lang: Lang) -> (StrBuf<12>, &'static str) {
    let mut b = StrBuf::<12>::new();
    let signed = |b: &mut StrBuf<12>| {
        if v > 0 {
            b.push('+');
        }
        b.push_i32(i32::from(v));
    };
    let u = match unit {
        Unit::None => {
            b.push_i32(i32::from(v));
            ""
        }
        Unit::Db => {
            signed(&mut b);
            lang.get(Str::Db)
        }
        Unit::Balance => {
            b.push_u32(u32::from(v.unsigned_abs()));
            match v.signum() {
                -1 => "L",
                1 => "R",
                _ => "",
            }
        }
        Unit::UtcMinutes => {
            b.push(if v < 0 { '-' } else { '+' });
            let m = u32::from(v.unsigned_abs());
            b.push_u32_padded(m / 60, 2);
            b.push(':');
            b.push_u32_padded(m % 60, 2);
            "UTC"
        }
        Unit::PerDay => {
            signed(&mut b);
            lang.get(Str::PerDay)
        }
        Unit::Cm => {
            b.push_i32(i32::from(v));
            lang.get(Str::Cm)
        }
        Unit::Kg => {
            b.push_i32(i32::from(v));
            lang.get(Str::Kg)
        }
        Unit::MetresPerSecond => {
            b.push_i32(i32::from(v));
            "m/s"
        }
        #[cfg(feature = "screen-reader")]
        Unit::Quarters => {
            let mut t = StrBuf::<12>::new();
            crate::settings::format_choice(
                crate::settings::Choice::Speed(v.unsigned_abs() * 25),
                lang,
                &mut t,
            );
            b.push_str(t.as_str().trim_end_matches('×'));
            "×"
        }
    };
    (b, u)
}

impl<S: Services + ?Sized> Screen<S> for SliderScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.sid = u16::try_from(arg.0).unwrap_or(id::BRIGHTNESS.0);
        self.value = cx.svc.setting(SettingId(self.sid));
        self.lang = cx.svc.lang();
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) || ev.is_tap(Key::Ok) {
            return Nav::Pop;
        }
        let Some((min, max, step, _)) = self.range() else { return Nav::Stay };
        let dir = match *ev {
            Event::Key(Key::Up | Key::Next, KeyPhase::Down | KeyPhase::Repeat) => 1,
            Event::Key(Key::Down | Key::Prev, KeyPhase::Down | KeyPhase::Repeat) => -1,
            _ => return Nav::Stay,
        };
        let v = (self.value + dir * step).clamp(min, max);
        if v != self.value {
            self.value = v;
            cx.svc.set_setting(SettingId(self.sid), v);
        }
        Nav::Stay
    }

    fn draw(&mut self, _cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn == Some(self.value) {
            return;
        }
        let (w, _) = g.size();
        let Some((min, max, _, unit)) = self.range() else { return };
        if dirty == Dirty::All {
            g.clear(Role::Bg);
            let label = settings::def(SettingId(self.sid)).map_or("", |d| self.lang.get(d.label));
            let dim = g.c(Role::TextDim);
            g.text_in(6, 17, w - 12, label, FontId::UiBold, dim, Align::Center);
        }
        let (num, u) = big_parts(self.value, unit, self.lang);
        draw_big_value(g, Rect::new(0, 22, w, 34), num.as_str(), u, "", Role::Bg);
        draw_slider(
            g,
            Rect::new(10, 62, w - 20, 18),
            i32::from(self.value),
            i32::from(min),
            i32::from(max),
            Role::Bg,
        );
        self.drawn = Some(self.value);
    }

    fn wants(&self) -> Wants {
        Wants { strip: StripUse::Hint(self.lang.get(Str::HintChange)), ..Wants::default() }
    }

    #[cfg(feature = "screen-reader")]
    fn describe(&self, _cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let Some(def) = settings::def(SettingId(self.sid)) else { return };
        let label = self.lang.get(def.label);
        d.title(label);
        let mut v = StrBuf::<24>::new();
        settings::format_value(def, self.value, self.lang, &mut v);
        d.focus(orpheus_ui::a11y::Role::Slider, label).value(v.as_str());
    }
}

/// What the picker edits.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Pick {
    /// The clock's time (arg 0).
    #[default]
    Time,
    /// The clock's date (arg 1).
    Date,
    /// The timer's duration, h:m:s (arg [`PICK_TIMER`]).
    Timer,
    /// Alarm `i`'s time (arg [`PICK_ALARM`] + i).
    Alarm(u8),
}

/// Time (arg 0) or date (arg 1) picker, the timer's duration or an
/// alarm's time (05 §7.8): Prev/Next choose the field, Up/Down change it,
/// OK sets it.
#[derive(Default)]
pub struct PickerScreen {
    pick: Pick,
    date: bool,
    t: DateTime,
    field: u8,
    order: u8,
    lang: Lang,
    drawn: Option<(u8, DateTime)>,
}

const fn days_in(month: u8, year: u16) -> u8 {
    match month {
        2 if year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400)) => 29,
        2 => 28,
        4 | 6 | 9 | 11 => 30,
        _ => 31,
    }
}

impl PickerScreen {
    /// Field kinds in display order: 0 hour, 1 minute, 2 day, 3 month, 4
    /// year, 5 second.
    fn fields(&self) -> &'static [u8] {
        if self.pick == Pick::Timer {
            return &[0, 1, 5];
        }
        if !self.date {
            return &[0, 1];
        }
        match self.order {
            1 => &[3, 2, 4],
            2 => &[4, 3, 2],
            _ => &[2, 3, 4],
        }
    }

    fn sep(&self) -> char {
        if !self.date {
            return ':';
        }
        match self.order {
            1 => '/',
            2 => '-',
            _ => '.',
        }
    }

    fn value(&self, f: u8) -> u32 {
        match f {
            0 => u32::from(self.t.hour),
            1 => u32::from(self.t.minute),
            2 => u32::from(self.t.day),
            3 => u32::from(self.t.month),
            5 => u32::from(self.t.second),
            _ => u32::from(self.t.year),
        }
    }

    fn change(&mut self, f: u8, dir: i32) {
        let wrap = |v: i32, lo: i32, hi: i32| (v - lo + dir).rem_euclid(hi - lo + 1) + lo;
        let timer = self.pick == Pick::Timer;
        let hours = if timer { 9 } else { 23 };
        let t = &mut self.t;
        match f {
            0 => t.hour = u8::try_from(wrap(i32::from(t.hour), 0, hours)).unwrap_or(0),
            5 => {
                t.second = u8::try_from(wrap(i32::from(t.second), 0, 59)).unwrap_or(0);
                return;
            }
            1 => t.minute = u8::try_from(wrap(i32::from(t.minute), 0, 59)).unwrap_or(0),
            2 => {
                let days = i32::from(days_in(t.month, t.year));
                t.day = u8::try_from(wrap(i32::from(t.day), 1, days)).unwrap_or(1);
            }
            3 => t.month = u8::try_from(wrap(i32::from(t.month), 1, 12)).unwrap_or(1),
            _ => t.year = u16::try_from(wrap(i32::from(t.year), 2020, 2099)).unwrap_or(2026),
        }
        t.day = t.day.clamp(1, days_in(t.month, t.year));
        if !timer {
            t.second = 0;
        }
    }

    fn field_name(f: u8) -> Str {
        match f {
            0 => Str::Hours,
            1 => Str::Minutes,
            2 => Str::Day,
            3 => Str::Month,
            5 => Str::SecondsField,
            _ => Str::Year,
        }
    }

    fn title(&self) -> Str {
        match self.pick {
            Pick::Time | Pick::Alarm(_) => Str::SetTime,
            Pick::Date => Str::SetDate,
            Pick::Timer => Str::Duration,
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for PickerScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.pick = match arg.0 {
            1 => Pick::Date,
            PICK_TIMER => Pick::Timer,
            a if (PICK_ALARM..PICK_ALARM + crate::tools::ALARMS as u32).contains(&a) => {
                Pick::Alarm(u8::try_from(a - PICK_ALARM).unwrap_or(0))
            }
            _ => Pick::Time,
        };
        self.date = self.pick == Pick::Date;
        self.t = cx.svc.now();
        match self.pick {
            Pick::Timer => {
                let s = cx.svc.tools().timer.duration() / 1000;
                self.t = DateTime {
                    hour: u8::try_from(s / 3600).unwrap_or(0).min(9),
                    minute: u8::try_from(s / 60 % 60).unwrap_or(0),
                    second: u8::try_from(s % 60).unwrap_or(0),
                    ..DateTime::default()
                };
            }
            Pick::Alarm(i) => {
                let a = crate::tools::alarm(cx.svc, usize::from(i));
                self.t = DateTime { hour: a.hour(), minute: a.min(), ..DateTime::default() };
            }
            _ => {}
        }
        self.order = u8::try_from(cx.svc.setting(id::DATE_FORMAT)).unwrap_or(0);
        self.lang = cx.svc.lang();
        self.field = 0;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) {
            return Nav::Pop;
        }
        if ev.is_tap(Key::Ok) {
            let t = self.t;
            match self.pick {
                Pick::Time | Pick::Date => {
                    cx.svc.set_now(t);
                    cx.ui.toast(self.lang.get(Str::Done));
                }
                Pick::Timer => {
                    let secs = u32::from(t.hour) * 3600 + u32::from(t.minute) * 60 + u32::from(t.second);
                    if secs > 0 {
                        cx.svc.tools_mut().timer.set(secs * 1000);
                    }
                }
                Pick::Alarm(i) => {
                    let (time, _, on) = crate::tools::alarm_keys(usize::from(i));
                    cx.svc.set_setting(time, i16::from(t.hour) * 60 + i16::from(t.minute));
                    cx.svc.set_setting(on, 1);
                }
            }
            return Nav::Pop;
        }
        let n = u8::try_from(self.fields().len()).unwrap_or(1);
        match *ev {
            Event::Key(Key::Next, KeyPhase::Down | KeyPhase::Repeat) => self.field = (self.field + 1) % n,
            Event::Key(Key::Prev, KeyPhase::Down | KeyPhase::Repeat) => self.field = (self.field + n - 1) % n,
            Event::Key(k @ (Key::Up | Key::Down), KeyPhase::Down | KeyPhase::Repeat) => {
                let f = self.fields().get(usize::from(self.field)).copied().unwrap_or(0);
                self.change(f, if k == Key::Up { 1 } else { -1 });
            }
            _ => {}
        }
        Nav::Stay
    }

    fn draw(&mut self, _cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn == Some((self.field, self.t)) {
            return;
        }
        let (w, _) = g.size();
        g.clear(Role::Bg);
        // Three fields (a date, a duration) in 16 px numerals.
        let small = self.fields().len() == 3;
        let font = if small { FontId::Num16 } else { FontId::Num28 };
        let base = if small { 52 } else { 58 };
        let dim = g.c(Role::TextDim);
        let title = self.lang.get(self.title());
        g.text_in(6, 17, w - 12, title, FontId::UiBold, dim, Align::Center);
        // Measure the whole line to centre it.
        let mut parts: [StrBuf<6>; 3] = [StrBuf::new(); 3];
        let fields = self.fields();
        let mut sep = [0u8; 4];
        let sep = self.sep().encode_utf8(&mut sep);
        let pad = 4;
        let mut total = 0;
        for (i, f) in fields.iter().enumerate() {
            parts[i].push_u32_padded(self.value(*f), if *f == 4 { 4 } else { 2 });
            total += g.measure(parts[i].as_str(), font) + 2 * pad;
            if i + 1 < fields.len() {
                total += g.measure(sep, font);
            }
        }
        let mut x = (w - total) / 2;
        let text = g.c(Role::Text);
        let h = if small { 24 } else { 38 };
        let asc = if small { 19 } else { 33 };
        for (i, f) in fields.iter().enumerate() {
            let fw = g.measure(parts[i].as_str(), font) + 2 * pad;
            let focused = usize::from(self.field) == i;
            let c = if focused {
                let acc = g.c(Role::Accent);
                g.fill_round(Rect::new(x, base - asc, fw, h), 6, acc);
                g.c(Role::OnAccent)
            } else {
                text
            };
            g.text(x + pad, base, parts[i].as_str(), font, c);
            if focused {
                let name = self.lang.get(Self::field_name(*f));
                let nw = g.measure(name, FontId::Ui);
                let cx = (x + fw / 2 - nw / 2).clamp(4, w - 4 - nw);
                g.text(cx, base - asc + h + 15, name, FontId::Ui, dim);
            }
            x += fw;
            if i + 1 < fields.len() {
                x = g.text(x, base, sep, font, text);
            }
        }
        self.drawn = Some((self.field, self.t));
    }

    fn wants(&self) -> Wants {
        Wants { strip: StripUse::Hint(self.lang.get(Str::HintChange)), ..Wants::default() }
    }

    #[cfg(feature = "screen-reader")]
    fn describe(&self, _cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        d.title(self.lang.get(self.title()));
        let fields = self.fields();
        let i = usize::from(self.field).min(fields.len().saturating_sub(1));
        let f = fields.get(i).copied().unwrap_or(0);
        let mut v = StrBuf::<8>::new();
        v.push_u32(self.value(f));
        let n = u32::try_from(fields.len()).unwrap_or(1);
        d.focus(orpheus_ui::a11y::Role::Slider, self.lang.get(Self::field_name(f)))
            .value(v.as_str())
            .at(u32::try_from(i).unwrap_or(0), n);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn date_arithmetic() {
        assert_eq!(days_in(2, 2024), 29);
        assert_eq!(days_in(2, 2100), 28);
        let mut p = PickerScreen { date: true, ..PickerScreen::default() };
        p.t = DateTime { year: 2024, month: 3, day: 31, ..DateTime::default() };
        p.change(3, -1);
        assert_eq!((p.t.month, p.t.day), (2, 29));
        p.change(2, 1);
        assert_eq!(p.t.day, 1, "wraps within the month");
        p.change(0, -1);
        assert_eq!(p.t.hour, 23);
    }

    #[test]
    fn big_value_parts() {
        let (n, u) = big_parts(-330, Unit::UtcMinutes, Lang::ENGLISH);
        assert_eq!((n.as_str(), u), ("-05:30", "UTC"));
        let (n, u) = big_parts(-3, Unit::Balance, Lang::ENGLISH);
        assert_eq!((n.as_str(), u), ("3", "L"));
    }
}
