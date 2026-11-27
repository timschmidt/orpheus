//! Settings › System › Update firmware (UPD-1, 02 §2.5): find `.ofw` files
//! on the card, show version and changes, install into the inactive slot,
//! restart.
//!
//! Views: searching → the files found (or how to put one there) → one
//! file's version and change list → "Install this version?" → checking,
//! writing, verifying with a progress bar → installed, restarting; or why
//! it was not installed. The service does the work (the firmware's card
//! task, `orpheus_update::install`); this screen only asks and shows.

use orpheus_ui::event::{Event, Instant, Key, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::misc::{draw_progress, draw_spinner};

use super::push_size;
use crate::icons;
use crate::services::{PowerRequest, Services, UpdError, UpdPhase, UpdateInfo, UpdateRequest};
use crate::strings::{Lang, Str};

/// Battery the player needs without the cable (the update writes for about
/// a minute; a cut is safe, but a flat battery mid-way wastes the run).
pub const MIN_BATTERY: u8 = 30;
/// "Update installed" shows this long before the restart.
pub const RESTART_MS: u64 = 2000;
const DLG_INSTALL: u16 = 0x5501;
/// Row pitch of the file list.
const ROW: i16 = 26;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum View {
    #[default]
    Search,
    List,
    Details,
    Progress,
    Done,
    Failed,
}

#[derive(Default)]
pub struct UpdateScreen {
    lang: Lang,
    view: View,
    focus: u8,
    chosen: u8,
    /// First wrapped line of the change list shown.
    scroll: u16,
    /// Lines the change list wrapped into at the last draw.
    lines: u16,
    version: StrBuf<16>,
    info: UpdateInfo,
    done_at: Option<Instant>,
    restart_sent: bool,
    /// The spinner moves: redraw on ticks while searching.
    spin: bool,
}

impl UpdateScreen {
    fn set_view<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, v: View) {
        self.view = v;
        cx.ui.redraw();
    }

    /// Follow the service: phase changes move the view.
    fn follow<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let i = cx.svc.update();
        let changed = i != self.info;
        self.info = i;
        let next = match (self.view, i.phase) {
            (View::Search, UpdPhase::Listed) => Some(View::List),
            (View::Search | View::Progress, UpdPhase::Failed) => Some(View::Failed),
            (View::Progress, UpdPhase::Done) => Some(View::Done),
            _ => None,
        };
        if let Some(v) = next {
            if v == View::Done {
                self.done_at = Some(cx.now);
            }
            self.set_view(cx, v);
        } else if changed {
            cx.ui.redraw();
        }
    }

    fn ask_install<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let b = cx.svc.battery();
        if b.percent < MIN_BATTERY && !b.charging() {
            cx.ui.toast(self.lang.get(Str::UpdBattery));
            return;
        }
        let l = self.lang;
        cx.ui.dialog(DialogSpec::confirm(
            DLG_INSTALL,
            l.get(Str::UpdInstallQ),
            l.get(Str::UpdInstallText),
            l.get(Str::Cancel),
            l.get(Str::UpdInstall),
        ));
    }

    fn error_text(e: UpdError) -> Str {
        match e {
            UpdError::Damaged | UpdError::None => Str::UpdDamaged,
            UpdError::WrongPlayer => Str::UpdWrongPlayer,
            UpdError::Battery => Str::UpdBattery,
            UpdError::Card => Str::UpdCard,
            UpdError::Write => Str::UpdWrite,
            UpdError::Busy => Str::UpdBusy,
        }
    }

    /// Icon, title and wrapped text, centred (empty list, done, failed).
    fn notice(g: &mut Gfx<'_>, icon: &orpheus_ui::icons::Icon2, role: Role, title: &str, text: &str) {
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let c = g.c(role);
        g.icon2((w - 24) / 2, 3, icon, c);
        let tc = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        // `y` is the baseline.
        let mut y = 40;
        let pitch = g.line_pitch(title, FontId::UiBold);
        g.wrap(title, FontId::UiBold, w - 12, |g, line| {
            if y <= h - 2 {
                g.text_in(6, y, w - 12, line, FontId::UiBold, tc, Align::Center);
            }
            y += pitch;
        });
        let pitch = g.line_pitch(text, FontId::Ui);
        g.wrap(text, FontId::Ui, w - 12, |g, line| {
            if y <= h - 2 {
                g.text_in(6, y, w - 12, line, FontId::Ui, dim, Align::Center);
            }
            y += pitch;
        });
    }

    fn draw_list<S: Services + ?Sized>(&self, cx: &Cx<'_, S>, g: &mut Gfx<'_>) {
        let l = self.lang;
        if self.info.files == 0 {
            Self::notice(g, &icons::CARD, Role::TextDim, l.get(Str::UpdNone), l.get(Str::UpdNoneAction));
            return;
        }
        let (w, _) = g.size();
        g.clear(Role::Bg);
        let dim = g.c(Role::TextDim);
        let text = g.c(Role::Text);
        let mut head = StrBuf::<48>::new();
        head.push_str(l.get(Str::UpdInstalledVersion));
        head.push_str(": ");
        head.push_str(cx.svc.version());
        g.text_in(4, 11, w - 8, head.as_str(), FontId::Ui, dim, Align::Left);
        for i in 0..self.info.files {
            let Some(f) = cx.svc.update_file(usize::from(i)) else { continue };
            let y = 16 + i16::from(i) * ROW;
            let r = Rect::new(2, y, w - 4, ROW - 2);
            let focused = i == self.focus;
            let (fg, sub) = if focused {
                let acc = g.c(Role::Accent);
                g.fill_round(r, 4, acc);
                (g.c(Role::OnAccent), g.c(Role::OnAccent))
            } else {
                (text, dim)
            };
            g.text_in(r.x + 4, y + 11, r.w - 8, f.version, FontId::UiBold, fg, Align::Left);
            let mut bottom = StrBuf::<64>::new();
            if f.installed {
                bottom.push_str(l.get(Str::UpdThisOne));
                bottom.push_str(" · ");
            }
            push_size(&mut bottom, u64::from(f.size));
            bottom.push_str(" · ");
            bottom.push_str_or_ellipsis(f.name);
            g.text_in(r.x + 4, y + 22, r.w - 8, bottom.as_str(), FontId::Ui, sub, Align::Left);
        }
    }

    fn draw_details<S: Services + ?Sized>(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>) {
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let Some(f) = cx.svc.update_file(usize::from(self.chosen)) else { return };
        g.text_in(4, 12, w - 8, f.version, FontId::UiBold, text, Align::Left);
        let mut sub = StrBuf::<48>::new();
        push_size(&mut sub, u64::from(f.size));
        sub.push_str(" · ");
        sub.push_str_or_ellipsis(f.name);
        g.text_in(4, 25, w - 8, sub.as_str(), FontId::Ui, dim, Align::Left);
        let hair = g.pal.mix(Role::Bg, Role::TextDim, 90);
        g.hline(4, 29, w - 8, hair);
        let changes = cx.svc.update_changes();
        let changes = if changes.is_empty() { self.lang.get(Str::UpdNoChanges) } else { changes };
        let mut n: u16 = 0;
        let first = self.scroll;
        let mut y = 30;
        for para in changes.split('\n') {
            let para = para.trim_end();
            if para.is_empty() {
                continue;
            }
            let pitch = g.line_pitch(para, FontId::Ui);
            g.wrap(para, FontId::Ui, w - 8, |g, line| {
                if n >= first && y + pitch < h {
                    y += pitch;
                    g.text_in(4, y, w - 8, line, FontId::Ui, text, Align::Left);
                }
                n += 1;
            });
        }
        self.lines = n;
    }

    fn draw_progress_view(&self, g: &mut Gfx<'_>) {
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let l = self.lang;
        let acc = g.c(Role::Accent);
        g.icon2((w - 24) / 2, 4, &icons::LYRE, acc);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        // Baselines.
        g.text_in(4, 42, w - 8, self.version.as_str(), FontId::UiBold, text, Align::Center);
        let what = match self.info.phase {
            UpdPhase::Writing => Str::UpdWriting,
            UpdPhase::Verifying => Str::UpdVerifying,
            _ => Str::UpdChecking,
        };
        g.text_in(4, 57, w - 8, l.get(what), FontId::Ui, dim, Align::Center);
        draw_progress(g, Rect::new(12, 64, w - 24, 6), self.info.permille, None, Role::Bg);
        let mut pct = StrBuf::<8>::new();
        pct.push_u32(u32::from(self.info.permille / 10));
        pct.push_str(" %");
        g.text_in(4, 83, w - 8, pct.as_str(), FontId::Ui, dim, Align::Center);
        if self.info.phase != UpdPhase::Checking && h > 100 {
            let warn = g.c(Role::Warning);
            g.text_in(4, h - 4, w - 8, l.get(Str::UpdKeepOn), FontId::Ui, warn, Align::Center);
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for UpdateScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        let i = cx.svc.update();
        // An update already running (opened again from Settings): show it.
        self.view = match i.phase {
            UpdPhase::Checking | UpdPhase::Writing | UpdPhase::Verifying => View::Progress,
            UpdPhase::Done => View::Done,
            _ => {
                cx.svc.update_request(UpdateRequest::Scan);
                View::Search
            }
        };
        self.info = cx.svc.update();
        self.done_at = (self.view == View::Done).then_some(cx.now);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let l = self.lang;
        match *ev {
            Event::Tick(now) => {
                self.follow(cx);
                if self.view == View::Done
                    && !self.restart_sent
                    && self.done_at.is_some_and(|t| now.since(t) >= RESTART_MS)
                {
                    self.restart_sent = true;
                    cx.svc.power(PowerRequest::Restart);
                }
                if self.view == View::Search || self.info.phase == UpdPhase::Checking {
                    self.spin = true;
                }
                return Nav::Stay;
            }
            Event::Service(_) | Event::Data(..) => {
                self.follow(cx);
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Dialog { id: DLG_INSTALL, button: 1 }) => {
                cx.svc.update_request(UpdateRequest::Install(self.chosen));
                self.info = cx.svc.update();
                self.set_view(cx, View::Progress);
                return Nav::Stay;
            }
            _ => {}
        }
        match self.view {
            View::Search => {
                if ev.is_tap(Key::Back) {
                    return Nav::Pop;
                }
            }
            View::List => {
                let n = self.info.files;
                if n == 0 && (ev.is_tap(Key::Ok) || ev.is_tap(Key::Back)) {
                    return Nav::Pop;
                }
                if ev.is_tap(Key::Back) {
                    return Nav::Pop;
                }
                if ev.is_step(Key::Down) && self.focus + 1 < n {
                    self.focus += 1;
                    cx.ui.redraw();
                } else if ev.is_step(Key::Up) && self.focus > 0 {
                    self.focus -= 1;
                    cx.ui.redraw();
                } else if ev.is_tap(Key::Ok) && n > 0 {
                    self.chosen = self.focus;
                    self.scroll = 0;
                    self.version = StrBuf::new();
                    if let Some(f) = cx.svc.update_file(usize::from(self.chosen)) {
                        self.version.push_str(f.version);
                    }
                    cx.svc.update_request(UpdateRequest::Details(self.chosen));
                    self.set_view(cx, View::Details);
                }
            }
            View::Details => {
                if ev.is_tap(Key::Back) {
                    self.set_view(cx, View::List);
                } else if ev.is_step(Key::Down) && self.scroll + 1 < self.lines {
                    self.scroll += 1;
                    cx.ui.redraw();
                } else if ev.is_step(Key::Up) && self.scroll > 0 {
                    self.scroll -= 1;
                    cx.ui.redraw();
                } else if ev.is_tap(Key::Ok) {
                    self.ask_install(cx);
                }
            }
            View::Progress => {
                if ev.is_tap(Key::Back) {
                    if self.info.phase == UpdPhase::Checking {
                        cx.svc.update_request(UpdateRequest::Cancel);
                        return Nav::Pop;
                    }
                    cx.ui.toast(l.get(Str::UpdKeepOn));
                }
            }
            View::Done => {}
            View::Failed => {
                if ev.is_tap(Key::Back) || ev.is_tap(Key::Ok) {
                    return Nav::Pop;
                }
            }
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let l = self.lang;
        match self.view {
            View::Search => {
                if dirty == Dirty::All || !self.spin {
                    let (w, _) = g.size();
                    g.clear(Role::Bg);
                    let dim = g.c(Role::TextDim);
                    g.text_in(4, 40, w - 8, l.get(Str::UpdSearching), FontId::Ui, dim, Align::Center);
                }
                let (w, _) = g.size();
                draw_spinner(g, (w - 14) / 2, 52, cx.now, Role::Bg);
                self.spin = false;
            }
            View::List => self.draw_list(cx, g),
            View::Details => self.draw_details(cx, g),
            View::Progress => self.draw_progress_view(g),
            View::Done => {
                Self::notice(g, &icons::LYRE, Role::Good, l.get(Str::UpdDone), l.get(Str::UpdRestarting));
            }
            View::Failed => {
                let mut t = StrBuf::<128>::new();
                let why = l.get(Self::error_text(self.info.error));
                t.push_str(why);
                t.push_str(if why.ends_with('.') { " " } else { ". " });
                t.push_str(l.get(Str::UpdUnchanged));
                Self::notice(g, &icons::WARNING, Role::Warning, l.get(Str::UpdFailed), t.as_str());
            }
        }
    }

    fn wants(&self) -> Wants {
        let l = self.lang;
        let strip = match self.view {
            View::Details => StripUse::Hint(l.get(Str::HintUpdInstall)),
            View::Failed | View::Done | View::Progress => StripUse::Content,
            View::List if self.info.files == 0 => StripUse::Content,
            _ => StripUse::Auto,
        };
        Wants { ticks: true, strip, title: Some(l.get(Str::UpdTitle)), ..Wants::default() }
    }

    /// What the view shows: searching, the files (the focused one with
    /// its version and size), a file's changes, the progress, the result.
    #[cfg(feature = "screen-reader")]
    fn describe(&self, cx: &Cx<'_, S>, d: &mut orpheus_ui::a11y::Desc) {
        let l = self.lang;
        d.title(l.get(Str::UpdTitle));
        match self.view {
            View::Search => {
                d.say(l.get(Str::UpdSearching));
            }
            View::List if self.info.files == 0 => {
                d.say(l.get(Str::UpdNone));
                d.say(l.get(Str::UpdNoneAction));
            }
            View::List => {
                let mut head = StrBuf::<48>::new();
                head.push_str(l.get(Str::UpdInstalledVersion));
                head.push_str(": ");
                head.push_str(cx.svc.version());
                d.say(head.as_str());
                if let Some(f) = cx.svc.update_file(usize::from(self.focus)) {
                    let mut v = StrBuf::<48>::new();
                    if f.installed {
                        v.push_str(l.get(Str::UpdThisOne));
                        v.push_str(" · ");
                    }
                    push_size(&mut v, u64::from(f.size));
                    d.focus(orpheus_ui::a11y::Role::Item, f.version)
                        .value(v.as_str())
                        .at(u32::from(self.focus), u32::from(self.info.files));
                }
            }
            View::Details => {
                if let Some(f) = cx.svc.update_file(usize::from(self.chosen)) {
                    d.say(f.version);
                }
                let changes = cx.svc.update_changes();
                d.say(if changes.is_empty() { l.get(Str::UpdNoChanges) } else { changes });
            }
            View::Progress => {
                d.say(self.version.as_str());
                d.say(l.get(match self.info.phase {
                    UpdPhase::Writing => Str::UpdWriting,
                    UpdPhase::Verifying => Str::UpdVerifying,
                    _ => Str::UpdChecking,
                }));
                d.say_u32("percent", u32::from(self.info.permille / 10));
            }
            View::Done => {
                d.say(l.get(Str::UpdDone));
                d.say(l.get(Str::UpdRestarting));
            }
            View::Failed => {
                d.say(l.get(Str::UpdFailed));
                d.say(l.get(Self::error_text(self.info.error)));
                d.say(l.get(Str::UpdUnchanged));
            }
        }
    }
}
