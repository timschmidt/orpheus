//! Files (05 §7.2) and Music › Folders: a windowed browser over the
//! `BrowseService`. Each folder is one screen on the navigation stack, so
//! Back goes up and the resume record brings the focus back to the folder
//! you came from. Rows not loaded yet draw as placeholders; the service
//! answers `want` with `Event::Data(BROWSE)`.

use yoton_ui::event::{Event, Key, KeyPhase, UiEvent};
use yoton_ui::font::FontId;
use yoton_ui::gfx::{Align, Gfx};
use yoton_ui::icons::{self as ui_icons, Icon};
use yoton_ui::menu::MenuBuilder;
use yoton_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, StripUse, Wants};
use yoton_ui::text::StrBuf;
use yoton_ui::theme::Role;
use yoton_ui::widgets::dialog::DialogSpec;
use yoton_ui::widgets::list::{Accessory, Item, ListAction, ListSource, ListView, Row};

use super::{
    FILES, INFO, MUSIC_FOLDERS, NOTICE, NOW_PLAYING, Notice, PICTURE_VIEW, PICTURES, PROPERTIES,
    PictureScreen, TEXT_VIEW,
};
use crate::App;
use crate::browse::{DirId, EntryKind, FileRef};
use crate::services::{BrowseFilter, BrowseService, PlayerCmd, Services, SortBy, data};
use crate::settings::id;
use crate::strings::{Lang, Str};
use yoton_ui::nav::ScreenId;

const ROWS: u32 = 5;

/// Icon for an entry kind.
#[must_use]
pub fn kind_icon(k: EntryKind) -> &'static Icon {
    match k {
        EntryKind::Dir => &ui_icons::FOLDER,
        EntryKind::Audio => &ui_icons::NOTE,
        EntryKind::Playlist => &ui_icons::PLAYLIST,
        EntryKind::Image => &ui_icons::IMAGE,
        EntryKind::Text => &ui_icons::TEXT,
        EntryKind::DiskImage => &ui_icons::DISC,
        EntryKind::Firmware => &ui_icons::CHIP,
        EntryKind::OtpImport => &ui_icons::KEY,
        EntryKind::Other => &ui_icons::FILE,
    }
}

/// Name of an entry kind.
#[must_use]
pub const fn kind_label(k: EntryKind) -> Str {
    match k {
        EntryKind::Dir => Str::KindFolder,
        EntryKind::Audio => Str::KindAudio,
        EntryKind::Playlist => Str::KindPlaylist,
        EntryKind::Image => Str::KindImage,
        EntryKind::Text => Str::KindText,
        EntryKind::DiskImage => Str::KindDiskImage,
        EntryKind::Firmware => Str::KindFirmware,
        EntryKind::OtpImport => Str::KindOtp,
        EntryKind::Other => Str::KindOther,
    }
}

/// "1.2 MB", "340 KB", "12 B".
pub fn push_size<const N: usize>(b: &mut StrBuf<N>, bytes: u64) {
    let (unit, div) = if bytes >= 1 << 30 {
        (" GB", 1u64 << 30)
    } else if bytes >= 1 << 20 {
        (" MB", 1 << 20)
    } else if bytes >= 1 << 10 {
        (" KB", 1 << 10)
    } else {
        (" B", 1)
    };
    let tenths = bytes * 10 / div;
    b.push_u32(u32::try_from(tenths / 10).unwrap_or(u32::MAX));
    if div > 1 && tenths < 100 {
        b.push('.');
        b.push_u32(u32::try_from(tenths % 10).unwrap_or(0));
    }
    b.push_str(unit);
}

/// The listing as a `ListSource` (borrowed for one draw or key).
struct DirSource<'a, S: ?Sized> {
    svc: &'a S,
    count: u32,
    focus: u32,
    size: StrBuf<16>,
}

impl<'a, S: BrowseService + ?Sized> DirSource<'a, S> {
    fn new(svc: &'a S, count: u32, focus: u32) -> Self {
        let mut size = StrBuf::new();
        if let Some(e) = svc.entry(focus)
            && e.kind != EntryKind::Dir
        {
            push_size(&mut size, e.size);
        }
        DirSource { svc, count, focus, size }
    }
}

