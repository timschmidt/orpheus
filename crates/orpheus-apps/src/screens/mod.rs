//! The screens and their dispatch enum (static, no heap: only the top
//! screen holds state; covered screens are rebuilt from resume records).

mod auth;
mod bluetooth;
#[cfg(feature = "books")]
mod books;
#[cfg(feature = "bricks")]
mod bricks;
mod clocks;
mod drive;
mod edit;
mod files;
#[cfg(feature = "games")]
mod game;
#[cfg(feature = "games")]
mod games;
mod home;
mod library;
mod lists;
mod pictures;
mod player;
mod radio;
mod recorder;
mod rows;
#[cfg(feature = "salvo")]
mod salvo;
#[cfg(feature = "games")]
mod solitaire;
mod steps;
mod system;
mod text;
mod update;

pub use auth::AuthScreen;
pub use bluetooth::{BluetoothScreen, BtConnectScreen, BtView, bt_kind_icon, connect_arg, phase_label};
#[cfg(feature = "books")]
pub use books::{BookList, BookListScreen, GotoScreen, ReaderScreen};
pub use clocks::{
    AlarmDaysScreen, AlertScreen, PICK_ALARM, PICK_TIMER, ToolsScreen, push_days, push_hm, push_hms,
    push_lap, push_stopwatch,
};
pub use drive::{DriveView, UsbDriveScreen, medium_label, refusal_text};
pub use edit::{PickerScreen, SliderScreen};
pub use files::{BrowseMode, FilesScreen, kind_icon, kind_label, push_size};
#[cfg(feature = "games")]
pub use game::GameScreen;
#[cfg(feature = "games")]
pub use games::{GameHelpScreen, GameList, GameListScreen, game_screen, push_time};
pub use home::HomeScreen;
pub use library::LibraryScreen;
pub use lists::{ListKind, ListScreen};
pub use pictures::PictureScreen;
pub use player::{EqualiserScreen, NowPlayingScreen};
pub use radio::{PresetsScreen, RadioScreen};
pub use recorder::RecorderScreen;
pub use rows::{MAX_ROWS, RowData, Rows};
pub use steps::{DAY_SHORT, StepsScreen, push_distance, push_energy, push_grouped, steps_hint};
pub use system::{BootScreen, ChargingScreen, GoodbyeScreen, LockScreen, Notice, NoticeScreen};
pub use text::TextScreen;
pub use update::{MIN_BATTERY as UPDATE_MIN_BATTERY, UpdateScreen};

use orpheus_ui::event::{Event, ServiceEvent};
use orpheus_ui::gfx::Gfx;
use orpheus_ui::menu::MenuBuilder;
use orpheus_ui::nav::{Arg, Cx, Dirty, Nav, Resume, Screen, ScreenId, ScreenSet, Wants};

use crate::services::{Services, UsbMode, bt_event, other, radio_event, tools_event};
use crate::settings::id;
use crate::strings::Str;

