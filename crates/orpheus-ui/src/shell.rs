//! The shell: owns the navigation stack, the top screen and the overlays;
//! applies the global key grammar (05 §3); decides what to redraw.
//!
//! Firmware loop (UI task), roughly:
//!
//! ```text
//! loop {
//!     select(key_channel, service_channel, timer(if shell.wants_ticks() 33 ms)) {
//!         key(k, p)   => req = shell.key(&mut svc, k, p, now),
//!         ev          => req = shell.event(&mut svc, ev, now),
//!         timer       => req = shell.tick(&mut svc, now),
//!     }
//!     act on req (screen off, power menu, wake …)
//!     shell.draw(&mut svc, &mut Gfx::new(fb, &mut dirty, palette, fallback), now);
//!     blit dirty.take() to the panel (DMA)
//! }
//! ```

use core::marker::PhantomData;

use crate::event::{Event, Instant, Key, KeyPhase, UiEvent};
use crate::geom::Rect;
use crate::gfx::{CONTENT, Gfx, SCREEN, STATUS_BAR, STRIP};
use crate::input::{Gate, KeyGate};
use crate::menu::{MenuBuilder, MenuSheet, SheetResult};
use crate::nav::{Arg, Cx, Dirty, Nav, NavEntry, NavStack, ScreenId, ScreenSet, StripUse, UiRequests};
use crate::theme::Role;
use crate::widgets::bars::{
    MiniPlayer, Status, draw_blank_strip, draw_hint, draw_mini_player, draw_status_bar, mini_player_overflow,
};
use crate::widgets::dialog::{Dialog, DialogResult};
use crate::widgets::list::{ListCx, marquee_offset};
use crate::widgets::overlay::{Toast, VOLUME_MS, VolumeOverlay};

/// What the shell needs from the system, beyond what screens use.
pub trait System {
    fn status(&self) -> Status;
    /// Step the volume by `delta`; returns (level, max).
    fn volume_step(&mut self, delta: i8) -> (u8, u8);
    /// What plays, if anything (drives the mini-player).
    fn mini_player(&self) -> Option<MiniPlayer<'_>>;
    /// The **List roll-around** setting (Settings › Display, 05 §3): every
    /// list rolls round at its ends while this is true. Read once per key.
    fn list_roll_around(&self) -> bool {
        true
    }
}

/// Things only the caller can do, returned from event handling.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Requests {
    /// Turn the screen on (first key while off).
    pub wake: bool,
    /// Power tap: screen off and lock.
    pub screen_off: bool,
    /// Power held 1.5 s.
    pub power_menu: bool,
    /// Power held 4 s.
    pub power_off: bool,
    /// Locked key pressed: show the lock screen for 2 s.
    pub lock_notice: bool,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum StripKey {
    None,
    Blank,
    Hint(u32),
    Mini { hash: u32, playing: bool, progress: u16, scroll: i16 },
}

