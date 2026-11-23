//! `ListView`: a virtual list of 19 px rows over a [`ListSource`] (05 §4
//! "Rows", §5.2). The view holds only focus, scroll position and what it
//! drew last; rows are asked for while drawing and never copied.

use crate::event::{Event, Instant, Key, KeyPhase};
use crate::font::FontId;
use crate::geom::Rect;
use crate::gfx::{Align, Gfx};
use crate::icons::{self, Icon};
use crate::nav::Dirty;
use crate::theme::Role;

/// Row pitch (05 §4).
pub const ROW_H: i16 = 19;
/// Baseline of the UI font inside a row: caps are vertically centred and a
/// 16 px fallback glyph still fits.
pub const ROW_BASELINE: i16 = 14;

/// Right-hand decoration of a row.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Accessory {
    #[default]
    None,
    /// Opens a sub-level.
    Chevron,
    Toggle(bool),
    Radio(bool),
    Check(bool),
}

/// A focusable row.
#[derive(Clone, Copy, Debug)]
pub struct Item<'a> {
    pub text: &'a str,
    pub value: Option<&'a str>,
    pub icon: Option<&'static Icon>,
    pub accessory: Accessory,
    /// Drawn in the dim text colour (unavailable entries).
    pub dim: bool,
}

impl<'a> Item<'a> {
    #[must_use]
    pub const fn new(text: &'a str) -> Item<'a> {
        Item { text, value: None, icon: None, accessory: Accessory::None, dim: false }
    }

    #[must_use]
    pub const fn value(mut self, v: &'a str) -> Self {
        self.value = Some(v);
        self
    }

    #[must_use]
    pub const fn icon(mut self, i: &'static Icon) -> Self {
        self.icon = Some(i);
        self
    }

    #[must_use]
    pub const fn accessory(mut self, a: Accessory) -> Self {
        self.accessory = a;
        self
    }

    #[must_use]
    pub const fn dim(mut self) -> Self {
        self.dim = true;
        self
    }
}

/// One row as the source describes it.
#[derive(Clone, Copy, Debug)]
pub enum Row<'a> {
    Item(Item<'a>),
    /// Section header; not focusable.
    Header(&'a str),
    /// Not loaded yet: drawn as a dim placeholder; the screen redraws when
    /// the data arrives (`Event::Data`).
    Pending,
}

/// Where rows come from. Implementations keep their own cache (two pages
/// for directory windows); `row` must not block.
pub trait ListSource {
    fn len(&self) -> u32;
    fn is_empty(&self) -> bool {
        self.len() == 0
    }
    fn row(&self, i: u32) -> Row<'_>;
    /// First letter of row `i` for letter jump (hold Prev/Next); `None`
    /// disables letter jump.
    fn letter(&self, _i: u32) -> Option<char> {
        None
    }
}

/// What a key did to the list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ListAction {
    None,
    Moved,
    /// OK on a focusable row.
    Activated(u32),
}

#[derive(Clone, Copy, Debug, Default)]
struct Drawn {
    valid: bool,
    focus: u32,
    top: u32,
    len: u32,
    marquee: i16,
    jump: Option<char>,
}

/// Focus, scroll and marquee state of a list (about 48 bytes).
#[derive(Clone, Copy, Debug, Default)]
pub struct ListView {
    pub focus: u32,
    pub top: u32,
    /// Wrap from the last row to the first (off by default, 05 §5.2).
    pub wrap: bool,
    focus_since: Instant,
    /// Pixels by which the focused row's text overflows (measured on draw).
    overflow: i16,
    jump: Option<char>,
    drawn: Drawn,
}

/// Marquee offset after `ms` of focus for text overflowing by `over` px:
/// starts after 800 ms, 30 px/s, pauses 1 s at each end (05 §4).
#[must_use]
pub fn marquee_offset(ms: u64, over: i16) -> i16 {
    if over <= 0 || ms < 800 {
        return 0;
    }
    let over64 = u64::try_from(over).unwrap_or(0);
    let travel = over64 * 1000 / 30;
    let cycle = 2 * travel + 2000;
    let t = (ms - 800) % cycle;
    let px = if t < travel {
        t * 30 / 1000
    } else if t < travel + 1000 {
        over64
    } else if t < 2 * travel + 1000 {
        over64 - (t - travel - 1000) * 30 / 1000
    } else {
        0
    };
    i16::try_from(px.min(over64)).unwrap_or(0)
}

impl ListView {
    #[must_use]
    pub const fn new() -> ListView {
        ListView {
            focus: 0,
            top: 0,
            wrap: false,
            focus_since: Instant(0),
            overflow: 0,
            jump: None,
            drawn: Drawn { valid: false, focus: 0, top: 0, len: 0, marquee: 0, jump: None },
        }
    }

