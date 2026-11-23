//! Demo screens: one per toolkit widget, with Latin, Cyrillic and CJK
//! sample text. `Home` is the T-4a dummy screen. These are test fixtures, not
//! the product's screens (those live in `yoton-apps`, WP UI-2 onwards), so
//! the sample strings are drawn directly instead of through `tr!`.

use yoton_ui::event::{DIALOG_CANCELLED, Event, Key, Source, UiEvent};
use yoton_ui::font::FontId;
use yoton_ui::geom::Rect;
use yoton_ui::gfx::{Align, Gfx};
use yoton_ui::icons;
use yoton_ui::menu::MenuBuilder;
use yoton_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, ScreenSet, StripUse, Wants};
use yoton_ui::theme::Role;
use yoton_ui::widgets::dialog::DialogSpec;
use yoton_ui::widgets::list::{Accessory, Item, ListAction, ListSource, ListView, Row};
use yoton_ui::widgets::misc::{draw_big_value, draw_progress, draw_spinner};

use crate::system::{Lang, SimSystem};

pub const HOME: ScreenId = ScreenId(0);
pub const LIBRARY: ScreenId = ScreenId(1);
pub const SETTINGS: ScreenId = ScreenId(2);
pub const PLAYER: ScreenId = ScreenId(3);
pub const TEXT: ScreenId = ScreenId(4);
pub const WIDGETS: ScreenId = ScreenId(5);

type C<'a> = Cx<'a, SimSystem>;

/// Rows visible in the 96 px content area.
const ROWS: u32 = 5;

// ---------------------------------------------------------------- strings

const HOME_ITEMS: [[&str; 3]; 5] = [
    ["Library", "Библиотека", "ライブラリ"],
    ["Settings", "Настройки", "設定"],
    ["Now Playing", "Сейчас играет", "再生中"],
    ["Text samples", "Образцы текста", "テキスト見本"],
    ["Widgets", "Виджеты", "ウィジェット"],
];
const HOME_HINT: [&str; 3] = ["OK open · Menu more", "OK открыть · Menu ещё", "OK 開く"];
const HOME_VALUES: [&str; 5] = ["3 012", "", "", "", ""];

const RECENT_HEADER: [&str; 3] = ["Recently added", "Недавно добавлено", "最近追加した曲"];
const ARTISTS_HEADER: [&str; 3] = ["Artists", "Исполнители", "アーティスト"];
const RECENT: [[(&str, &str); 2]; 3] = [
    [("Jóga", "5:05"), ("Hoppípolla", "4:28")],
    [("Группа крови", "4:46"), ("Звезда по имени Солнце", "3:45")],
    [("夜に駆ける", "4:21"), ("红豆", "4:14")],
];
const ARTISTS: [[&str; 12]; 3] = [
    [
        "ABBA",
        "Ásgeir",
        "Beyoncé",
        "Björk",
        "Chopin: Nocturnes, Op. 9 — Maria João Pires",
        "Daft Punk",
        "Dvořák",
        "Édith Piaf",
        "Łona",
        "Mötley Crüe",
        "Sigur Rós",
        "Zaz",
    ],
    [
        "Агата Кристи",
        "Би-2",
        "Високосный год",
        "ДДТ",
        "Земфира",
        "Кино",
        "Король и Шут",
        "Мумий Тролль",
        "Пётр Ильич Чайковский — Времена года, соч. 37a",
        "Сплин",
        "Чайф",
        "Ёлка",
    ],
    [
        "宇多田ヒカル",
        "久石譲",
        "坂本龍一 — 戦場のメリークリスマス",
        "椎名林檎",
        "周杰倫",
        "王菲",
        "米津玄師",
        "YOASOBI",
        "아이유",
        "방탄소년단",
        "邓丽君",
        "陳奕迅",
    ],
];

const M_PLAY: [&str; 3] = ["Play", "Воспроизвести", "再生"];
const M_SHUFFLE: [&str; 3] = ["Shuffle", "Перемешать", "シャッフル"];
const M_REPEAT: [&str; 3] = ["Repeat", "Повтор", "リピート"];
const M_REPEAT_OPTS: [[&str; 3]; 3] =
    [["Off", "Выкл.", "オフ"], ["One", "Один", "1曲"], ["All", "Все", "全曲"]];