pub const HOME: ScreenId = ScreenId(0);
pub const BOOT: ScreenId = ScreenId(1);
pub const SETTINGS: ScreenId = ScreenId(2);
/// Arg: group index.
pub const GROUP: ScreenId = ScreenId(3);
/// Arg: setting key.
pub const CHOICE: ScreenId = ScreenId(4);
/// Arg: setting key.
pub const SLIDER: ScreenId = ScreenId(5);
/// Arg: 0 time, 1 date.
pub const PICKER: ScreenId = ScreenId(6);
/// Arg: `Info as u32`.
pub const INFO: ScreenId = ScreenId(7);
pub const HOME_APPS: ScreenId = ScreenId(8);
pub const POWER_MENU: ScreenId = ScreenId(9);
pub const LOCK: ScreenId = ScreenId(10);
pub const QUICK_PANEL: ScreenId = ScreenId(11);
/// Arg: [`Notice::arg`].
pub const NOTICE: ScreenId = ScreenId(12);
pub const CHARGING: ScreenId = ScreenId(13);
pub const USB_CONNECT: ScreenId = ScreenId(14);
pub const GOODBYE: ScreenId = ScreenId(15);
/// Arg: `DirId` (0 = card root).
pub const FILES: ScreenId = ScreenId(16);
/// Music › Folders. Arg: `DirId`.
pub const MUSIC_FOLDERS: ScreenId = ScreenId(17);
/// Arg: row of the file in the current listing.
pub const TEXT_VIEW: ScreenId = ScreenId(18);
pub const MUSIC: ScreenId = ScreenId(19);
pub const NOW_PLAYING: ScreenId = ScreenId(20);
pub const EQUALISER: ScreenId = ScreenId(21);
/// Arg: row in the current listing.
pub const PROPERTIES: ScreenId = ScreenId(22);
pub const BOOKMARKS: ScreenId = ScreenId(23);
/// Pictures (folders and pictures). Arg: `DirId`.
pub const PICTURES: ScreenId = ScreenId(24);
/// The picture viewer. Arg: [`PictureScreen::arg`].
pub const PICTURE_VIEW: ScreenId = ScreenId(25);
/// A music-library list (Artists, an artist's albums, …). Arg:
/// [`LibraryScreen::arg`].
pub const LIBRARY: ScreenId = ScreenId(26);
/// The Bluetooth app (05 §7.4).
pub const BLUETOOTH: ScreenId = ScreenId(27);
/// Pair new device (inquiry results).
pub const BT_SCAN: ScreenId = ScreenId(28);
pub const BT_PAIRED: ScreenId = ScreenId(29);
/// Connecting. Arg: [`connect_arg`].
pub const BT_CONNECT: ScreenId = ScreenId(30);
/// Settings › System › Developer (hidden until unlocked in About; PWR-5).
pub const DEVELOPER: ScreenId = ScreenId(31);
/// The Radio app (05 §7.3).
pub const RADIO: ScreenId = ScreenId(32);
/// Radio › Presets….
pub const RADIO_PRESETS: ScreenId = ScreenId(33);
/// Steps (05 §7.7). Arg: 0 Today, 1 Week.
pub const STEPS: ScreenId = ScreenId(34);
/// Stopwatch · Timer · Alarm (05 §7.8). Arg: the tab (0, 1, 2).
pub const STOPWATCH: ScreenId = ScreenId(35);
/// An alarm's repeat days. Arg: the alarm.
pub const ALARM_DAYS: ScreenId = ScreenId(36);
/// The timer or an alarm ringing (pushed over any screen).
pub const ALERT: ScreenId = ScreenId(37);
/// The Authenticator (05 §7.9, OTP-2).
pub const AUTHENTICATOR: ScreenId = ScreenId(38);
/// The Recorder app (05 §7.5).
pub const RECORDER: ScreenId = ScreenId(40);
/// Recorder › Recordings…. Arg: the folder's `DirId`.
pub const RECORDINGS: ScreenId = ScreenId(41);
/// USB Drive (05 §7.10): the image folders.
pub const USB_DRIVE: ScreenId = ScreenId(42);
/// A folder of disk images. Arg: `DirId` (0 = card root, "Choose file…").
pub const DRIVE_FILES: ScreenId = ScreenId(43);
/// Mode / read-only / Connect. Arg: the image's row in the current listing.
pub const DRIVE_OPTIONS: ScreenId = ScreenId(44);
/// The image presented to the computer.
pub const DRIVE_STATUS: ScreenId = ScreenId(45);
/// Settings › System › Update firmware (UPD-1).
pub const UPDATE: ScreenId = ScreenId(46);
/// The Games app (05 §7.13, M14; feature `games`): the list of games.
#[cfg(feature = "games")]
pub const GAMES: ScreenId = ScreenId(47);
/// Blocks and 4096 being played.
#[cfg(feature = "games")]
pub const GAME_BLOCKS: ScreenId = ScreenId(48);
#[cfg(feature = "games")]
pub const GAME_TILES: ScreenId = ScreenId(49);
/// A game's pause menu. Arg: the game's engine id.
#[cfg(feature = "games")]
pub const GAME_PAUSE: ScreenId = ScreenId(50);
/// A game's settings. Arg: the game's engine id.
#[cfg(feature = "games")]
pub const GAME_SETTINGS: ScreenId = ScreenId(51);
/// How to play. Arg: the game's engine id.
#[cfg(feature = "games")]
pub const GAME_HELP: ScreenId = ScreenId(52);
/// Solitaire being played (GAME-2).
#[cfg(feature = "games")]
pub const GAME_SOLITAIRE: ScreenId = ScreenId(53);
/// Salvo being played (GAME-4/5).
#[cfg(feature = "salvo")]
pub const GAME_SALVO: ScreenId = ScreenId(54);
/// Bricks being played (GAME-6).
#[cfg(feature = "bricks")]
pub const GAME_BRICKS: ScreenId = ScreenId(55);
/// The Books app (05 §7.15, BOOK-1; feature `books`): the library.
#[cfg(feature = "books")]
pub const BOOKS: ScreenId = ScreenId(56);
/// The reader. Arg: the book's library row.
#[cfg(feature = "books")]
pub const BOOK_READ: ScreenId = ScreenId(57);
/// The reader menu (contents, bookmarks, go to, reading settings).
#[cfg(feature = "books")]
pub const BOOK_MENU: ScreenId = ScreenId(58);
#[cfg(feature = "books")]
pub const BOOK_TOC: ScreenId = ScreenId(59);
#[cfg(feature = "books")]
pub const BOOK_MARKS: ScreenId = ScreenId(60);
/// Go to a per cent of the book.
#[cfg(feature = "books")]
pub const BOOK_GOTO: ScreenId = ScreenId(61);

