//! Authenticator (05 §7.9, WP OTP-2): PIN pad (set-up, unlock with the
//! back-off, change), the account list with each code and a shrinking bar
//! for its seconds, the big-code view that types the code on the USB
//! keyboard, imports, provisioning and the vault's housekeeping.
//!
//! One screen with modes, so the vault's state is never split across
//! screens; leaving it with Back locks the vault (the service also locks
//! it after 60 s without a key and when the screen goes off).
//!
//! The screen keeps a small cache of the rows it shows (names and codes,
//! never secrets): the service decrypts them when the list moves, when a
//! code's period rolls over and when the vault changes.
//!
//! PIN pad keys: Up/Down/Prev/Next move over the 3 × 4 keys, OK presses
//! one, Back deletes a digit (with none, leaves), hold OK finishes.

use orpheus_ui::event::{Event, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::dialog::DialogSpec;
use orpheus_ui::widgets::misc::draw_progress;

use crate::services::{
    AuthOutcome, AuthRequest, Code, Pin, Services, VaultState, VaultWork, auth_event, other,
};
use crate::strings::{Lang, Str};

/// Rows on screen (content 114 px with the strip: 5 × 19 + a 19-px line).
const ROWS: usize = 5;
const ROW_H: i16 = 19;
/// The list's own strip line (focused account, clock warning).
const STRIP_Y: i16 = 96;

const M_IMPORT: u16 = 1;
const M_NEXT: u16 = 2;
const M_UP: u16 = 3;
const M_DOWN: u16 = 4;
const M_DELETE: u16 = 5;
const M_TIME: u16 = 6;
const M_CHANGE: u16 = 7;
const M_LOCK: u16 = 8;
const M_WIPE: u16 = 9;
const M_TYPE: u16 = 10;
const DLG_DELETE: u16 = 0x400;
const DLG_WIPE: u16 = 0x401;
const DLG_PROVISION: u16 = 0x402;
const DLG_INFO: u16 = 0x403;

/// Pad keys in grid order: digits, then delete, 0, done.
const PAD_DEL: u8 = 9;
const PAD_ZERO: u8 = 10;
const PAD_DONE: u8 = 11;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum Mode {
    #[default]
    List,
    /// The big code of the focused entry.
    Code,
    /// Set-up: first entry, then the confirmation.
    NewPin,
    ConfirmPin,
    /// Change PIN (unlocked).
    ChangePin,
    ChangeConfirm,
}

/// What the content area shows (from the vault's state and the mode).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum View {
    Message(Str, Str),
    Pad(Mode),
    Working(VaultWork, u8),
    List,
    Code,
}

/// A row as drawn: the name, the code, never the secret (and only the
/// focused row's account, in `AuthScreen::acct`, to keep the screen small).
#[derive(Clone, Copy, Debug, Default)]
struct RowCache {
    /// The issuer, or the account without one.
    name: StrBuf<32>,
    has_issuer: bool,
    /// TOTP period; 0 for HOTP.
    period: u32,
    digits: u8,
    code: Option<Code>,
    /// The TOTP step the code belongs to.
    step: u64,
}

impl RowCache {
    const fn is_hotp(&self) -> bool {
        self.period == 0
    }
}

/// Everything a frame depends on (a change redraws).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Shown {
    view: View,
    second: Option<u64>,
    focus: u8,
    top: u8,
    count: u8,
    pin: u8,
    cursor: u8,
    error: Option<Str>,
    wait_s: u32,
    serial: u32,
}

#[derive(Default)]
pub struct AuthScreen {
    lang: Lang,
    mode: Mode,
    focus: u8,
    top: u8,
    count: u8,
    rows: [RowCache; ROWS],
    /// The focused entry's account.
    acct: StrBuf<64>,
    /// Bumped whenever `rows` is reloaded (redraw).
    serial: u32,
    pin: Pin,
    first: Pin,
    cursor: u8,
    error: Option<Str>,
    drawn: Option<Shown>,
}

/// `code` grouped for reading: 3 + 3, 4 + 4, 3 + 4.
fn push_grouped_code<const N: usize>(b: &mut StrBuf<N>, code: &str) {
    let split = code.len() / 2;
    b.push_str(code.get(..split).unwrap_or(""));
    b.push(' ');
    b.push_str(code.get(split..).unwrap_or(""));
}

/// A placeholder of `digits` dashes in the numeral font.
fn push_dashes<const N: usize>(b: &mut StrBuf<N>, digits: u8) {
    for i in 0..digits.clamp(6, 8) {
        if i == digits.clamp(6, 8) / 2 {
            b.push(' ');
        }
        b.push('-');
    }
}

