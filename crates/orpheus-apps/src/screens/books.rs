//! The Books app's screens (05 §7.15, BOOK-1; feature `books`): the
//! library, the reader, the reader menu (contents, bookmarks, go to, the
//! reading settings), the contents, the bookmarks and Go to.
//!
//! The model is the service's (`BooksService::books`, an
//! `orpheus_books::shelf::Shelf`): screens hold only their focus, so any
//! of them is rebuilt from it after being covered.
//!
//! Reader keys (05 §7.15): Next/Prev turn pages, hold them for the next or
//! previous chapter, Up/Down move by a line, OK opens the reader menu,
//! Menu the quick menu (bookmark, contents, bookmarks, go to), Back closes
//! the book (its position is kept).

use orpheus_books::files::{Bookmark, PROGRESS_NEW};
use orpheus_books::layout::{self, Align, Line, Mark, PageInfo, Skip};
use orpheus_books::norm::{Item, Kind, item_at};
use orpheus_books::shelf::{LibPhase, Library, Phase, Req, Shelf};
use orpheus_ui::color::Color;
use orpheus_ui::event::{Event, Instant, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::{Align as TextAlign, Gfx};
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::list::{Accessory, Item as ListItem, ListAction, ListSource, ListView, Row};

use super::rows::Rows;
use super::{BOOK_GOTO, BOOK_MARKS, BOOK_MENU, BOOK_READ, BOOK_TOC, BOOKS, CHOICE};
use crate::books::{self, Face, TURN_SECS};
use crate::services::{Services, other};
use crate::settings::{Kind as SettingKind, format_value};
use crate::strings::{Lang, Str};

/// Rows visible in the content area.
const VISIBLE: u32 = 5;

/// Which list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BookList {
    Library,
    Menu,
    Contents,
    Bookmarks,
}

fn push_permille<const N: usize>(b: &mut StrBuf<N>, pm: u16) {
    b.push_u32(u32::from(pm.min(1000) / 10));
    b.push_str(" %");
}

/// The library rows as a list (titles, progress).
struct LibSrc<'a> {
    lib: &'a Library,
    vals: [StrBuf<8>; 8],
    first: u32,
}

impl<'a> LibSrc<'a> {
    fn new(lib: &'a Library, top: u32, lang: Lang) -> LibSrc<'a> {
        let mut vals = [StrBuf::new(); 8];
        for (k, v) in vals.iter_mut().enumerate() {
            if let Some(r) = lib.row(u16::try_from(top + k as u32).unwrap_or(u16::MAX)) {
                if r.progress == PROGRESS_NEW {
                    v.push_str(lang.get(Str::BooksNew));
                } else {
                    push_permille(v, r.progress);
                }
            }
        }
        LibSrc { lib, vals, first: top }
    }
}

impl ListSource for LibSrc<'_> {
    fn len(&self) -> u32 {
        u32::from(self.lib.count)
    }
    fn row(&self, i: u32) -> Row<'_> {
        let Some(r) = self.lib.row(u16::try_from(i).unwrap_or(u16::MAX)) else { return Row::Pending };
        let mut it = ListItem::new(r.title.as_str());
        if let Some(v) = i.checked_sub(self.first).and_then(|k| self.vals.get(k as usize)) {
            it = it.value(v.as_str());
        }
        if r.progress == PROGRESS_NEW {
            it = it.accessory(Accessory::None);
        }
        Row::Item(it)
    }
}

/// The contents as a list (titles indented by level, where they are).
struct TocSrc<'a> {
    shelf: &'a Shelf,
    labels: [StrBuf<52>; 6],
    vals: [StrBuf<8>; 6],
    first: u32,
}