    /// Rows that fit in a height.
    #[must_use]
    pub fn rows_for(h: i16) -> u32 {
        u32::try_from((h / ROW_H).max(1)).unwrap_or(1)
    }

    /// Force a full redraw (data arrived, rows changed).
    pub fn invalidate(&mut self) {
        self.drawn.valid = false;
    }

    fn focusable(src: &dyn ListSource, i: u32) -> bool {
        !matches!(src.row(i), Row::Header(_))
    }

    /// Put focus on the first focusable row at or after `i` and scroll it
    /// into view (for screens restoring a resume record).
    pub fn set_focus(&mut self, src: &dyn ListSource, i: u32, rows: u32, now: Instant) {
        let n = src.len();
        if n == 0 {
            self.focus = 0;
            self.top = 0;
            return;
        }
        let mut f = i.min(n - 1);
        while f + 1 < n && !Self::focusable(src, f) {
            f += 1;
        }
        self.focus = f;
        self.focus_since = now;
        self.scroll_to_focus(src, rows);
    }

    fn scroll_to_focus(&mut self, src: &dyn ListSource, rows: u32) {
        let n = src.len();
        if self.focus < self.top {
            self.top = self.focus;
            // Keep a section header above the focused row visible.
            if self.top > 0 && !Self::focusable(src, self.top - 1) {
                self.top -= 1;
            }
        } else if self.focus >= self.top + rows {
            self.top = self.focus + 1 - rows;
        }
        if n <= rows {
            self.top = 0;
        } else if self.top + rows > n {
            self.top = n - rows;
        }
    }

    fn step(&mut self, src: &dyn ListSource, forward: bool) -> bool {
        let n = src.len();
        if n == 0 {
            return false;
        }
        let mut i = self.focus;
        for _ in 0..n {
            if forward {
                if i + 1 >= n {
                    if !self.wrap {
                        return false;
                    }
                    i = 0;
                } else {
                    i += 1;
                }
            } else if i == 0 {
                if !self.wrap {
                    return false;
                }
                i = n - 1;
            } else {
                i -= 1;
            }
            if Self::focusable(src, i) {
                self.focus = i;
                return true;
            }
        }
        false
    }

    fn page(&mut self, src: &dyn ListSource, forward: bool, rows: u32) -> bool {
        let n = src.len();
        if n == 0 {
            return false;
        }
        let old = self.focus;
        let target = if forward { (self.focus + rows).min(n - 1) } else { self.focus.saturating_sub(rows) };
        self.focus = target;
        if !Self::focusable(src, target) {
            // Move to the nearest focusable row in the paging direction,
            // else the other way.
            if !self.step(src, forward) {
                self.step(src, !forward);
            }
        }
        if forward {
            self.top = (self.top + rows).min(n.saturating_sub(rows));
        } else {
            self.top = self.top.saturating_sub(rows);
        }
        self.focus != old
    }

    fn jump_letter(&mut self, src: &dyn ListSource, forward: bool) -> bool {
        let n = src.len();
        let Some(cur) = src.letter(self.focus) else { return false };
        let mut i = self.focus;
        loop {
            if forward {
                i += 1;
                if i >= n {
                    return false;
                }
            } else {
                if i == 0 {
                    return false;
                }
                i -= 1;
            }
            match src.letter(i) {
                Some(l) if l != cur && Self::focusable(src, i) => {
                    if !forward {
                        // Land on the first row of that letter.
                        while i > 0 && src.letter(i - 1) == Some(l) {
                            i -= 1;
                        }
                    }
                    self.focus = i;
                    self.jump = Some(l);
                    return true;
                }
                _ => {}
            }
        }
    }

