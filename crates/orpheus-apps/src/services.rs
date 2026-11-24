//! Service traits the screens talk to (05 §5.1). All methods are
//! synchronous and cheap: the firmware's UI task answers them from state it
//! caches from the owning tasks, and turns the `&mut self` requests into
//! channel messages. Changes arrive at the screens as
//! `Event::Service(ServiceEvent::…)`.

use orpheus_power::charger::ChargeState;
use orpheus_ui::event::{BtState, Source};
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::Gfx;
use orpheus_ui::shell::System;
use orpheus_ui::theme::{Accent, Palette, Theme};
use orpheus_ui::widgets::bars::{MiniPlayer, Status};

use crate::App;
use crate::settings::{SettingId, id};
use crate::strings::Lang;

/// Typed settings (05 §7.11); values are those of [`crate::settings`].
pub trait SettingsService {
    /// Current validated value (the default if never set).
    fn setting(&self, id: SettingId) -> i16;
    /// Apply now; the store write is the firmware's (debounced). The
    /// firmware also reacts to the keys it owns (brightness, theme,
    /// Bluetooth, …) and sends `ServiceEvent::SettingChanged`.
    fn set_setting(&mut self, id: SettingId, value: i16);
    /// Every setting back to its default (the card is not touched).
    fn factory_reset(&mut self);
}

/// Battery as the UI sees it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct BatteryInfo {
    pub percent: u8,
    pub millivolts: u16,
    pub charge: ChargeState,
}

impl BatteryInfo {
    #[must_use]
    pub fn charging(&self) -> bool {
        matches!(self.charge, ChargeState::Charging | ChargeState::Trickle | ChargeState::Detecting)
    }
}

/// Things only the power task can do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PowerRequest {
    /// Save state, fade audio, PMU off.
    PowerOff,
    Restart,
    /// Dual-boot layout: restart into the stock firmware.
    StartOriginal,
    /// Restart into ADFU.
    Recovery,
    /// Lock the keys, leave the screen on.
    LockKeys,
    /// The lock screen's 2 s are over: back to the previous screen state.
    LockNoticeDone,
    /// Leave the charging screen and start the player.
    StartFromCharging,
}

pub trait PowerService {
    fn battery(&self) -> BatteryInfo;
    /// Minutes left on the sleep timer, if running.
    fn sleep_timer_left(&self) -> Option<u16>;
    fn power(&mut self, req: PowerRequest);
}

/// Local wall-clock time.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DateTime {
    pub year: u16,
    /// 1..=12
    pub month: u8,
    /// 1..=31
    pub day: u8,
    pub hour: u8,
    pub minute: u8,
    pub second: u8,
    /// 0 = Monday.
    pub weekday: u8,
}