const M_SPEED: [&str; 3] = ["Speed", "Скорость", "再生速度"];
const M_FAV: [&str; 3] = ["Add to Favourites", "В избранное", "お気に入りに追加"];
const M_DELETE: [&str; 3] = ["Delete…", "Удалить…", "削除…"];
const T_FAV: [&str; 3] = ["Added to Favourites", "Добавлено в избранное", "追加しました"];
const T_DELETED: [&str; 3] = ["Deleted", "Удалено", "削除しました"];
const D_TITLE: [&str; 3] = ["Delete file?", "Удалить файл?", "削除しますか？"];
const D_TEXT: [&str; 3] = [
    "“Hoppípolla.flac” will be removed from the card.",
    "«Звезда по имени Солнце.mp3» будет удалён с карты.",
    "「夜に駆ける」をカードから削除します。",
];
const D_CANCEL: [&str; 3] = ["Cancel", "Отмена", "やめる"];
const D_DELETE: [&str; 3] = ["Delete", "Удалить", "削除"];

/// Mini-player title per language.
pub const MINI_TITLE: [&str; 3] =
    ["Sigur Rós — Hoppípolla (Takk…)", "Кино — Звезда по имени Солнце", "YOASOBI — 夜に駆ける"];

const S_PLAYBACK: [&str; 3] = ["Playback", "Воспроизведение", "再生"];
const S_GAPLESS: [&str; 3] = ["Gapless", "Без пауз", "ギャップレス"];
const S_EQ: [&str; 3] = ["Equaliser", "Эквалайзер", "イコライザー"];
const S_EQ_VAL: [&str; 3] = ["Rock", "Рок", "ロック"];
const S_HOME: [&str; 3] = ["Home screen", "Главный экран", "ホーム画面"];
const S_HOME_ITEMS: [[&str; 3]; 3] =
    [["Music", "Музыка", "音楽"], ["Radio", "Радио", "ラジオ"], ["Steps", "Шаги", "歩数"]];

const P_TRACK: [(&str, &str, &str); 3] = [
    ("Hoppípolla", "Sigur Rós", "Takk…"),
    ("Звезда по имени Солнце", "Кино", "Звезда по имени Солнце"),
    ("夜に駆ける", "YOASOBI", "THE BOOK"),
];
const P_HINT: [&str; 3] = ["OK pause · Next skip", "OK пауза · Next далее", "OK 一時停止 · Next 次へ"];

const TEXT_LINES: [&str; 8] = [
    "Ünïcödé: Łódź, Ærøskøbing, Đàng",
    "Ελληνικά: Καλημέρα κόσμε",
    "Русский: Съешь же ещё этих мягких французских булок",
    "日本語: 夜に駆ける、宇多田ヒカル",
    "中文: 红豆 · 周杰倫",
    "한국어: 아이유 — 좋은 날",
    "עברית: שלום עולם · العربية: مرحبا",
    "ไทย: สวัสดี",
];

/// Every string the demo screens draw (coverage test).
pub fn all_strings() -> Vec<&'static str> {
    let mut v: Vec<&'static str> = Vec::new();
    for l in Lang::ALL {
        for t in HOME_ITEMS {
            v.push(l.pick(t));
        }
        v.extend([
            l.pick(HOME_HINT),
            l.pick(RECENT_HEADER),
            l.pick(ARTISTS_HEADER),
            l.pick(M_PLAY),
            l.pick(M_SHUFFLE),
            l.pick(M_REPEAT),
            l.pick(M_SPEED),
            l.pick(M_FAV),
            l.pick(M_DELETE),
            l.pick(T_FAV),
            l.pick(T_DELETED),
            l.pick(D_TITLE),
            l.pick(D_TEXT),
            l.pick(D_CANCEL),
            l.pick(D_DELETE),
            l.pick(MINI_TITLE),
            l.pick(S_PLAYBACK),
            l.pick(S_GAPLESS),
            l.pick(S_EQ),
            l.pick(S_EQ_VAL),
            l.pick(S_HOME),
            l.pick(P_HINT),
        ]);
        for t in M_REPEAT_OPTS.iter().chain(S_HOME_ITEMS.iter()) {
            v.push(l.pick(*t));
        }
    }
    for r in RECENT {
        for (a, b) in r {
            v.push(a);
            v.push(b);
        }
    }
    for a in ARTISTS {
        v.extend(a);
    }
    for (a, b, c) in P_TRACK {
        v.extend([a, b, c]);
    }
    v.extend(TEXT_LINES);
    v
}