    /// Handle a key. `rows` is the number of visible rows.
    pub fn handle(&mut self, src: &dyn ListSource, ev: &Event, rows: u32, now: Instant) -> ListAction {
        let Event::Key(k, p) = *ev else { return ListAction::None };
        let moved = match (k, p) {
            (Key::Up, KeyPhase::Down | KeyPhase::Repeat) => self.step(src, false),
            (Key::Down, KeyPhase::Down | KeyPhase::Repeat) => self.step(src, true),
            (Key::Prev | Key::Next, KeyPhase::Up { held: false }) => self.page(src, k == Key::Next, rows),
            (Key::Prev | Key::Next, KeyPhase::Hold) => {
                if src.letter(self.focus).is_some() {
                    self.jump = src.letter(self.focus);
                }
                false
            }
            (Key::Prev | Key::Next, KeyPhase::Repeat) if self.jump.is_some() => {
                self.jump_letter(src, k == Key::Next)
            }
            (Key::Prev | Key::Next, KeyPhase::Up { held: true }) => {
                let had = self.jump.take().is_some();
                if had {
                    self.drawn.valid = false;
                }
                false
            }
            (Key::Ok, KeyPhase::Up { held: false }) => {
                if !src.is_empty()
                    && Self::focusable(src, self.focus)
                    && !matches!(src.row(self.focus), Row::Pending)
                {
                    return ListAction::Activated(self.focus);
                }
                false
            }
            _ => false,
        };
        if moved {
            self.focus_since = now;
            self.scroll_to_focus(src, rows);
            ListAction::Moved
        } else {
            ListAction::None
        }
    }

    /// True while the focused row needs ticks (marquee).
    #[must_use]
    pub fn wants_ticks(&self) -> bool {
        self.overflow > 0
    }

    /// Draw into the whole current area of `g` on background `bg`.
    pub fn draw(&mut self, src: &dyn ListSource, g: &mut Gfx<'_>, dirty: Dirty, now: Instant, bg: Role) {
        let (w, h) = g.size();
        let rows = Self::rows_for(h);
        let n = src.len();
        let scroll = n > rows;
        let row_w = if scroll { w - 4 } else { w };
        let marquee = marquee_offset(now.since(self.focus_since), self.overflow);
        let all = dirty == Dirty::All
            || !self.drawn.valid
            || self.drawn.top != self.top
            || self.drawn.len != n
            || self.drawn.jump != self.jump;
        if all {
            g.clear(bg);
            for r in 0..rows {
                self.draw_row(src, g, self.top + r, r, row_w, marquee, bg);
            }
            if scroll {
                draw_scrollbar(g, Rect::new(w - 3, 1, 2, h - 2), self.top, rows, n);
            }
        } else {
            let top = self.top;
            let vis = |i: u32| i >= top && i < top + rows;
            if self.drawn.focus != self.focus && vis(self.drawn.focus) {
                self.draw_row(src, g, self.drawn.focus, self.drawn.focus - self.top, row_w, 0, bg);
            }
            if (self.drawn.focus != self.focus || self.drawn.marquee != marquee) && vis(self.focus) {
                self.draw_row(src, g, self.focus, self.focus - self.top, row_w, marquee, bg);
            }
        }
        if let Some(l) = self.jump {
            draw_letter_box(g, l);
        }
        self.drawn =
            Drawn { valid: true, focus: self.focus, top: self.top, len: n, marquee, jump: self.jump };
    }