/// One variant per screen. Only the top screen exists, so the enum's size
/// (its largest variant, the list screen) is the UI's screen RAM.
#[allow(clippy::large_enum_variant)]
pub enum AppScreen {
    Boot(BootScreen),
    Home(HomeScreen),
    List(ListScreen),
    Slider(SliderScreen),
    Picker(PickerScreen),
    Lock(LockScreen),
    Notice(NoticeScreen),
    Charging(ChargingScreen),
    Goodbye(GoodbyeScreen),
    Files(FilesScreen),
    Text(TextScreen),
    NowPlaying(NowPlayingScreen),
    Equaliser(EqualiserScreen),
    Picture(PictureScreen),
    Library(LibraryScreen),
    Bluetooth(BluetoothScreen),
    BtConnect(BtConnectScreen),
    Radio(RadioScreen),
    RadioPresets(PresetsScreen),
    Steps(StepsScreen),
    Tools(ToolsScreen),
    AlarmDays(AlarmDaysScreen),
    Alert(AlertScreen),
    Auth(AuthScreen),
    Recorder(RecorderScreen),
    Drive(UsbDriveScreen),
    Update(UpdateScreen),
    #[cfg(feature = "games")]
    GameList(GameListScreen),
    #[cfg(feature = "games")]
    Game(GameScreen),
    #[cfg(feature = "games")]
    GameHelp(GameHelpScreen),
    #[cfg(feature = "books")]
    BookList(BookListScreen),
    #[cfg(feature = "books")]
    Reader(ReaderScreen),
    #[cfg(feature = "books")]
    BookGoto(GotoScreen),
}

macro_rules! each {
    ($self:ident, $s:ident => $e:expr) => {
        match $self {
            AppScreen::Boot($s) => $e,
            AppScreen::Home($s) => $e,
            AppScreen::List($s) => $e,
            AppScreen::Slider($s) => $e,
            AppScreen::Picker($s) => $e,
            AppScreen::Lock($s) => $e,
            AppScreen::Notice($s) => $e,
            AppScreen::Charging($s) => $e,
            AppScreen::Goodbye($s) => $e,
            AppScreen::Files($s) => $e,
            AppScreen::Text($s) => $e,
            AppScreen::NowPlaying($s) => $e,
            AppScreen::Equaliser($s) => $e,
            AppScreen::Picture($s) => $e,
            AppScreen::Library($s) => $e,
            AppScreen::Bluetooth($s) => $e,
            AppScreen::BtConnect($s) => $e,
            AppScreen::Radio($s) => $e,
            AppScreen::RadioPresets($s) => $e,
            AppScreen::Steps($s) => $e,
            AppScreen::Tools($s) => $e,
            AppScreen::AlarmDays($s) => $e,
            AppScreen::Alert($s) => $e,
            AppScreen::Auth($s) => $e,
            AppScreen::Recorder($s) => $e,
            AppScreen::Drive($s) => $e,
            AppScreen::Update($s) => $e,
            #[cfg(feature = "games")]
            AppScreen::GameList($s) => $e,
            #[cfg(feature = "games")]
            AppScreen::Game($s) => $e,
            #[cfg(feature = "games")]
            AppScreen::GameHelp($s) => $e,
            #[cfg(feature = "books")]
            AppScreen::BookList($s) => $e,
            #[cfg(feature = "books")]
            AppScreen::Reader($s) => $e,
            #[cfg(feature = "books")]
            AppScreen::BookGoto($s) => $e,
        }
    };
}