impl<'a> TocSrc<'a> {
    fn new(shelf: &'a Shelf, top: u32) -> TocSrc<'a> {
        let mut labels = [StrBuf::new(); 6];
        let mut vals = [StrBuf::new(); 6];
        for k in 0..6 {
            if let Some(e) = shelf.book.toc_row(top + k as u32) {
                for _ in 1..e.level {
                    labels[k].push_str("  ");
                }
                labels[k].push_str_or_ellipsis(e.title());
                push_permille(&mut vals[k], books_permille(e.offset, shelf.book.text_len));
            }
        }
        TocSrc { shelf, labels, vals, first: top }
    }
}

fn books_permille(a: u32, b: u32) -> u16 {
    if b == 0 { 0 } else { u16::try_from(u64::from(a) * 1000 / u64::from(b)).unwrap_or(1000) }
}

impl ListSource for TocSrc<'_> {
    fn len(&self) -> u32 {
        self.shelf.book.toc_count
    }
    fn row(&self, i: u32) -> Row<'_> {
        let Some(k) = i.checked_sub(self.first).map(|k| k as usize).filter(|&k| k < 6) else {
            return Row::Pending;
        };
        if self.shelf.book.toc_row(i).is_none() {
            return Row::Pending;
        }
        Row::Item(ListItem::new(self.labels[k].as_str()).value(self.vals[k].as_str()))
    }
}

/// The bookmarks as a list (their first words, where they are).
struct MarkSrc<'a> {
    marks: &'a [Bookmark],
    vals: [StrBuf<8>; orpheus_books::files::MAX_BOOKMARKS],
}

impl<'a> MarkSrc<'a> {
    fn new(shelf: &'a Shelf) -> MarkSrc<'a> {
        let marks = shelf.book.state.bookmarks();
        let mut vals = [StrBuf::new(); orpheus_books::files::MAX_BOOKMARKS];
        for (v, m) in vals.iter_mut().zip(marks) {
            push_permille(v, books_permille(m.mark.offset, shelf.book.text_len));
        }
        MarkSrc { marks, vals }
    }
}

impl ListSource for MarkSrc<'_> {
    fn len(&self) -> u32 {
        self.marks.len() as u32
    }
    fn row(&self, i: u32) -> Row<'_> {
        let Some(m) = self.marks.get(i as usize) else { return Row::Pending };
        let label = if m.snippet.is_empty() { "…" } else { m.snippet.as_str() };
        Row::Item(ListItem::new(label).value(self.vals[i as usize].as_str()))
    }
}

const T_CONTENTS: u16 = 1;
const T_BOOKMARKS: u16 = 2;
const T_ADD: u16 = 3;
const T_GOTO: u16 = 4;
const T_READ_ALOUD: u16 = 5;
const T_PAGE: u16 = 6;
const M_DELETE: u16 = 0x20;
const M_SEARCH: u16 = 0x21;
const M_ADD: u16 = 0x22;
const M_CONTENTS: u16 = 0x23;
const M_MARKS: u16 = 0x24;
const M_GOTO: u16 = 0x25;
/// Library menu › Find books in, and its two choices.
const M_FIND: u16 = 0x26;
const M_FIND_0: u16 = 0x27;

/// The library, the reader menu, the contents and the bookmarks.
pub struct BookListScreen {
    kind: BookList,
    list: ListView,
    rows: Rows,
    lang: Lang,
    /// The window asked for (library rows, contents rows).
    asked: Option<u32>,
}

impl BookListScreen {
    #[must_use]
    pub fn new(kind: BookList) -> BookListScreen {
        BookListScreen {
            kind,
            list: ListView::new(),
            rows: Rows::default(),
            lang: Lang::ENGLISH,
            asked: None,
        }
    }

    #[must_use]
    pub fn screen_id(&self) -> ScreenId {
        match self.kind {
            BookList::Library => BOOKS,
            BookList::Menu => BOOK_MENU,
            BookList::Contents => BOOK_TOC,
            BookList::Bookmarks => BOOK_MARKS,
        }
    }