pub trait ClockService {
    fn now(&self) -> DateTime;
    /// Set the RTC (local time; the UTC offset setting converts).
    fn set_now(&mut self, t: DateTime);
    /// False if the RTC lost power since it was last set.
    fn clock_valid(&self) -> bool;
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CardState {
    Absent,
    Mounting,
    Ready,
    Unreadable,
    /// Lent to a computer (file transfer).
    UsbOwned,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FsKind {
    Fat32,
    ExFat,
    Other,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CardInfo {
    pub capacity_mib: u32,
    pub free_mib: u32,
    pub fs: FsKind,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StorageRequest {
    RebuildLibrary,
    ClearCaches,
    /// Read-only scan.
    CheckCard,
    /// Erase and create a file system (confirmed by the user).
    Format,
}

pub trait StorageService {
    fn card(&self) -> CardState;
    fn card_info(&self) -> Option<CardInfo>;
    fn storage(&mut self, req: StorageRequest);
}

/// What plays (status bar, mini-player, lock screen, quick panel).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Playing<'a> {
    pub source: Source,
    pub title: &'a str,
    pub artist: &'a str,
    /// 0..=1000
    pub progress: u16,
}

pub trait PlaybackService {
    /// `None` when nothing is loaded.
    fn playing(&self) -> Option<Playing<'_>>;
    /// Step the volume; returns (level, max).
    fn volume_step(&mut self, delta: i8) -> (u8, u8);
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UsbState {
    Detached,
    /// Power only.
    Charger,
    /// A computer enumerated us.
    Host,
}

/// USB personalities offered by the connect dialog (05 §6.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UsbMode {
    ChargeOnly,
    FileTransfer,
    DiskImage,
    Keyboard,
    Audio,
}

pub trait UsbService {
    fn usb(&self) -> UsbState;
    fn select_usb(&mut self, mode: UsbMode);
}

pub trait BluetoothService {
    fn bt(&self) -> BtState;
    /// Name of the connected (or last) device, `""` if none.
    fn bt_device(&self) -> &str;
}

pub trait SystemService {
    /// The active language (the firmware switches packs when the Language
    /// setting changes).
    fn lang(&self) -> Lang;
    /// Languages available: index 0 is English.
    fn language_count(&self) -> usize;
    fn language(&self, i: usize) -> Lang;
    fn version(&self) -> &str;
    fn build_id(&self) -> &str;
    fn serial(&self) -> &str;
    fn hardware(&self) -> &str;
    /// Apps whose hardware or code is absent are hidden (05 §6.2).
    fn app_available(&self, app: App) -> bool;
    /// Dual-boot layout: "Start original firmware" is offered.
    fn dual_boot(&self) -> bool;
    fn usb_audio_built(&self) -> bool;
    /// Keys are locked (status bar).
    fn keys_locked(&self) -> bool;
}

/// Everything the apps need. Implemented automatically for any type with
/// all the parts; the toolkit's `System` (status bar, volume, mini-player)
/// can be implemented with [`status`], [`mini_player`] and
/// `PlaybackService::volume_step`.
pub trait Services:
    System
    + BrowseService
    + TextService
    + PlayerService
    + LibraryService
    + PictureService
    + SettingsService
    + PowerService
    + ClockService
    + StorageService
    + PlaybackService
    + UsbService
    + BluetoothService
    + SystemService
{
}

impl<T> Services for T where
    T: System
        + BrowseService
        + TextService
        + PlayerService
        + LibraryService
        + PictureService
        + SettingsService
        + PowerService
        + ClockService
        + StorageService
        + PlaybackService
        + UsbService
        + BluetoothService
        + SystemService
{
}

/// The status bar contents from the services.
pub fn status<S>(s: &S) -> Status
where
    S: SettingsService
        + PowerService
        + ClockService
        + PlaybackService
        + BluetoothService
        + SystemService
        + ?Sized,
{
    let b = s.battery();
    let t = s.now();
    Status {
        // No time while the clock is not set (the RTC can stop): the bar
        // shows the screen title rather than a misleading 00:00.
        clock: (s.setting(id::CLOCK_IN_BAR) != 0 && s.clock_valid()).then(|| {
            let h = if s.setting(id::CLOCK_24H) != 0 { t.hour } else { (t.hour + 11) % 12 + 1 };
            (h, t.minute)
        }),
        battery: b.percent,
        charging: b.charging(),
        bt: s.bt(),
        source: s.playing().map_or(Source::Idle, |p| p.source),
        locked: s.keys_locked(),
        sleep_timer: s.sleep_timer_left().is_some(),
        alarm: false,
        activity: false,
    }
}

/// The mini-player from the playback service.
pub fn mini_player<S: PlaybackService + ?Sized>(s: &S) -> Option<MiniPlayer<'_>> {
    s.playing().filter(|p| p.source != Source::Idle).map(|p| MiniPlayer {
        title: p.title,
        playing: matches!(p.source, Source::Playing | Source::Radio | Source::Recording),
        progress: p.progress,
    })
}

/// The palette chosen in Settings → Display.
pub fn palette<S: SettingsService + ?Sized>(s: &S) -> Palette {
    let theme = match s.setting(id::THEME) {
        1 => Theme::Light,
        2 => Theme::Amber,
        _ => Theme::Dark,
    };
    let accent = match s.setting(id::ACCENT) {
        1 => Accent::Green,
        2 => Accent::Pink,
        3 => Accent::Red,
        4 => Accent::Grey,
        _ => Accent::Blue,
    };
    theme.palette(accent)
}

// ---------------------------------------------------------------- Files

pub use crate::browse::{DirId, EntryKind, FileRef};

/// `Event::Data` tags the services answer with.
pub mod data {
    /// Rows of the current listing arrived, or it was re-sorted.
    pub const BROWSE: u16 = 1;
    /// Queue rows arrived.
    pub const QUEUE: u16 = 2;
    /// The text window moved.
    pub const TEXT: u16 = 3;
    /// Lyrics of the current track were loaded.
    pub const LYRICS: u16 = 4;
    /// The picture asked for with `PictureService::show_picture` is in the
    /// frame buffer (or could not be shown): see `picture_state`.
    pub const PICTURE: u16 = 5;
    /// Rows of the current music-library list arrived (`LibraryService`).
    pub const LIBRARY: u16 = 6;
}

/// `ServiceEvent::Other { kind, .. }` kinds the services send.
pub mod other {
    /// The music library changed state (`PlayerService::library`): it
    /// was opened, is being built, or a build finished.
    pub const LIBRARY: u16 = 1;
}

/// Browser order (Files menu › Sort).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SortBy {
    #[default]
    Name,
    Date,
    Size,
}

/// What a listing shows.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BrowseFilter {
    pub sort: SortBy,
    pub hidden: bool,
    /// Music › Folders: directories, audio and playlists only.
    pub audio_only: bool,
    /// Pictures: directories and pictures only.
    pub images_only: bool,
}

