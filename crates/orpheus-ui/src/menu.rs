//! Context menus (05 §5.2 `Menu`): a list in a bottom sheet over the dimmed
//! screen. Screens describe items into a [`MenuBuilder`]; the shell owns the
//! one open [`MenuSheet`].

use crate::event::{Event, Instant, Key, KeyPhase};
use crate::font::{FontId, TextSize};
use crate::geom::Rect;
use crate::gfx::{Align, Gfx, SCREEN_H, SCREEN_W};
use crate::nav::Dirty;
use crate::text::StrBuf;
use crate::theme::Role;
use crate::widgets::list::{Accessory, Item, ListAction, ListCx, ListSource, ListView, Row, row_metrics};

/// Most items in one menu level.
pub const MENU_MAX: usize = 12;
/// Rows visible in the sheet before it scrolls.
pub const SHEET_ROWS: u32 = 4;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ItemKind {
    /// Do something and close the menu.
    Action,
    /// On/off; flips in place, menu stays open.
    Toggle(bool),
    /// One of a group; chosen in place, menu stays open.
    Radio(bool),
    /// Opens a sub-menu (the screen is asked again with this id as parent).
    Sub,
    /// Shows a value and launches an editor (slider, picker); closes.
    Value(StrBuf<12>),
}

#[derive(Clone, Copy, Debug)]
pub struct MenuItem {
    pub id: u16,
    pub label: &'static str,
    pub kind: ItemKind,
}

/// Filled by [`Screen::menu`](crate::Screen::menu).
#[derive(Clone, Copy, Debug)]
pub struct MenuBuilder {
    items: [MenuItem; MENU_MAX],
    n: u8,
    parent: Option<u16>,
    title: Option<&'static str>,
}

impl MenuBuilder {
    #[must_use]
    pub const fn new(parent: Option<u16>) -> MenuBuilder {
        const I: MenuItem = MenuItem { id: 0, label: "", kind: ItemKind::Action };
        MenuBuilder { items: [I; MENU_MAX], n: 0, parent, title: None }
    }

    /// `None` at the top level, else the id of the sub-menu being built.
    #[must_use]
    pub const fn parent(&self) -> Option<u16> {
        self.parent
    }

    pub fn title(&mut self, t: &'static str) {
        self.title = Some(t);
    }

    fn add(&mut self, id: u16, label: &'static str, kind: ItemKind) -> bool {
        match self.items.get_mut(usize::from(self.n)) {
            Some(slot) => {
                *slot = MenuItem { id, label, kind };
                self.n += 1;
                true
            }
            None => false,
        }
    }

    pub fn action(&mut self, id: u16, label: &'static str) -> bool {
        self.add(id, label, ItemKind::Action)
    }

    pub fn toggle(&mut self, id: u16, label: &'static str, on: bool) -> bool {
        self.add(id, label, ItemKind::Toggle(on))
    }

    pub fn radio(&mut self, id: u16, label: &'static str, selected: bool) -> bool {
        self.add(id, label, ItemKind::Radio(selected))
    }

    pub fn sub(&mut self, id: u16, label: &'static str) -> bool {
        self.add(id, label, ItemKind::Sub)
    }

    pub fn value(&mut self, id: u16, label: &'static str, value: &str) -> bool {
        self.add(id, label, ItemKind::Value(StrBuf::from(value)))
    }

    #[must_use]
    pub fn items(&self) -> &[MenuItem] {
        self.items.get(..usize::from(self.n)).unwrap_or(&[])
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }
}

impl ListSource for MenuBuilder {
    fn len(&self) -> u32 {
        u32::from(self.n)
    }

    fn row(&self, i: u32) -> Row<'_> {
        let Some(it) = usize::try_from(i).ok().and_then(|i| self.items().get(i)) else {
            return Row::Pending;
        };
        let item = Item::new(it.label);
        Row::Item(match &it.kind {
            ItemKind::Action => item,
            ItemKind::Toggle(on) => item.accessory(Accessory::Toggle(*on)),
            ItemKind::Radio(on) => item.accessory(Accessory::Radio(*on)),
            ItemKind::Sub => item.accessory(Accessory::Chevron),
            ItemKind::Value(v) => item.value(v.as_str()).accessory(Accessory::Chevron),
        })
    }
}

/// What a key did to the open sheet.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SheetResult {
    None,
    /// Close the menu without choosing.
    Close,
    /// Deliver `UiEvent::Menu { id, value }`; then close if `close`, else
    /// rebuild the level in place.
    Pick {
        id: u16,
        value: u16,
        close: bool,
    },
    /// Rebuild with this parent (sub-menu opened, or `Back` to the parent
    /// level when `None`/the previous id).
    Level(Option<u16>),
}