impl<S: BrowseService + ?Sized> ListSource for DirSource<'_, S> {
    fn len(&self) -> u32 {
        self.count
    }

    fn row(&self, i: u32) -> Row<'_> {
        let Some(e) = self.svc.entry(i) else { return Row::Pending };
        let mut it = Item::new(e.name).icon(kind_icon(e.kind));
        if e.kind == EntryKind::Dir {
            it = it.accessory(Accessory::Chevron);
        } else if i == self.focus && !self.size.is_empty() {
            it = it.value(self.size.as_str());
        }
        Row::Item(it)
    }

    fn letter(&self, i: u32) -> Option<char> {
        self.svc.letter(i)
    }

    fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        self.svc.seek_letter(from, forward)
    }
}

/// Which app the browser belongs to.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BrowseMode {
    /// Files: everything.
    #[default]
    Files,
    /// Music › Folders: folders, audio, playlists.
    Music,
    /// Pictures: folders and pictures.
    Pictures,
}

/// One folder.
#[derive(Default)]
pub struct FilesScreen {
    mode: BrowseMode,
    dir: DirId,
    list: ListView,
    lang: Lang,
    count: u32,
    sorted: bool,
    complete: bool,
    focus_file: Option<FileRef>,
}

// Menu ids.
const M_PLAY: u16 = 1;
const M_SHUFFLE: u16 = 2;
const M_NEXT: u16 = 3;
const M_QUEUE: u16 = 4;
const M_PROPS: u16 = 5;
const M_DELETE: u16 = 6;
const M_SORT: u16 = 7;
const M_SORT_BY: u16 = 70;
const M_HIDDEN: u16 = 8;
const M_CARD: u16 = 9;
const M_SLIDESHOW: u16 = 10;
const DLG_DELETE: u16 = 6;

impl FilesScreen {
    #[must_use]
    pub fn new(mode: BrowseMode) -> FilesScreen {
        FilesScreen { mode, ..FilesScreen::default() }
    }

    #[must_use]
    pub fn mode(&self) -> BrowseMode {
        self.mode
    }

    /// The screen a sub-folder opens in.
    fn same_screen(&self) -> ScreenId {
        match self.mode {
            BrowseMode::Files => FILES,
            BrowseMode::Music => MUSIC_FOLDERS,
            BrowseMode::Pictures => PICTURES,
        }
    }

    /// The focused row (tests, diagnostics).
    #[must_use]
    pub fn focus(&self) -> u32 {
        self.list.focus
    }

    fn filter<S: Services + ?Sized>(&self, svc: &S) -> BrowseFilter {
        let sort = match svc.setting(id::FILES_SORT) {
            1 => SortBy::Date,
            2 => SortBy::Size,
            _ => SortBy::Name,
        };
        BrowseFilter {
            sort,
            hidden: svc.setting(id::SHOW_HIDDEN) != 0,
            audio_only: self.mode == BrowseMode::Music,
            images_only: self.mode == BrowseMode::Pictures,
        }
    }

    fn browse<S: Services + ?Sized>(&mut self, svc: &mut S) {
        let f = self.filter(svc);
        let l = svc.browse(self.dir, f);
        self.count = l.count;
        self.sorted = l.sorted;
        self.complete = l.complete;
        self.list.invalidate();
    }

    fn want<S: Services + ?Sized>(&self, svc: &mut S) {
        svc.want(self.list.top, ROWS + 1);
    }

    fn remember_focus<S: Services + ?Sized>(&mut self, svc: &mut S) {
        if let Some(e) = svc.entry(self.list.focus) {
            let f = e.file;
            self.focus_file = Some(f);
            svc.focus_hint(f);
        }
    }

    fn focus_on<S: Services + ?Sized>(&mut self, svc: &S, row: u32, now: yoton_ui::Instant) {
        let src = DirSource::new(svc, self.count, row);
        let mut l = self.list;
        l.set_focus(&src, row, ROWS, now);
        self.list = l;
    }