/// One screen's event handler, never inlined into [`AppScreen`]'s
/// dispatch: inlined, all 26 handlers shared one stack frame as large as
/// their locals together (3,448 B on the player, VER-7 stack check
/// 2026-11-25); apart, a key costs only the open screen's own frame.
#[inline(never)]
fn event_of<S: Services + ?Sized, T: Screen<S>>(s: &mut T, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
    s.event(cx, ev)
}

/// As [`event_of`], for entering a screen.
#[inline(never)]
fn enter_of<S: Services + ?Sized, T: Screen<S>>(s: &mut T, cx: &mut Cx<'_, S>, arg: Arg) {
    s.enter(cx, arg);
}

impl<S: Services + ?Sized> Screen<S> for AppScreen {
    fn enter(&mut self, cx: &mut Cx<'_, S>, arg: Arg) {
        each!(self, s => enter_of(s, cx, arg));
    }
    fn event(&mut self, cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        each!(self, s => event_of(s, cx, ev))
    }
    fn draw(&mut self, cx: &Cx<'_, S>, g: &mut Gfx<'_>, dirty: Dirty) {
        each!(self, s => s.draw(cx, g, dirty));
    }
    fn menu(&self, cx: &Cx<'_, S>, out: &mut MenuBuilder) {
        each!(self, s => s.menu(cx, out));
    }
    fn wants(&self) -> Wants {
        each!(self, s => Screen::<S>::wants(s))
    }
    fn save(&self) -> Resume {
        each!(self, s => Screen::<S>::save(s))
    }
    fn restore(&mut self, cx: &mut Cx<'_, S>, r: &Resume) {
        each!(self, s => s.restore(cx, r));
    }
}

impl<S: Services + ?Sized> ScreenSet<S> for AppScreen {
    const HOME: ScreenId = HOME;
    const QUICK_PANEL: Option<ScreenId> = Some(QUICK_PANEL);
    const LOCK_SCREEN: Option<ScreenId> = Some(LOCK);
    const POWER_MENU: Option<ScreenId> = Some(POWER_MENU);