/// The open context menu: the current level's items and the path to it.
pub struct MenuSheet {
    pub items: MenuBuilder,
    list: ListView,
    path: [u16; 3],
    depth: u8,
    frame_drawn: bool,
}

impl MenuSheet {
    #[must_use]
    pub fn new(items: MenuBuilder, now: Instant) -> MenuSheet {
        let mut list = ListView::new();
        list.set_focus(&items, 0, SHEET_ROWS, now);
        MenuSheet { items, list, path: [0; 3], depth: 0, frame_drawn: false }
    }

    /// Replace the items of the current level, keeping focus if possible.
    pub fn rebuild(&mut self, items: MenuBuilder, keep_focus: bool, now: Instant) {
        let f = if keep_focus { self.list.focus } else { 0 };
        let height_changed = items.len() != self.items.len() || items.title != self.items.title;
        self.items = items;
        self.list = ListView::new();
        self.list.set_focus(&self.items, f, SHEET_ROWS, now);
        if height_changed {
            self.frame_drawn = false;
        }
    }

    /// What the screen reader says of the sheet (A11Y-1): its title (or
    /// "Menu") and the focused item.
    #[cfg(feature = "screen-reader")]
    pub fn describe(&self, d: &mut crate::a11y::Desc) {
        d.layer = crate::a11y::Layer::Menu;
        d.title(self.items.title.unwrap_or("Menu"));
        d.list(&self.items, &self.list, "empty");
    }

    /// The parent id of the level shown.
    #[must_use]
    pub fn parent(&self) -> Option<u16> {
        self.depth.checked_sub(1).and_then(|d| self.path.get(usize::from(d)).copied())
    }

    /// Whether the sheet's frame must be redrawn over a fresh scrim.
    pub fn invalidate(&mut self) {
        self.frame_drawn = false;
    }

    /// Title band and rows shown at text size `size`: up to
    /// [`SHEET_ROWS`], fewer with large text (A11Y-2) so that the sheet
    /// stays below the status bar's band (at most 110 px high).
    fn layout(&self, size: TextSize) -> (i16, i16) {
        let head = match (self.items.title.is_some(), size) {
            (false, _) => 10,
            (true, TextSize::Normal) => 20,
            (true, _) => 24,
        };
        let (row_h, _) = row_metrics(size);
        let fit = u32::try_from((110 - head - 4) / row_h).unwrap_or(1).max(1);
        let rows = i16::try_from(self.items.len().min(SHEET_ROWS).min(fit)).unwrap_or(1);
        (head, rows)
    }

    /// Height of the sheet for the current level at text size `size`.
    #[must_use]
    pub fn height(&self, size: TextSize) -> i16 {
        let (head, rows) = self.layout(size);
        head + rows * row_metrics(size).0 + 4
    }