impl BrowseFilter {
    /// Whether an entry of `kind` is listed (hidden entries aside).
    #[must_use]
    pub fn keeps(&self, kind: EntryKind) -> bool {
        (!self.audio_only || matches!(kind, EntryKind::Dir | EntryKind::Audio | EntryKind::Playlist))
            && (!self.images_only || matches!(kind, EntryKind::Dir | EntryKind::Image))
    }
}

/// One row of a listing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Entry<'a> {
    pub name: &'a str,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: DateTime,
    pub file: FileRef,
    /// For directories: the id to browse into.
    pub child: DirId,
}

/// The current listing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Listing {
    pub dir: DirId,
    /// Entries (after filtering) known so far.
    pub count: u32,
    /// False while the rows are in on-disk order (directory index being
    /// built, 04 §6.2); an `Event::Data(BROWSE)` follows when sorted.
    pub sorted: bool,
    /// False while `count` may still grow.
    pub complete: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FileOp {
    Delete(FileRef),
}

/// Directory browsing over a windowed, indexed listing (05 §7.2, 04 §6.2).
/// Nothing here holds a whole directory: `entry` answers from a window of
/// rows the service keeps, `want` moves the window.
pub trait BrowseService {
    /// Make `dir` the current listing (cheap if it already is).
    fn browse(&mut self, dir: DirId, filter: BrowseFilter) -> Listing;
    fn listing(&self) -> Listing;
    /// Row `i` in display order, if loaded (else draw a placeholder).
    fn entry(&self, i: u32) -> Option<Entry<'_>>;
    /// The UI shows rows `[first, first + count)`; load around them and
    /// answer with `Event::Data { tag: data::BROWSE, .. }`.
    fn want(&mut self, first: u32, count: u32);
    /// Where `f` is in the current listing (focus survives re-sorting).
    fn position(&self, f: FileRef) -> Option<u32>;
    /// The file the UI has focused in the current listing. A service that
    /// sorts in the background loads the window around it when the sorted
    /// order arrives, so that [`BrowseService::position`] finds it then.
    fn focus_hint(&mut self, _f: FileRef) {}
    /// Letter of row `i` ([`crate::browse::letter_of`]), if loaded.
    fn letter(&self, i: u32) -> Option<char>;
    /// Letter jump from row `from` (see `ListSource::seek_letter`),
    /// answered from the index, not by scanning.
    fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)>;
    /// Name of a directory (`""` for the root).
    fn dir_name(&self, dir: DirId) -> &str;
    fn file_op(&mut self, op: FileOp) -> bool;
}

