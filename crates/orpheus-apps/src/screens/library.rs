//! Music › Artists / Albums / Songs / Genres / Recently added (05 §7.1): a
//! windowed list over the `LibraryService`, one screen per level (Artists →
//! an artist's albums → an album's tracks), so Back goes up and the resume
//! record brings the focus back. OK on an artist, album or genre opens it;
//! OK on a track plays the list from there. Menu: Play / Shuffle the
//! focused item (or the list), Play all of the level.

use orpheus_ui::event::{Event, Key, KeyPhase, ServiceEvent, UiEvent};
use orpheus_ui::font::FontId;
use orpheus_ui::gfx::{Align, Gfx};
use orpheus_ui::icons as ui_icons;
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use orpheus_ui::text::StrBuf;
use orpheus_ui::theme::Role;
use orpheus_ui::widgets::list::{Accessory, Item, ListAction, ListSource, ListView, Row};

use super::{LIBRARY, NOW_PLAYING};
use crate::services::{LibKind, LibList, LibNow, LibraryService, PlayerCmd, Services, data, other};
use crate::strings::{Lang, Str};

const ROWS: u32 = 5;

// Menu ids.
const M_PLAY: u16 = 1;
const M_SHUFFLE: u16 = 2;
const M_PLAY_ALL: u16 = 3;
const M_SHUFFLE_ALL: u16 = 4;

/// The name shown for a row (`""`: unknown artist, album or genre).
fn shown(lang: Lang, kind: LibKind, name: &str) -> &str {
    if !name.is_empty() {
        return name;
    }
    lang.get(match kind {
        LibKind::Artist | LibKind::GenreArtist => Str::UnknownArtist,
        LibKind::Album | LibKind::GenreAlbum => Str::UnknownAlbum,
        LibKind::Genre => Str::UnknownGenre,
        LibKind::Track => Str::Name,
    })
}

/// The list as a `ListSource` (borrowed for one draw or key).
struct LibSource<'a, S: ?Sized> {
    svc: &'a S,
    lang: Lang,
    count: u32,
    focus: u32,
    time: StrBuf<16>,
}

impl<'a, S: LibraryService + ?Sized> LibSource<'a, S> {
    fn new(svc: &'a S, lang: Lang, count: u32, focus: u32) -> Self {
        let mut time = StrBuf::new();
        if let Some(r) = svc.lib_row(focus)
            && r.kind == LibKind::Track
            && r.duration_ms > 0
        {
            time.push_duration(r.duration_ms.div_ceil(1000));
        }
        LibSource { svc, lang, count, focus, time }
    }
}

impl<S: LibraryService + ?Sized> ListSource for LibSource<'_, S> {
    fn len(&self) -> u32 {
        self.count
    }

    fn row(&self, i: u32) -> Row<'_> {
        let Some(r) = self.svc.lib_row(i) else { return Row::Pending };
        let mut it = Item::new(shown(self.lang, r.kind, r.name));
        match r.kind {
            LibKind::Track => {
                it = it.icon(&ui_icons::NOTE);
                if i == self.focus && !self.time.is_empty() {
                    it = it.value(self.time.as_str());
                }
            }
            LibKind::Album | LibKind::GenreAlbum => {
                it = it.icon(&ui_icons::DISC).accessory(Accessory::Chevron);
            }
            _ => it = it.accessory(Accessory::Chevron),
        }
        Row::Item(it)
    }

    fn letter(&self, i: u32) -> Option<char> {
        self.svc.lib_letter(i)
    }

    fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        self.svc.lib_seek_letter(from, forward)
    }
}

/// One library list.
pub struct LibraryScreen {
    list: LibList,
    view: ListView,
    lang: Lang,
    count: u32,
    complete: bool,
    /// The table row focused (kept across windows and resume), or the
    /// row to focus once it is loaded ("Go to album").
    focus_id: Option<u32>,
    /// `focus_id` still has to be found in the rows.
    seeking: bool,
}