    fn create(sid: ScreenId) -> Self {
        match sid {
            BOOT => AppScreen::Boot(BootScreen::default()),
            SETTINGS => AppScreen::List(ListScreen::new(ListKind::Root)),
            GROUP => AppScreen::List(ListScreen::new(ListKind::Group(0))),
            CHOICE => AppScreen::List(ListScreen::new(ListKind::Choice(id::THEME))),
            INFO => AppScreen::List(ListScreen::new(ListKind::Info(crate::settings::Info::About))),
            HOME_APPS => AppScreen::List(ListScreen::new(ListKind::HomeApps)),
            POWER_MENU => AppScreen::List(ListScreen::new(ListKind::PowerMenu)),
            QUICK_PANEL => AppScreen::List(ListScreen::new(ListKind::Quick)),
            USB_CONNECT => AppScreen::List(ListScreen::new(ListKind::Usb)),
            SLIDER => AppScreen::Slider(SliderScreen::default()),
            PICKER => AppScreen::Picker(PickerScreen::default()),
            LOCK => AppScreen::Lock(LockScreen::default()),
            NOTICE => AppScreen::Notice(NoticeScreen::default()),
            CHARGING => AppScreen::Charging(ChargingScreen::default()),
            GOODBYE => AppScreen::Goodbye(GoodbyeScreen::default()),
            FILES => AppScreen::Files(FilesScreen::new(BrowseMode::Files)),
            MUSIC_FOLDERS => AppScreen::Files(FilesScreen::new(BrowseMode::Music)),
            PICTURES => AppScreen::Files(FilesScreen::new(BrowseMode::Pictures)),
            PICTURE_VIEW => AppScreen::Picture(PictureScreen::default()),
            TEXT_VIEW => AppScreen::Text(TextScreen::default()),
            MUSIC => AppScreen::List(ListScreen::new(ListKind::MusicRoot)),
            NOW_PLAYING => AppScreen::NowPlaying(NowPlayingScreen::default()),
            EQUALISER => AppScreen::Equaliser(EqualiserScreen::default()),
            PROPERTIES => AppScreen::List(ListScreen::new(ListKind::Properties(0))),
            BOOKMARKS => AppScreen::List(ListScreen::new(ListKind::Bookmarks)),
            DEVELOPER => AppScreen::List(ListScreen::new(ListKind::Developer)),
            LIBRARY => AppScreen::Library(LibraryScreen::default()),
            BLUETOOTH => AppScreen::Bluetooth(BluetoothScreen::new(BtView::Home)),
            BT_SCAN => AppScreen::Bluetooth(BluetoothScreen::new(BtView::Scan)),
            BT_PAIRED => AppScreen::Bluetooth(BluetoothScreen::new(BtView::Paired)),
            BT_CONNECT => AppScreen::BtConnect(BtConnectScreen::default()),
            RADIO => AppScreen::Radio(RadioScreen::default()),
            RADIO_PRESETS => AppScreen::RadioPresets(PresetsScreen::default()),
            STEPS => AppScreen::Steps(StepsScreen::default()),
            STOPWATCH => AppScreen::Tools(ToolsScreen::default()),
            ALARM_DAYS => AppScreen::AlarmDays(AlarmDaysScreen::default()),
            ALERT => AppScreen::Alert(AlertScreen::default()),
            AUTHENTICATOR => AppScreen::Auth(AuthScreen::default()),
            RECORDER => AppScreen::Recorder(RecorderScreen::default()),
            RECORDINGS => AppScreen::Files(FilesScreen::new(BrowseMode::Recordings)),
            USB_DRIVE => AppScreen::Drive(UsbDriveScreen::new(DriveView::Home)),
            DRIVE_FILES => AppScreen::Files(FilesScreen::new(BrowseMode::DiskImages)),
            DRIVE_OPTIONS => AppScreen::Drive(UsbDriveScreen::new(DriveView::Options)),
            DRIVE_STATUS => AppScreen::Drive(UsbDriveScreen::new(DriveView::Status)),
            UPDATE => AppScreen::Update(UpdateScreen::default()),
            #[cfg(feature = "games")]
            GAMES => AppScreen::GameList(GameListScreen::new(GameList::Games)),
            #[cfg(feature = "games")]
            GAME_BLOCKS => AppScreen::Game(GameScreen::new(crate::games::Game::Blocks)),
            #[cfg(feature = "games")]
            GAME_TILES => AppScreen::Game(GameScreen::new(crate::games::Game::Tiles)),
            #[cfg(feature = "games")]
            GAME_SOLITAIRE => AppScreen::Game(GameScreen::new(crate::games::Game::Solitaire)),
            #[cfg(feature = "salvo")]
            GAME_SALVO => AppScreen::Game(GameScreen::new(crate::games::Game::Salvo)),
            #[cfg(feature = "bricks")]
            GAME_BRICKS => AppScreen::Game(GameScreen::new(crate::games::Game::Bricks)),
            #[cfg(feature = "games")]
            GAME_PAUSE => {
                AppScreen::GameList(GameListScreen::new(GameList::Pause(crate::games::Game::Blocks)))
            }
            #[cfg(feature = "games")]
            GAME_SETTINGS => {
                AppScreen::GameList(GameListScreen::new(GameList::Settings(crate::games::Game::Blocks)))
            }
            #[cfg(feature = "games")]
            GAME_HELP => AppScreen::GameHelp(GameHelpScreen::default()),
            #[cfg(feature = "books")]
            BOOKS => AppScreen::BookList(BookListScreen::new(BookList::Library)),
            #[cfg(feature = "books")]
            BOOK_MENU => AppScreen::BookList(BookListScreen::new(BookList::Menu)),
            #[cfg(feature = "books")]
            BOOK_TOC => AppScreen::BookList(BookListScreen::new(BookList::Contents)),
            #[cfg(feature = "books")]
            BOOK_MARKS => AppScreen::BookList(BookListScreen::new(BookList::Bookmarks)),
            #[cfg(feature = "books")]
            BOOK_READ => AppScreen::Reader(ReaderScreen::default()),
            #[cfg(feature = "books")]
            BOOK_GOTO => AppScreen::BookGoto(GotoScreen::default()),
            _ => AppScreen::Home(HomeScreen::default()),
        }
    }