    #[must_use]
    pub fn name(&self) -> &'static str {
        match self.kind {
            BookList::Library => "Books",
            BookList::Menu => "BookMenu",
            BookList::Contents => "BookContents",
            BookList::Bookmarks => "BookBookmarks",
        }
    }

    fn title(&self) -> &'static str {
        self.lang.get(match self.kind {
            BookList::Library | BookList::Menu => Str::AppBooks,
            BookList::Contents => Str::BooksContents,
            BookList::Bookmarks => Str::BooksBookmarks,
        })
    }

    /// The reader menu's rows.
    fn refresh<S: Services + ?Sized>(&mut self, svc: &S) {
        self.lang = svc.lang();
        if self.kind != BookList::Menu {
            return;
        }
        let l = self.lang;
        let b = &svc.books().book;
        let mut rows = Rows::default();
        if let Some(r) = rows.push(l.get(Str::BooksPage), T_PAGE) {
            match b.page_no {
                Some(n) => {
                    r.value.push_u32(n + 1);
                    if b.complete {
                        r.value.push_str(" / ");
                        r.value.push_u32(b.pages);
                    }
                }
                None => {
                    r.value.push('…');
                }
            }
            r.value.push_str(" · ");
            push_permille(&mut r.value, b.read_permille());
            r.dim = true;
        }
        if let Some(r) = rows.push(l.get(Str::BooksContents), T_CONTENTS) {
            r.acc = Accessory::Chevron;
            r.dim = b.toc_count == 0;
        }
        if let Some(r) = rows.push(l.get(Str::BooksBookmarks), T_BOOKMARKS) {
            r.acc = Accessory::Chevron;
            r.dim = b.state.n == 0;
        }
        rows.push(l.get(Str::BooksAddBookmark), T_ADD);
        if let Some(r) = rows.push(l.get(Str::BooksGoTo), T_GOTO) {
            r.acc = Accessory::Chevron;
        }
        for s in books::reader_settings() {
            let v = svc.setting(s.id);
            let Some(r) = rows.push(l.get(s.label), s.id.0) else { break };
            match s.kind {
                SettingKind::Toggle => r.acc = Accessory::Toggle(v != 0),
                _ => {
                    format_value(s, v, l, &mut r.value);
                    r.acc = Accessory::Chevron;
                }
            }
        }
        if let Some(r) = rows.push(l.get(Str::BooksReadAloud), T_READ_ALOUD) {
            r.dim = true;
        }
        self.rows = rows;
    }

    /// Ask for the rows of the window the list shows.
    fn want<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let top = self.list.top;
        let need = match self.kind {
            BookList::Library => {
                let lib = &cx.svc.books().lib;
                (lib.phase == LibPhase::Ready && (0..VISIBLE).any(|k| lib.row((top + k) as u16).is_none()))
                    .then_some(top)
            }
            BookList::Contents => {
                let b = &cx.svc.books().book;
                let end = (top + VISIBLE).min(b.toc_count);
                (top..end).any(|i| b.toc_row(i).is_none()).then_some(top)
            }
            _ => None,
        };
        if let Some(first) = need
            && self.asked != Some(first)
        {
            self.asked = Some(first);
            match self.kind {
                BookList::Library => cx.svc.books_request(Req::Rows(first as u16)),
                _ => cx.svc.books_request(Req::TocRows(first)),
            }
        }
    }

    fn handle_list<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> ListAction {
        let mut l = self.list;
        let lc = cx.list();
        let act = match self.kind {
            BookList::Menu => l.handle(&self.rows, ev, VISIBLE, lc),
            BookList::Library => {
                let shelf = cx.svc.books();
                l.handle(&LibSrc::new(&shelf.lib, l.top, self.lang), ev, VISIBLE, lc)
            }
            BookList::Contents => l.handle(&TocSrc::new(cx.svc.books(), l.top), ev, VISIBLE, lc),
            BookList::Bookmarks => l.handle(&MarkSrc::new(cx.svc.books()), ev, VISIBLE, lc),
        };
        self.list = l;
        act
    }

    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, i: u32) -> Nav {
        match self.kind {
            BookList::Library => {
                if cx.svc.books().lib.row(i as u16).is_none() {
                    return Nav::Stay;
                }
                books::send_params(cx.svc);
                cx.svc.books_request(Req::Open(i as u16));
                Nav::Push(BOOK_READ, Arg(i))
            }
            BookList::Contents => {
                cx.svc.books_request(Req::Toc(i));
                Nav::PopTo(BOOK_READ)
            }
            BookList::Bookmarks => {
                cx.svc.books_request(Req::Bookmark(i as u8));
                Nav::PopTo(BOOK_READ)
            }
            BookList::Menu => {
                let Some(tag) = self.rows.get(i).map(|r| r.tag) else { return Nav::Stay };
                match tag {
                    T_CONTENTS if cx.svc.books().book.toc_count > 0 => Nav::Push(BOOK_TOC, Arg(0)),
                    T_CONTENTS => {
                        cx.ui.toast(self.lang.get(Str::BooksNoContents));
                        Nav::Stay
                    }
                    T_BOOKMARKS if cx.svc.books().book.state.n > 0 => Nav::Push(BOOK_MARKS, Arg(0)),
                    T_BOOKMARKS => {
                        cx.ui.toast(self.lang.get(Str::BooksNoBookmarks));
                        Nav::Stay
                    }
                    T_ADD => {
                        add_bookmark(cx, self.lang);
                        Nav::Pop
                    }
                    T_GOTO => Nav::Push(BOOK_GOTO, Arg(0)),
                    T_READ_ALOUD => {
                        cx.ui.toast(self.lang.get(Str::NotYet));
                        Nav::Stay
                    }
                    T_PAGE => Nav::Stay,
                    key => {
                        let sid = crate::settings::SettingId(key);
                        let Some(d) = books::reader_settings().iter().find(|s| s.id == sid) else {
                            return Nav::Stay;
                        };
                        if d.kind == SettingKind::Toggle {
                            let v = d.step(cx.svc.setting(sid), 1);
                            cx.svc.set_setting(sid, v);
                            books::send_params(cx.svc);
                            self.refresh(cx.svc);
                            self.list.invalidate();
                            Nav::Stay
                        } else {
                            Nav::Push(CHOICE, Arg(u32::from(key)))
                        }
                    }
                }
            }
        }
    }
}