fn seconds_left(t: u64, period: u32) -> u32 {
    let p = u64::from(period.max(1));
    u32::try_from(p - t % p).unwrap_or(0)
}

impl AuthScreen {
    fn view<S: Services + ?Sized>(&self, svc: &S) -> View {
        match svc.vault() {
            VaultState::Unknown => View::Message(Str::AppAuthenticator, Str::AuthFailed),
            VaultState::Absent => match self.mode {
                Mode::ConfirmPin => View::Pad(Mode::ConfirmPin),
                _ => View::Pad(Mode::NewPin),
            },
            VaultState::Damaged => View::Message(Str::AuthDamaged, Str::AuthDamagedAction),
            VaultState::Unsupported => View::Message(Str::AuthUnsupported, Str::AuthDamagedAction),
            VaultState::Locked { .. } => View::Pad(Mode::List),
            VaultState::Working { work, percent } => View::Working(work, percent),
            VaultState::Unlocked { .. } => match self.mode {
                Mode::Code if self.count > 0 => View::Code,
                Mode::ChangePin | Mode::ChangeConfirm => View::Pad(self.mode),
                _ => View::List,
            },
        }
    }

    fn wait_s<S: Services + ?Sized>(svc: &S) -> u32 {
        match svc.vault() {
            VaultState::Locked { wait_ms, .. } => u32::try_from(wait_ms.div_ceil(1000)).unwrap_or(u32::MAX),
            _ => 0,
        }
    }

    fn shown<S: Services + ?Sized>(&self, svc: &S) -> Shown {
        Shown {
            view: self.view(svc),
            second: svc.unix_time(),
            focus: self.focus,
            top: self.top,
            count: self.count,
            pin: u8::try_from(self.pin.len()).unwrap_or(0),
            cursor: self.cursor,
            error: self.error,
            wait_s: Self::wait_s(svc),
            serial: self.serial,
        }
    }

    /// Reload the rows on screen from the service.
    fn reload<S: Services + ?Sized>(&mut self, svc: &mut S) {
        self.count = match svc.vault() {
            VaultState::Unlocked { count } => count,
            _ => 0,
        };
        if self.count == 0 {
            self.focus = 0;
            self.top = 0;
        } else {
            self.focus = self.focus.min(self.count - 1);
            if self.focus < self.top {
                self.top = self.focus;
            }
            if usize::from(self.focus) >= usize::from(self.top) + ROWS {
                self.top = self.focus + 1 - ROWS as u8;
            }
        }
        let t = svc.unix_time();
        for k in 0..ROWS {
            let i = usize::from(self.top) + k;
            let mut r = RowCache::default();
            if i < usize::from(self.count)
                && let Some(e) = svc.auth_entry(i)
            {
                r.code = svc.auth_code(i);
                r.step = match t {
                    Some(t) if !e.is_hotp() => t / u64::from(e.period.max(1)),
                    _ => 0,
                };
                r.name = StrBuf::from(e.name());
                r.has_issuer = !e.issuer.is_empty();
                r.period = e.period;
                r.digits = e.digits;
                if i == usize::from(self.focus) {
                    self.acct = e.account;
                }
            }
            self.rows[k] = r;
        }
        self.serial = self.serial.wrapping_add(1);
    }

    /// A TOTP code on screen rolled over (or the clock came or went).
    fn stale<S: Services + ?Sized>(&self, svc: &S) -> bool {
        let t = svc.unix_time();
        self.rows.iter().take(usize::from(self.count)).any(|r| {
            if r.is_hotp() || r.digits == 0 {
                return false;
            }
            match (t, r.code) {
                (Some(t), Some(_)) => t / u64::from(r.period.max(1)) != r.step,
                (None, None) => false,
                _ => true,
            }
        })
    }

    fn focused(&self) -> Option<&RowCache> {
        self.rows.get(usize::from(self.focus.wrapping_sub(self.top))).filter(|_| self.count > 0)
    }

    fn reset_pad(&mut self) {
        self.pin.clear();
        self.first.clear();
        self.cursor = 0;
    }

    // ------------------------------------------------------------ keys