// ---------------------------------------------------------------- the set

/// The demo app's screens (static dispatch, as the product's will be).
pub enum Demo {
    Home(HomeScreen),
    Library(LibraryScreen),
    Settings(SettingsScreen),
    Player(PlayerScreen),
    Text(TextScreen),
    Widgets(WidgetsScreen),
}

macro_rules! each {
    ($self:ident, $s:ident => $e:expr) => {
        match $self {
            Demo::Home($s) => $e,
            Demo::Library($s) => $e,
            Demo::Settings($s) => $e,
            Demo::Player($s) => $e,
            Demo::Text($s) => $e,
            Demo::Widgets($s) => $e,
        }
    };
}

impl Screen<SimSystem> for Demo {
    fn enter(&mut self, cx: &mut C<'_>, arg: Arg) {
        each!(self, s => s.enter(cx, arg));
    }
    fn event(&mut self, cx: &mut C<'_>, ev: &Event) -> Nav {
        each!(self, s => s.event(cx, ev))
    }
    fn draw(&mut self, cx: &C<'_>, g: &mut Gfx<'_>, dirty: Dirty) {
        each!(self, s => s.draw(cx, g, dirty));
    }
    fn menu(&self, cx: &C<'_>, out: &mut MenuBuilder) {
        each!(self, s => s.menu(cx, out));
    }
    fn wants(&self) -> Wants {
        each!(self, s => s.wants())
    }
    fn save(&self) -> Resume {
        each!(self, s => s.save())
    }
    fn restore(&mut self, cx: &mut C<'_>, r: &Resume) {
        each!(self, s => s.restore(cx, r));
    }
}

impl ScreenSet<SimSystem> for Demo {
    const HOME: ScreenId = HOME;

    fn create(id: ScreenId) -> Self {
        match id {
            LIBRARY => Demo::Library(LibraryScreen::default()),
            SETTINGS => Demo::Settings(SettingsScreen::default()),
            PLAYER => Demo::Player(PlayerScreen::default()),
            TEXT => Demo::Text(TextScreen::default()),
            WIDGETS => Demo::Widgets(WidgetsScreen),
            _ => Demo::Home(HomeScreen::default()),
        }
    }

    fn id(&self) -> ScreenId {
        match self {
            Demo::Home(_) => HOME,
            Demo::Library(_) => LIBRARY,
            Demo::Settings(_) => SETTINGS,
            Demo::Player(_) => PLAYER,
            Demo::Text(_) => TEXT,
            Demo::Widgets(_) => WIDGETS,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            Demo::Home(_) => "Home",
            Demo::Library(_) => "Library",
            Demo::Settings(_) => "Settings",
            Demo::Player(_) => "NowPlaying",
            Demo::Text(_) => "Text",
            Demo::Widgets(_) => "Widgets",
        }
    }
}

/// Screen id by script name (`goto:Library`).
#[must_use]
pub fn screen_by_name(n: &str) -> Option<ScreenId> {
    Some(match n {
        "Home" => HOME,
        "Library" => LIBRARY,
        "Settings" => SETTINGS,
        "NowPlaying" => PLAYER,
        "Text" => TEXT,
        "Widgets" => WIDGETS,
        _ => return None,
    })
}

fn save_list(l: &ListView) -> Resume {
    Resume { focus: l.focus, top: l.top, ..Resume::default() }
}

fn restore_list(l: &mut ListView, src: &dyn ListSource, r: &Resume, now: yoton_ui::Instant) {
    l.top = r.top;
    l.set_focus(src, r.focus, ROWS, now);
}

// ---------------------------------------------------------------- Home

/// T-4a's dummy screen and the demo index.
#[derive(Default)]
pub struct HomeScreen {
    list: ListView,
    lang: Lang,
}

impl ListSource for HomeScreen {
    fn len(&self) -> u32 {
        5
    }
    fn row(&self, i: u32) -> Row<'_> {
        let i = usize::try_from(i).unwrap_or(0);
        let icon = [&icons::NOTE, &icons::FILE, &icons::PLAY, &icons::FILE, &icons::IMAGE][i.min(4)];
        let mut it = Item::new(self.lang.pick(HOME_ITEMS[i.min(4)])).icon(icon).accessory(Accessory::Chevron);
        if !HOME_VALUES[i.min(4)].is_empty() {
            it = it.value(HOME_VALUES[i.min(4)]);
        }
        Row::Item(it)
    }
}