/// Bookmark the reader's first shown line.
fn add_bookmark<S: Services + ?Sized>(cx: &mut Cx<'_, S>, lang: Lang) {
    let Some((info, _)) = view(cx.svc, None) else { return };
    let Some(first) = info.first else { return };
    let before = cx.svc.books().book.state.n;
    cx.svc.books_request(Req::AddBookmark(first));
    let after = &cx.svc.books().book.state;
    let full = after.n == before && !after.bookmarks().iter().any(|b| b.mark.offset == first.offset);
    cx.ui.toast(lang.get(if full { Str::BooksBookmarksFull } else { Str::BooksBookmarkAdded }));
}

impl<S: Services + ?Sized> Screen<S> for BookListScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        if self.kind == BookList::Library {
            let scope = books::scope(cx.svc);
            cx.svc.books_request(Req::Library(scope));
        }
        self.refresh(cx.svc);
        let mut l = self.list;
        match self.kind {
            BookList::Menu => l.set_focus(&self.rows, 1, VISIBLE, cx.now),
            _ => {
                l.focus = 0;
                l.top = 0;
            }
        }
        self.list = l;
        self.asked = None;
        self.want(cx);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Service(ServiceEvent::Other { kind: other::BOOKS, .. }) => {
                self.refresh(cx.svc);
                self.asked = None;
                self.want(cx);
                self.list.invalidate();
                cx.ui.redraw();
                return Nav::Stay;
            }
            Event::Service(ServiceEvent::SettingChanged(k)) => {
                if self.kind == BookList::Library && (k == books::id::FIND_IN.0 || k == books::id::WORD.0) {
                    let scope = books::scope(cx.svc);
                    cx.svc.books_request(Req::Library(scope));
                }
                self.refresh(cx.svc);
                self.list.invalidate();
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id: M_SEARCH, .. }) => {
                let scope = books::scope(cx.svc);
                cx.svc.books_request(Req::Rescan(scope));
                self.list.focus = 0;
                self.list.top = 0;
                self.list.invalidate();
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id, .. }) if (M_FIND_0..M_FIND_0 + 2).contains(&id) => {
                // Changed: the library is searched again (SettingChanged).
                cx.svc.set_setting(books::id::FIND_IN, i16::try_from(id - M_FIND_0).unwrap_or(0));
                self.list.focus = 0;
                self.list.top = 0;
                self.list.invalidate();
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id: M_DELETE, .. }) => {
                cx.svc.books_request(Req::DeleteBookmark(self.list.focus as u8));
                let n = u32::from(cx.svc.books().book.state.n);
                if n == 0 {
                    return Nav::Pop;
                }
                self.list.focus = self.list.focus.min(n - 1);
                self.list.top = self.list.top.min(self.list.focus);
                self.list.invalidate();
                return Nav::Stay;
            }
            _ => {}
        }
        let act = self.handle_list(cx, ev);
        self.want(cx);
        match act {
            ListAction::Activated(i) => self.activate(cx, i),
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut l = self.list;
        match self.kind {
            BookList::Menu => l.draw(&self.rows, g, dirty, cx.now, Role::Bg),
            BookList::Library => {
                let lib = &cx.svc.books().lib;
                if lib.phase != LibPhase::Ready || lib.count == 0 {
                    draw_library_state(cx.svc, lib, g, self.lang);
                } else {
                    l.draw(&LibSrc::new(lib, l.top, self.lang), g, dirty, cx.now, Role::Bg);
                }
            }
            BookList::Contents => l.draw(&TocSrc::new(cx.svc.books(), l.top), g, dirty, cx.now, Role::Bg),
            BookList::Bookmarks => l.draw(&MarkSrc::new(cx.svc.books()), g, dirty, cx.now, Role::Bg),
        }
        self.list = l;
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        let l = self.lang;
        if out.parent() == Some(M_FIND) {
            out.title(l.get(Str::BooksFindIn));
            let cur = cx.svc.setting(books::id::FIND_IN);
            out.radio(M_FIND_0, l.get(Str::BooksFolder), cur == 0);
            out.radio(M_FIND_0 + 1, l.get(Str::BooksWholeCard), cur == 1);
            return;
        }
        if out.parent().is_some() {
            return;
        }
        match self.kind {
            BookList::Library => {
                out.action(M_SEARCH, l.get(Str::BooksSearchAgain));
                out.sub(M_FIND, l.get(Str::BooksFindIn));
            }
            BookList::Bookmarks => {
                out.action(M_DELETE, self.lang.get(Str::BooksDeleteBookmark));
            }
            _ => {}
        }
    }

    fn wants(&self) -> Wants {
        Wants { ticks: self.list.wants_ticks(), title: Some(self.title()), ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: self.list.focus, top: self.list.top, ..Resume::default() }
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        let mut l = self.list;
        l.top = r.top;
        l.focus = r.focus;
        if self.kind == BookList::Menu {
            l.set_focus(&self.rows, r.focus, VISIBLE, cx.now);
        }
        self.list = l;
        self.want(cx);
    }
}