    fn pad_key<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, k: Key, phase: KeyPhase) -> Nav {
        let View::Pad(pad) = self.view(cx.svc) else { return Nav::Stay };
        match (k, phase) {
            (Key::Up, KeyPhase::Down | KeyPhase::Repeat) => self.cursor = (self.cursor + 9) % 12,
            (Key::Down, KeyPhase::Down | KeyPhase::Repeat) => self.cursor = (self.cursor + 3) % 12,
            (Key::Prev, KeyPhase::Down | KeyPhase::Repeat) => {
                self.cursor = self.cursor / 3 * 3 + (self.cursor % 3 + 2) % 3;
            }
            (Key::Next, KeyPhase::Down | KeyPhase::Repeat) => {
                self.cursor = self.cursor / 3 * 3 + (self.cursor % 3 + 1) % 3;
            }
            (Key::Ok, KeyPhase::Up { held: false }) => match self.cursor {
                PAD_DEL => self.pin.pop(),
                PAD_DONE => return self.submit(cx, pad),
                c => {
                    let d = if c == PAD_ZERO { 0 } else { c + 1 };
                    if self.pin.push(d) {
                        self.error = None;
                    }
                }
            },
            (Key::Ok, KeyPhase::Hold) => return self.submit(cx, pad),
            (Key::Back, KeyPhase::Up { held: false }) => {
                if !self.pin.is_empty() {
                    self.pin.pop();
                } else if matches!(pad, Mode::ChangePin | Mode::ChangeConfirm) {
                    self.reset_pad();
                    self.mode = Mode::List;
                } else if pad == Mode::ConfirmPin {
                    self.reset_pad();
                    self.mode = Mode::NewPin;
                } else {
                    return Nav::Pop;
                }
            }
            _ => {}
        }
        Nav::Stay
    }

    fn submit<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, pad: Mode) -> Nav {
        if !self.pin.complete() {
            return Nav::Stay;
        }
        match pad {
            Mode::List => {
                if Self::wait_s(cx.svc) > 0 {
                    return Nav::Stay;
                }
                let pin = core::mem::take(&mut self.pin);
                cx.svc.auth_request(AuthRequest::Unlock(pin));
            }
            Mode::NewPin | Mode::ChangePin => {
                self.first = core::mem::take(&mut self.pin);
                self.mode = if pad == Mode::NewPin { Mode::ConfirmPin } else { Mode::ChangeConfirm };
            }
            Mode::ConfirmPin | Mode::ChangeConfirm => {
                if self.pin != self.first {
                    self.error = Some(Str::AuthPinsDiffer);
                    self.reset_pad();
                    self.mode = if pad == Mode::ConfirmPin { Mode::NewPin } else { Mode::ChangePin };
                    return Nav::Stay;
                }
                let pin = core::mem::take(&mut self.pin);
                self.first.clear();
                cx.svc.auth_request(if pad == Mode::ConfirmPin {
                    AuthRequest::Create(pin)
                } else {
                    AuthRequest::ChangePin(pin)
                });
                self.mode = Mode::List;
            }
            Mode::Code => {}
        }
        self.cursor = 0;
        Nav::Stay
    }

    fn list_key<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, k: Key, phase: KeyPhase) -> Nav {
        let n = self.count;
        let before = (self.focus, self.top);
        match (k, phase) {
            (Key::Up, KeyPhase::Down | KeyPhase::Repeat) => self.focus = self.focus.saturating_sub(1),
            (Key::Down, KeyPhase::Down | KeyPhase::Repeat) => {
                self.focus = (self.focus + 1).min(n.saturating_sub(1));
            }
            (Key::Prev, KeyPhase::Up { held: false }) => self.focus = self.focus.saturating_sub(ROWS as u8),
            (Key::Next, KeyPhase::Up { held: false }) => {
                self.focus = (self.focus + ROWS as u8).min(n.saturating_sub(1));
            }
            (Key::Ok, KeyPhase::Up { held: false }) if n > 0 => self.mode = Mode::Code,
            (Key::Back, KeyPhase::Up { held: false }) => {
                cx.svc.auth_request(AuthRequest::Lock);
                return Nav::Pop;
            }
            _ => {}
        }
        if (self.focus, self.top) != before {
            self.reload(cx.svc);
        }
        Nav::Stay
    }

    fn code_key<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, k: Key, phase: KeyPhase) -> Nav {
        match (k, phase) {
            (Key::Ok, KeyPhase::Up { held: false }) => self.type_code(cx),
            (Key::Back, KeyPhase::Up { held: false }) => self.mode = Mode::List,
            (Key::Up | Key::Down, KeyPhase::Down | KeyPhase::Repeat) => return self.list_key(cx, k, phase),
            _ => {}
        }
        Nav::Stay
    }

    fn type_code<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        cx.svc.auth_request(AuthRequest::Type(self.focus));
    }

    // ------------------------------------------------------------ events

    fn outcome<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let l = self.lang;
        let toast = match cx.svc.auth_outcome() {
            AuthOutcome::Imported { report, deleted } => {
                let mut t = StrBuf::<96>::new();
                let part = |n: u8, s: Str, t: &mut StrBuf<96>| {
                    if n > 0 {
                        if !t.is_empty() {
                            t.push_str(" · ");
                        }
                        t.push_u32(u32::from(n));
                        t.push(' ');
                        t.push_str(l.get(s));
                    }
                };
                part(report.added, Str::AuthAdded, &mut t);
                part(report.duplicates, Str::AuthSkipped, &mut t);
                part(report.bad, Str::AuthBadLines, &mut t);
                if report.full {
                    if !t.is_empty() {
                        t.push_str(" · ");
                    }
                    t.push_str(l.get(Str::AuthVaultFull));
                }
                if deleted {
                    t.push_str(". ");
                    t.push_str(l.get(Str::AuthOldCopies));
                }
                let title = l.get(if deleted { Str::AuthFileDeleted } else { Str::AuthFileKept });
                cx.ui.dialog(DialogSpec::info(DLG_INFO, title, t.as_str(), l.get(Str::Ok)));
                None
            }
            AuthOutcome::NoImportFile => Some(Str::AuthNoImportFile),
            AuthOutcome::ImportTooLarge => Some(Str::AuthImportTooLarge),
            AuthOutcome::Typing => Some(Str::AuthTyping),
            AuthOutcome::Typed => Some(Str::AuthTyped),
            AuthOutcome::TypeFailed => Some(Str::AuthTypeFailed),
            AuthOutcome::NoComputer => Some(Str::AuthNoComputer),
            AuthOutcome::NoCode => Some(Str::AuthNoCode),
            AuthOutcome::Added => Some(Str::Done),
            AuthOutcome::AlreadyThere => Some(Str::AuthAlreadyThere),
            AuthOutcome::PinChanged => Some(Str::AuthPinChanged),
            AuthOutcome::Failed => Some(Str::AuthFailed),
            AuthOutcome::None => None,
        };
        if let Some(s) = toast {
            cx.ui.toast(l.get(s));
        }
    }

    fn ask_provision<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let Some(e) = cx.svc.provision_pending() else { return };
        let l = self.lang;
        let mut t = StrBuf::<96>::new();
        t.push_str_or_ellipsis(e.issuer.as_str());
        if !e.account.is_empty() {
            if !t.is_empty() {
                t.push_str(" · ");
            }
            t.push_str_or_ellipsis(e.account.as_str());
        }
        cx.ui.dialog(DialogSpec::confirm(
            DLG_PROVISION,
            l.get(Str::AuthAddQ),
            t.as_str(),
            l.get(Str::Cancel),
            l.get(Str::AddBtn),
        ));
    }

    fn menu_pick<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, id: u16) {
        let l = self.lang;
        match id {
            M_IMPORT => cx.svc.auth_request(AuthRequest::Import),
            M_NEXT => cx.svc.auth_request(AuthRequest::NextCode(self.focus)),
            M_UP if self.focus > 0 => {
                cx.svc.auth_request(AuthRequest::SwapNext(self.focus - 1));
                self.focus -= 1;
            }
            M_DOWN if self.focus + 1 < self.count => {
                cx.svc.auth_request(AuthRequest::SwapNext(self.focus));
                self.focus += 1;
            }
            M_DELETE => cx.ui.dialog(DialogSpec::destructive(
                DLG_DELETE,
                l.get(Str::AuthDeleteQ),
                l.get(Str::AuthDeleteText),
                l.get(Str::Cancel),
                l.get(Str::DeleteBtn),
            )),
            M_TIME => {
                let mut t = StrBuf::<96>::new();
                let now = cx.svc.now();
                if cx.svc.unix_time().is_some() {
                    t.push_u32(u32::from(now.year));
                    t.push('-');
                    t.push_u32_padded(u32::from(now.month), 2);
                    t.push('-');
                    t.push_u32_padded(u32::from(now.day), 2);
                    t.push(' ');
                    t.push_u32_padded(u32::from(now.hour), 2);
                    t.push(':');
                    t.push_u32_padded(u32::from(now.minute), 2);
                    t.push(':');
                    t.push_u32_padded(u32::from(now.second), 2);
                    t.push(' ');
                    t.push_str(l.get(Str::AuthUtc));
                } else {
                    t.push_str(l.get(Str::AuthClockNotSetAction));
                }
                let title = l.get(if cx.svc.unix_time().is_some() {
                    Str::AuthTimeCheck
                } else {
                    Str::AuthClockNotSet
                });
                cx.ui.dialog(DialogSpec::info(DLG_INFO, title, t.as_str(), l.get(Str::Ok)));
            }
            M_CHANGE => {
                self.reset_pad();
                self.error = None;
                self.mode = Mode::ChangePin;
            }
            M_LOCK => {
                cx.svc.auth_request(AuthRequest::Lock);
                self.mode = Mode::List;
            }
            M_WIPE => cx.ui.dialog(DialogSpec::destructive(
                DLG_WIPE,
                l.get(Str::AuthWipeQ),
                l.get(Str::AuthWipeText),
                l.get(Str::Cancel),
                l.get(Str::WipeBtn),
            )),
            M_TYPE => self.type_code(cx),
            _ => {}
        }
    }

    // ------------------------------------------------------------ drawing

    fn draw_message(&self, g: &mut Gfx<'_>, title: Str, text: Str) {
        let (w, _) = g.size();
        let l = self.lang;
        let t = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        g.text_in(4, 30, w - 8, l.get(title), FontId::UiBold, t, Align::Center);
        let mut y = 50;
        g.wrap(l.get(text), FontId::Ui, w - 12, |g, line| {
            g.text_in(6, y, w - 12, line, FontId::Ui, dim, Align::Center);
            y += g.line_pitch(line, FontId::Ui).max(14);
        });
    }

    fn draw_working(&self, g: &mut Gfx<'_>, work: VaultWork, percent: u8) {
        let (w, _) = g.size();
        let label = match work {
            VaultWork::Unlock => Str::AuthUnlocking,
            VaultWork::Create => Str::AuthSettingUp,
            VaultWork::ChangePin => Str::AuthChangingPin,
        };
        let t = g.c(Role::Text);
        g.text_in(4, 44, w - 8, self.lang.get(label), FontId::UiBold, t, Align::Center);
        draw_progress(g, Rect::new(14, 56, w - 28, 6), u16::from(percent.min(100)) * 10, None, Role::Bg);
    }

    fn draw_pad<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>, pad: Mode) {
        let (w, _) = g.size();
        let l = self.lang;
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        let warn = g.c(Role::Warning);
        let failures = match svc.vault() {
            VaultState::Locked { failures, .. } => failures,
            _ => 0,
        };
        let wait = Self::wait_s(svc);
        // Prompt, or the error / back-off in the warning colour.
        let mut line = StrBuf::<64>::new();
        let mut c = dim;
        if wait > 0 {
            line.push_str(l.get(Str::AuthWrongPin));
            line.push_str(" · ");
            line.push_str(l.get(Str::AuthWait));
            line.push(' ');
            line.push_u32(wait);
            line.push(' ');
            line.push_str(l.get(Str::Seconds));
            c = warn;
        } else if let Some(e) = self.error {
            line.push_str(l.get(e));
            c = warn;
        } else if pad == Mode::List && failures > 0 {
            line.push_str(l.get(Str::AuthWrongPin));
            c = warn;
        } else {
            line.push_str(l.get(match pad {
                Mode::NewPin => Str::AuthNewPin,
                Mode::ConfirmPin | Mode::ChangeConfirm => Str::AuthConfirmPin,
                Mode::ChangePin => Str::AuthNewPinChange,
                _ => Str::AuthEnterPin,
            }));
        }
        g.text_in(2, 12, w - 4, line.as_str(), FontId::Ui, c, Align::Center);
        // The digits so far as dots (at least four places).
        let n = self.pin.len();
        let places = i16::try_from(n.max(4)).unwrap_or(4);
        let pitch = 12;
        let x0 = (w - places * pitch) / 2 + 2;
        for k in 0..places {
            let r = Rect::new(x0 + k * pitch, 17, 8, 8);
            if usize::try_from(k).is_ok_and(|k| k < n) {
                g.fill_disc(r, text);
            } else {
                let ring = g.pal.mix(Role::Bg, Role::TextDim, 140);
                g.stroke_round(r, 4, ring);
            }
        }
        // 3 × 4 keys.
        let (kw, kh, gx, gy) = (36i16, 17i16, (w - 3 * 36) / 2, 29i16);
        for k in 0u8..12 {
            let col = i16::from(k % 3);
            let row = i16::from(k / 3);
            let r = Rect::new(gx + col * kw + 1, gy + row * kh + 1, kw - 2, kh - 2);
            let focused = k == self.cursor;
            let fg = if focused {
                g.fill_round(r, 4, g.c(Role::Accent));
                g.c(Role::OnAccent)
            } else {
                let face = g.pal.mix(Role::Bg, Role::TextDim, 40);
                g.fill_round(r, 4, face);
                text
            };
            match k {
                PAD_DEL => {
                    // A left-pointing triangle.
                    let (cx, cy) = (r.x + r.w / 2 - 3, r.y + r.h / 2);
                    for i in 0..6 {
                        g.vline(cx + i, cy - i / 2 - 1, i + 2 - i % 2, fg);
                    }
                }
                PAD_DONE => {
                    // A tick.
                    let (cx, cy) = (r.x + r.w / 2 - 4, r.y + r.h / 2);
                    for i in 0..3 {
                        g.fill(Rect::new(cx + i, cy + i - 1, 2, 2), fg);
                    }
                    for i in 0..5 {
                        g.fill(Rect::new(cx + 3 + i, cy + 1 - i, 2, 2), fg);
                    }
                }
                _ => {
                    let d = if k == PAD_ZERO { 0 } else { k + 1 };
                    let mut s = StrBuf::<2>::new();
                    s.push(char::from(b'0' + d));
                    g.text_in(r.x, r.y + 12, r.w, s.as_str(), FontId::UiBold, fg, Align::Center);
                }
            }
        }
        // The strip line: how to finish.
        let hint = if self.pin.complete() { Str::HintPinDone } else { Str::HintPinPad };
        g.text_in(2, STRIP_Y + 13 + 2, w - 4, l.get(hint), FontId::Ui, dim, Align::Center);
    }

    /// The code (or its placeholder) and its numeral font for `width`.
    fn code_text<S: Services + ?Sized>(svc: &S, r: &RowCache, out: &mut StrBuf<16>) -> bool {
        match (r.code, svc.unix_time(), r.is_hotp()) {
            (Some(c), _, true) | (Some(c), Some(_), false) => {
                push_grouped_code(out, c.as_str());
                true
            }
            _ => {
                push_dashes(out, r.digits);
                false
            }
        }
    }

    fn draw_list<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let l = self.lang;
        let dim = g.c(Role::TextDim);
        if self.count == 0 {
            self.draw_message(g, Str::AuthNoAccounts, Str::AuthNoAccountsAction);
            return;
        }
        let t = svc.unix_time();
        for k in 0..ROWS {
            let i = usize::from(self.top) + k;
            if i >= usize::from(self.count) {
                break;
            }
            let r = &self.rows[k];
            let y = i16::try_from(k).unwrap_or(0) * ROW_H;
            let focused = i == usize::from(self.focus);
            let (fg, sub) = if focused {
                g.fill_round(Rect::new(1, y + 1, w - 2, ROW_H - 2), 4, g.c(Role::Accent));
                (g.c(Role::OnAccent), g.c(Role::OnAccent))
            } else {
                (g.c(Role::Text), dim)
            };
            let mut code = StrBuf::<16>::new();
            let live = Self::code_text(svc, r, &mut code);
            // 16-px numerals; eight digits would leave no room for the
            // name, so they use the bold UI font.
            let font = if r.digits > 6 { FontId::UiBold } else { FontId::Num16 };
            let cw = g.measure(code.as_str(), font);
            let cx = w - 5 - cw;
            let base = if font == FontId::Num16 { y + 16 } else { y + 14 };
            g.text(cx, base, code.as_str(), font, if live { fg } else { sub });
            g.text_in(5, y + 14, cx - 9, r.name.as_str(), FontId::Ui, fg, Align::Left);
            // The seconds left as a shrinking bar under the code.
            if let (Some(t), false, true) = (t, r.is_hotp(), live) {
                let left = seconds_left(t, r.period);
                let bw =
                    i16::try_from(i64::from(cw) * i64::from(left) / i64::from(r.period.max(1))).unwrap_or(0);
                let c = if left <= 5 {
                    g.c(Role::Warning)
                } else if focused {
                    fg
                } else {
                    g.c(Role::Accent)
                };
                g.fill(Rect::new(cx, y + 17, bw.max(1), 1), c);
            }
        }
        // The strip line: the clock warning, or the focused account.
        if t.is_none() {
            let c = g.c(Role::Warning);
            g.text_in(
                4,
                STRIP_Y + 13 + 2,
                w - 8,
                l.get(Str::AuthClockNotSet),
                FontId::UiBold,
                c,
                Align::Center,
            );
        } else if let Some(r) = self.focused() {
            let s = if r.has_issuer { self.acct.as_str() } else { "" };
            g.text_in(4, STRIP_Y + 13 + 2, w - 8, s, FontId::Ui, dim, Align::Center);
        }
    }

    fn draw_code<S: Services + ?Sized>(&self, svc: &S, g: &mut Gfx<'_>) {
        let (w, _) = g.size();
        let l = self.lang;
        let Some(r) = self.focused() else { return };
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        g.text_in(4, 16, w - 8, r.name.as_str(), FontId::UiBold, text, Align::Center);
        if r.has_issuer {
            g.text_in(4, 31, w - 8, self.acct.as_str(), FontId::Ui, dim, Align::Center);
        }
        let t = svc.unix_time();
        let hotp = r.is_hotp();
        if !hotp && t.is_none() {
            let c = g.c(Role::Warning);
            g.text_in(4, 58, w - 8, l.get(Str::AuthClockNotSet), FontId::UiBold, c, Align::Center);
            let mut y = 78;
            g.wrap(l.get(Str::AuthClockNotSetAction), FontId::Ui, w - 12, |g, line| {
                g.text_in(6, y, w - 12, line, FontId::Ui, dim, Align::Center);
                y += g.line_pitch(line, FontId::Ui).max(14);
            });
            return;
        }
        let mut code = StrBuf::<16>::new();
        let live = Self::code_text(svc, r, &mut code);
        let font =
            if g.measure(code.as_str(), FontId::Num28) <= w - 8 { FontId::Num28 } else { FontId::Num16 };
        g.text_in(0, 68, w, code.as_str(), font, if live { text } else { dim }, Align::Center);
        if let (Some(t), false) = (t, hotp) {
            let left = seconds_left(t, r.period);
            let full = w - 28;
            let bw =
                i16::try_from(i64::from(full) * i64::from(left) / i64::from(r.period.max(1))).unwrap_or(0);
            let track = g.pal.mix(Role::Bg, Role::TextDim, 70);
            g.fill_round(Rect::new(14, 76, full, 4), 2, track);
            let c = if left <= 5 { g.c(Role::Warning) } else { g.c(Role::Accent) };
            g.fill_round(Rect::new(14, 76, bw.max(4), 4), 2, c);
            let mut s = StrBuf::<16>::new();
            s.push_u32(left);
            s.push(' ');
            s.push_str(l.get(Str::Seconds));
            g.text_in(4, 93, w - 8, s.as_str(), FontId::Ui, dim, Align::Center);
        }
        let hint = if hotp && !live { Str::HintAuthHotp } else { Str::HintAuthCode };
        g.text_in(4, STRIP_Y + 15, w - 8, l.get(hint), FontId::Ui, dim, Align::Center);
    }
}

