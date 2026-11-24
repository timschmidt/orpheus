//! System screens (05 §6): boot logo, charging, lock screen, notices and
//! errors, goodbye.

use orpheus_ui::event::{Event, Instant, Key};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx, SCREEN_W};
use orpheus_ui::icons as ui_icons;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;

use super::HOME;
use crate::App;
use crate::icons;
use crate::services::{BatteryInfo, PowerRequest, Services};
use crate::settings::id;
use crate::strings::{Lang, Str};
use orpheus_power::charger::ChargeState;

/// Boot logo time after the display is ready (05 §6.1).
pub const BOOT_MS: u64 = 300;
/// Lock screen time after a locked key (05 §6.3).
pub const LOCK_MS: u64 = 2000;
/// "Goodbye" time before power-off (05 §6.1).
pub const GOODBYE_MS: u64 = 300;

fn draw_logo(g: &mut Gfx<'_>, y: i16) {
    let acc = g.c(Role::Accent);
    let mark = Rect::new((SCREEN_W - 40) / 2, y, 40, 40);
    g.fill_round(mark, 11, acc);
    let on = g.c(Role::OnAccent);
    g.icon2(mark.x + 8, mark.y + 8, &icons::LYRE, on);
    let text = g.c(Role::Text);
    g.text_in(0, y + 60, SCREEN_W, "Orpheus", FontId::UiBold, text, Align::Center);
}

/// Logo, then Home after [`BOOT_MS`].
#[derive(Default)]
pub struct BootScreen {
    since: Instant,
    drawn: bool,
}

impl<S: Services + ?Sized> Screen<S> for BootScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.since = cx.now;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match ev {
            Event::Tick(now) if now.since(self.since) >= BOOT_MS => Nav::Replace(HOME, Arg(0)),
            _ => {
                let _ = cx;
                Nav::Stay
            }
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn {
            return;
        }
        g.clear(Role::Bg);
        draw_logo(g, 34);
        let dim = g.c(Role::TextDim);
        g.text_in(0, 112, SCREEN_W, cx.svc.version(), FontId::Ui, dim, Align::Center);
        self.drawn = true;
    }

    fn wants(&self) -> Wants {
        Wants { ticks: true, full_screen: true, ..Wants::default() }
    }
}

/// A big battery outline with `pct` fill (charging screen, lock screen).
fn draw_battery(g: &mut Gfx<'_>, r: Rect, pct: u8, charging: bool) {
    let outline = g.c(Role::Text);
    g.stroke_round(r, 4, outline);
    g.stroke_round(r.inset(1, 1), 3, outline);
    let nub = Rect::new(r.right(), r.y + r.h / 2 - r.h / 6, 3, r.h / 3);
    g.fill_round(nub, 1, outline);
    let inner = r.inset(4, 4);
    let fw = i16::try_from(i32::from(inner.w) * i32::from(pct.min(100)) / 100).unwrap_or(0);
    let c = if charging {
        g.c(Role::Good)
    } else if pct < 10 {
        g.c(Role::Warning)
    } else {
        g.c(Role::Text)
    };
    if fw > 0 {
        g.fill_round(Rect::new(inner.x, inner.y, fw.max(3), inner.h), 2, c);
    }
}

/// Plugged in while off (05 §6.1): battery, percent, state, "USB
/// connected"; OK or Power starts the player.
#[derive(Default)]
pub struct ChargingScreen {
    lang: Lang,
    drawn: Option<BatteryInfo>,
}

impl<S: Services + ?Sized> Screen<S> for ChargingScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Ok) {
            cx.svc.power(PowerRequest::StartFromCharging);
            return Nav::Replace(HOME, Arg(0));
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let b = cx.svc.battery();
        if dirty == Dirty::Changed && self.drawn == Some(b) {
            return;
        }
        let l = self.lang;
        g.clear(Role::Bg);
        let charging = b.charging();
        draw_battery(g, Rect::new(34, 16, 56, 30), b.percent, charging || b.charge == ChargeState::Full);
        if charging {
            let on = g.c(Role::Bg);
            g.icon(59, 27, &ui_icons::BOLT, on);
        }
        let mut pct = StrBuf::<8>::new();
        pct.push_u32(u32::from(b.percent));
        pct.push_str(" %");
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        g.text_in(0, 82, SCREEN_W, pct.as_str(), FontId::Num28, text, Align::Center);
        let state = match b.charge {
            ChargeState::Charging | ChargeState::Trickle | ChargeState::Detecting => Str::Charging,
            ChargeState::Full => Str::Charged,
            ChargeState::NoBattery => Str::NoBattery,
            ChargeState::Fault => Str::ChargerFault,
            ChargeState::NotPresent => Str::NotCharging,
        };
        g.text_in(4, 101, SCREEN_W - 8, l.get(state), FontId::UiBold, text, Align::Center);
        g.text_in(4, 119, SCREEN_W - 8, l.get(Str::UsbConnected), FontId::Ui, dim, Align::Center);
        self.drawn = Some(b);
    }

    fn wants(&self) -> Wants {
        Wants { full_screen: true, ..Wants::default() }
    }
}