/// Plain-text files for the text viewer, read through a window.
pub trait TextService {
    /// Open `f` (closing the previous one); `None` if it cannot be read.
    /// Returns the file size.
    fn text_open(&mut self, f: FileRef) -> Option<u32>;
    /// The loaded window: (file offset of `data[0]`, data).
    fn text_window(&self) -> (u32, &[u8]);
    /// Load the window around `offset`; answer with `Event::Data(TEXT)`.
    fn want_text(&mut self, offset: u32);
    /// Reading position remembered for `f` (05 §7.2), 0 if none.
    fn text_position(&self, f: FileRef) -> u32;
    fn set_text_position(&mut self, f: FileRef, offset: u32);
}

// ---------------------------------------------------------------- Player

/// Playback state for Now Playing.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct PlayerState {
    /// `Idle` when nothing is queued.
    pub source: Source,
    pub position_ms: u32,
    pub duration_ms: u32,
    /// Position of the current track in the queue (play order).
    pub queue_pos: u32,
    pub queue_len: u32,
    /// A–B repeat points.
    pub a_ms: Option<u32>,
    pub b_ms: Option<u32>,
    /// Stream ring fill and decoder CPU load, percent (Details view).
    pub buffer_pct: u8,
    pub cpu_pct: u8,
    /// Album art of the current track: 0 while there is none (yet), else
    /// a value that changes when it does (Now Playing redraws then and
    /// asks for it with `PictureService::show_picture`).
    pub art: u32,
}

/// The current track.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct TrackInfo<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: &'a str,
    pub file: FileRef,
    pub path: &'a str,
    pub size: u64,
    /// "MP3", "FLAC", …
    pub codec: &'a str,
    pub sample_rate: u32,
    pub bits: u8,
    pub channels: u8,
    pub bitrate_kbps: u16,
    /// ReplayGain applied, hundredths of a dB.
    pub gain_cdb: Option<i32>,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QueueEntry<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub duration_ms: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LyricLine<'a> {
    /// `None` for unsynchronised lyrics.
    pub time_ms: Option<u32>,
    pub text: &'a str,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bookmark<'a> {
    pub title: &'a str,
    pub position_ms: u32,
}

/// Peak meters and spectrum, 0..=255 (FFT engine, 04 §5).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Levels {
    pub peak: [u8; 2],
    pub hold: [u8; 2],
    pub bands: [u8; 16],
}

/// Album art sizes the player provides, decoded and scaled (RGB565).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ArtSize {
    /// 52 × 52 (Standard view).
    Small,
    /// 128 × 128 (Big art).
    Full,
}

impl ArtSize {
    #[must_use]
    pub const fn px(self) -> i16 {
        match self {
            ArtSize::Small => 52,
            ArtSize::Full => 128,
        }
    }
}

/// The tag library (04 §6.4).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibraryState {
    Missing,
    Building { files: u32 },
    Ready,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PlayerCmd {
    PlayPause,
    /// Stop and forget the resume point (OK held).
    Stop,
    Next,
    Prev,
    SeekTo(u32),
    SeekBy(i32),
    /// Play queue position `n`.
    Jump(u32),
    Remove(u32),
    MoveUp(u32),
    MoveDown(u32),
    /// Queue a directory (Files order) and play, from `start` if given.
    PlayDir {
        dir: DirId,
        start: Option<FileRef>,
        shuffle: bool,
    },
    ShuffleAll,
    /// Queue the tracks of a music-library track list (05 §7.1) and play,
    /// from list row `start` if given (else the first, or a random one
    /// when shuffled).
    PlayLib {
        list: LibList,
        start: Option<u32>,
        shuffle: bool,
    },
    PlayNext(FileRef),
    AddToQueue(FileRef),
    SetA,
    SetB,
    ClearAb,
    Bookmark,
    Favourite,
    PlayBookmark(u32),
    Resume,
}