/// The library while it is read or searched, or when it is empty.
fn draw_library_state<S: Services + ?Sized>(svc: &S, lib: &Library, g: &mut Gfx<'_>, lang: Lang) {
    let (w, h) = g.size();
    g.clear(Role::Bg);
    let text = g.c(Role::Text);
    let dim = g.c(Role::TextDim);
    let mut head = StrBuf::<48>::new();
    let help = match lib.phase {
        LibPhase::Ready => {
            head.push_str(lang.get(Str::BooksNone));
            if books::scope(svc) == orpheus_books::shelf::Scope::WholeCard {
                Str::BooksNoneCard
            } else {
                Str::BooksNoneFolder
            }
        }
        _ => {
            head.push_str(lang.get(Str::BooksSearching));
            head.push('…');
            if lib.found > 0 {
                head.push_str(" ");
                head.push_u32(u32::from(lib.found));
            }
            Str::BooksFolder
        }
    };
    g.text_in(0, 30, w, head.as_str(), FontId::UiBold, text, TextAlign::Center);
    if lib.phase == LibPhase::Ready {
        let mut y = 52;
        g.wrap(lang.get(help), FontId::Ui, w - 16, |g, line| {
            g.text_in(0, y, w, line, FontId::Ui, dim, TextAlign::Center);
            y += 14;
        });
    } else {
        // A small moving bar: found so far.
        let track = Rect::new(24, h / 2 + 8, w - 48, 3);
        let c = g.pal.mix(Role::Bg, Role::TextDim, 60);
        g.fill_round(track, 1, c);
        let k = i16::try_from(lib.found % 8).unwrap_or(0);
        g.fill_round(Rect::new(track.x + k * (track.w - 16) / 8, track.y, 16, 3), 1, dim);
    }
}

/// The reader: a page of the open book from the service's window.
#[derive(Default)]
pub struct ReaderScreen {
    lang: Lang,
    /// The last view drawn (its top) and whether the frame must be drawn.
    stale: bool,
    /// The first line sent as the reading position.
    seen: Option<Mark>,
    /// Auto page turn: when the page was turned last, and whether it is
    /// on (the screen asks for ticks only then).
    turned: Instant,
    ticking: bool,
}