impl Default for LibraryScreen {
    fn default() -> Self {
        LibraryScreen {
            list: LibList::Artists,
            view: ListView::new(),
            lang: Lang::ENGLISH,
            count: 0,
            complete: false,
            focus_id: None,
            seeking: false,
        }
    }
}

impl LibraryScreen {
    /// The navigation argument of a list.
    #[must_use]
    pub const fn arg(list: LibList) -> Arg {
        Arg(list.code())
    }

    /// The focused row (tests, diagnostics).
    #[must_use]
    pub fn focus(&self) -> u32 {
        self.view.focus
    }

    fn title(&self) -> &'static str {
        self.lang.get(match self.list {
            LibList::Artists | LibList::GenreArtists(_) => Str::Artists,
            LibList::Albums | LibList::ArtistAlbums(_) | LibList::GenreArtistAlbums(_) => Str::Albums,
            LibList::Genres => Str::Genres,
            LibList::Recent => Str::RecentlyAdded,
            _ => Str::Songs,
        })
    }

    fn open_list<S: Services + ?Sized>(&mut self, svc: &mut S) {
        let l = svc.lib_open(self.list);
        self.count = l.count;
        self.complete = l.complete;
        self.view.invalidate();
    }

    fn want<S: Services + ?Sized>(&self, svc: &mut S) {
        svc.lib_want(self.view.top, ROWS + 1);
    }

    fn focus_on<S: Services + ?Sized>(&mut self, svc: &S, row: u32, now: orpheus_ui::Instant) {
        let src = LibSource::new(svc, self.lang, self.count, row);
        let mut v = self.view;
        v.set_focus(&src, row, ROWS, now);
        self.view = v;
    }

    fn remember_focus<S: Services + ?Sized>(&mut self, svc: &S) {
        if let Some(r) = svc.lib_row(self.view.focus) {
            self.focus_id = Some(r.id);
        }
    }

    /// The row "Go to album / artist" should land on: the current track in
    /// its album, its album among the artist's.
    fn now_target(list: LibList, now: Option<LibNow>) -> Option<u32> {
        let n = now?;
        match list {
            LibList::AlbumTracks(a) if a == n.album => Some(n.track),
            LibList::ArtistAlbums(a) if a == n.artist => Some(n.album),
            _ => None,
        }
    }

    /// Focus `focus_id` if its row is loaded now.
    fn seek_focus<S: Services + ?Sized>(&mut self, svc: &S, now: orpheus_ui::Instant) {
        if !self.seeking {
            return;
        }
        if let Some(row) = self.focus_id.and_then(|id| svc.lib_position(id)) {
            self.seeking = false;
            self.focus_on(svc, row, now);
        }
    }

    fn play<S: Services + ?Sized>(
        cx: &mut Cx<'_, S>,
        list: LibList,
        start: Option<u32>,
        shuffle: bool,
    ) -> Nav {
        cx.svc.command(PlayerCmd::PlayLib { list, start, shuffle });
        Nav::Push(NOW_PLAYING, Arg(0))
    }

    fn activate<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, i: u32) -> Nav {
        let Some(r) = cx.svc.lib_row(i) else { return Nav::Stay };
        match LibList::open(r.kind, r.id) {
            Some(next) => Nav::Push(LIBRARY, Self::arg(next)),
            None => Self::play(cx, self.list, Some(i), false),
        }
    }

    fn menu_pick<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, mid: u16) -> Nav {
        let focused = cx.svc.lib_row(self.view.focus).map(|r| (r.kind, r.id));
        match mid {
            M_PLAY | M_SHUFFLE => {
                let shuffle = mid == M_SHUFFLE;
                match focused.and_then(|(k, id)| LibList::tracks_of(k, id)) {
                    Some(l) => Self::play(cx, l, None, shuffle),
                    None if self.list.is_tracks() => Self::play(cx, self.list, None, shuffle),
                    None => Nav::Stay,
                }
            }
            M_PLAY_ALL | M_SHUFFLE_ALL => match self.list.all_tracks() {
                Some(l) => Self::play(cx, l, None, mid == M_SHUFFLE_ALL),
                None => Nav::Stay,
            },
            _ => Nav::Stay,
        }
    }
}