/// The audio task's state as the UI sees it (04 §2, §6.5). Shuffle,
/// repeat, speed and EQ are settings; the player follows them.
pub trait PlayerService {
    fn player(&self) -> PlayerState;
    fn track(&self) -> Option<TrackInfo<'_>>;
    /// Queue row `i` (play order), if loaded.
    fn queue_entry(&self, i: u32) -> Option<QueueEntry<'_>>;
    /// Answer with `Event::Data(QUEUE)`.
    fn want_queue(&mut self, first: u32, count: u32);
    /// Lines of the current track's lyrics (`.lrc` beside the file or
    /// embedded); 0 if none.
    fn lyrics_count(&self) -> u32;
    fn lyric(&self, i: u32) -> Option<LyricLine<'_>>;
    fn levels(&self) -> Levels;
    /// Title of the resume point when stopped ("Resume ‹title›").
    fn resume_title(&self) -> Option<&str>;
    fn bookmark_count(&self) -> u32;
    fn bookmark(&self, i: u32) -> Option<Bookmark<'_>>;
    fn library(&self) -> LibraryState;
    fn command(&mut self, c: PlayerCmd);
}

// ---------------------------------------------------------------- Library

/// A list of the music library (05 §7.1; `orpheus_library::List`). The
/// number is a row of the parent's table (an artist, album, genre, …).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibList {
    Artists,
    /// Albums of an artist (by year).
    ArtistAlbums(u32),
    /// Every track of an artist.
    ArtistTracks(u32),
    Albums,
    AlbumTracks(u32),
    /// All tracks by title.
    Songs,
    Genres,
    GenreArtists(u32),
    GenreArtistAlbums(u32),
    GenreAlbumTracks(u32),
    GenreTracks(u32),
    GenreArtistTracks(u32),
    /// Newest first.
    Recent,
}

/// Rows of Recently added the services show (the newest).
pub const RECENT_ROWS: u32 = 100;

/// What a library row is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LibKind {
    Artist,
    Album,
    Genre,
    GenreArtist,
    GenreAlbum,
    Track,
}

/// Bits of a table row in [`LibList::code`] (16 M rows).
const LIB_ID_BITS: u32 = 24;
const LIB_ID_MASK: u32 = (1 << LIB_ID_BITS) - 1;

impl LibList {
    /// A number for navigation arguments and resume records: the kind in
    /// bits 24..28, the row below.
    #[must_use]
    pub const fn code(self) -> u32 {
        let (k, id) = match self {
            LibList::Artists => (0, 0),
            LibList::ArtistAlbums(i) => (1, i),
            LibList::ArtistTracks(i) => (2, i),
            LibList::Albums => (3, 0),
            LibList::AlbumTracks(i) => (4, i),
            LibList::Songs => (5, 0),
            LibList::Genres => (6, 0),
            LibList::GenreArtists(i) => (7, i),
            LibList::GenreArtistAlbums(i) => (8, i),
            LibList::GenreAlbumTracks(i) => (9, i),
            LibList::GenreTracks(i) => (10, i),
            LibList::GenreArtistTracks(i) => (11, i),
            LibList::Recent => (12, 0),
        };
        (k << LIB_ID_BITS) | (id & LIB_ID_MASK)
    }