    fn id(&self) -> ScreenId {
        match self {
            AppScreen::Boot(_) => BOOT,
            AppScreen::Home(_) => HOME,
            AppScreen::List(l) => l.screen_id(),
            AppScreen::Slider(_) => SLIDER,
            AppScreen::Picker(_) => PICKER,
            AppScreen::Lock(_) => LOCK,
            AppScreen::Notice(_) => NOTICE,
            AppScreen::Charging(_) => CHARGING,
            AppScreen::Goodbye(_) => GOODBYE,
            AppScreen::Files(f) => match f.mode() {
                BrowseMode::Files => FILES,
                BrowseMode::Music => MUSIC_FOLDERS,
                BrowseMode::Pictures => PICTURES,
                BrowseMode::Recordings => RECORDINGS,
                BrowseMode::DiskImages => DRIVE_FILES,
            },
            AppScreen::Text(_) => TEXT_VIEW,
            AppScreen::NowPlaying(_) => NOW_PLAYING,
            AppScreen::Equaliser(_) => EQUALISER,
            AppScreen::Picture(_) => PICTURE_VIEW,
            AppScreen::Library(_) => LIBRARY,
            AppScreen::Bluetooth(b) => b.screen_id(),
            AppScreen::BtConnect(_) => BT_CONNECT,
            AppScreen::Radio(_) => RADIO,
            AppScreen::RadioPresets(_) => RADIO_PRESETS,
            AppScreen::Steps(_) => STEPS,
            AppScreen::Tools(_) => STOPWATCH,
            AppScreen::AlarmDays(_) => ALARM_DAYS,
            AppScreen::Alert(_) => ALERT,
            AppScreen::Auth(_) => AUTHENTICATOR,
            AppScreen::Recorder(_) => RECORDER,
            AppScreen::Drive(d) => d.screen_id(),
            AppScreen::Update(_) => UPDATE,
            #[cfg(feature = "games")]
            AppScreen::GameList(l) => l.screen_id(),
            #[cfg(feature = "games")]
            AppScreen::Game(g) => g.screen_id(),
            #[cfg(feature = "games")]
            AppScreen::GameHelp(_) => GAME_HELP,
            #[cfg(feature = "books")]
            AppScreen::BookList(l) => l.screen_id(),
            #[cfg(feature = "books")]
            AppScreen::Reader(_) => BOOK_READ,
            #[cfg(feature = "books")]
            AppScreen::BookGoto(_) => BOOK_GOTO,
        }
    }