fn hash(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// Ticks are delivered at this period while anything animates.
pub const TICK_MS: u32 = 33;

pub struct Shell<T, S: ?Sized> {
    stack: NavStack,
    top: T,
    pub gate: KeyGate,
    ui: UiRequests,
    sheet: Option<MenuSheet>,
    dialog: Option<Dialog>,
    toast: Option<Toast>,
    toast_drawn: bool,
    volume: Option<VolumeOverlay>,
    volume_drawn: bool,
    content_all: bool,
    status_drawn: Option<Status>,
    strip_drawn: StripKey,
    strip_since: Instant,
    strip_hash: u32,
    strip_scrolls: bool,
    modal_fresh: bool,
    now: Instant,
    /// [`System::list_roll_around`], read at each key.
    roll: bool,
    /// When the open menu or dialog opened: the screen under it is drawn
    /// as of that moment (a marquee or clock under the scrim stays still,
    /// so redrawing it there gives the same frame).
    underlay_at: Instant,
    _svc: PhantomData<fn(&mut S)>,
}

impl<T: ScreenSet<S>, S: System + ?Sized> Shell<T, S> {
    /// Start at the home screen.
    pub fn new(svc: &mut S, now: Instant) -> Self {
        Self::with_root(svc, T::HOME, Arg(0), now)
    }

    /// Start with `root` as the only screen (boot logo, charging screen).
    /// `Nav::Home` later replaces it with `T::HOME`.
    pub fn with_root(svc: &mut S, root: ScreenId, arg: Arg, now: Instant) -> Self {
        let mut s = Self::unentered(svc, root, arg, now);
        s.enter_root(svc);
        s
    }

    /// [`Shell::with_root`] in two steps, for a caller that keeps the shell
    /// in a static: build it here (the root screen created and on the
    /// navigation stack, not entered), move it into place, then call
    /// [`Shell::enter_root`] on it there. Built and entered in one call,
    /// the whole shell is a temporary in the caller's frame while the root
    /// screen's `enter` runs below it (4.8 KB of the firmware's UI task,
    /// VER-10).
    pub fn unentered(svc: &mut S, root: ScreenId, arg: Arg, now: Instant) -> Self {
        let mut s = Shell {
            stack: NavStack::new(),
            top: T::create(root),
            gate: KeyGate::new(),
            ui: UiRequests::default(),
            sheet: None,
            dialog: None,
            toast: None,
            toast_drawn: false,
            volume: None,
            volume_drawn: false,
            content_all: true,
            status_drawn: None,
            strip_drawn: StripKey::None,
            strip_since: now,
            strip_hash: 0,
            strip_scrolls: false,
            modal_fresh: false,
            now,
            roll: svc.list_roll_around(),
            underlay_at: now,
            _svc: PhantomData,
        };
        s.stack.push(NavEntry { id: root, arg, resume: Default::default() });
        s
    }

    /// Enter the root screen of a shell built with [`Shell::unentered`]
    /// (once, before any event).
    pub fn enter_root(&mut self, svc: &mut S) {
        let (now, arg) = (self.now, self.stack.top().map_or(Arg(0), |e| e.arg));
        let mut cx = Cx { svc, ui: &mut self.ui, now, roll: self.roll };
        self.top.enter(&mut cx, arg);
        self.apply_requests(svc, now);
    }

    /// The top screen.
    pub fn top(&self) -> &T {
        &self.top
    }

    pub fn top_mut(&mut self) -> &mut T {
        &mut self.top
    }

    #[must_use]
    pub fn top_name(&self) -> &'static str {
        self.top.name()
    }

    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Something is drawn over the screen (menu, dialog, toast, volume):
    /// pixels written into the frame buffer by a service would cover it.
    #[must_use]
    pub fn overlay_shown(&self) -> bool {
        self.sheet.is_some() || self.dialog.is_some() || self.toast.is_some() || self.volume.is_some()
    }

    #[must_use]
    pub fn menu_open(&self) -> bool {
        self.sheet.is_some()
    }

    #[must_use]
    pub fn dialog_open(&self) -> bool {
        self.dialog.is_some()
    }

    /// Redraw everything on the next `draw` (theme change, screen back on).
    pub fn invalidate(&mut self) {
        self.content_all = true;
        self.status_drawn = None;
        self.strip_drawn = StripKey::None;
        self.toast_drawn = false;
        self.volume_drawn = false;
        if self.sheet.is_some() || self.dialog.is_some() {
            self.modal_fresh = true;
        }
    }

    /// True while the caller should call [`Shell::tick`] every [`TICK_MS`].
    #[must_use]
    pub fn wants_ticks(&self, svc: &S) -> bool {
        self.top.wants().ticks
            || self.toast.is_some()
            || self.volume.is_some()
            || self.sheet.as_ref().is_some_and(MenuSheet::wants_ticks)
            || (self.strip_scrolls && svc.mini_player().is_some())
    }

    /// A key event from the [`KeyMapper`](crate::input::KeyMapper).
    pub fn key(&mut self, svc: &mut S, k: Key, p: KeyPhase, now: Instant) -> Requests {
        self.now = now;
        self.roll = svc.list_roll_around();
        let mut req = Requests::default();
        match self.gate.filter(k, p) {
            Gate::Pass(k, p) => self.key_pass(svc, k, p, now, &mut req),
            Gate::Wake => req.wake = true,
            Gate::LockNotice => {
                req.lock_notice = true;
                if let Some(id) = T::LOCK_SCREEN
                    && self.top.id() != id
                {
                    self.close_modal();
                    self.apply_nav(svc, Nav::Push(id, Arg(0)), now);
                    self.apply_requests(svc, now);
                }
            }
            Gate::Drop => {}
        }
        req
    }

    fn key_pass(&mut self, svc: &mut S, k: Key, p: KeyPhase, now: Instant, req: &mut Requests) {
        // Volume works everywhere (05 §3).
        if matches!(k, Key::VolUp | Key::VolDown) {
            if matches!(p, KeyPhase::Down | KeyPhase::Repeat) {
                let (level, max) = svc.volume_step(if k == Key::VolUp { 1 } else { -1 });
                if self.gate.screen_on {
                    self.volume = Some(VolumeOverlay { level, max, until: now.add_ms(VOLUME_MS) });
                    self.volume_drawn = false;
                }
            }
            return;
        }
        if k == Key::Power {
            match p {
                KeyPhase::Up { held: false } => req.screen_off = true,
                KeyPhase::Hold => {
                    req.power_menu = true;
                    if let Some(id) = T::POWER_MENU
                        && self.top.id() != id
                    {
                        self.close_modal();
                        self.apply_nav(svc, Nav::Push(id, Arg(0)), now);
                        self.apply_requests(svc, now);
                    }
                }
                KeyPhase::Long => req.power_off = true,
                _ => {}
            }
            return;
        }
        let ev = Event::Key(k, p);
        if let Some(d) = self.dialog.as_mut() {
            if let DialogResult::Done(b) = d.handle(&ev) {
                let id = d.spec.id;
                self.close_modal();
                self.deliver(svc, Event::Ui(UiEvent::Dialog { id, button: b }), now);
            }
            return;
        }
        if self.sheet.is_some() {
            if ev.is_hold(Key::Back) {
                self.close_modal();
                self.apply_nav(svc, Nav::Home, now);
                return;
            }
            self.sheet_key(svc, &ev, now);
            return;
        }
        if ev.is_hold(Key::Back) {
            self.apply_nav(svc, Nav::Home, now);
            return;
        }
        if k == Key::Menu {
            match p {
                KeyPhase::Hold => {
                    if let Some(q) = T::QUICK_PANEL
                        && self.top.id() != q
                    {
                        self.apply_nav(svc, Nav::Push(q, Arg(0)), now);
                    }
                }
                // A screen that takes Menu itself gets every other phase.
                #[cfg(feature = "menu-key")]
                _ if self.top.wants().menu_key => self.deliver(svc, ev, now),
                KeyPhase::Up { held: false } => self.open_menu(svc, now),
                _ => {}
            }
            return;
        }
        self.deliver(svc, ev, now);
    }

    fn build_menu(&mut self, svc: &mut S, parent: Option<u16>, now: Instant) -> MenuBuilder {
        let mut b = MenuBuilder::new(parent);
        let cx = Cx { svc, ui: &mut self.ui, now, roll: self.roll };
        self.top.menu(&cx, &mut b);
        b
    }

    fn open_menu(&mut self, svc: &mut S, now: Instant) {
        let b = self.build_menu(svc, None, now);
        if b.is_empty() {
            return;
        }
        self.sheet = Some(MenuSheet::new(b, now));
        self.modal_fresh = true;
        self.underlay_at = now;
    }

    fn sheet_key(&mut self, svc: &mut S, ev: &Event, now: Instant) {
        let Some(sheet) = self.sheet.as_mut() else { return };
        match sheet.handle(ev, ListCx { now, roll: self.roll }) {
            SheetResult::None => {}
            SheetResult::Close => self.close_modal(),
            SheetResult::Level(parent) => {
                let b = self.build_menu(svc, parent, now);
                if let Some(s) = self.sheet.as_mut() {
                    s.rebuild(b, false, now);
                }
                self.modal_fresh = true;
            }
            SheetResult::Pick { id, value, close } => {
                let parent = sheet.parent();
                if close {
                    self.close_modal();
                }
                self.deliver(svc, Event::Ui(UiEvent::Menu { id, value }), now);
                if !close && self.sheet.is_some() {
                    let b = self.build_menu(svc, parent, now);
                    if let Some(s) = self.sheet.as_mut() {
                        s.rebuild(b, true, now);
                    }
                }
            }
        }
    }

    fn close_modal(&mut self) {
        self.sheet = None;
        self.dialog = None;
        self.modal_fresh = false;
        self.content_all = true;
        self.strip_drawn = StripKey::None;
        self.toast_drawn = false;
        self.volume_drawn = false;
    }

    /// A non-key event (service, data). Ticks go through [`Shell::tick`].
    pub fn event(&mut self, svc: &mut S, ev: Event, now: Instant) -> Requests {
        self.now = now;
        // App-wide reactions first (low battery, USB host, card removed).
        let nav = {
            let mut cx = Cx { svc: &mut *svc, ui: &mut self.ui, now, roll: self.roll };
            T::global(&mut cx, &ev)
        };
        if nav != Nav::Stay {
            self.close_modal();
            self.apply_nav(svc, nav, now);
            self.apply_requests(svc, now);
        }
        self.deliver(svc, ev, now);
        Requests::default()
    }

    /// Timers: overlay expiry, marquee, and `Event::Tick` for screens that
    /// asked for it.
    pub fn tick(&mut self, svc: &mut S, now: Instant) -> Requests {
        self.now = now;
        if self.toast.is_some_and(|t| now >= t.until) {
            self.toast = None;
            self.strip_drawn = StripKey::None;
            // Screens that own the strip area must repaint under the toast.
            let w = self.top.wants();
            if w.strip == StripUse::Content || w.full_screen {
                self.content_all = true;
            }
            if self.sheet.is_some() || self.dialog.is_some() {
                self.modal_fresh = true;
            }
        }
        if self.volume.is_some_and(|v| now >= v.until) {
            self.volume = None;
            self.content_all = true;
            if self.sheet.is_some() || self.dialog.is_some() {
                self.modal_fresh = true;
            }
        }
        if self.top.wants().ticks {
            self.deliver(svc, Event::Tick(now), now);
        }
        Requests::default()
    }

    fn deliver(&mut self, svc: &mut S, ev: Event, now: Instant) {
        let nav = {
            let mut cx = Cx { svc: &mut *svc, ui: &mut self.ui, now, roll: self.roll };
            self.top.event(&mut cx, &ev)
        };
        self.apply_nav(svc, nav, now);
        self.apply_requests(svc, now);
    }

    fn apply_requests(&mut self, _svc: &mut S, now: Instant) {
        if let Some(t) = self.ui.toast.take() {
            let mut toast = Toast::new(t, now);
            if let Some(ms) = self.ui.toast_ms.take() {
                toast.until = now.add_ms(u64::from(ms));
            }
            self.toast = Some(toast);
            self.toast_drawn = false;
        }
        if core::mem::take(&mut self.ui.invalidate) {
            self.invalidate();
        }
        if let Some(d) = self.ui.dialog.take() {
            self.sheet = None;
            self.dialog = Some(Dialog::new(d));
            self.modal_fresh = true;
            self.underlay_at = now;
        }
        if core::mem::take(&mut self.ui.redraw) {
            self.content_all = true;
        }
    }

    /// The new screen is built in [`Self::replace_top`]'s frame: built here,
    /// the temporary (the size of the largest screen) stayed in this frame
    /// under the new screen's `enter` (VER-10).
    fn rebuild(&mut self, svc: &mut S, e: NavEntry, restore: bool, now: Instant) {
        self.replace_top(e.id);
        let mut cx = Cx { svc, ui: &mut self.ui, now, roll: self.roll };
        self.top.enter(&mut cx, e.arg);
        if restore {
            self.top.restore(&mut cx, &e.resume);
        }
        self.content_all = true;
        self.strip_drawn = StripKey::None;
        self.status_drawn = None;
        self.sheet = None;
        self.dialog = None;
        self.modal_fresh = false;
    }

    #[inline(never)]
    fn replace_top(&mut self, id: ScreenId) {
        self.top = T::create(id);
    }

    fn apply_nav(&mut self, svc: &mut S, nav: Nav, now: Instant) {
        match nav {
            Nav::Stay => {}
            Nav::Push(id, arg) => {
                let saved = self.top.save();
                if let Some(t) = self.stack.top_mut() {
                    t.resume = saved;
                }
                let e = NavEntry { id, arg, resume: Default::default() };
                self.stack.push(e);
                self.rebuild(svc, e, false, now);
            }
            Nav::Replace(id, arg) => {
                let e = NavEntry { id, arg, resume: Default::default() };
                if let Some(t) = self.stack.top_mut() {
                    *t = e;
                }
                self.rebuild(svc, e, false, now);
            }
            Nav::Pop => {
                if self.stack.len() > 1 {
                    self.stack.pop();
                    if let Some(e) = self.stack.top().copied() {
                        self.rebuild(svc, e, true, now);
                    }
                }
            }
            Nav::PopTo(id) => {
                let mut popped = false;
                while self.stack.len() > 1 && self.stack.top().is_some_and(|e| e.id != id) {
                    self.stack.pop();
                    popped = true;
                }
                if popped && let Some(e) = self.stack.top().copied() {
                    self.rebuild(svc, e, true, now);
                }
            }
            Nav::Home => {
                if self.stack.len() == 1 && self.top.id() == T::HOME {
                    return;
                }
                self.stack.clear();
                let e = NavEntry { id: T::HOME, arg: Arg(0), resume: Default::default() };
                self.stack.push(e);
                self.rebuild(svc, e, false, now);
            }
        }
    }

    /// Jump to a screen as if `Nav` had been returned (tests, resume at boot).
    pub fn navigate(&mut self, svc: &mut S, nav: Nav, now: Instant) {
        self.apply_nav(svc, nav, now);
        self.apply_requests(svc, now);
    }

    /// Bring the frame buffer up to date. Only changed regions are drawn and
    /// marked dirty in `g`.
    pub fn draw(&mut self, svc: &mut S, g: &mut Gfx<'_>) {
        let now = self.now;
        let wants = self.top.wants();
        let modal = self.sheet.is_some() || self.dialog.is_some();
        let full = wants.full_screen;
        let content = if full {
            SCREEN
        } else if wants.strip == StripUse::Content {
            Rect::from_edges(0, CONTENT.y, CONTENT.w, SCREEN.h)
        } else {
            CONTENT
        };

        if !full {
            let status = svc.status();
            if self.status_drawn != Some(status) {
                g.with_area(STATUS_BAR, |g| draw_status_bar(g, &status, wants.title));
                self.status_drawn = Some(status);
            }
        } else {
            self.status_drawn = None;
        }

        let underlay_changed = !modal || self.modal_fresh;
        if underlay_changed {
            // Under a fresh scrim everything is redrawn: the previous scrim
            // and sheet must not show through (and must not be dimmed twice).
            let dirty = if self.content_all || self.modal_fresh { Dirty::All } else { Dirty::Changed };
            if self.modal_fresh {
                self.strip_drawn = StripKey::None;
            }
            let top = &mut self.top;
            let at = if modal { self.underlay_at } else { now };
            let cx = Cx { svc: &mut *svc, ui: &mut self.ui, now: at, roll: self.roll };
            g.with_area(content, |g| {
                if dirty == Dirty::All {
                    g.clear(Role::Bg);
                }
                top.draw(&cx, g, dirty);
            });
            self.content_all = false;
            if !full && wants.strip != StripUse::Content {
                self.draw_strip(svc, g, wants.strip, wants.is_player, now);
            }
            if dirty == Dirty::All {
                // A full redraw may have covered the overlays.
                self.volume_drawn = false;
                self.toast_drawn = false;
            }
        }

        if self.modal_fresh {
            let scrim = g.c(Role::Bg);
            let dark = g.pal.is_dark();
            let r = Rect::from_edges(0, if full { 0 } else { CONTENT.y }, SCREEN.w, SCREEN.h);
            g.dim(r, if dark { scrim } else { 0 }, if dark { 150 } else { 110 });
            if let Some(s) = self.sheet.as_mut() {
                s.invalidate();
            }
            if let Some(d) = self.dialog.as_mut() {
                d.invalidate();
            }
            self.modal_fresh = false;
            self.toast_drawn = false;
            self.volume_drawn = false;
        }
        if let Some(s) = self.sheet.as_mut() {
            s.draw(g, now);
        }
        if let Some(d) = self.dialog.as_mut() {
            d.draw(g);
        }
        // So may a partial one, or a menu or dialog redrawn under a shown
        // overlay (a focus moving under the volume overlay): draw the
        // overlay on top again (VER-4 finding F-UI-2).
        if g.drew_into(VolumeOverlay::rect()) {
            self.volume_drawn = false;
        }
        if g.drew_into(Toast::rect().inset(-2, -1)) {
            self.toast_drawn = false;
        }
        if let Some(t) = &self.toast
            && !self.toast_drawn
        {
            t.draw(g);
            self.toast_drawn = true;
        }
        if let Some(v) = &self.volume
            && !self.volume_drawn
        {
            v.draw(g);
            self.volume_drawn = true;
        }
    }

    fn draw_strip(&mut self, svc: &S, g: &mut Gfx<'_>, use_: StripUse, is_player: bool, now: Instant) {
        let mini = if is_player { None } else { svc.mini_player() };
        let key = match (&mini, use_) {
            (Some(m), _) => {
                let h = hash(m.title);
                if h != self.strip_hash {
                    self.strip_hash = h;
                    self.strip_since = now;
                }
                let over = mini_player_overflow(g, m.title);
                self.strip_scrolls = over > 0;
                let scroll = marquee_offset(now.since(self.strip_since), over);
                StripKey::Mini { hash: h, playing: m.playing, progress: m.progress, scroll }
            }
            (None, StripUse::Hint(h)) => StripKey::Hint(hash(h)),
            _ => StripKey::Blank,
        };
        if !matches!(key, StripKey::Mini { .. }) {
            self.strip_scrolls = false;
        }
        if key == self.strip_drawn {
            return;
        }
        g.with_area(STRIP, |g| match (&mini, key) {
            (Some(m), StripKey::Mini { scroll, .. }) => draw_mini_player(g, m, scroll),
            (None, StripKey::Hint(_)) => {
                if let StripUse::Hint(h) = use_ {
                    draw_hint(g, h);
                }
            }
            _ => draw_blank_strip(g),
        });
        self.strip_drawn = key;
        if self.toast.is_some() {
            self.toast_drawn = false;
        }
    }

    /// The screen id at the top.
    #[must_use]
    pub fn top_id(&self) -> ScreenId {
        self.top.id()
    }
}