    /// The list of [`LibList::code`]`(c)`.
    #[must_use]
    pub const fn from_code(c: u32) -> Option<LibList> {
        let id = c & LIB_ID_MASK;
        Some(match c >> LIB_ID_BITS {
            0 => LibList::Artists,
            1 => LibList::ArtistAlbums(id),
            2 => LibList::ArtistTracks(id),
            3 => LibList::Albums,
            4 => LibList::AlbumTracks(id),
            5 => LibList::Songs,
            6 => LibList::Genres,
            7 => LibList::GenreArtists(id),
            8 => LibList::GenreArtistAlbums(id),
            9 => LibList::GenreAlbumTracks(id),
            10 => LibList::GenreTracks(id),
            11 => LibList::GenreArtistTracks(id),
            12 => LibList::Recent,
            _ => return None,
        })
    }

    /// The list that opens from a row of kind `kind` and table row `id`
    /// (`None` for a track).
    #[must_use]
    pub const fn open(kind: LibKind, id: u32) -> Option<LibList> {
        Some(match kind {
            LibKind::Artist => LibList::ArtistAlbums(id),
            LibKind::Album => LibList::AlbumTracks(id),
            LibKind::Genre => LibList::GenreArtists(id),
            LibKind::GenreArtist => LibList::GenreArtistAlbums(id),
            LibKind::GenreAlbum => LibList::GenreAlbumTracks(id),
            LibKind::Track => return None,
        })
    }

    /// The tracks of a row (Play / Shuffle on an artist, album, genre).
    #[must_use]
    pub const fn tracks_of(kind: LibKind, id: u32) -> Option<LibList> {
        Some(match kind {
            LibKind::Artist => LibList::ArtistTracks(id),
            LibKind::Album => LibList::AlbumTracks(id),
            LibKind::Genre => LibList::GenreTracks(id),
            LibKind::GenreArtist => LibList::GenreArtistTracks(id),
            LibKind::GenreAlbum => LibList::GenreAlbumTracks(id),
            LibKind::Track => return None,
        })
    }

    /// Every track under this list's parent (Play all in an artist's
    /// albums, a genre's artists, …); a track list is its own.
    #[must_use]
    pub const fn all_tracks(self) -> Option<LibList> {
        match self {
            LibList::ArtistAlbums(a) => Some(LibList::ArtistTracks(a)),
            LibList::GenreArtists(g) => Some(LibList::GenreTracks(g)),
            LibList::GenreArtistAlbums(g) => Some(LibList::GenreArtistTracks(g)),
            l if l.is_tracks() => Some(l),
            _ => None,
        }
    }

    /// Whether the rows are tracks.
    #[must_use]
    pub const fn is_tracks(self) -> bool {
        matches!(
            self,
            LibList::ArtistTracks(_)
                | LibList::AlbumTracks(_)
                | LibList::Songs
                | LibList::GenreAlbumTracks(_)
                | LibList::GenreTracks(_)
                | LibList::GenreArtistTracks(_)
                | LibList::Recent
        )
    }
}

/// One row of a library list.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LibRow<'a> {
    pub kind: LibKind,
    /// Row of its table ([`LibList::open`]).
    pub id: u32,
    /// Artist, album or genre name, or track title; `""` for an unknown
    /// artist, album or genre.
    pub name: &'a str,
    /// Tracks: playing time in ms (0 = unknown).
    pub duration_ms: u32,
}

/// The library list the service holds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LibListing {
    pub list: LibList,
    pub count: u32,
    /// False until the count is known.
    pub complete: bool,
}

/// The current track's place in the library ("Go to album / artist").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct LibNow {
    pub track: u32,
    pub album: u32,
    pub artist: u32,
}