    #[allow(clippy::too_many_arguments)]
    fn draw_row(
        &mut self,
        src: &dyn ListSource,
        g: &mut Gfx<'_>,
        i: u32,
        slot: u32,
        w: i16,
        marquee: i16,
        bg: Role,
    ) {
        let y = i16::try_from(slot).unwrap_or(0) * ROW_H;
        let rect = Rect::new(0, y, w, ROW_H);
        g.fill_role(rect, bg);
        if i >= src.len() {
            return;
        }
        let focused = i == self.focus;
        match src.row(i) {
            Row::Header(t) => {
                let c = g.c(Role::Accent);
                let c = if g.pal.is_dark() || bg != Role::Bg { blend_text(g, c) } else { c };
                g.text_in(7, y + ROW_BASELINE, w - 14, t, FontId::UiBold, c, Align::Left);
            }
            Row::Pending => {
                let pw = 36 + i16::try_from((i * 37) % 48).unwrap_or(0);
                let c = if focused {
                    g.fill_round(Rect::new(2, y + 1, w - 4, ROW_H - 2), 4, g.c(Role::Accent));
                    g.pal.mix(Role::Accent, Role::OnAccent, 90)
                } else {
                    g.pal.mix(bg, Role::TextDim, 70)
                };
                g.fill_round(Rect::new(7, y + 6, pw.min(w - 20), 7), 3, c);
            }
            Row::Item(it) => {
                let (fg, dim) = if focused {
                    g.fill_round(Rect::new(2, y + 1, w - 4, ROW_H - 2), 4, g.c(Role::Accent));
                    (g.c(Role::OnAccent), g.pal.mix(Role::Accent, Role::OnAccent, 200))
                } else if it.dim {
                    (g.c(Role::TextDim), g.c(Role::TextDim))
                } else {
                    (g.c(Role::Text), g.c(Role::TextDim))
                };
                let mut x = 7;
                if let Some(ic) = it.icon {
                    let iy = y + (ROW_H - i16::from(ic.h)) / 2;
                    g.icon(6, iy, ic, if focused { fg } else { dim });
                    x = 22;
                }
                let mut right = w - 7;
                right = draw_accessory(g, it.accessory, right, y, focused);
                if let Some(v) = it.value {
                    let vw = g.measure(v, FontId::Ui).min((right - x) / 2);
                    g.text_in(right - vw, y + ROW_BASELINE, vw, v, FontId::Ui, dim, Align::Right);
                    right -= vw + 6;
                }
                let avail = right - x;
                if focused {
                    let tw = g.measure(it.text, FontId::Ui);
                    self.overflow = (tw - avail).max(0);
                    let off = marquee.min(self.overflow);
                    if off > 0 {
                        let clip = Rect::new(x, y, avail, ROW_H);
                        g.with_clip(clip, |g| g.text(x - off, y + ROW_BASELINE, it.text, FontId::Ui, fg));
                    } else {
                        // At rest (before the marquee starts) long text
                        // ends in an ellipsis like unfocused rows.
                        g.text_in(x, y + ROW_BASELINE, avail, it.text, FontId::Ui, fg, Align::Left);
                    }
                } else {
                    g.text_in(x, y + ROW_BASELINE, avail, it.text, FontId::Ui, fg, Align::Left);
                }
            }
        }
        if focused && !matches!(src.row(i), Row::Item(_)) {
            self.overflow = 0;
        }
    }
}

fn blend_text(g: &Gfx<'_>, accent: u16) -> u16 {
    // On dark backgrounds the accent is lifted towards the text colour so a
    // header stays readable (accents are tuned for white text on them).
    crate::color::blend(accent, g.c(Role::Text), 110)
}

/// Draw a row accessory right-aligned at `right`; returns the new right edge.
fn draw_accessory(g: &mut Gfx<'_>, a: Accessory, right: i16, y: i16, focused: bool) -> i16 {
    let on_acc = g.c(Role::OnAccent);
    match a {
        Accessory::None => right,
        Accessory::Chevron => {
            let c = if focused { on_acc } else { g.c(Role::TextDim) };
            g.icon(right - 4, y + 6, &icons::CHEVRON, c);
            right - 4 - 6
        }
        Accessory::Toggle(on) => {
            let tr = Rect::new(right - 20, y + 4, 20, 11);
            let (track, knob) = match (focused, on) {
                (false, true) => (g.c(Role::Accent), on_acc),
                (false, false) => (g.pal.mix(Role::Bg, Role::TextDim, 150), on_acc),
                (true, true) => (on_acc, g.c(Role::Accent)),
                (true, false) => (g.pal.mix(Role::Accent, Role::OnAccent, 90), on_acc),
            };
            g.fill_round(tr, 5, track);
            let kx = if on { tr.right() - 10 } else { tr.x + 1 };
            g.fill_disc(Rect::new(kx, tr.y + 1, 9, 9), knob);
            tr.x - 6
        }
        Accessory::Radio(sel) => {
            let r = Rect::new(right - 12, y + 3, 12, 12);
            let ring = if focused {
                on_acc
            } else if sel {
                g.c(Role::Accent)
            } else {
                g.c(Role::TextDim)
            };
            g.stroke_round(r, 6, ring);
            if sel {
                g.fill_disc(Rect::new(r.x + 3, r.y + 3, 6, 6), ring);
            }
            r.x - 6
        }
        Accessory::Check(on) => {
            if on {
                let c = if focused { on_acc } else { g.c(Role::Accent) };
                g.icon(right - 10, y + 6, &icons::CHECK, c);
            }
            right - 10 - 6
        }
    }
}

/// 2 px scroll indicator (05 §4).
pub fn draw_scrollbar(g: &mut Gfx<'_>, track: Rect, top: u32, rows: u32, n: u32) {
    if n == 0 {
        return;
    }
    let tc = g.pal.mix(Role::Bg, Role::TextDim, 60);
    g.fill_round(track, 1, tc);
    let th = i64::from(track.h);
    let hh = (th * i64::from(rows) / i64::from(n)).max(6);
    let span = th - hh;
    let max_top = i64::from(n.saturating_sub(rows)).max(1);
    let y = span * i64::from(top) / max_top;
    let thumb =
        Rect::new(track.x, track.y + i16::try_from(y).unwrap_or(0), track.w, i16::try_from(hh).unwrap_or(6));
    let c = g.c(Role::TextDim);
    g.fill_round(thumb, 1, c);
}

fn draw_letter_box(g: &mut Gfx<'_>, l: char) {
    let (w, h) = g.size();
    let r = Rect::new((w - 30) / 2, (h - 30) / 2, 30, 30);
    g.fill_round(r, 7, g.c(Role::Accent));
    let mut buf = [0u8; 4];
    let s = l.encode_utf8(&mut buf);
    let c = g.c(Role::OnAccent);
    g.text_in(r.x, r.y + 20, r.w, s, FontId::UiBold, c, Align::Center);
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Src;
    impl ListSource for Src {
        fn len(&self) -> u32 {
            20
        }
        fn row(&self, i: u32) -> Row<'_> {
            if i.is_multiple_of(7) { Row::Header("H") } else { Row::Item(Item::new("x")) }
        }
        fn letter(&self, i: u32) -> Option<char> {
            Some(char::from(b'A' + u8::try_from(i / 4).unwrap()))
        }
    }