/// One page of the view: its info and the second shown line's mark (a
/// line down). With `draw`, it is drawn too. `None` while the window is
/// not there.
fn view<S: Services + ?Sized>(
    svc: &S,
    draw: Option<(&mut Gfx<'_>, Color, Color)>,
) -> Option<(PageInfo, Option<Mark>)> {
    let shelf = svc.books();
    let b = &shelf.book;
    if b.phase != Phase::Ready {
        return None;
    }
    let (base, win, at_end) = b.window()?;
    let p = books::params(svc);
    let face = Face::of(svc);
    let margin = books::margin(svc);
    let mut second = None;
    let mut n = 0;
    let mut g = draw;
    let info = layout::page(win, base, at_end, b.top, b.skip, p, &face, &mut |l: &Line, top: u16| {
        if n == 1 {
            second = Some(l.mark);
        }
        n += 1;
        if let Some((g, _, fg)) = g.as_mut() {
            draw_line(g, win, base, l, top, &p, &face, margin, *fg);
        }
    });
    Some((info, second))
}

/// Draw one line at `top` (pixels from the text area's top).
#[allow(clippy::too_many_arguments)]
fn draw_line(
    g: &mut Gfx<'_>,
    win: &[u8],
    base: u32,
    l: &Line,
    top: u16,
    p: &layout::Params,
    face: &Face,
    margin: i16,
    color: Color,
) {
    let kind = l.mark.kind;
    if kind == Kind::Break {
        return;
    }
    let first = l.mark.first;
    let avail = i16::try_from(p.avail(kind, first)).unwrap_or(0);
    let extra = (avail - i16::try_from(l.width).unwrap_or(0)).max(0);
    let from = (l.mark.offset - base) as usize;
    let to = (l.end - base) as usize;
    let align = p.align(kind, l.last);
    let spaces = if align == Align::Justify {
        let mut n = 0;
        let mut i = from;
        while i < to {
            let Some((it, k)) = item_at(win, i) else { break };
            if it == Item::Char(' ') {
                n += 1;
            }
            i += k;
        }
        n
    } else {
        0
    };
    let mut x = margin
        + i16::try_from(p.left(kind, first)).unwrap_or(0)
        + match align {
            Align::Center => extra / 2,
            Align::Right => extra,
            _ => 0,
        };
    let pitch = i16::try_from(p.line).unwrap_or(16);
    let y =
        books::TOP + i16::try_from(top).unwrap_or(0) + (pitch - face.ascent - face.descent) / 2 + face.ascent;
    let mut style = l.mark.style;
    let mut sp = 0i16;
    let mut i = from;
    while i < to {
        let Some((it, k)) = item_at(win, i) else { break };
        match it {
            Item::Style(f) => style = f,
            Item::Char(c) => {
                let st = layout::draw_style(kind, style);
                if c != ' ' {
                    face.draw(g, x, y, c, st, color);
                }
                x += i16::try_from(face.advance(c, st)).unwrap_or(0);
                if c == ' ' && spaces > 0 {
                    // Justified: the extra width spread over the spaces.
                    x += extra / spaces + i16::from(sp < extra % spaces);
                    sp += 1;
                }
            }
            Item::Para(_) => {}
        }
        i += k;
    }
}

impl ReaderScreen {
    /// Send the first shown line as the reading position (when it moved).
    fn note_position<S: Services + ?Sized>(
        &mut self,
        cx: &mut Cx<'_, S>,
    ) -> Option<(PageInfo, Option<Mark>)> {
        let v = view(cx.svc, None)?;
        if let Some(first) = v.0.first
            && self.seen != Some(first)
        {
            self.seen = Some(first);
            cx.svc.books_request(Req::Seen(first));
        }
        // A view one line up starts at the shown line from now on.
        if cx.svc.books().book.skip != Skip::None
            && let Some(first) = v.0.first
        {
            cx.svc.books_request(Req::Show(first));
        }
        Some(v)
    }

    fn next_page<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        let Some((info, _)) = self.note_position(cx) else { return };
        match info.next {
            Some(m) => {
                cx.svc.books_request(Req::Show(m));
                self.turned = cx.now;
            }
            None if !info.short => cx.ui.toast(self.lang.get(Str::BooksEnd)),
            None => {}
        }
    }

    fn changed<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        self.stale = true;
        cx.ui.redraw();
    }
}