/// The music library (04 §6.4, 05 §7.1) through a window of rows, like
/// [`BrowseService`]: `lib_row` answers from the rows the service keeps,
/// `lib_want` moves the window (`Event::Data(data::LIBRARY)` follows).
/// Only while `PlayerService::library` is `Ready`.
pub trait LibraryService {
    /// Make `list` the current library list (cheap if it already is).
    fn lib_open(&mut self, list: LibList) -> LibListing;
    fn lib_listing(&self) -> LibListing;
    /// Row `i`, if loaded.
    fn lib_row(&self, i: u32) -> Option<LibRow<'_>>;
    fn lib_want(&mut self, first: u32, count: u32);
    /// Where table row `id` is in the current list, if it is in the rows
    /// loaded.
    fn lib_position(&self, id: u32) -> Option<u32>;
    /// Letter-jump letter of row `i` (Artists, Albums, Songs, Genres).
    fn lib_letter(&self, i: u32) -> Option<char>;
    fn lib_seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)>;
    /// The current track in the library, once it is known.
    fn lib_now(&self) -> Option<LibNow>;
}

// ---------------------------------------------------------------- Pictures

/// What to show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PicSource {
    /// A picture file on the card.
    File(FileRef),
    /// The current track's album art.
    Art(ArtSize),
}

/// A picture to show in a box of the screen.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PicReq {
    pub src: PicSource,
    /// Crop to fill the box (else the whole picture, letterboxed).
    pub fill: bool,
    /// User rotation in quarter turns clockwise, after the EXIF orientation.
    pub turns: u8,
}

impl PicReq {
    #[must_use]
    pub const fn art(size: ArtSize) -> PicReq {
        PicReq { src: PicSource::Art(size), fill: true, turns: 0 }
    }
}

/// Picture formats (Details).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PicFormat {
    Jpeg,
    Png,
    Gif,
    Bmp,
    Qoi,
}

impl PicFormat {
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            PicFormat::Jpeg => "JPEG",
            PicFormat::Png => "PNG",
            PicFormat::Gif => "GIF",
            PicFormat::Bmp => "BMP",
            PicFormat::Qoi => "QOI",
        }
    }
}

/// What was shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct PicInfo {
    /// Stored size of the picture.
    pub width: u32,
    pub height: u32,
    pub format: Option<PicFormat>,
    /// Shown at reduced detail (progressive JPEG, interlaced PNG or GIF).
    pub reduced: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PicError {
    /// Not a picture format (or variant) the player reads.
    Unsupported,
    /// Damaged or cut short.
    Corrupt,
    /// Needs more decoder memory than there is.
    TooLarge,
    /// The file could not be read.
    Io,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PicState {
    /// Being put into the frame buffer; `Event::Data(PICTURE)` follows.
    /// After that event it means the request was dropped (a key cancelled
    /// it, or the screen was covered): draw the box again to ask again.
    Pending,
    /// In the box now.
    Shown(PicInfo),
    /// Nothing to show (the track has no album art).
    Nothing,
    /// The decoder memory belongs to the audio decoder while a track is
    /// loaded (playing or paused): stop playback to view pictures.
    Busy,
    Failed(PicError),
}

/// Pictures are decoded by the service straight into the screen; nothing
/// keeps a decoded picture in RAM (04 §7). On the player the card task
/// writes the pixels into the frame buffer after the frame that asked for
/// them was drawn and sent to the panel (the UI draws nothing until it is
/// done). A screen draws a placeholder for `Pending`; when
/// `Event::Data(PICTURE)` says `Shown` it marks the box dirty without
/// drawing into it (`Gfx::mark`). A full redraw clears the box: the screen
/// asks again then. Ask only from a draw that painted the box.
pub trait PictureService {
    /// Show `req` in `r` (local coordinates of `g`). The simulator draws at
    /// once and answers `Shown`; the firmware answers `Pending`, or at once
    /// `Busy` or `Nothing`. Only the last request of a draw is carried out.
    fn show_picture(&self, g: &mut Gfx<'_>, r: Rect, req: PicReq) -> PicState;
    /// The outcome of the last `show_picture` (after `Event::Data(PICTURE)`;
    /// `Pending` before).
    fn picture_state(&self) -> PicState;
}