impl Screen<SimSystem> for HomeScreen {
    fn enter(&mut self, cx: &mut C<'_>, _arg: Arg) {
        self.lang = cx.svc.lang;
        let mut list = self.list;
        list.set_focus(self, 0, ROWS, cx.now);
        self.list = list;
    }

    fn event(&mut self, cx: &mut C<'_>, ev: &Event) -> Nav {
        self.lang = cx.svc.lang;
        let mut list = self.list;
        let act = list.handle(self, ev, ROWS, cx.now);
        self.list = list;
        match act {
            ListAction::Activated(i) => {
                let id = [LIBRARY, SETTINGS, PLAYER, TEXT, WIDGETS][usize::try_from(i).unwrap_or(0).min(4)];
                Nav::Push(id, Arg(0))
            }
            _ => Nav::Stay,
        }
    }

    fn draw(&mut self, cx: &C<'_>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut list = self.list;
        list.draw(self, g, dirty, cx.now, Role::Bg);
        self.list = list;
    }

    fn wants(&self) -> Wants {
        Wants {
            strip: StripUse::Hint(self.lang.pick(HOME_HINT)),
            ticks: self.list.wants_ticks(),
            title: Some("yoton"),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        save_list(&self.list)
    }

    fn restore(&mut self, cx: &mut C<'_>, r: &Resume) {
        let mut list = self.list;
        restore_list(&mut list, self, r, cx.now);
        self.list = list;
    }
}

// ---------------------------------------------------------------- Library

/// `ListView` with headers, values, icons, placeholders, menu and dialog.
#[derive(Default)]
pub struct LibraryScreen {
    list: ListView,
    lang: Lang,
    shuffle: bool,
    repeat: u8,
}

const LIB_LEN: u32 = 1 + 2 + 1 + 12 + 2;

impl ListSource for LibraryScreen {
    fn len(&self) -> u32 {
        LIB_LEN
    }
    fn row(&self, i: u32) -> Row<'_> {
        let l = self.lang as usize;
        match i {
            0 => Row::Header(self.lang.pick(RECENT_HEADER)),
            1 | 2 => {
                let (t, v) = RECENT[l][usize::try_from(i - 1).unwrap_or(0)];
                Row::Item(Item::new(t).icon(&icons::NOTE).value(v))
            }
            3 => Row::Header(self.lang.pick(ARTISTS_HEADER)),
            4..=15 => Row::Item(
                Item::new(ARTISTS[l][usize::try_from(i - 4).unwrap_or(0)])
                    .icon(&icons::FOLDER)
                    .accessory(Accessory::Chevron),
            ),
            _ => Row::Pending,
        }
    }
    fn letter(&self, i: u32) -> Option<char> {
        match self.row(i) {
            Row::Item(it) if i >= 4 => it.text.chars().next().map(|c| c.to_uppercase().next().unwrap_or(c)),
            _ => None,
        }
    }
}

impl Screen<SimSystem> for LibraryScreen {
    fn wants(&self) -> Wants {
        Wants {
            ticks: self.list.wants_ticks(),
            title: Some(self.lang.pick(HOME_ITEMS[0])),
            ..Wants::default()
        }
    }

    fn enter(&mut self, cx: &mut C<'_>, _arg: Arg) {
        self.lang = cx.svc.lang;
        self.shuffle = cx.svc.shuffle;
        self.repeat = cx.svc.repeat;
        let mut list = self.list;
        list.set_focus(self, 0, ROWS, cx.now);
        self.list = list;
    }