impl<S: Services + ?Sized> Screen<S> for ReaderScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        books::send_params(cx.svc);
        self.stale = true;
        self.seen = None;
        self.turned = cx.now;
        self.ticking = auto_turn(cx.svc) > 0;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => {
                self.note_position(cx);
                cx.svc.books_request(Req::Close);
                return Nav::Pop;
            }
            Event::Key(Key::Next, KeyPhase::Up { held: false }) => self.next_page(cx),
            Event::Key(Key::Prev, KeyPhase::Up { held: false }) => {
                self.note_position(cx);
                cx.svc.books_request(Req::PrevPage);
                self.turned = cx.now;
            }
            Event::Key(k @ (Key::Next | Key::Prev), KeyPhase::Hold) => {
                self.note_position(cx);
                cx.svc.books_request(Req::Chapter(k == Key::Next));
            }
            Event::Key(Key::Down, KeyPhase::Down | KeyPhase::Repeat) => {
                if let Some((_, Some(second))) = self.note_position(cx) {
                    cx.svc.books_request(Req::Show(second));
                }
            }
            Event::Key(Key::Up, KeyPhase::Down | KeyPhase::Repeat) => {
                self.note_position(cx);
                cx.svc.books_request(Req::LineUp);
            }
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                self.note_position(cx);
                return Nav::Push(BOOK_MENU, Arg(0));
            }
            Event::Ui(UiEvent::Menu { id, .. }) => {
                self.note_position(cx);
                match id {
                    M_ADD => add_bookmark(cx, self.lang),
                    M_CONTENTS if cx.svc.books().book.toc_count > 0 => return Nav::Push(BOOK_TOC, Arg(0)),
                    M_CONTENTS => cx.ui.toast(self.lang.get(Str::BooksNoContents)),
                    M_MARKS if cx.svc.books().book.state.n > 0 => return Nav::Push(BOOK_MARKS, Arg(0)),
                    M_MARKS => cx.ui.toast(self.lang.get(Str::BooksNoBookmarks)),
                    M_GOTO => return Nav::Push(BOOK_GOTO, Arg(0)),
                    _ => {}
                }
            }
            Event::Tick(now) => {
                let secs = auto_turn(cx.svc);
                if secs > 0 && now.since(self.turned) >= u64::from(secs) * 1000 {
                    self.turned = now;
                    self.next_page(cx);
                }
                return Nav::Stay;
            }
            Event::Service(ServiceEvent::Other { kind: other::BOOKS, .. }) => {
                if cx.svc.books().book.skip != Skip::None {
                    self.note_position(cx);
                }
            }
            Event::Service(ServiceEvent::SettingChanged(k)) if books::is_reading_setting(k) => {
                books::send_params(cx.svc);
                self.ticking = auto_turn(cx.svc) > 0;
            }
            _ => return Nav::Stay,
        }
        self.changed(cx);
        Nav::Stay
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && !self.stale {
            return;
        }
        self.stale = false;
        let (bg, fg) = books::colours(cx.svc, g);
        let (w, h) = g.size();
        g.fill(Rect::new(0, 0, w, h), bg);
        let b = &cx.svc.books().book;
        let dim = orpheus_ui::color::blend(bg, fg, 110);
        match b.phase {
            Phase::Ready => {}
            Phase::Converting | Phase::Opening => {
                g.text_in(4, h / 2 - 10, w - 8, b.title.as_str(), FontId::UiBold, fg, TextAlign::Center);
                let prep = self.lang.get(Str::BooksPreparing);
                g.text_in(4, h / 2 + 10, w - 8, prep, FontId::Ui, dim, TextAlign::Center);
                if b.phase == Phase::Converting {
                    let mut s = StrBuf::<8>::new();
                    push_permille(&mut s, b.permille);
                    g.text_in(0, h / 2 + 26, w, s.as_str(), FontId::Ui, dim, TextAlign::Center);
                }
                return;
            }
            Phase::Failed | Phase::Closed => {
                g.text_in(0, h / 2, w, self.lang.get(Str::BooksFailed), FontId::Ui, fg, TextAlign::Center);
                return;
            }
        }
        if view(cx.svc, Some((g, bg, fg))).is_none() {
            // The window is on its way: placeholder lines.
            let c = orpheus_ui::color::blend(bg, fg, 40);
            for i in 0..6 {
                let lw = 60 + (i * 23) % 50;
                g.fill_round(Rect::new(books::margin(cx.svc), 8 + i * 17, lw, 7), 3, c);
            }
        }
        // Where in the book: a bar at the right edge.
        if b.text_len > 0 {
            let track = Rect::new(w - 3, 2, 2, h - 4);
            g.fill_round(track, 1, orpheus_ui::color::blend(bg, fg, 40));
            let th = i64::from(track.h);
            let y0 = th * i64::from(b.top.offset) / i64::from(b.text_len);
            let thumb = Rect::new(track.x, track.y + i16::try_from(y0).unwrap_or(0), 2, 6.min(track.h));
            g.fill_round(thumb, 1, dim);
        }
    }

    fn menu(&self, _cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
        out.action(M_ADD, l.get(Str::BooksAddBookmark));
        out.action(M_CONTENTS, l.get(Str::BooksContents));
        out.action(M_MARKS, l.get(Str::BooksBookmarks));
        out.action(M_GOTO, l.get(Str::BooksGoTo));
    }

    fn wants(&self) -> Wants {
        Wants {
            strip: StripUse::Content,
            title: Some(self.lang.get(Str::AppBooks)),
            ticks: self.ticking,
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        Resume::default()
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, _r: &Resume) {
        self.stale = true;
        self.turned = cx.now;
        self.ticking = auto_turn(cx.svc) > 0;
    }
}