    fn name(&self) -> &'static str {
        match self {
            AppScreen::Boot(_) => "Boot",
            AppScreen::Home(_) => "Home",
            AppScreen::List(l) => l.name(),
            AppScreen::Slider(_) => "Slider",
            AppScreen::Picker(_) => "Picker",
            AppScreen::Lock(_) => "Lock",
            AppScreen::Notice(_) => "Notice",
            AppScreen::Charging(_) => "Charging",
            AppScreen::Goodbye(_) => "Goodbye",
            AppScreen::Files(f) => match f.mode() {
                BrowseMode::Files => "Files",
                BrowseMode::Music => "Folders",
                BrowseMode::Pictures => "Pictures",
                BrowseMode::Recordings => "Recordings",
                BrowseMode::DiskImages => "DriveFiles",
            },
            AppScreen::Text(_) => "TextViewer",
            AppScreen::NowPlaying(_) => "NowPlaying",
            AppScreen::Equaliser(_) => "Equaliser",
            AppScreen::Picture(_) => "PictureViewer",
            AppScreen::Library(_) => "MusicLibrary",
            AppScreen::Bluetooth(b) => b.name(),
            AppScreen::BtConnect(_) => "BtConnect",
            AppScreen::Radio(_) => "Radio",
            AppScreen::RadioPresets(_) => "RadioPresets",
            AppScreen::Steps(_) => "Steps",
            AppScreen::Tools(_) => "Stopwatch",
            AppScreen::AlarmDays(_) => "AlarmDays",
            AppScreen::Alert(_) => "Alert",
            AppScreen::Auth(_) => "Authenticator",
            AppScreen::Recorder(_) => "Recorder",
            AppScreen::Drive(d) => d.name(),
            AppScreen::Update(_) => "UpdateFirmware",
            #[cfg(feature = "games")]
            AppScreen::GameList(l) => l.name(),
            #[cfg(feature = "games")]
            AppScreen::Game(g) => g.name(),
            #[cfg(feature = "games")]
            AppScreen::GameHelp(_) => "GameHelp",
            #[cfg(feature = "books")]
            AppScreen::BookList(l) => l.name(),
            #[cfg(feature = "books")]
            AppScreen::Reader(_) => "Reader",
            #[cfg(feature = "books")]
            AppScreen::BookGoto(_) => "BookGoTo",
        }
    }

    /// Low battery (05 §6.1), computer connected (§6.5), card removed,
    /// theme and language changes.
    fn global(cx: &mut Cx<'_, S>, ev: &Event) -> Nav {
        let Event::Service(se) = *ev else { return Nav::Stay };
        let lang = cx.svc.lang();
        match se {
            ServiceEvent::Battery { percent, charging: false } => match percent {
                15 | 10 => {
                    let mut b = orpheus_ui::text::StrBuf::<48>::new();
                    b.push_str(lang.get(Str::BatteryLow));
                    b.push_str(" · ");
                    b.push_u32(u32::from(percent));
                    b.push_str(" %");
                    cx.ui.toast(b.as_str());
                    Nav::Stay
                }
                5 => Nav::Push(NOTICE, Arg(Notice::BatteryLow.arg())),
                _ => Nav::Stay,
            },
            ServiceEvent::Usb { host: true } => match cx.svc.setting(id::USB_WHEN_CONNECTED) {
                0 => Nav::Push(USB_CONNECT, Arg(0)),
                // Present disk image…: the USB Drive app chooses the image.
                4 => Nav::Push(USB_DRIVE, Arg(0)),
                n => {
                    let mode = match n {
                        2 => UsbMode::FileTransfer,
                        3 => UsbMode::Keyboard,
                        _ => UsbMode::ChargeOnly,
                    };
                    cx.svc.select_usb(mode);
                    Nav::Stay
                }
            },
            ServiceEvent::Other { kind: other::BLUETOOTH, value } => {
                // A connection made or lost, also by a reconnect or the
                // headset itself: say so wherever the user is.
                match value {
                    bt_event::CONNECTED => {
                        let mut b = orpheus_ui::text::StrBuf::<48>::new();
                        b.push_str(lang.get(Str::BtConnected));
                        let name = cx.svc.bt_device();
                        if !name.is_empty() {
                            b.push_str(" · ");
                            b.push_str_or_ellipsis(name);
                        }
                        cx.ui.toast(b.as_str());
                    }
                    bt_event::DISCONNECTED => cx.ui.toast(lang.get(Str::BtDisconnected)),
                    _ => {}
                }
                Nav::Stay
            }
            ServiceEvent::Other { kind: other::RADIO, value: radio_event::SCANNED } => {
                // An auto-scan ended (also when the user left the app).
                let n = cx.svc.presets().len();
                if n == 0 {
                    cx.ui.toast(lang.get(Str::NoStations));
                } else {
                    let mut b = orpheus_ui::text::StrBuf::<48>::new();
                    b.push_u32(u32::try_from(n).unwrap_or(0));
                    b.push(' ');
                    b.push_str(lang.get(Str::StationsFound));
                    cx.ui.toast(b.as_str());
                }
                Nav::Stay
            }
            // The timer or an alarm rings: the alert goes over whatever
            // is shown (an older alert under it closes itself).
            ServiceEvent::Other { kind: other::TOOLS, value: tools_event::ALERT } => Nav::Push(ALERT, Arg(0)),
            ServiceEvent::CardRemoved => {
                cx.ui.toast(lang.get(Str::CardRemoved));
                Nav::Stay
            }
            ServiceEvent::SettingChanged(k) if [id::THEME.0, id::ACCENT.0, id::LANGUAGE.0].contains(&k) => {
                cx.ui.invalidate_all();
                Nav::Stay
            }
            _ => Nav::Stay,
        }
    }
}
