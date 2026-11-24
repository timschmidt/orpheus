//! Screens and navigation (05 §5.1): the `Screen` trait, the `Nav` result,
//! and the fixed 8-entry navigation stack of resume records.

use crate::event::{Event, Instant};
use crate::gfx::Gfx;
use crate::menu::MenuBuilder;
use crate::text::StrBuf;
use crate::widgets::dialog::DialogSpec;

/// Identifies a screen. The app crate defines the constants.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ScreenId(pub u8);

/// Argument passed to a screen when it is opened (an index, a path handle,
/// a mode); its meaning is the screen's.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Arg(pub u32);

/// What a screen keeps across being covered: popped-to screens are rebuilt
/// from this record, not kept alive (≤ 64 bytes, 05 §5.1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Resume {
    pub focus: u32,
    pub top: u32,
    pub handle: u32,
    pub data: [u8; 48],
}

impl Default for Resume {
    fn default() -> Self {
        Resume { focus: 0, top: 0, handle: 0, data: [0; 48] }
    }
}

/// What the screen wants from the shell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StripUse {
    /// Mini-player while something plays (unless this is the player),
    /// otherwise the background.
    Auto,
    /// A hint line (mini-player takes precedence while playing).
    Hint(&'static str),
    /// The screen draws the strip area itself (content area is 128 × 114).
    Content,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Wants {
    /// Deliver `Event::Tick` about every 33 ms (marquee, progress, clocks).
    pub ticks: bool,
    /// Hide status bar and strip (pictures, big art).
    pub full_screen: bool,
    pub strip: StripUse,
    /// Shown in the status bar when the clock is off.
    pub title: Option<&'static str>,
    /// This screen is the player of the current source (no mini-player).
    pub is_player: bool,
}

impl Default for Wants {
    fn default() -> Self {
        Wants { ticks: false, full_screen: false, strip: StripUse::Auto, title: None, is_player: false }
    }
}

/// How much of the content area must be drawn.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Dirty {
    /// Everything: the area holds garbage (screen change, overlay closed).
    All,
    /// Only what changed since the last draw (the screen tracks that).
    Changed,
}

/// Result of handling an event.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Nav {
    Stay,
    Push(ScreenId, Arg),
    Replace(ScreenId, Arg),
    Pop,
    PopTo(ScreenId),
    Home,
}

/// Requests a screen makes of the toolkit while handling an event.
#[derive(Default)]
pub struct UiRequests {
    pub(crate) toast: Option<StrBuf<48>>,
    pub(crate) toast_ms: Option<u16>,
    pub(crate) dialog: Option<DialogSpec>,
    pub(crate) redraw: bool,
    pub(crate) invalidate: bool,
}

impl UiRequests {
    /// Show a one-line toast over the strip for 2 s.
    pub fn toast(&mut self, text: &str) {
        self.toast = Some(StrBuf::from(text));
    }

    /// Show a toast for `ms` milliseconds instead of 2 s.
    pub fn toast_for(&mut self, text: &str, ms: u16) {
        self.toast = Some(StrBuf::from(text));
        self.toast_ms = Some(ms);
    }

    /// Redraw everything, status bar and strip included (theme changed).
    pub fn invalidate_all(&mut self) {
        self.invalidate = true;
    }

    /// Open a dialog; the answer arrives as `Event::Ui(UiEvent::Dialog)`.
    pub fn dialog(&mut self, d: DialogSpec) {
        self.dialog = Some(d);
    }

    /// Redraw the whole content area on the next frame (`Dirty::All`).
    pub fn redraw(&mut self) {
        self.redraw = true;
    }
}

/// What a screen sees of the world: the services `S` (implemented by the
/// firmware over channels and by the simulator with host code), toolkit
/// requests, and the time of the event.
pub struct Cx<'a, S: ?Sized> {
    pub svc: &'a mut S,
    pub ui: &'a mut UiRequests,
    pub now: Instant,
}