    fn open<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, i: u32) -> Nav {
        let Some(e) = cx.svc.entry(i) else { return Nav::Stay };
        let notice = |n: Notice| Nav::Push(NOTICE, Arg(n.arg()));
        match e.kind {
            EntryKind::Dir => Nav::Push(self.same_screen(), Arg(e.child.0)),
            EntryKind::Audio => {
                let file = e.file;
                cx.svc.command(PlayerCmd::PlayDir { dir: self.dir, start: Some(file), shuffle: false });
                Nav::Push(NOW_PLAYING, Arg(0))
            }
            EntryKind::Text => Nav::Push(TEXT_VIEW, Arg(i)),
            EntryKind::Playlist => notice(Notice::NotYet(App::Music)),
            EntryKind::Image => Nav::Push(PICTURE_VIEW, Arg(PictureScreen::arg(i, false))),
            EntryKind::DiskImage => notice(Notice::NotYet(App::UsbDrive)),
            EntryKind::OtpImport => notice(Notice::NotYet(App::Authenticator)),
            EntryKind::Firmware => notice(Notice::NotYetSettings),
            EntryKind::Other => notice(Notice::Unsupported),
        }
    }

    fn menu_pick<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>, mid: u16) -> Nav {
        let lang = self.lang;
        let focused = cx.svc.entry(self.list.focus).map(|e| (e.file, e.kind));
        match mid {
            M_PLAY | M_SHUFFLE => {
                cx.svc.command(PlayerCmd::PlayDir { dir: self.dir, start: None, shuffle: mid == M_SHUFFLE });
                Nav::Push(NOW_PLAYING, Arg(0))
            }
            M_NEXT | M_QUEUE => {
                if let Some((f, _)) = focused {
                    let (c, t) = if mid == M_NEXT {
                        (PlayerCmd::PlayNext(f), Str::PlayingNext)
                    } else {
                        (PlayerCmd::AddToQueue(f), Str::AddedToQueue)
                    };
                    cx.svc.command(c);
                    cx.ui.toast(lang.get(t));
                }
                Nav::Stay
            }
            M_SLIDESHOW => Nav::Push(PICTURE_VIEW, Arg(PictureScreen::arg(self.list.focus, true))),
            M_PROPS => Nav::Push(PROPERTIES, Arg(self.list.focus)),
            M_DELETE => {
                cx.ui.dialog(DialogSpec::destructive(
                    DLG_DELETE,
                    lang.get(Str::DeleteQ),
                    cx.svc.entry(self.list.focus).map_or("", |e| e.name),
                    lang.get(Str::Cancel),
                    lang.get(Str::DeleteBtn),
                ));
                Nav::Stay
            }
            70..=72 => {
                cx.svc.set_setting(id::FILES_SORT, i16::try_from(mid - M_SORT_BY).unwrap_or(0));
                self.rebrowse(cx);
                Nav::Stay
            }
            M_HIDDEN => {
                let v = cx.svc.setting(id::SHOW_HIDDEN);
                cx.svc.set_setting(id::SHOW_HIDDEN, 1 - v);
                self.rebrowse(cx);
                Nav::Stay
            }
            M_CARD => Nav::Push(INFO, Arg(1)),
            _ => Nav::Stay,
        }
    }

    /// Browse again (filter changed, file deleted) keeping the focused file.
    fn rebrowse<S: Services + ?Sized>(&mut self, cx: &mut Cx<'_, S>) {
        self.remember_focus(cx.svc);
        self.browse(cx.svc);
        let row = self.focus_file.and_then(|f| cx.svc.position(f)).unwrap_or(self.list.focus);
        self.focus_on(cx.svc, row.min(self.count.saturating_sub(1)), cx.now);
        self.want(cx.svc);
    }
}