/// Shown for 2 s when a locked key is pressed (05 §6.3).
#[derive(Default)]
pub struct LockScreen {
    since: Instant,
    lang: Lang,
    drawn_minute: Option<u8>,
}

/// Append the date in the configured format.
pub fn push_date(b: &mut StrBuf<24>, t: &crate::services::DateTime, format: i16) {
    let (d, m, y) = (u32::from(t.day), u32::from(t.month), u32::from(t.year));
    match format {
        1 => {
            b.push_u32_padded(m, 2);
            b.push('/');
            b.push_u32_padded(d, 2);
            b.push('/');
            b.push_u32(y);
        }
        2 => {
            b.push_u32(y);
            b.push('-');
            b.push_u32_padded(m, 2);
            b.push('-');
            b.push_u32_padded(d, 2);
        }
        _ => {
            b.push_u32_padded(d, 2);
            b.push('.');
            b.push_u32_padded(m, 2);
            b.push('.');
            b.push_u32(y);
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for LockScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.since = cx.now;
        self.lang = cx.svc.lang();
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match ev {
            Event::Tick(now) if now.since(self.since) >= LOCK_MS => {
                cx.svc.power(PowerRequest::LockNoticeDone);
                Nav::Pop
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let t = cx.svc.now();
        if dirty == Dirty::Changed && self.drawn_minute == Some(t.minute) {
            return;
        }
        g.clear(Role::Bg);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        // Top: lock and battery.
        g.icon(6, 5, &ui_icons::LOCK, dim);
        let b = cx.svc.battery();
        let mut pct = StrBuf::<8>::new();
        pct.push_u32(u32::from(b.percent));
        pct.push_str(" %");
        g.text_in(SCREEN_W - 60, 14, 54, pct.as_str(), FontId::Ui, dim, Align::Right);
        // Clock and date.
        let mut clock = StrBuf::<8>::new();
        let h24 = cx.svc.setting(id::CLOCK_24H) != 0;
        let hour = if h24 { t.hour } else { (t.hour + 11) % 12 + 1 };
        clock.push_u32_padded(u32::from(hour), if h24 { 2 } else { 1 });
        clock.push(':');
        clock.push_u32_padded(u32::from(t.minute), 2);
        g.text_in(0, 60, SCREEN_W, clock.as_str(), FontId::Num28, text, Align::Center);
        let mut date = StrBuf::<24>::new();
        push_date(&mut date, &t, cx.svc.setting(id::DATE_FORMAT));
        g.text_in(0, 78, SCREEN_W, date.as_str(), FontId::Ui, dim, Align::Center);
        if let Some(p) = cx.svc.playing() {
            g.text_in(8, 98, SCREEN_W - 16, p.title, FontId::Ui, text, Align::Center);
        }
        let hint = self.lang.get(Str::PressPowerToUnlock);
        g.text_in(4, 121, SCREEN_W - 8, hint, FontId::Ui, dim, Align::Center);
        self.drawn_minute = Some(t.minute);
    }

    fn wants(&self) -> Wants {
        Wants { ticks: true, full_screen: true, ..Wants::default() }
    }
}

/// What a notice says (05 §6.6: icon, cause, action).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Notice {
    NoCard,
    CardUnreadable,
    /// The app is not built yet.
    NotYet(App),
    /// A Settings entry not built yet.
    NotYetSettings,
    BatteryLow,
    /// No app opens this file type.
    Unsupported,
}

impl Notice {
    #[must_use]
    pub fn arg(self) -> u32 {
        match self {
            Notice::NoCard => 0x100,
            Notice::CardUnreadable => 0x200,
            Notice::NotYet(a) => 0x300 | a as u32,
            Notice::NotYetSettings => 0x400,
            Notice::BatteryLow => 0x500,
            Notice::Unsupported => 0x600,
        }
    }

    #[must_use]
    pub fn from_arg(a: u32) -> Notice {
        match a >> 8 {
            1 => Notice::NoCard,
            2 => Notice::CardUnreadable,
            3 => {
                Notice::NotYet(App::from_index(u8::try_from(a & 0xFF).unwrap_or(0)).unwrap_or(App::Settings))
            }
            5 => Notice::BatteryLow,
            6 => Notice::Unsupported,
            _ => Notice::NotYetSettings,
        }
    }
}

#[derive(Default)]
pub struct NoticeScreen {
    arg: u32,
    lang: Lang,
    drawn: bool,
}

impl<S: Services + ?Sized> Screen<S> for NoticeScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.arg = arg.0;
        self.lang = cx.svc.lang();
    }

    fn event(&mut self, _cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) || ev.is_tap(Key::Ok) { Nav::Pop } else { Nav::Stay }
    }

    fn draw(&mut self, _cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn {
            return;
        }
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let n = Notice::from_arg(self.arg);
        let l = self.lang;
        let (icon, role, cause, action) = match n {
            Notice::NoCard => (&icons::CARD, Role::TextDim, Str::NoCard, Str::NoCardAction),
            Notice::CardUnreadable => {
                (&icons::CARD, Role::Warning, Str::CardUnreadable, Str::CardUnreadableAction)
            }
            Notice::NotYet(app) => (app.icon(), Role::TextDim, app.label(), Str::NotYetAction),
            Notice::NotYetSettings => (&icons::SETTINGS, Role::TextDim, Str::NotYet, Str::NotYetAction),
            Notice::BatteryLow => (&icons::WARNING, Role::Warning, Str::BatteryLow, Str::BatteryLowAction),
            Notice::Unsupported => (&icons::CARD, Role::TextDim, Str::CannotOpen, Str::CannotOpenAction),
        };
        let c = g.c(role);
        g.icon2((w - 24) / 2, 10, icon, c);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let cause = l.get(cause);
        let mut y = 52;
        let pitch = g.line_pitch(cause, FontId::UiBold);
        g.wrap(cause, FontId::UiBold, w - 12, |g, line| {
            if y <= h - 2 {
                g.text_in(6, y, w - 12, line, FontId::UiBold, text, Align::Center);
            }
            y += pitch;
        });
        if let Notice::NotYet(_) = n {
            // Second line names the situation, third the action.
            let s = l.get(Str::NotYet);
            g.text_in(6, y, w - 12, s, FontId::Ui, dim, Align::Center);
            y += g.line_pitch(s, FontId::Ui);
        }
        y += 2;
        let action = l.get(action);
        let pitch = g.line_pitch(action, FontId::Ui);
        g.wrap(action, FontId::Ui, w - 12, |g, line| {
            if y <= h - 2 {
                g.text_in(6, y, w - 12, line, FontId::Ui, dim, Align::Center);
            }
            y += pitch;
        });
        self.drawn = true;
    }

    fn wants(&self) -> Wants {
        // The strip's 18 px are needed for three wrapped CJK lines.
        Wants { strip: StripUse::Content, ..Wants::default() }
    }
}

/// "Goodbye" for [`GOODBYE_MS`], then power off (05 §6.1).
#[derive(Default)]
pub struct GoodbyeScreen {
    since: Instant,
    lang: Lang,
    sent: bool,
    drawn: bool,
}

impl<S: Services + ?Sized> Screen<S> for GoodbyeScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.since = cx.now;
        self.lang = cx.svc.lang();
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        if let Event::Tick(now) = ev
            && now.since(self.since) >= GOODBYE_MS
            && !self.sent
        {
            self.sent = true;
            cx.svc.power(PowerRequest::PowerOff);
        }
        Nav::Stay
    }

    fn draw(&mut self, _cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn {
            return;
        }
        g.clear(Role::Bg);
        let dim = g.c(Role::TextDim);
        g.icon2((SCREEN_W - 24) / 2, 40, &icons::POWER, dim);
        let text = g.c(Role::Text);
        g.text_in(0, 86, SCREEN_W, self.lang.get(Str::Goodbye), FontId::UiBold, text, Align::Center);
        self.drawn = true;
    }

    fn wants(&self) -> Wants {
        Wants { ticks: !self.sent, full_screen: true, ..Wants::default() }
    }
}