    pub fn handle(&mut self, ev: &Event, lc: ListCx) -> SheetResult {
        let Event::Key(k, p) = *ev else { return SheetResult::None };
        match (k, p) {
            (Key::Menu, KeyPhase::Up { held: false }) => return SheetResult::Close,
            (Key::Back, KeyPhase::Up { held: false }) => {
                if self.depth == 0 {
                    return SheetResult::Close;
                }
                self.depth -= 1;
                return SheetResult::Level(self.parent());
            }
            (Key::Next, KeyPhase::Up { held: false }) | (Key::Prev, KeyPhase::Up { held: false }) => {
                // Prev/Next change a value row in place where that makes
                // sense: toggles flip, radios are chosen.
                if let Some(it) = self.focused() {
                    match it.kind {
                        ItemKind::Toggle(on) => {
                            return SheetResult::Pick { id: it.id, value: u16::from(!on), close: false };
                        }
                        ItemKind::Radio(false) => {
                            return SheetResult::Pick { id: it.id, value: 1, close: false };
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        }
        match self.list.handle(&self.items, ev, SHEET_ROWS, lc) {
            ListAction::Activated(i) => {
                let Some(it) = usize::try_from(i).ok().and_then(|i| self.items.items().get(i)).copied()
                else {
                    return SheetResult::None;
                };
                match it.kind {
                    ItemKind::Action | ItemKind::Value(_) => {
                        SheetResult::Pick { id: it.id, value: 0, close: true }
                    }
                    ItemKind::Toggle(on) => {
                        SheetResult::Pick { id: it.id, value: u16::from(!on), close: false }
                    }
                    ItemKind::Radio(_) => SheetResult::Pick { id: it.id, value: 1, close: false },
                    ItemKind::Sub => {
                        if let Some(slot) = self.path.get_mut(usize::from(self.depth)) {
                            *slot = it.id;
                            self.depth += 1;
                            SheetResult::Level(Some(it.id))
                        } else {
                            SheetResult::None
                        }
                    }
                }
            }
            _ => SheetResult::None,
        }
    }

    fn focused(&self) -> Option<MenuItem> {
        usize::try_from(self.list.focus).ok().and_then(|i| self.items.items().get(i)).copied()
    }

    #[must_use]
    pub fn wants_ticks(&self) -> bool {
        self.list.wants_ticks()
    }

    /// Draw over the whole screen (`g` at panel coordinates).
    pub fn draw(&mut self, g: &mut Gfx<'_>, now: Instant) {
        let size = g.text;
        let h = self.height(size);
        let y = SCREEN_H - h;
        let dirty = if self.frame_drawn { Dirty::Changed } else { Dirty::All };
        if !self.frame_drawn {
            // Rounded top corners; the bottom corners fall off the panel.
            let surf = g.c(Role::Surface);
            g.fill_round(Rect::new(0, y, SCREEN_W, h + 10), 9, surf);
            match self.items.title {
                Some(t) => {
                    let c = g.c(Role::TextDim);
                    // The title band holds the 16 px face at most.
                    let (f, b) = match size {
                        TextSize::Normal => (FontId::UiBold, 14),
                        _ => (FontId::LargeBold, 17),
                    };
                    g.text_in(8, y + b, SCREEN_W - 16, t, f, c, Align::Center);
                }
                None => {
                    let c = g.pal.mix(Role::Surface, Role::TextDim, 120);
                    g.fill_round(Rect::new((SCREEN_W - 20) / 2, y + 4, 20, 3), 1, c);
                }
            }
            self.frame_drawn = true;
        }
        let (head, rows) = self.layout(size);
        let area = Rect::new(2, y + head, SCREEN_W - 4, rows * row_metrics(size).0);
        let items = &self.items;
        let list = &mut self.list;
        g.with_area(area, |g| list.draw(items, g, dirty, now, Role::Surface));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ON: ListCx = ListCx::new(Instant(0));

    fn tap(k: Key) -> Event {
        Event::Key(k, KeyPhase::Up { held: false })
    }

    #[test]
    fn picks_and_levels() {
        let mut b = MenuBuilder::new(None);
        b.action(1, "Play");
        b.toggle(2, "Shuffle", false);
        b.sub(3, "Repeat");
        let mut s = MenuSheet::new(b, Instant(0));
        assert_eq!(s.handle(&tap(Key::Ok), ON), SheetResult::Pick { id: 1, value: 0, close: true });
        s.handle(&Event::Key(Key::Down, KeyPhase::Down), ON);
        assert_eq!(s.handle(&tap(Key::Ok), ON), SheetResult::Pick { id: 2, value: 1, close: false });
        assert_eq!(s.handle(&tap(Key::Next), ON), SheetResult::Pick { id: 2, value: 1, close: false });
        s.handle(&Event::Key(Key::Down, KeyPhase::Down), ON);
        assert_eq!(s.handle(&tap(Key::Ok), ON), SheetResult::Level(Some(3)));
        assert_eq!(s.parent(), Some(3));
        assert_eq!(s.handle(&tap(Key::Back), ON), SheetResult::Level(None));
        assert_eq!(s.handle(&tap(Key::Back), ON), SheetResult::Close);
    }

    #[test]
    fn sheet_rolls_round_with_the_setting() {
        let mut b = MenuBuilder::new(None);
        for i in 0..6 {
            b.action(i, "x");
        }
        let mut s = MenuSheet::new(b, Instant(0));
        let up = Event::Key(Key::Up, KeyPhase::Down);
        s.handle(&up, ListCx::clamped(Instant(0)));
        assert_eq!(s.focused().map(|i| i.id), Some(0), "off: the top clamps");
        s.handle(&up, ON);
        assert_eq!(s.focused().map(|i| i.id), Some(5), "on: Up on the first item goes to the last");
        s.handle(&Event::Key(Key::Down, KeyPhase::Down), ON);
        assert_eq!(s.focused().map(|i| i.id), Some(0));
    }

    #[test]
    fn capacity() {
        let mut b = MenuBuilder::new(None);
        for i in 0..20 {
            b.action(i, "x");
        }
        assert_eq!(b.items().len(), MENU_MAX);
        if cfg!(target_pointer_width = "32") {
            assert!(core::mem::size_of::<MenuSheet>() < 600);
        }
    }
}