impl<S: Services + ?Sized> Screen<S> for AuthScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.mode = Mode::List;
        self.reset_pad();
        self.error = None;
        self.reload(cx.svc);
        self.drawn = None;
        if cx.svc.vault() == VaultState::Absent {
            // The honest limit, in one sentence, before a PIN is chosen
            // (06 §5.2).
            let l = self.lang;
            cx.ui.dialog(DialogSpec::info(
                DLG_INFO,
                l.get(Str::AuthPinTitle),
                l.get(Str::AuthPinWarning),
                l.get(Str::Ok),
            ));
        }
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(k, phase) => {
                cx.svc.auth_request(AuthRequest::Touch);
                match self.view(cx.svc) {
                    View::Pad(_) => self.pad_key(cx, k, phase),
                    View::List => self.list_key(cx, k, phase),
                    View::Code => self.code_key(cx, k, phase),
                    View::Working(..) => Nav::Stay,
                    View::Message(..) => match (k, phase) {
                        (Key::Back, KeyPhase::Up { held: false }) => Nav::Pop,
                        _ => Nav::Stay,
                    },
                }
            }
            Event::Tick(_) => {
                if self.stale(cx.svc) {
                    self.reload(cx.svc);
                }
                Nav::Stay
            }
            Event::Ui(UiEvent::Menu { id, .. }) => {
                cx.svc.auth_request(AuthRequest::Touch);
                self.menu_pick(cx, id);
                Nav::Stay
            }
            Event::Ui(UiEvent::Dialog { id, button }) => {
                match (id, button) {
                    (DLG_DELETE, 1) => {
                        cx.svc.auth_request(AuthRequest::Delete(self.focus));
                        self.mode = Mode::List;
                    }
                    (DLG_WIPE, 1) => {
                        cx.svc.auth_request(AuthRequest::Wipe);
                        self.mode = Mode::List;
                        self.reset_pad();
                    }
                    (DLG_PROVISION, b) => cx.svc.auth_request(AuthRequest::Provision(b == 1)),
                    _ => {}
                }
                Nav::Stay
            }
            Event::Service(ServiceEvent::Other { kind: other::AUTH, value }) => {
                match value {
                    auth_event::STATE => {
                        if !matches!(cx.svc.vault(), VaultState::Unlocked { .. } | VaultState::Working { .. })
                        {
                            self.mode = Mode::List;
                            self.pin.clear();
                        }
                        self.error = None;
                        if matches!(cx.svc.vault(), VaultState::Unlocked { .. }) {
                            self.reset_pad();
                        }
                        self.reload(cx.svc);
                    }
                    auth_event::WRONG_PIN => {
                        self.error = Some(Str::AuthWrongPin);
                        self.pin.clear();
                    }
                    auth_event::ENTRIES => self.reload(cx.svc),
                    auth_event::OUTCOME => self.outcome(cx),
                    auth_event::PROVISION => self.ask_provision(cx),
                    _ => {}
                }
                Nav::Stay
            }
            Event::Service(ServiceEvent::SettingChanged(_)) => {
                self.lang = cx.svc.lang();
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
        g.clear(Role::Bg);
        match s.view {
            View::Message(t, a) => self.draw_message(g, t, a),
            View::Pad(m) => self.draw_pad(cx.svc, g, m),
            View::Working(w, p) => self.draw_working(g, w, p),
            View::List => self.draw_list(cx.svc, g),
            View::Code => self.draw_code(cx.svc, g),
        }
        self.drawn = Some(s);
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
        match self.view(cx.svc) {
            View::List | View::Code => {
                let code = self.view(cx.svc) == View::Code;
                let hotp = self.focused().is_some_and(RowCache::is_hotp);
                if code {
                    out.action(M_TYPE, l.get(Str::AuthTypeCode));
                }
                if hotp {
                    out.action(M_NEXT, l.get(Str::AuthNextCode));
                }
                if !code {
                    out.action(M_IMPORT, l.get(Str::AuthImport));
                    if self.focus > 0 {
                        out.action(M_UP, l.get(Str::MoveUp));
                    }
                    if self.focus + 1 < self.count {
                        out.action(M_DOWN, l.get(Str::MoveDown));
                    }
                }
                if self.count > 0 {
                    out.action(M_DELETE, l.get(Str::DeleteMenu));
                }
                out.action(M_TIME, l.get(Str::AuthTimeCheck));
                if !code {
                    out.action(M_CHANGE, l.get(Str::AuthChangePin));
                }
                out.action(M_LOCK, l.get(Str::AuthLock));
                if !code {
                    out.action(M_WIPE, l.get(Str::AuthWipe));
                }
            }
            View::Pad(Mode::List) | View::Message(..) => {
                out.action(M_TIME, l.get(Str::AuthTimeCheck));
                if cx.svc.vault() != VaultState::Unknown {
                    out.action(M_WIPE, l.get(Str::AuthWipe));
                }
            }
            _ => {}
        }
    }

    fn wants(&self) -> Wants {
        // Codes roll over and bars shrink every second; the pad counts the
        // back-off down; the progress bar moves.
        let ticks = !matches!(self.drawn.map(|d| d.view), Some(View::Message(..)));
        Wants {
            ticks,
            strip: StripUse::Content,
            title: Some(self.lang.get(Str::AppAuthenticator)),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        let mut r = Resume { focus: u32::from(self.focus), top: u32::from(self.top), ..Resume::default() };
        r.data[0] = u8::from(self.mode == Mode::Code);
        r
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        self.focus = u8::try_from(r.focus).unwrap_or(0);
        self.top = u8::try_from(r.top).unwrap_or(0);
        if r.data[0] == 1 {
            self.mode = Mode::Code;
        }
        self.reload(cx.svc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn grouping_and_placeholders() {
        let mut b = StrBuf::<16>::new();
        push_grouped_code(&mut b, "287082");
        assert_eq!(b.as_str(), "287 082");
        b.clear();
        push_grouped_code(&mut b, "07081804");
        assert_eq!(b.as_str(), "0708 1804");
        b.clear();
        push_dashes(&mut b, 6);
        assert_eq!(b.as_str(), "--- ---");
        b.clear();
        push_dashes(&mut b, 8);
        assert_eq!(b.as_str(), "---- ----");
        assert_eq!(seconds_left(59, 30), 1);
        assert_eq!(seconds_left(60, 30), 30);
    }
}