    fn event(&mut self, cx: &mut C<'_>, ev: &Event) -> Nav {
        let lang = self.lang;
        match *ev {
            Event::Key(Key::Back, yoton_ui::KeyPhase::Up { held: false }) => return Nav::Pop,
            Event::Ui(UiEvent::Menu { id, value }) => {
                match id {
                    1 => {
                        cx.svc.play(lang.pick(MINI_TITLE));
                        return Nav::Push(PLAYER, Arg(0));
                    }
                    2 => {
                        self.shuffle = value != 0;
                        cx.svc.shuffle = self.shuffle;
                    }
                    10..=12 => {
                        self.repeat = u8::try_from(id - 10).unwrap_or(0);
                        cx.svc.repeat = self.repeat;
                    }
                    4 => cx.svc.log.push("speed".into()),
                    5 => cx.ui.toast(lang.pick(T_FAV)),
                    6 => cx.ui.dialog(DialogSpec::destructive(
                        6,
                        lang.pick(D_TITLE),
                        lang.pick(D_TEXT),
                        lang.pick(D_CANCEL),
                        lang.pick(D_DELETE),
                    )),
                    _ => {}
                }
                return Nav::Stay;
            }
            Event::Ui(UiEvent::Dialog { id: 6, button }) => {
                if button == 1 {
                    cx.ui.toast(lang.pick(T_DELETED));
                    cx.svc.log.push("deleted".into());
                } else if button == DIALOG_CANCELLED {
                    cx.svc.log.push("dialog cancelled".into());
                }
                return Nav::Stay;
            }
            _ => {}
        }
        let mut list = self.list;
        let act = list.handle(self, ev, ROWS, cx.now);
        self.list = list;
        if let ListAction::Activated(i) = act {
            if i < 4 {
                cx.svc.play(lang.pick(MINI_TITLE));
                cx.svc.status.source = Source::Playing;
                return Nav::Push(PLAYER, Arg(i));
            }
            cx.ui.toast(ARTISTS[self.lang as usize][usize::try_from(i - 4).unwrap_or(0).min(11)]);
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &C<'_>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut list = self.list;
        list.draw(self, g, dirty, cx.now, Role::Bg);
        self.list = list;
    }

    fn menu(&self, _cx: &C<'_>, out: &mut MenuBuilder) {
        let l = self.lang;
        match out.parent() {
            None => {
                out.action(1, l.pick(M_PLAY));
                out.toggle(2, l.pick(M_SHUFFLE), self.shuffle);
                out.sub(3, l.pick(M_REPEAT));
                out.value(4, l.pick(M_SPEED), "1.0×");
                out.action(5, l.pick(M_FAV));
                out.action(6, l.pick(M_DELETE));
            }
            Some(3) => {
                out.title(l.pick(M_REPEAT));
                for (i, o) in M_REPEAT_OPTS.iter().enumerate() {
                    let idx = u8::try_from(i).unwrap_or(0);
                    out.radio(10 + u16::from(idx), l.pick(*o), self.repeat == idx);
                }
            }
            Some(_) => {}
        }
    }

    fn save(&self) -> Resume {
        save_list(&self.list)
    }

    fn restore(&mut self, cx: &mut C<'_>, r: &Resume) {
        let mut list = self.list;
        restore_list(&mut list, self, r, cx.now);
        self.list = list;
    }
}

// ---------------------------------------------------------------- Settings

/// Toggles, radios, checks, values.
#[derive(Default)]
pub struct SettingsScreen {
    list: ListView,
    lang: Lang,
    shuffle: bool,
    gapless: bool,
    repeat: u8,
    home: [bool; 3],
}

impl ListSource for SettingsScreen {
    fn len(&self) -> u32 {
        12
    }
    fn row(&self, i: u32) -> Row<'_> {
        let l = self.lang;
        Row::Item(match i {
            0 => return Row::Header(l.pick(S_PLAYBACK)),
            1 => Item::new(l.pick(M_SHUFFLE)).accessory(Accessory::Toggle(self.shuffle)),
            2 => Item::new(l.pick(S_GAPLESS)).accessory(Accessory::Toggle(self.gapless)),
            3 => Item::new(l.pick(S_EQ)).value(l.pick(S_EQ_VAL)).accessory(Accessory::Chevron),
            4 => return Row::Header(l.pick(M_REPEAT)),
            5..=7 => {
                let k = usize::try_from(i - 5).unwrap_or(0);
                Item::new(l.pick(M_REPEAT_OPTS[k])).accessory(Accessory::Radio(usize::from(self.repeat) == k))
            }
            8 => return Row::Header(l.pick(S_HOME)),
            _ => {
                let k = usize::try_from(i - 9).unwrap_or(0).min(2);
                Item::new(l.pick(S_HOME_ITEMS[k])).accessory(Accessory::Check(self.home[k]))
            }
        })
    }
}