/// Seconds per page of Auto page turn (0: off).
fn auto_turn<S: Services + ?Sized>(svc: &S) -> u16 {
    TURN_SECS[usize::try_from(svc.setting(books::id::AUTO_TURN)).unwrap_or(0).min(TURN_SECS.len() - 1)]
}

/// Go to: a per cent of the book.
#[derive(Default)]
pub struct GotoScreen {
    lang: Lang,
    percent: u16,
}

impl<S: Services + ?Sized> Screen<S> for GotoScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, _arg: Arg) {
        self.lang = cx.svc.lang();
        self.percent = cx.svc.books().book.read_permille() / 10;
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let step = match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Key(Key::Ok, KeyPhase::Up { held: false }) => {
                cx.svc.books_request(Req::Permille(self.percent * 10));
                return Nav::PopTo(BOOK_READ);
            }
            Event::Key(Key::Up, KeyPhase::Down | KeyPhase::Repeat) => 1,
            Event::Key(Key::Down, KeyPhase::Down | KeyPhase::Repeat) => -1,
            Event::Key(Key::Next, KeyPhase::Down | KeyPhase::Repeat) => 10,
            Event::Key(Key::Prev, KeyPhase::Down | KeyPhase::Repeat) => -10,
            _ => return Nav::Stay,
        };
        self.percent = (i32::from(self.percent) + step).clamp(0, 100) as u16;
        cx.ui.redraw();
        Nav::Stay
    }

    fn draw(&mut self, _cx: &Cx<'_, S>, g: &mut Gfx<'_>, _dirty: Dirty) {
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let text = g.c(Role::Text);
        let mut s = StrBuf::<8>::new();
        s.push_u32(u32::from(self.percent));
        s.push('%');
        g.text_in(0, 40, w, s.as_str(), FontId::Num28, text, TextAlign::Center);
        let track = Rect::new(14, 58, w - 28, 4);
        let tc = g.pal.mix(Role::Bg, Role::TextDim, 60);
        g.fill_round(track, 2, tc);
        let fill = i16::try_from(i32::from(track.w) * i32::from(self.percent) / 100).unwrap_or(0);
        let acc = g.c(Role::Accent);
        g.fill_round(Rect::new(track.x, track.y, fill.max(4), track.h), 2, acc);
        let dim = g.c(Role::TextDim);
        g.text_in(0, h - 12, w, self.lang.get(Str::BooksHintGoTo), FontId::Ui, dim, TextAlign::Center);
    }

    fn wants(&self) -> Wants {
        Wants { title: Some(self.lang.get(Str::BooksGoTo)), ..Wants::default() }
    }

    fn save(&self) -> Resume {
        Resume { focus: u32::from(self.percent), ..Resume::default() }
    }

    fn restore(&mut self, _cx: &mut Cx<'_, S>, r: &Resume) {
        self.percent = u16::try_from(r.focus).unwrap_or(0).min(100);
    }
}