/// One screen (05 §5.1). Implemented by each screen struct and by the app's
/// `enum ScreenState` that dispatches to them (static dispatch, no heap).
pub trait Screen<S: ?Sized> {
    /// Opened with `arg` (fresh, or about to be restored).
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg);
    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav;
    /// Draw the content area. With `Dirty::Changed`, draw only what changed.
    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty);
    /// Context-menu items. `out.parent()` is `None` for the top level, or
    /// the id of the sub-menu being opened.
    fn menu(&self, _cx: &Cx<'_, S>, _out: &mut MenuBuilder) {}
    fn wants(&self) -> Wants {
        Wants::default()
    }
    /// Resume record taken when another screen is pushed on top.
    fn save(&self) -> Resume {
        Resume::default()
    }
    /// Called after `enter` when the screen is rebuilt by a pop.
    fn restore(&mut self, _cx: &mut Cx<'_, S>, _r: &Resume) {}
}

/// The app's set of screens: one enum, one variant per screen.
pub trait ScreenSet<S: ?Sized>: Screen<S> + Sized {
    /// Root of the stack; `Nav::Home` and hold-Back return here.
    const HOME: ScreenId;
    /// Pushed on hold-Menu (05 §6.4), if the app has one.
    const QUICK_PANEL: Option<ScreenId> = None;
    /// Pushed when a locked key is pressed (05 §6.3).
    const LOCK_SCREEN: Option<ScreenId> = None;
    /// Pushed when Power is held 1.5 s (05 §3).
    const POWER_MENU: Option<ScreenId> = None;
    /// Sees every non-key event before the top screen does (low battery,
    /// USB host connected, card removed); may navigate or toast.
    fn global(_cx: &mut Cx<'_, S>, _ev: &Event) -> Nav {
        Nav::Stay
    }
    /// Build the (empty) state for `id`; `enter` follows.
    fn create(id: ScreenId) -> Self;
    fn id(&self) -> ScreenId;
    /// Name for scripts (`expect-screen:`) and logs.
    fn name(&self) -> &'static str;
}

/// Depth of the navigation stack.
pub const NAV_DEPTH: usize = 8;

#[derive(Clone, Copy, Debug, Default)]
pub struct NavEntry {
    pub id: ScreenId,
    pub arg: Arg,
    pub resume: Resume,
}

/// Fixed-size stack of screens below the top one (05 §5.1). Entry 0 is the
/// root. When full, the entry above the root is dropped.
#[derive(Debug)]
pub struct NavStack {
    e: [NavEntry; NAV_DEPTH],
    n: u8,
}

impl Default for NavStack {
    fn default() -> Self {
        Self::new()
    }
}

impl NavStack {
    #[must_use]
    pub const fn new() -> NavStack {
        const E: NavEntry = NavEntry {
            id: ScreenId(0),
            arg: Arg(0),
            resume: Resume { focus: 0, top: 0, handle: 0, data: [0; 48] },
        };
        NavStack { e: [E; NAV_DEPTH], n: 0 }
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.n)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    pub fn clear(&mut self) {
        self.n = 0;
    }

    pub fn push(&mut self, entry: NavEntry) {
        if self.len() == NAV_DEPTH {
            self.e.copy_within(2.., 1);
            self.n -= 1;
        }
        self.e[self.len()] = entry;
        self.n += 1;
    }

    pub fn pop(&mut self) -> Option<NavEntry> {
        if self.n == 0 {
            return None;
        }
        self.n -= 1;
        Some(self.e[self.len()])
    }

    #[must_use]
    pub fn top(&self) -> Option<&NavEntry> {
        self.len().checked_sub(1).and_then(|i| self.e.get(i))
    }

    pub fn top_mut(&mut self) -> Option<&mut NavEntry> {
        let i = self.len().checked_sub(1)?;
        self.e.get_mut(i)
    }

    #[must_use]
    pub fn entries(&self) -> &[NavEntry] {
        &self.e[..self.len()]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_drops_above_root_when_full() {
        let mut s = NavStack::new();
        for i in 0..10u8 {
            s.push(NavEntry { id: ScreenId(i), ..NavEntry::default() });
        }
        assert_eq!(s.len(), NAV_DEPTH);
        assert_eq!(s.entries()[0].id, ScreenId(0));
        assert_eq!(s.top().unwrap().id, ScreenId(9));
        assert_eq!(s.entries()[1].id, ScreenId(3));
        assert_eq!(s.pop().unwrap().id, ScreenId(9));
        assert_eq!(s.len(), NAV_DEPTH - 1);
    }

    #[test]
    fn sizes() {
        assert!(core::mem::size_of::<Resume>() <= 64);
        assert!(core::mem::size_of::<NavStack>() <= 8 * 72 + 8);
    }
}