impl<S: Services + ?Sized> Screen<S> for LibraryScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.list = LibList::from_code(arg.0).unwrap_or(LibList::Artists);
        self.lang = cx.svc.lang();
        self.open_list(cx.svc);
        self.focus_id = Self::now_target(self.list, cx.svc.lib_now());
        self.seeking = self.focus_id.is_some();
        self.focus_on(cx.svc, 0, cx.now);
        self.seek_focus(cx.svc, cx.now);
        self.want(cx.svc);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Data(d) if d.tag == data::LIBRARY => {
                let l = cx.svc.lib_listing();
                if l.list == self.list {
                    self.count = l.count;
                    self.complete = l.complete;
                    self.seek_focus(cx.svc, cx.now);
                    self.view.invalidate();
                }
                return Nav::Stay;
            }
            Event::Service(ServiceEvent::Other { kind: other::LIBRARY, .. }) => {
                // A new library: rows may have moved.
                self.open_list(cx.svc);
                self.seeking = self.focus_id.is_some();
                let row = self.view.focus.min(self.count.saturating_sub(1));
                self.focus_on(cx.svc, row, cx.now);
                self.want(cx.svc);
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id, .. }) => return self.menu_pick(cx, id),
            Event::Service(ServiceEvent::CardRemoved) => return Nav::Home,
            _ => {}
        }
        let act = {
            let src = LibSource::new(&*cx.svc, self.lang, self.count, self.view.focus);
            let mut v = self.view;
            let a = v.handle(&src, ev, ROWS, cx.now);
            self.view = v;
            a
        };
        match act {
            ListAction::Activated(i) => self.activate(cx, i),
            ListAction::Moved => {
                self.seeking = false;
                self.remember_focus(cx.svc);
                self.want(cx.svc);
                Nav::Stay
            }
            ListAction::None => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        if self.count == 0 && self.complete {
            if dirty == Dirty::All {
                g.clear(Role::Bg);
                let dim = g.c(Role::TextDim);
                let (w, h) = g.size();
                g.text_in(4, h / 2, w - 8, self.lang.get(Str::NoMusic), FontId::Ui, dim, Align::Center);
            }
            return;
        }
        let src = LibSource::new(&*cx.svc, self.lang, self.count, self.view.focus);
        let mut v = self.view;
        v.draw(&src, g, dirty, cx.now, Role::Bg);
        self.view = v;
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        if out.parent().is_some() {
            return;
        }
        let l = self.lang;
        let focused = cx.svc.lib_row(self.view.focus).map(|r| r.kind);
        if focused.is_some_and(|k| k != LibKind::Track) || self.list.is_tracks() {
            out.action(M_PLAY, l.get(Str::PlayItem));
            out.action(M_SHUFFLE, l.get(Str::Shuffle));
        }
        if !self.list.is_tracks() && self.list.all_tracks().is_some() {
            out.action(M_PLAY_ALL, l.get(Str::PlayAll));
            out.action(M_SHUFFLE_ALL, l.get(Str::ShuffleAll));
        }
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: self.view.wants_ticks(),
            title: Some(self.title()),
            strip: StripUse::Auto,
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        let mut r = Resume {
            focus: self.view.focus,
            top: self.view.top,
            handle: self.list.code(),
            ..Resume::default()
        };
        if let Some(id) = self.focus_id {
            r.data[..4].copy_from_slice(&id.to_le_bytes());
            r.data[4] = 1;
        }
        r
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        let id = (r.data[4] == 1).then(|| u32::from_le_bytes([r.data[0], r.data[1], r.data[2], r.data[3]]));
        self.focus_id = id;
        self.view.top = r.top;
        let row = id.and_then(|id| cx.svc.lib_position(id));
        self.seeking = id.is_some() && row.is_none();
        self.focus_on(cx.svc, row.unwrap_or(r.focus).min(self.count.saturating_sub(1)), cx.now);
        self.want(cx.svc);
    }
}