impl Screen<SimSystem> for SettingsScreen {
    fn enter(&mut self, cx: &mut C<'_>, _arg: Arg) {
        self.lang = cx.svc.lang;
        self.shuffle = cx.svc.shuffle;
        self.gapless = cx.svc.gapless;
        self.repeat = cx.svc.repeat;
        self.home = cx.svc.home_items;
        let mut list = self.list;
        list.set_focus(self, 0, ROWS, cx.now);
        self.list = list;
    }

    fn event(&mut self, cx: &mut C<'_>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) {
            return Nav::Pop;
        }
        let mut list = self.list;
        let act = list.handle(self, ev, ROWS, cx.now);
        self.list = list;
        if let ListAction::Activated(i) = act {
            match i {
                1 => self.shuffle = !self.shuffle,
                2 => self.gapless = !self.gapless,
                3 => cx.ui.toast(self.lang.pick(S_EQ_VAL)),
                5..=7 => self.repeat = u8::try_from(i - 5).unwrap_or(0),
                9..=11 => {
                    let k = usize::try_from(i - 9).unwrap_or(0).min(2);
                    self.home[k] = !self.home[k];
                }
                _ => {}
            }
            cx.svc.shuffle = self.shuffle;
            cx.svc.gapless = self.gapless;
            cx.svc.repeat = self.repeat;
            cx.svc.home_items = self.home;
            self.list.invalidate();
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &C<'_>, g: &mut Gfx<'_>, dirty: Dirty) {
        let mut list = self.list;
        list.draw(self, g, dirty, cx.now, Role::Bg);
        self.list = list;
    }

    fn wants(&self) -> Wants {
        Wants {
            ticks: self.list.wants_ticks(),
            title: Some(self.lang.pick(HOME_ITEMS[1])),
            ..Wants::default()
        }
    }

    fn save(&self) -> Resume {
        save_list(&self.list)
    }

    fn restore(&mut self, cx: &mut C<'_>, r: &Resume) {
        let mut list = self.list;
        restore_list(&mut list, self, r, cx.now);
        self.list = list;
    }
}

// ---------------------------------------------------------------- Player

/// A minimal Now Playing: art placeholder, wrapped title, progress.
#[derive(Default)]
pub struct PlayerScreen {
    lang: Lang,
    playing: bool,
    drawn_playing: Option<bool>,
}

impl Screen<SimSystem> for PlayerScreen {
    fn enter(&mut self, cx: &mut C<'_>, _arg: Arg) {
        self.lang = cx.svc.lang;
        if cx.svc.now_playing.is_none() {
            cx.svc.play(self.lang.pick(MINI_TITLE));
        }
        self.playing = cx.svc.now_playing.is_some_and(|n| n.playing);
    }

    fn event(&mut self, cx: &mut C<'_>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) {
            return Nav::Pop;
        }
        if ev.is_tap(Key::Ok)
            && let Some(n) = cx.svc.now_playing.as_mut()
        {
            n.playing = !n.playing;
            self.playing = n.playing;
            cx.svc.status.source = if n.playing { Source::Playing } else { Source::Paused };
        }
        Nav::Stay
    }

    fn draw(&mut self, cx: &C<'_>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn_playing == Some(self.playing) {
            return;
        }
        let (w, _) = g.size();
        g.clear(Role::Bg);
        let (title, artist, album) = P_TRACK[self.lang as usize];
        let art = Rect::new(6, 5, 40, 40);
        let surf = g.c(Role::Surface);
        g.fill_round(art, 6, surf);
        let dim = g.c(Role::TextDim);
        g.icon(art.x + 14, art.y + 14, &icons::NOTE, dim);
        let tx = 52;
        let tw = w - tx - 5;
        let text = g.c(Role::Text);
        let mut lines = 0i16;
        let pitch = g.line_pitch(title, FontId::UiBold);
        g.wrap(title, FontId::UiBold, tw, |g, line| {
            if lines < 2 {
                g.text_in(tx, 16 + pitch * lines, tw, line, FontId::UiBold, text, Align::Left);
            }
            lines += 1;
        });
        let y = 16 + pitch * lines.min(2);
        g.text_in(tx, y, tw, artist, FontId::Ui, dim, Align::Left);
        if lines < 2 {
            g.text_in(tx, y + 14, tw, album, FontId::Ui, dim, Align::Left);
        }
        g.text(6, 62, "FLAC 44.1/16", FontId::Ui, dim);
        g.text_in(w - 50, 62, 44, "7 / 12", FontId::Ui, dim, Align::Right);
        let progress = cx.svc.now_playing.map_or(0, |n| n.progress);
        draw_progress(g, Rect::new(6, 68, w - 12, 4), progress, Some(620), Role::Bg);
        g.text(6, 87, "1:47", FontId::Ui, text);
        g.text_in(w - 50, 87, 44, "-2:05", FontId::Ui, text, Align::Right);
        let ic = if self.playing { &icons::PLAY } else { &icons::PAUSE };
        g.icon((w - 10) / 2, 78, ic, text);
        self.drawn_playing = Some(self.playing);
    }

    fn wants(&self) -> Wants {
        Wants { is_player: true, strip: StripUse::Hint(self.lang.pick(P_HINT)), ..Wants::default() }
    }
}

