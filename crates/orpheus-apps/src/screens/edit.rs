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
}

/// Time (arg 0) or date (arg 1) picker: Prev/Next choose the field,
/// Up/Down change it, OK sets the clock.
#[derive(Default)]
pub struct PickerScreen {
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
    /// Field kinds in display order: 0 hour, 1 minute, 2 day, 3 month, 4 year.
    fn fields(&self) -> &'static [u8] {
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
            _ => u32::from(self.t.year),
        }
    }

    fn change(&mut self, f: u8, dir: i32) {
        let wrap = |v: i32, lo: i32, hi: i32| (v - lo + dir).rem_euclid(hi - lo + 1) + lo;
        let t = &mut self.t;
        match f {
            0 => t.hour = u8::try_from(wrap(i32::from(t.hour), 0, 23)).unwrap_or(0),
            1 => t.minute = u8::try_from(wrap(i32::from(t.minute), 0, 59)).unwrap_or(0),
            2 => {
                let days = i32::from(days_in(t.month, t.year));
                t.day = u8::try_from(wrap(i32::from(t.day), 1, days)).unwrap_or(1);
            }
            3 => t.month = u8::try_from(wrap(i32::from(t.month), 1, 12)).unwrap_or(1),
            _ => t.year = u16::try_from(wrap(i32::from(t.year), 2020, 2099)).unwrap_or(2026),
        }
        t.day = t.day.clamp(1, days_in(t.month, t.year));
        t.second = 0;
    }

    fn field_name(f: u8) -> Str {
        match f {
            0 => Str::Hours,
            1 => Str::Minutes,
            2 => Str::Day,
            3 => Str::Month,
            _ => Str::Year,
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for PickerScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.date = arg.0 == 1;
        self.t = cx.svc.now();
        self.order = u8::try_from(cx.svc.setting(id::DATE_FORMAT)).unwrap_or(0);
        self.lang = cx.svc.lang();
        self.field = 0;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) {
            return Nav::Pop;
        }
        if ev.is_tap(Key::Ok) {
            cx.svc.set_now(self.t);
            cx.ui.toast(self.lang.get(Str::Done));
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
        let font = if self.date { FontId::Num16 } else { FontId::Num28 };
        let base = if self.date { 52 } else { 58 };
        let dim = g.c(Role::TextDim);
        let title = self.lang.get(if self.date { Str::SetDate } else { Str::SetTime });
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
        let h = if self.date { 24 } else { 38 };
        let asc = if self.date { 19 } else { 33 };
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