impl<S: Services + ?Sized> Screen<S> for FilesScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        self.dir = DirId(arg.0);
        self.lang = cx.svc.lang();
        self.browse(cx.svc);
        self.focus_on(cx.svc, 0, cx.now);
        self.want(cx.svc);
    }

    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        match *ev {
            Event::Key(Key::Back, KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Data(d) if d.tag == data::BROWSE => {
                let l = cx.svc.listing();
                let resorted = l.sorted != self.sorted;
                self.count = l.count;
                self.sorted = l.sorted;
                self.complete = l.complete;
                if resorted && let Some(row) = self.focus_file.and_then(|f| cx.svc.position(f)) {
                    // The index is ready: keep the focused item in place.
                    self.focus_on(cx.svc, row, cx.now);
                    self.want(cx.svc);
                }
                self.list.invalidate();
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Menu { id, .. }) => return self.menu_pick(cx, id),
            Event::Ui(UiEvent::Dialog { id: DLG_DELETE, button: 1 }) => {
                if let Some(e) = cx.svc.entry(self.list.focus) {
                    let f = e.file;
                    if cx.svc.file_op(crate::services::FileOp::Delete(f)) {
                        cx.ui.toast(self.lang.get(Str::Deleted));
                    }
                }
                self.rebrowse(cx);
                return Nav::Stay;
            }
            Event::Service(yoton_ui::event::ServiceEvent::CardRemoved) => return Nav::Home,
            _ => {}
        }
        let act = {
            let src = DirSource::new(&*cx.svc, self.count, self.list.focus);
            let mut l = self.list;
            let a = l.handle(&src, ev, ROWS, cx.now);
            self.list = l;
            a
        };
        match act {
            ListAction::Activated(i) => self.open(cx, i),
            ListAction::Moved => {
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
                g.text_in(4, h / 2, w - 8, self.lang.get(Str::EmptyFolder), FontId::Ui, dim, Align::Center);
            }
            return;
        }
        let src = DirSource::new(&*cx.svc, self.count, self.list.focus);
        let mut l = self.list;
        l.draw(&src, g, dirty, cx.now, Role::Bg);
        self.list = l;
    }

    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        let l = self.lang;
        let focused = cx.svc.entry(self.list.focus).map(|e| e.kind);
        match out.parent() {
            None => {
                if self.mode == BrowseMode::Pictures {
                    out.action(M_SLIDESHOW, l.get(Str::Slideshow));
                } else {
                    out.action(M_PLAY, l.get(Str::PlayFolder));
                    out.action(M_SHUFFLE, l.get(Str::ShuffleFolder));
                }
                if focused == Some(EntryKind::Audio) {
                    out.action(M_NEXT, l.get(Str::PlayNext));
                    out.action(M_QUEUE, l.get(Str::AddToQueue));
                }
                if focused.is_some() {
                    out.action(M_PROPS, l.get(Str::Properties));
                }
                if focused.is_some_and(|k| k != EntryKind::Dir) {
                    out.action(M_DELETE, l.get(Str::DeleteMenu));
                }
                out.sub(M_SORT, l.get(Str::SortBy));
                out.toggle(M_HIDDEN, l.get(Str::ShowHidden), cx.svc.setting(id::SHOW_HIDDEN) != 0);
                out.action(M_CARD, l.get(Str::CardInfo));
            }
            Some(M_SORT) => {
                out.title(l.get(Str::SortBy));
                let cur = cx.svc.setting(id::FILES_SORT);
                for (i, s) in [Str::SortName, Str::SortDate, Str::SortSize].iter().enumerate() {
                    let v = i16::try_from(i).unwrap_or(0);
                    out.radio(M_SORT_BY + u16::try_from(i).unwrap_or(0), l.get(*s), cur == v);
                }
            }
            Some(_) => {}
        }
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: self.list.wants_ticks(),
            title: Some(self.lang.get(match self.mode {
                BrowseMode::Files => Str::AppFiles,
                BrowseMode::Music => Str::Folders,
                BrowseMode::Pictures => Str::AppPictures,
            })),
            strip: StripUse::Auto,
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        let mut r =
            Resume { focus: self.list.focus, top: self.list.top, handle: self.dir.0, ..Resume::default() };
        if let Some(f) = self.focus_file {
            r.data[..8].copy_from_slice(&f.to_bytes());
            r.data[8] = 1;
        }
        r
    }

    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        let f = (r.data[8] == 1).then(|| FileRef::from_bytes(&r.data[..8]));
        let row = f.and_then(|f| cx.svc.position(f)).unwrap_or(r.focus);
        self.focus_file = f;
        if let Some(f) = f {
            cx.svc.focus_hint(f);
        }
        self.list.top = r.top;
        self.focus_on(cx.svc, row, cx.now);
        self.want(cx.svc);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes() {
        let mut b = StrBuf::<16>::new();
        push_size(&mut b, 1536);
        assert_eq!(b.as_str(), "1.5 KB");
        b.clear();
        push_size(&mut b, 12);
        assert_eq!(b.as_str(), "12 B");
        b.clear();
        push_size(&mut b, 250 << 20);
        assert_eq!(b.as_str(), "250 MB");
    }
}