// ---------------------------------------------------------------- Text

/// Mixed scripts through the fallback font, wrapped, scrolled by line.
#[derive(Default)]
pub struct TextScreen {
    first: usize,
    drawn: Option<usize>,
}

impl Screen<SimSystem> for TextScreen {
    fn enter(&mut self, _cx: &mut C<'_>, _arg: Arg) {}

    fn event(&mut self, _cx: &mut C<'_>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) {
            return Nav::Pop;
        }
        if ev.is_step(Key::Down) {
            self.first = (self.first + 1).min(TEXT_LINES.len() - 1);
        }
        if ev.is_step(Key::Up) {
            self.first = self.first.saturating_sub(1);
        }
        Nav::Stay
    }

    fn draw(&mut self, _cx: &C<'_>, g: &mut Gfx<'_>, dirty: Dirty) {
        if dirty == Dirty::Changed && self.drawn == Some(self.first) {
            return;
        }
        let (w, h) = g.size();
        g.clear(Role::Bg);
        let text = g.c(Role::Text);
        let dim = g.c(Role::TextDim);
        // Label line (bold, dim) then the sample wrapped at 16 px pitch;
        // only whole lines are drawn.
        let mut y = 12;
        for line in TEXT_LINES.iter().skip(self.first) {
            let (label, body) = line.split_once(": ").unwrap_or(("", line));
            if y + 3 > h {
                break;
            }
            g.text(5, y, label, FontId::UiBold, dim);
            y += 16;
            let mut fits = true;
            g.wrap(body, FontId::Ui, w - 10, |g, l| {
                if y + 3 > h {
                    fits = false;
                    return;
                }
                g.text(5, y, l, FontId::Ui, text);
                y += 16;
            });
            if !fits {
                break;
            }
            y += 1;
        }
        self.drawn = Some(self.first);
    }
}

// ---------------------------------------------------------------- Widgets

/// `BigValue`, 16 px numerals, `Progress` and `Spinner`.
#[derive(Default)]
pub struct WidgetsScreen;

impl Screen<SimSystem> for WidgetsScreen {
    fn enter(&mut self, _cx: &mut C<'_>, _arg: Arg) {}

    fn event(&mut self, _cx: &mut C<'_>, ev: &Event) -> Nav {
        if ev.is_tap(Key::Back) { Nav::Pop } else { Nav::Stay }
    }

    fn draw(&mut self, cx: &C<'_>, g: &mut Gfx<'_>, dirty: Dirty) {
        let (w, _) = g.size();
        if dirty == Dirty::All {
            draw_big_value(g, Rect::new(0, 2, w, 50), "101.90", "MHz", "FM · P03 Radio Paradise", Role::Bg);
            draw_progress(g, Rect::new(8, 58, w - 16, 5), 640, Some(820), Role::Bg);
            let text = g.c(Role::Text);
            let dim = g.c(Role::TextDim);
            let e = g.text(30, 88, "6 482", FontId::Num16, text);
            g.text(e + 4, 88, "/ 10 000", FontId::Ui, dim);
        }
        draw_spinner(g, 8, 75, cx.now, Role::Bg);
    }

    fn wants(&self) -> Wants {
        Wants { ticks: true, ..Wants::default() }
    }
}