    fn key(k: Key) -> Event {
        Event::Key(k, KeyPhase::Down)
    }

    #[test]
    fn navigation_skips_headers() {
        let mut l = ListView::new();
        l.set_focus(&Src, 0, 5, Instant(0));
        assert_eq!(l.focus, 1);
        assert_eq!(l.top, 0);
        for _ in 0..5 {
            l.handle(&Src, &key(Key::Down), 5, Instant(0));
        }
        assert_eq!(l.focus, 6);
        l.handle(&Src, &key(Key::Down), 5, Instant(0));
        assert_eq!(l.focus, 8, "skips header at 7");
        assert_eq!(l.top, 4);
        // Back up across the header: the header stays in view.
        l.handle(&Src, &key(Key::Up), 5, Instant(0));
        assert_eq!(l.focus, 6);
        // Bottom: no wrap.
        for _ in 0..40 {
            l.handle(&Src, &key(Key::Down), 5, Instant(0));
        }
        assert_eq!(l.focus, 19);
        assert_eq!(l.top, 15);
        l.wrap = true;
        l.handle(&Src, &key(Key::Down), 5, Instant(0));
        assert_eq!(l.focus, 1);
    }

    #[test]
    fn paging_and_letters() {
        let mut l = ListView::new();
        l.set_focus(&Src, 0, 5, Instant(0));
        l.handle(&Src, &Event::Key(Key::Next, KeyPhase::Up { held: false }), 5, Instant(0));
        assert_eq!(l.focus, 6);
        l.handle(&Src, &Event::Key(Key::Next, KeyPhase::Hold), 5, Instant(0));
        assert_eq!(l.jump, Some('B'));
        l.handle(&Src, &Event::Key(Key::Next, KeyPhase::Repeat), 5, Instant(0));
        assert_eq!((l.focus, l.jump), (8, Some('C')));
        l.handle(&Src, &Event::Key(Key::Next, KeyPhase::Up { held: true }), 5, Instant(0));
        assert_eq!(l.jump, None);
    }

    #[test]
    fn activation() {
        let mut l = ListView::new();
        l.set_focus(&Src, 3, 5, Instant(0));
        let a = l.handle(&Src, &Event::Key(Key::Ok, KeyPhase::Up { held: false }), 5, Instant(0));
        assert_eq!(a, ListAction::Activated(3));
    }

    #[test]
    fn marquee() {
        assert_eq!(marquee_offset(500, 30), 0);
        assert_eq!(marquee_offset(800 + 500, 30), 15);
        assert_eq!(marquee_offset(800 + 1000 + 500, 30), 30);
        assert_eq!(marquee_offset(800 + 1000, 0), 0);
    }
}
