//! Card-backed services for the UI (WP FS-2 / AU-5 firmware part): folder
//! listings, the text viewer window, the play queue and the playback
//! service (`orpheus-player`).
//!
//! The task that owns the mounted volume (`audio`) serves [`Req`]s, also
//! between ring refills while a track plays, and [`PlayReq`]s (checked
//! after every decoded block). Results go into the shared windows below;
//! the UI copies a window into `Fw` when the matching [`READY`] tag arrives
//! (or, for the player snapshot, on every loop) and turns it into
//! `Event::Data`.
//!
//! Listings are sorted through the directory index (AU-12, `orpheus-index`,
//! files under `/.orpheus/index/`): a folder whose index is current (its
//! signature is checked during the counting pass every browse makes) is
//! served from it at once (`sorted: true`; names through the locators).
//! Otherwise the on-disk listing goes out with `sorted: false` and the
//! index is built: at once for folders of up to [`INLINE_MAX`] entries
//! when the card task is idle, else in the background while it is idle
//! (`audio::build_slot`, between requests, suspended when playback takes
//! the arena); the sorted listing follows as another `Data(BROWSE)`.
//!
//! The queue is the audio files of one folder (in the listing's order when
//! the folder's index is current), a library list or a Shuffle-all sample,
//! kept in RAM and in `/.orpheus/queue.bin`; Up next edits it (remove,
//! move) and so do Play next / Add to queue (`Server::edit`; written to the
//! card with the player state);
//! the player's resume record and bookmarks are a RAM image
//! ([`StoreImage`]) written to `/.orpheus/player.bin` at safe moments (pause,
//! stop, track change, card hand-over, power-off). Formats:
//! `orpheus_player::image`.
//!
//! Music-library lists (Artists, Albums, Songs, …; `crate::library`) share
//! the listing window: `Req::LibList` makes one the listing, `Req::Want`
//! moves its window; their queues (`PlayReq::PlayLib`) and Shuffle all come
//! from the library too.
//!
//! Not yet: tags in the track info, persistence of text positions.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use orpheus_apps::browse::{EntryKind, is_hidden_name, kind_of};
use orpheus_apps::services::{
    BrowseFilter, DateTime, DirId, FileRef, LibList, LibNow, LibraryState, Listing, PicReq, PicState,
};
use orpheus_index::LetterTable;
use orpheus_player::image::StoreImage;
use orpheus_player::{Action, Actions, Bookmark, Command, Config, Event, Player, Repeat, State, TrackRef};
use orpheus_ui::event::Source;

use crate::frame::Own;

/// Rows per listing window.
pub const WINDOW: u32 = 32;
/// Bytes kept of a name (cut at a character boundary).
pub const NAME: usize = 80;
/// Bytes per text-viewer window.
pub const TEXT_WIN: usize = 1536;
/// Listing checkpoints (one per `WINDOW` rows): folders up to 2048 rows
/// seek directly, larger ones scan from the last checkpoint.
pub const CHECKPOINTS: usize = 64;
/// Play-queue length (one folder; Play next / Add to queue stop there).
pub const QUEUE: usize = 256;
const _: () =
    assert!(QUEUE == orpheus_player::queue::EDIT_MAX && QUEUE as u32 == orpheus_apps::services::QUEUE_MAX);
/// Rows of the Up-next window.
pub const QWIN: usize = 8;
/// Bytes kept of a title in the Up-next and bookmark windows.
pub const SHORT: usize = 23;
/// Bookmarks the player keeps.
pub const BOOKMARKS: usize = orpheus_player::store::MAX_BOOKMARKS;
/// Files of the playback state (only `/.orpheus/` is writable).
pub const QUEUE_FILE: &str = "/.orpheus/queue.bin";
pub const PLAYER_FILE: &str = "/.orpheus/player.bin";

/// Folders up to this many entries are indexed at once when the card task
/// is idle (one or two scratch runs), instead of in the background.
pub const INLINE_MAX: u32 = 128;
/// Scratch for index builds: the last bytes of the idle decoder arena (the
/// text window, the state files and uploads use its start).
pub const INDEX_SCRATCH: usize = 16 * 1024;
/// Directory entries (or rows) per build step in the background.
pub const BUILD_BUDGET: u32 = 64;
/// While playing, the focused file is looked for in sorted listings of at
/// most this many rows (8 KiB of index, one piece between ring refills).
pub const FOCUS_SCAN_PLAYING: u32 = 256;
/// While playing, a listing is made in pieces between ring refills
/// ([`Server::step`]): each stops when its chore budget is used up
/// (`audio::piece_left`, asked before each card operation: the DAC ring's
/// headroom, `orpheus_audio::chore`) or after this many directory entries
/// (counting) or rows (filling the window). Whole, a browse or a window
/// took longer than the PCM ring holds at 48 kHz (≈ 64 ms; 8–14 underruns
/// per track, all `serve`, 2026-11-24); pieces of a fixed 8 ms were still
/// forced in at too little headroom (soak seed 1, 2026-11-25).
pub const PIECE_ENTRIES: u32 = 32;
pub const PIECE_ROWS: u32 = 8;

/// Kinds of listing piece (`Server::job_kind`), each with its own measured
/// need (`audio`'s chore costs): the count of a folder, the index file's
/// open, the focused row's search, rows from the index, a window of an
/// unsorted folder, an Up next title.
pub const JOB_KINDS: usize = 6;
pub const JOB_NAMES: [&str; JOB_KINDS] = ["count", "index", "focus", "rows", "win", "qtitle"];

/// A listing being made while a track plays, one piece per chore.
#[derive(Clone, Copy)]
pub enum Job {
    /// The counting pass: the next entry, the signature and the rows so
    /// far.
    Count { cur: DirCursor, sig: Signature, n: u32 },
    /// Counted: the index is looked for next (a piece of its own).
    Counted { sig: Signature, n: u32 },
    /// Rows of the sorted listing `ix` into the window from `start`
    /// (`u32::MAX`: the focused file is looked for first), `n` filled;
    /// `letters`: the letter table follows.
    Rows { ix: Index, start: u32, n: u32, letters: bool },
    /// A window of an unsorted listing from `start`: `cur` (once placed at
    /// the checkpoint before it), `skip` entries still to pass, `n` rows
    /// filled.
    Win { cur: Option<DirCursor>, start: u32, skip: u32, n: u32 },
}
/// Letter runs kept in RAM for letter jumps ([`LETTERS`]).
pub const LETTER_RUNS: usize = 80;

pub mod tag {
    pub use orpheus_apps::services::data::{BROWSE, LIBRARY, PICTURE, QUEUE, TEXT};
    /// The Authenticator's import file was read or deleted (`OtpJob`).
    pub const OTP: u16 = 0x100;
}

#[derive(Clone, Copy, Debug)]
pub enum Req {
    /// List `dir`; `entry` is the folder's own directory entry (None for the
    /// root).
    Browse { dir: DirId, entry: Option<FileRef>, filter: BrowseFilter },
    /// Move the listing window to start at or before `first`.
    Want { first: u32 },
    /// Text window of `file` from `offset`.
    Text { file: FileRef, offset: u32 },
    /// Titles of queue positions `first..first + QWIN` (Up next).
    QueueWin { first: u32 },
    /// Hand the card to the USB task (stops playback first).
    Release,
    /// USB Drive (USB-6): map the wanted image (`crate::drive`), then hand
    /// the card to the USB task as for `Release`, or answer the refusal.
    Image,
    /// Put a picture into the frame buffer the UI lent (`PicJob`).
    Picture(PicJob),
    /// Make a music-library list the listing (`crate::library`).
    LibList { list: LibList },
    /// Update the music library at the next idle moment.
    Rebuild,
    /// The battery logger has something for the card (`crate::drain`):
    /// written at once when idle; while playing only its file is chosen
    /// here (the rows wait for room in the DAC's ring, `audio::play_file`).
    Drain,
    /// The step counter has something for the card (`crate::sensor`: the
    /// steps file, trace rows): written at once when idle; while playing
    /// only a trace file is chosen here (writes wait for room, as `Drain`).
    Sensor,
    /// An alert started (`audio::ALERT`): wakes the idle loop to sound it.
    Alert,
    /// The Authenticator's import file (`OtpJob`, OTP-2).
    Otp(OtpJob),
    /// Delete a recording (Recorder › Recordings…; `rec::delete` allows
    /// only files in `/Recordings`). In this queue so that the listing
    /// asked for after it already lacks the file.
    Delete(FileRef),
    /// Firmware update (UPD-1, `crate::update`): list the files, read one's
    /// change list (also while playing), install one (idle only: playback
    /// stops first).
    Update(crate::update::Op),
}

/// Request kinds ([`Req::kind`]) and their names for the log.
pub const REQ_KINDS: usize = 15;
pub const REQ_NAMES: [&str; REQ_KINDS] = [
    "browse",
    "want",
    "text",
    "queue-win",
    "release",
    "image",
    "picture",
    "lib-list",
    "rebuild",
    "drain",
    "sensor",
    "alert",
    "otp",
    "delete",
    "update",
];

impl Req {
    /// The request's kind (`0..REQ_KINDS`, the order of [`Req::name`]):
    /// each has its own measured need as a chore while playing.
    #[must_use]
    pub fn kind(&self) -> usize {
        match self {
            Req::Browse { .. } => 0,
            Req::Want { .. } => 1,
            Req::Text { .. } => 2,
            Req::QueueWin { .. } => 3,
            Req::Release => 4,
            Req::Image => 5,
            Req::Picture(_) => 6,
            Req::LibList { .. } => 7,
            Req::Rebuild => 8,
            Req::Drain => 9,
            Req::Sensor => 10,
            Req::Alert => 11,
            Req::Otp(_) => 12,
            Req::Delete(_) => 13,
            Req::Update(_) => 14,
        }
    }

    /// Short name for the log.
    #[must_use]
    pub fn name(&self) -> &'static str {
        REQ_NAMES.get(self.kind()).copied().unwrap_or("?")
    }
}

/// The Authenticator's import file `/.orpheus/otp-import.txt` (OTP-2, 06
/// §5.4): read it into bytes the UI lent (part of the frame buffer; the UI
/// draws nothing while they are out), or overwrite it with zeros and
/// delete it. Same slot protocol as pictures ([`OTP_SLOT`]).
#[derive(Clone, Copy, Debug)]
pub struct OtpJob {
    pub seq: u32,
    /// `buf..buf + cap`: the lent bytes (unused when deleting).
    pub buf: usize,
    pub cap: u32,
    pub delete: bool,
}

/// The import file (only `/.orpheus/` is written, CLAUDE.md rule 5).
pub const OTP_FILE: &str = "/.orpheus/otp-import.txt";
/// 0 none, `seq` queued, `seq | PIC_RUNNING` being carried out.
pub static OTP_SLOT: AtomicU32 = AtomicU32::new(0);
/// The last job's number and result: bytes read (or 0 for a delete), or
/// one of the `OTP_*` codes below.
pub static OTP_RESULT: Mutex<core::cell::Cell<(u32, i32)>> = Mutex::new(core::cell::Cell::new((0, 0)));
pub const OTP_NONE: i32 = -1;
pub const OTP_TOO_LARGE: i32 = -2;
pub const OTP_IO: i32 = -3;

/// The card task: start job `j` unless the UI took it back.
pub fn otp_start(j: &OtpJob) -> bool {
    OTP_SLOT.compare_exchange(j.seq, j.seq | PIC_RUNNING, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// The card task: job `j` ended with `r` (the lent bytes go back).
pub fn otp_done(j: &OtpJob, r: i32) {
    critical_section::with(|cs| OTP_RESULT.borrow(cs).set((j.seq, r)));
    OTP_SLOT.store(0, Ordering::Release);
    ready(tag::OTP);
}

/// A picture for a box of the frame buffer (`PictureService`). The UI
/// draws nothing while a job is out: the card task writes the box, then
/// answers with `tag::PICTURE` ([`picture_done`]).
#[derive(Clone, Copy, Debug)]
pub struct PicJob {
    /// Number of this job ([`PIC_SLOT`]).
    pub seq: u32,
    /// The frame buffer (128 × 128 RGB565), lent by the UI.
    pub fb: usize,
    /// The box, in panel pixels.
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
    pub req: PicReq,
}

/// The job the UI lent the frame buffer for: 0 none, `seq` queued,
/// `seq | PIC_RUNNING` being carried out. The UI takes a queued job back
/// (key, timeout) by swapping its number for 0; the card task starts a job
/// only by swapping `seq` for `seq | PIC_RUNNING`, so a job taken back (or
/// replaced) is never started.
pub static PIC_SLOT: AtomicU32 = AtomicU32::new(0);
pub const PIC_RUNNING: u32 = 0x8000_0000;
/// Set by the UI (a key): the running job ends early with `Pending`.
pub static PIC_CANCEL: AtomicBool = AtomicBool::new(false);
/// The last job's number and outcome.
pub static PIC_RESULT: Mutex<core::cell::Cell<(u32, PicState)>> =
    Mutex::new(core::cell::Cell::new((0, PicState::Nothing)));

/// The card task: start job `j` unless the UI took it back.
pub fn picture_start(j: &PicJob) -> bool {
    PIC_SLOT.compare_exchange(j.seq, j.seq | PIC_RUNNING, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// The card task: job `j` is done (the frame buffer goes back to the UI).
pub fn picture_done(j: &PicJob, st: PicState) {
    critical_section::with(|cs| PIC_RESULT.borrow(cs).set((j.seq, st)));
    PIC_SLOT.store(0, Ordering::Release);
    ready(tag::PICTURE);
}

/// Requests for the player (from `Fw::command` and the developer channel).
#[derive(Clone, Copy, Debug)]
pub enum PlayReq {
    /// Queue the audio files of a folder and play from `start` (shuffled:
    /// `start`, or a random item, comes first).
    PlayDir {
        entry: Option<FileRef>,
        start: Option<FileRef>,
        shuffle: bool,
    },
    /// Music → Shuffle all: a shuffled queue sampled from the library (or,
    /// without one, from every visible audio file on the card).
    ShuffleAll,
    /// Queue the tracks of a library list and play from row `start`.
    PlayLib {
        list: LibList,
        start: Option<u32>,
        shuffle: bool,
    },
    Cmd(Command),
    /// Play queue position `n` (play order).
    JumpPos(u32),
    /// Continue where playback stopped, or at the saved resume point.
    Resume,
    /// Stop and clear the resume point.
    Stop,
    /// A–B: set A here (clearing B), then B.
    SetA,
    SetB,
    /// Power-off: stop playback, save the player state, flush the volume,
    /// then set `SHUT_DOWN` (a player request: checked after every block).
    Shutdown,
    /// Change the queue (Up next: remove, move; a file's menu: Play next,
    /// Add to queue). Positions are play positions.
    Edit(QueueEdit),
}

pub type QueueEdit = orpheus_player::queue::Edit<Locator>;

/// Set by the card task once the volume is flushed for power-off.
static WALK_STEPS: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);
static WALK_SEEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

pub static SHUT_DOWN: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

pub static REQS: Channel<CriticalSectionRawMutex, Req, 4> = Channel::new();
pub static PLAY: Channel<CriticalSectionRawMutex, PlayReq, 4> = Channel::new();
/// Data-ready tags for the UI.
pub static READY: Channel<CriticalSectionRawMutex, u16, 4> = Channel::new();

/// Send a request; when the queue is full the oldest kind of request is
/// simply retried by the screen (it asks again on its next draw).
pub fn request(r: Req) {
    let _ = REQS.try_send(r);
}

/// Send a request; false if the queue is full.
pub fn try_request(r: Req) -> bool {
    REQS.try_send(r).is_ok()
}

/// Send a player request.
pub fn play(r: PlayReq) {
    let _ = PLAY.try_send(r);
}

pub(crate) fn ready(t: u16) {
    let _ = READY.try_send(t);
}

// ------------------------------------------------------------------ names

/// A name cut to at most `N` bytes (at a character boundary).
#[derive(Clone, Copy)]
pub struct Name<const N: usize = NAME> {
    buf: [u8; N],
    len: u8,
}

/// A title in the Up-next and bookmark windows.
pub type Short = Name<SHORT>;

impl<const N: usize> Name<N> {
    pub const EMPTY: Self = Self { buf: [0; N], len: 0 };

    #[must_use]
    pub fn new(s: &str) -> Self {
        let mut n = s.len().min(N).min(255);
        while !s.is_char_boundary(n) {
            n -= 1;
        }
        let mut buf = [0; N];
        buf[..n].copy_from_slice(&s.as_bytes()[..n]);
        Self { buf, len: n as u8 }
    }

    /// A file name without its extension (titles until tags are read).
    #[must_use]
    pub fn title(file: &str) -> Self {
        Self::new(file.rsplit_once('.').map_or(file, |(t, _)| t))
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        core::str::from_utf8(&self.buf[..usize::from(self.len)]).unwrap_or("")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

// ---------------------------------------------------------------- windows

#[derive(Clone, Copy)]
pub struct Row {
    pub name: Name,
    pub kind: EntryKind,
    pub size: u64,
    pub modified: DateTime,
    pub file: FileRef,
    pub child: DirId,
}

impl Row {
    pub const EMPTY: Row = Row {
        name: Name::EMPTY,
        kind: EntryKind::Other,
        size: 0,
        modified: DateTime { year: 2000, month: 1, day: 1, hour: 0, minute: 0, second: 0, weekday: 5 },
        file: FileRef { dir: 0, index: 0 },
        child: DirId(0),
    };
}

#[derive(Clone, Copy)]
pub struct BrowseWin {
    pub listing: Listing,
    /// Changes with every listing the card task publishes (also when it
    /// becomes sorted); [`LETTERS`] carries the one it belongs to.
    pub lgen: u32,
    pub first: u32,
    pub n: u32,
    pub rows: [Row; WINDOW as usize],
}

impl BrowseWin {
    pub const EMPTY: BrowseWin = BrowseWin {
        listing: Listing { dir: DirId(u32::MAX), count: 0, sorted: false, complete: false },
        lgen: 0,
        first: 0,
        n: 0,
        rows: [Row::EMPTY; WINDOW as usize],
    };

    #[must_use]
    pub fn row(&self, i: u32) -> Option<&Row> {
        if i < self.first || i >= self.first + self.n {
            return None;
        }
        self.rows.get((i - self.first) as usize)
    }
}

#[derive(Clone, Copy)]
pub struct TextWin {
    pub file: FileRef,
    /// `u32::MAX` while nothing is loaded.
    pub offset: u32,
    pub len: u32,
    pub buf: [u8; TEXT_WIN],
}

impl TextWin {
    pub const EMPTY: TextWin =
        TextWin { file: FileRef { dir: 0, index: 0 }, offset: u32::MAX, len: 0, buf: [0; TEXT_WIN] };
}

#[derive(Clone, Copy)]
pub struct PlayerSnap {
    pub source: Source,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub queue_pos: u32,
    pub queue_len: u32,
    /// The queue plays shuffled (developer `status`).
    pub shuffled: bool,
    /// The current track; while idle, the track a resume continues.
    pub title: Name,
    pub file: FileRef,
    pub size: u64,
    pub codec: &'static str,
    pub rate: u32,
    pub channels: u8,
    pub bits: u8,
    /// Average bit rate, kbit/s (`orpheus_apps::services::average_kbps`).
    pub kbps: u16,
    pub a_ms: Option<u32>,
    pub b_ms: Option<u32>,
    /// Sleep timer: minutes left (0: at the end of the track).
    pub sleep_min: Option<u16>,
    /// Counts sleep timers that ran out (the UI then resets the setting).
    pub sleep_fired: u16,
    /// `title` can be resumed (offered while idle).
    pub resume: bool,
    /// Changes when the queue or its order changes (Up-next window).
    pub qgen: u32,
    /// Changes when the bookmarks change ([`BMS`]).
    pub bm_gen: u32,
    /// Album art of the current track: 0 none, else changes with it
    /// (`Server::art`).
    pub art: u32,
    /// The music library (`crate::library`): its state, a generation that
    /// changes when another library is opened, and the current track in
    /// it.
    pub lib_state: LibraryState,
    pub lib_gen: u32,
    pub lib_now: Option<LibNow>,
}

impl PlayerSnap {
    pub const IDLE: PlayerSnap = PlayerSnap {
        source: Source::Idle,
        position_ms: 0,
        duration_ms: 0,
        queue_pos: 0,
        queue_len: 0,
        shuffled: false,
        title: Name::EMPTY,
        file: FileRef { dir: 0, index: 0 },
        size: 0,
        codec: "",
        rate: 0,
        channels: 0,
        bits: 0,
        kbps: 0,
        a_ms: None,
        b_ms: None,
        sleep_min: None,
        sleep_fired: 0,
        resume: false,
        qgen: 0,
        bm_gen: 0,
        art: 0,
        lib_state: LibraryState::Missing,
        lib_gen: 0,
        lib_now: None,
    };
}

/// Titles of queue positions `first..first + n` (play order).
#[derive(Clone, Copy)]
pub struct QueueWin {
    pub qgen: u32,
    pub first: u32,
    pub n: u32,
    pub titles: [Short; QWIN],
}

impl QueueWin {
    pub const EMPTY: QueueWin = QueueWin { qgen: u32::MAX, first: 0, n: 0, titles: [Short::EMPTY; QWIN] };

    #[must_use]
    pub fn title(&self, qgen: u32, pos: u32) -> Option<&str> {
        if self.qgen != qgen || pos < self.first || pos >= self.first + self.n {
            return None;
        }
        Some(self.titles.get((pos - self.first) as usize)?.as_str())
    }
}

#[derive(Clone, Copy)]
pub struct BmRow {
    pub title: Short,
    pub ms: u32,
}

/// The bookmarks, oldest first (as the player keeps them).
#[derive(Clone, Copy)]
pub struct BmWin {
    pub n: u32,
    pub rows: [BmRow; BOOKMARKS],
}

impl BmWin {
    pub const EMPTY: BmWin = BmWin { n: 0, rows: [BmRow { title: Short::EMPTY, ms: 0 }; BOOKMARKS] };
}

pub static BROWSE: Mutex<RefCell<BrowseWin>> = Mutex::new(RefCell::new(BrowseWin::EMPTY));
pub static TEXT: Mutex<RefCell<TextWin>> = Mutex::new(RefCell::new(TextWin::EMPTY));
pub static PLAYER: Mutex<RefCell<PlayerSnap>> = Mutex::new(RefCell::new(PlayerSnap::IDLE));
pub static QWINDOW: Mutex<RefCell<QueueWin>> = Mutex::new(RefCell::new(QueueWin::EMPTY));
pub static BMS: Mutex<RefCell<BmWin>> = Mutex::new(RefCell::new(BmWin::EMPTY));
/// The letter runs of the current sorted listing (listing generation,
/// table): letter jumps are answered from here, without the card.
pub static LETTERS: Mutex<RefCell<(u32, LetterTable<LETTER_RUNS>)>> =
    Mutex::new(RefCell::new((0, LetterTable::EMPTY)));
/// The file the Files screen has focused (`BrowseService::focus_hint`):
/// when a listing becomes sorted, the window around it is published.
static FOCUS: Mutex<core::cell::Cell<Option<FileRef>>> = Mutex::new(core::cell::Cell::new(None));

/// Set (or, entering another folder, clear) the focus hint.
pub fn set_focus(f: Option<FileRef>) {
    critical_section::with(|cs| FOCUS.borrow(cs).set(f));
}

pub fn player_update(f: impl FnOnce(&mut PlayerSnap)) {
    critical_section::with(|cs| f(&mut PLAYER.borrow(cs).borrow_mut()));
}

// ------------------------------------------------------------ player glue

/// What to open.
#[derive(Clone, Copy, Debug)]
pub enum Target {
    /// A queue item.
    Item(u32),
    /// A track outside the queue (bookmark, resume into another queue).
    Track(TrackRef),
}

impl Target {
    /// Queue item `i`.
    #[must_use]
    pub fn is_item(self, i: u32) -> bool {
        matches!(self, Target::Item(x) if x == i)
    }
}

#[derive(Clone, Copy, Debug)]
pub struct OpenReq {
    pub target: Target,
    pub frame: u64,
    pub paused: bool,
    /// The previous track ended and this one was read ahead: hand over
    /// without stopping the DAC (`audio::play_item`).
    pub gapless: bool,
    /// Started before the player heard of the previous track's end
    /// (`Player::gapless_next`); it hears after the first block.
    pub early: bool,
}

/// The player's actions for one event, as the playback loop carries them
/// out.
#[derive(Clone, Copy, Debug, Default)]
pub struct Flow {
    pub open: Option<OpenReq>,
    pub seek: Option<u64>,
    /// `Some(true)`: pause, `Some(false)`: resume.
    pub pause: Option<bool>,
    pub stop: bool,
    /// Forget the file read ahead (before `prefetch`).
    pub cancel: bool,
    /// Read this queue item ahead after the current file (item + 1: the
    /// flow stays as small as before, it is returned by value a lot).
    prefetch: Option<core::num::NonZeroU32>,
}

impl Flow {
    fn of(out: &Actions) -> Flow {
        let mut f = Flow::default();
        for a in out.iter() {
            match a {
                Action::Open { item, start_frame, paused, gapless } => {
                    f.open = Some(OpenReq {
                        target: Target::Item(item),
                        frame: start_frame,
                        paused,
                        gapless,
                        early: false,
                    });
                }
                Action::OpenTrack { track, start_frame, paused } => {
                    f.open = Some(OpenReq {
                        target: Target::Track(track),
                        frame: start_frame,
                        paused,
                        gapless: false,
                        early: false,
                    });
                }
                Action::Seek { frame } => f.seek = Some(frame),
                Action::Pause => f.pause = Some(true),
                Action::Resume => f.pause = Some(false),
                Action::Stop => f.stop = true,
                Action::Prefetch { item } => f.prefetch = core::num::NonZeroU32::new(item.wrapping_add(1)),
                Action::CancelPrefetch => {
                    f.cancel = true;
                    f.prefetch = None;
                }
            }
        }
        f
    }

    /// `o` happened after `self`.
    pub fn merge(&mut self, o: Flow) {
        if o.open.is_some() || o.stop {
            *self = o;
            return;
        }
        if o.seek.is_some() {
            self.seek = o.seek;
        }
        if o.pause.is_some() {
            self.pause = o.pause;
        }
        if o.cancel {
            self.cancel = true;
            self.prefetch = None;
        }
        if o.prefetch.is_some() {
            self.prefetch = o.prefetch;
        }
    }

    /// The queue item to read ahead, if any.
    #[must_use]
    pub fn prefetch(&self) -> Option<u32> {
        self.prefetch.map(|p| p.get() - 1)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_none()
            && self.seek.is_none()
            && self.pause.is_none()
            && !self.stop
            && !self.cancel
            && self.prefetch.is_none()
    }
}

fn bm_ms(b: Bookmark) -> u32 {
    (b.frame * 1000).checked_div(u64::from(b.rate)).unwrap_or(0) as u32
}

fn cmd(c: Command) -> Event {
    Event::Command(c)
}

// ------------------------------------------------------------- the server

use core::future::poll_fn;
use core::task::Poll;

use orpheus_fs::{DirCursor, DirEntry, Locator, Volume};
use orpheus_index::card::{CardIo, Handles, Mode, Place, is_dot, spin, src_entry};
use orpheus_index::{Builder, FileName, Index, Progress, Signature, Spec, Storage, Suspended};
use yoton_bsp::card::Card;

pub type Vol = Volume<'static, Card>;
/// One session of index I/O on the card (see `orpheus_index::card`).
pub type Io<'a, 'c> = CardIo<'a, 'static, 'c, Card>;

/// Code-line loads of the pager since boot.
fn page_loads() -> u32 {
    atj2157_hal::pager::stats().loads as u32
}

fn loc(f: FileRef) -> Locator {
    Locator { dir: f.dir, index: f.index }
}

fn fref(l: Locator) -> FileRef {
    FileRef { dir: l.dir, index: l.index }
}

fn date(d: orpheus_fs::DateTime) -> DateTime {
    DateTime {
        year: d.year,
        month: d.month,
        day: d.day,
        hour: d.hour,
        minute: d.minute,
        second: d.second,
        weekday: 0,
    }
}

/// The index-build part of the arena slice `buf`: its last
/// [`INDEX_SCRATCH`] bytes.
pub fn index_scratch(buf: &mut [u8]) -> &mut [u8] {
    let l = buf.len();
    &mut buf[l.saturating_sub(INDEX_SCRATCH)..]
}

/// Run `f` in an index session on the card. Inside the card task only:
/// the file-system calls spin with its context. Files are closed after.
pub async fn with_io<R>(vol: &mut Vol, mode: Mode, f: impl FnOnce(&Io<'_, '_>) -> R) -> R {
    let mut f = Some(f);
    poll_fn(|cx| io_session(vol, cx, mode, &mut f)).await
}

/// The session of [`with_io`], in a frame of its own: inlined into the
/// awaiting future's poll, every `with_io` of a function kept its handle
/// table, its `CardIo` and its closure's temporaries in that frame at once,
/// above the deepest of their card calls (VER-10).
#[inline(never)]
fn io_session<R>(
    vol: &mut Vol,
    cx: &mut core::task::Context<'_>,
    mode: Mode,
    f: &mut Option<impl FnOnce(&Io<'_, '_>) -> R>,
) -> Poll<R> {
    let mut h = Handles::new();
    let io = CardIo::new(vol, cx, &mut h, mode);
    let r = f.take().map(|f| f(&io));
    let _ = io.close_all();
    r.map_or(Poll::Pending, Poll::Ready)
}

/// [`with_io`], read-only, on the index file `f` reopened at `at` (where
/// the last session on it found it: no path walk, AU-PERF 2026-11-25);
/// `at` becomes where this session found it, for the next one. (The
/// place and the name by reference: by value they made the session's
/// frame and `Server::step`'s, on the deepest card path, 64 B larger.)
pub async fn with_index<R>(
    vol: &mut Vol,
    f: FileName,
    at: &mut Option<Place>,
    g: impl FnOnce(&Io<'_, '_>) -> R,
) -> R {
    let f = &f;
    with_io(vol, Mode::Read, |io| {
        if let Some(a) = *at {
            io.reuse(f, a);
        }
        let r = g(io);
        *at = io.place(f);
        r
    })
    .await
}

const ROW0: orpheus_index::Row = orpheus_index::Row {
    loc: Locator { dir: 0, index: 0 },
    size: 0,
    mtime: 0,
    letter: None,
    kind: EntryKind::Other,
    hidden: false,
};

/// An index build, wanted or in progress (one at a time).
#[derive(Clone, Copy)]
pub struct Build {
    dircl: u32,
    spec: Spec,
    /// The directory's first entry, and where the build's scan is.
    start: DirCursor,
    cur: DirCursor,
    /// `None` until the first step.
    st: Option<Suspended>,
}

/// Server state, owned by the card task.
pub struct Server {
    dir: DirId,
    entry: Option<FileRef>,
    filter: BrowseFilter,
    count: u32,
    checkpoints: [u32; CHECKPOINTS],
    /// The listing's directory identity (first cluster; the index's key).
    dircl: u32,
    /// The current index of the listing, when it is served sorted.
    index: Option<Index>,
    /// Where its file was found ([`with_index`]): the listing's pieces
    /// and windows reopen it from there instead of walking
    /// `/.orpheus/index/X/NAME` from the root (on the owner's card one
    /// 31-ms stretch per piece, soak seed 1, 2026-11-25). Set and dropped
    /// with `index` ([`Server::set_index`]); checked against the volume's
    /// serial and the entry's name on every use.
    index_at: Option<Place>,
    /// Listing generation ([`BrowseWin::lgen`]).
    lgen: u32,
    /// The background build.
    build: Option<Build>,
    /// The listing being made in pieces while a track plays.
    job: Option<Job>,
    /// The Up next window being filled while a track plays: (queue
    /// generation, first position, mask of the slots filled). One title
    /// per chore piece (`step`): a whole window at once took 465 ms with a
    /// shuffled queue (each title opens a track somewhere on the card) and
    /// emptied the DAC ring (soak, 2026-11-25). Titles the window had for
    /// the same generation are kept (`keep_titles`): when a track ends the
    /// window moves on by one and one title is new.
    qjob: Option<(u32, u32, u32)>,
    /// The index file of an abandoned build, removed (with the scratch
    /// files) before the next build starts.
    orphan: Option<FileName>,
    pub queue: [Locator; QUEUE],
    pub qlen: u32,
    /// Identity of the queue (`orpheus_player::image::queue_id`).
    pub queue_id: u32,
    /// The queue was edited and `queue.bin` not yet rewritten (with the
    /// player state, `audio::save_store`).
    pub queue_dirty: bool,
    /// Changes with the queue or its order.
    qgen: u32,
    /// A request for the idle loop (card hand-over).
    pub pending: Option<Req>,
    /// A player request for the idle loop (a new queue, power-off).
    pub pending_play: Option<PlayReq>,
    pub player: Option<Player>,
    pub store: StoreImage,
    /// The Repeat setting (re-applied after a resume, which restores the
    /// saved one).
    repeat: Repeat,
    can_resume: bool,
    sleep_fired: u16,
    bm_gen: u32,
    /// Album art of the current track (in the art cache), and its
    /// generation ([`PlayerSnap::art`]).
    pub art: Option<crate::pictures::ArtRef>,
    pub art_gen: u32,
    /// The folder whose cover was looked for last (first cluster) and its
    /// art: the other tracks of the folder use it without a scan.
    pub folder_art: Option<(u32, Option<crate::pictures::ArtRef>)>,
    /// The music library.
    pub lib: crate::library::LibState,
}

impl Server {
    pub const fn new() -> Self {
        Server {
            dir: DirId(u32::MAX),
            entry: None,
            filter: BrowseFilter {
                sort: orpheus_apps::services::SortBy::Name,
                hidden: false,
                audio_only: false,
                images_only: false,
            },
            count: 0,
            checkpoints: [0; CHECKPOINTS],
            dircl: u32::MAX,
            index: None,
            index_at: None,
            lgen: 0,
            build: None,
            job: None,
            qjob: None,
            orphan: None,
            queue: [Locator { dir: 0, index: 0 }; QUEUE],
            qlen: 0,
            queue_id: 0,
            queue_dirty: false,
            qgen: 0,
            pending: None,
            pending_play: None,
            player: None,
            store: StoreImage::new(),
            repeat: Repeat::Off,
            can_resume: false,
            sleep_fired: 0,
            bm_gen: 0,
            art: None,
            art_gen: 0,
            folder_art: None,
            lib: crate::library::LibState::new(),
        }
    }

    async fn cursor(vol: &mut Vol, entry: Option<FileRef>) -> Option<DirCursor> {
        match entry {
            None => Some(vol.root()),
            Some(f) => {
                let mut e = DirEntry::new();
                vol.open_locator(loc(f), &mut e).await.ok()?;
                vol.dir_cursor(&e).ok()
            }
        }
    }

    fn matches(filter: BrowseFilter, e: &DirEntry) -> bool {
        let name = e.name();
        if is_dot(e) {
            return false;
        }
        if !filter.hidden && (is_hidden_name(name) || e.attributes() & 0x06 != 0) {
            return false;
        }
        filter.keeps(kind_of(name, e.is_dir()))
    }

    fn row(e: &DirEntry) -> Row {
        let name = e.name();
        let dir = e.is_dir();
        Row {
            name: Name::new(name),
            kind: kind_of(name, dir),
            size: e.size(),
            modified: date(e.modified()),
            file: fref(e.locator()),
            child: DirId(if dir { e.first_cluster() } else { 0 }),
        }
    }

    /// Serve one request. Returns false for requests that belong to the
    /// idle loop (they are kept in `pending`). `scratch` (≥ `TEXT_WIN`
    /// bytes): free decoder-arena memory for file reads; `idle`: it is the
    /// whole idle arena (no playback), so small folders are indexed at once.
    pub async fn serve(&mut self, vol: &mut Vol, r: Req, scratch: &mut [u8], idle: bool) -> bool {
        match r {
            Req::Browse { dir, entry, filter } => {
                Own(self.browse(vol, dir, entry, filter, idle.then_some(scratch))).await;
                true
            }
            Req::Want { first } if self.lib.list.is_some() => {
                Own(crate::library::window(vol, self, first)).await;
                true
            }
            Req::Want { first } => {
                if idle {
                    Own(self.window(vol, first)).await;
                } else {
                    self.window_playing(first);
                }
                true
            }
            Req::LibList { list } => {
                self.job = None;
                Own(crate::library::open_list(vol, self, list)).await;
                true
            }
            Req::Rebuild => {
                crate::library::rebuild(self);
                true
            }
            Req::Drain => {
                if idle {
                    Own(crate::audio::drain_flush(vol, scratch)).await;
                } else {
                    Own(crate::audio::drain_prepare(vol)).await;
                }
                true
            }
            Req::Sensor => {
                if idle {
                    Own(crate::audio::sensor_flush(vol, scratch, false)).await;
                } else {
                    Own(crate::audio::trace_prepare(vol)).await;
                }
                true
            }
            Req::Alert => true,
            Req::Otp(j) => {
                Own(crate::audio::otp_file(vol, &j)).await;
                true
            }
            Req::Delete(f) => {
                Own(crate::rec::delete(vol, f)).await;
                // The file may be queued: Up next titles are looked up again.
                self.titles_stale();
                true
            }
            Req::Update(op) => {
                if !idle && matches!(op, crate::update::Op::Install(_)) {
                    self.pending = Some(r);
                    return false;
                }
                Own(crate::update::card_op(vol, op, scratch)).await;
                true
            }
            Req::Text { file, offset } => {
                Own(self.text(vol, file, offset, scratch)).await;
                true
            }
            Req::QueueWin { first } if idle => {
                Own(self.queue_win(vol, first)).await;
                true
            }
            Req::QueueWin { first } => {
                // While playing: in pieces (`step`); what the window had
                // is shown at once.
                let mask = self.keep_titles(first);
                self.qjob = Some((self.qgen, first, mask));
                true
            }
            Req::Release | Req::Image => {
                self.pending = Some(r);
                false
            }
            // While a track is loaded (the idle loop serves pictures with
            // the whole arena itself): album art from the cache only.
            Req::Picture(j) => {
                Own(crate::pictures::serve_playing(vol, self, &j)).await;
                true
            }
        }
    }

    /// A listing that is not a folder (a music-library list, `dir`)
    /// replaces the current one; returns its generation.
    pub fn start_listing(&mut self, dir: DirId) -> u32 {
        self.job = None;
        self.dir = dir;
        self.entry = None;
        self.count = 0;
        self.set_index(None);
        self.dircl = u32::MAX;
        self.lgen = self.lgen.wrapping_add(1);
        self.lgen
    }

    /// The listing's index (its file's place is not known yet).
    fn set_index(&mut self, ix: Option<Index>) {
        self.index = ix;
        self.index_at = None;
    }

    /// The current listing's generation.
    #[must_use]
    pub fn listing_gen(&self) -> u32 {
        self.lgen
    }

    /// The queue was filled with `n` items (elsewhere): its identity and
    /// generation follow.
    pub fn set_queue_len(&mut self, serial: u32, n: u32) {
        self.qlen = n.min(QUEUE as u32);
        self.qgen = self.qgen.wrapping_add(1);
        self.queue_id = orpheus_player::image::queue_id(serial, &self.queue[..self.qlen as usize]);
    }

    /// Set the current track's album art and publish it.
    pub fn set_art(&mut self, art: Option<crate::pictures::ArtRef>) {
        self.art = art;
        if art.is_some() {
            self.art_gen = self.art_gen.wrapping_add(1).max(1);
        }
        let g = if art.is_some() { self.art_gen } else { 0 };
        player_update(|p| p.art = g);
    }

    async fn browse(
        &mut self,
        vol: &mut Vol,
        dir: DirId,
        entry: Option<FileRef>,
        filter: BrowseFilter,
        idle: Option<&mut [u8]>,
    ) {
        self.dir = dir;
        self.entry = entry;
        self.filter = filter;
        self.count = 0;
        self.set_index(None);
        self.lib.list = None;
        self.lgen = self.lgen.wrapping_add(1);
        let lgen = self.lgen;
        let listing_of = |count, sorted| Listing { dir, count, sorted, complete: true };
        let Some(start) = Self::cursor(vol, entry).await else {
            self.dircl = u32::MAX;
            critical_section::with(|cs| {
                let mut w = BROWSE.borrow(cs).borrow_mut();
                w.listing = listing_of(0, true);
                w.lgen = lgen;
                w.first = 0;
                w.n = 0;
            });
            ready(tag::BROWSE);
            return;
        };
        self.dircl = start.dir_cluster();
        self.job = None;
        if idle.is_none() {
            // While a track plays: in pieces (`step`, one per chore).
            self.job = Some(Job::Count { cur: start, sig: Signature::default(), n: 0 });
            return;
        }
        let mut cur = start;
        let mut e = DirEntry::new();
        // The index's signature covers every entry, shown or not.
        let mut sig = Signature::default();
        let mut n = 0u32;
        loop {
            let before = cur.index();
            match vol.read_dir(&mut cur, &mut e).await {
                Ok(true) => {}
                _ => break,
            }
            if is_dot(&e) {
                continue;
            }
            sig.add(&src_entry(&e), e.name());
            if !Self::matches(filter, &e) {
                continue;
            }
            if n % WINDOW == 0 && ((n / WINDOW) as usize) < CHECKPOINTS {
                self.checkpoints[(n / WINDOW) as usize] = before;
            }
            if n < WINDOW {
                let row = Self::row(&e);
                critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
            }
            n += 1;
        }
        self.count = n;
        let spec = Spec::from(filter);
        // Idle (playing: `step`): the focused file in any listing.
        let scan = u32::MAX;
        if n >= 2 {
            // Idle and small: index it now (on the second pair of scratch
            // files: a background build may be suspended on the first).
            let inline = idle.filter(|_| n <= INLINE_MAX).map(index_scratch);
            let mode = if inline.is_some() { Mode::Write { alt: true } } else { Mode::Read };
            let e = &mut e;
            if let Some(ix) = with_io(vol, mode, |io| Self::resolve(io, start, spec, &sig, inline, e)).await {
                self.claim(ix.file());
                if self.build.is_some_and(|b| b.dircl == self.dircl && b.spec == spec) {
                    // Done another way; its scratch files go with the next
                    // build's start.
                    self.build = None;
                }
                self.set_index(Some(ix));
                let mut at = None;
                with_index(vol, ix.file(), &mut at, |io| self.publish_index(io, &ix, None, scan, e)).await;
                self.index_at = at;
                return;
            }
        }
        critical_section::with(|cs| {
            let mut w = BROWSE.borrow(cs).borrow_mut();
            // A single entry is in order already.
            w.listing = listing_of(n, n <= 1);
            w.lgen = lgen;
            w.first = 0;
            w.n = n.min(WINDOW);
        });
        ready(tag::BROWSE);
        if n >= 2 {
            self.want_build(start, spec);
        }
    }

    /// A listing piece is waiting (while a track plays).
    #[must_use]
    pub fn has_job(&self) -> bool {
        self.job.is_some() || self.qjob.is_some()
    }

    /// The kind of the next listing piece (`0..JOB_KINDS`, [`JOB_NAMES`]).
    #[must_use]
    pub fn job_kind(&self) -> usize {
        if self.qjob.is_some() {
            return 5;
        }
        match self.job {
            Some(Job::Count { .. }) | None => 0,
            Some(Job::Counted { .. }) => 1,
            Some(Job::Rows { start: u32::MAX, .. }) => 2,
            Some(Job::Rows { .. }) => 3,
            Some(Job::Win { .. }) => 4,
        }
    }

    /// One piece of the listing being made while a track plays: card
    /// operations while the chore's budget lasts (`audio::piece_left`, at
    /// least one), at most [`PIECE_ENTRIES`] entries or [`PIECE_ROWS`]
    /// rows. Up next titles come first, one per operation. The window is
    /// published as it fills, with `complete: false` until it is whole (the
    /// UI asks for nothing meanwhile).
    pub async fn step(&mut self, vol: &mut Vol) {
        if let Some((qgen, first, mut mask)) = self.qjob.take() {
            if qgen == self.qgen {
                let mut more = true;
                while more && crate::audio::piece_left() {
                    (mask, more) = Own(self.queue_title(vol, first, mask)).await;
                }
                self.publish_qwin(first, mask);
                if more {
                    self.qjob = Some((qgen, first, mask));
                }
            }
            return;
        }
        match self.job.take() {
            None => {}
            Some(Job::Count { mut cur, mut sig, mut n }) => {
                let mut e = DirEntry::new();
                let mut k = 0;
                let end = loop {
                    if k >= PIECE_ENTRIES || !crate::audio::piece_left() {
                        break false;
                    }
                    let before = cur.index();
                    match vol.read_dir(&mut cur, &mut e).await {
                        Ok(true) => {}
                        _ => break true,
                    }
                    k += 1;
                    if is_dot(&e) {
                        continue;
                    }
                    sig.add(&src_entry(&e), e.name());
                    if !Self::matches(self.filter, &e) {
                        continue;
                    }
                    if n % WINDOW == 0 && ((n / WINDOW) as usize) < CHECKPOINTS {
                        self.checkpoints[(n / WINDOW) as usize] = before;
                    }
                    if n < WINDOW {
                        let row = Self::row(&e);
                        critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
                    }
                    n += 1;
                };
                self.job = Some(if end { Job::Counted { sig, n } } else { Job::Count { cur, sig, n } });
            }
            Some(Job::Counted { sig, n }) => {
                self.count = n;
                let spec = Spec::from(self.filter);
                if n >= 2 {
                    let dircl = self.dircl;
                    // The one path walk of the listing: its pieces reopen
                    // the file from where this found it.
                    let f = FileName::index(dircl, spec);
                    let mut at = None;
                    let ix =
                        with_index(vol, f, &mut at, |io| match Index::open(&mut io.store(), dircl, spec) {
                            Ok(Some(ix)) if ix.is_current(&sig) => Some(ix),
                            _ => None,
                        })
                        .await;
                    if let Some(ix) = ix {
                        self.claim(ix.file());
                        if self.build.is_some_and(|b| b.dircl == dircl && b.spec == spec) {
                            self.build = None;
                        }
                        self.set_index(Some(ix));
                        self.index_at = at;
                        self.job = Some(Job::Rows { ix, start: u32::MAX, n: 0, letters: true });
                        return;
                    }
                }
                let (dir, lgen) = (self.dir, self.lgen);
                critical_section::with(|cs| {
                    let mut w = BROWSE.borrow(cs).borrow_mut();
                    // A single entry is in order already.
                    w.listing = Listing { dir, count: n, sorted: n <= 1, complete: true };
                    w.lgen = lgen;
                    w.first = 0;
                    w.n = n.min(WINDOW);
                });
                ready(tag::BROWSE);
                if n >= 2
                    && let Some(start) = Self::cursor(vol, self.entry).await
                {
                    self.want_build(start, spec);
                }
            }
            Some(Job::Rows { ix, start: u32::MAX, n, letters }) => {
                // The focused file first (a piece of its own).
                let mut at = self.index_at;
                let first =
                    with_index(vol, ix.file(), &mut at, |io| self.focus_row(io, &ix, FOCUS_SCAN_PLAYING))
                        .await;
                self.index_at = at;
                let start = first.unwrap_or(0).saturating_sub(WINDOW / 4).min(ix.count().saturating_sub(1));
                self.job = Some(Job::Rows { ix, start, n, letters });
            }
            Some(Job::Rows { ix, start, n, letters }) => {
                let mut e = DirEntry::new();
                let e = &mut e;
                let lgen = self.lgen;
                // One session, the file reopened where it was found; the
                // letter table in it too when the window is whole and the
                // budget lasts (else in the next piece).
                let mut at = self.index_at;
                let (n, done, letters) = with_index(vol, ix.file(), &mut at, |io| {
                    let (n, done) = Self::fill_some(io, &ix, start, n, e);
                    let letters =
                        letters && !(done && crate::audio::piece_left() && Self::load_letters(io, &ix, lgen));
                    (n, done, letters)
                })
                .await;
                self.index_at = at;
                let (dir, count) = (self.dir, ix.count());
                critical_section::with(|cs| {
                    let mut w = BROWSE.borrow(cs).borrow_mut();
                    w.listing = Listing { dir, count, sorted: true, complete: done && !letters };
                    w.lgen = lgen;
                    w.first = start;
                    w.n = n;
                });
                ready(tag::BROWSE);
                if !done || letters {
                    self.job = Some(Job::Rows { ix, start, n, letters });
                }
            }
            Some(Job::Win { cur, start, skip, n }) => Own(self.win_piece(vol, cur, start, skip, n)).await,
        }
    }

    /// A piece of the window of an unsorted listing (`Job::Win`): placed at
    /// the checkpoint before it (one card operation), then entries while
    /// the budget lasts; published when whole.
    async fn win_piece(
        &mut self,
        vol: &mut Vol,
        cur: Option<DirCursor>,
        start: u32,
        mut skip: u32,
        mut n: u32,
    ) {
        let mut cur = match cur {
            Some(c) => c,
            None => {
                // The first operation (always allowed): the entries are
                // asked for one by one after it.
                crate::audio::piece_left();
                let k = ((start / WINDOW) as usize).min(CHECKPOINTS - 1);
                let Some(mut c) = Self::cursor(vol, self.entry).await else { return };
                if vol.seek_dir(&mut c, self.checkpoints[k]).await.is_err() {
                    return;
                }
                c
            }
        };
        let mut e = DirEntry::new();
        while n < WINDOW {
            if !crate::audio::piece_left() {
                self.job = Some(Job::Win { cur: Some(cur), start, skip, n });
                return;
            }
            match vol.read_dir(&mut cur, &mut e).await {
                Ok(true) => {}
                _ => break,
            }
            if !Self::matches(self.filter, &e) {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let row = Self::row(&e);
            critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
            n += 1;
        }
        critical_section::with(|cs| {
            let mut w = BROWSE.borrow(cs).borrow_mut();
            w.first = start;
            w.n = n;
        });
        ready(tag::BROWSE);
    }

    /// A window of the listing while a track plays: in pieces (`step`),
    /// from the index, or (an unsorted listing) from the checkpoint before
    /// it, at most two windows of entries, as idle.
    fn window_playing(&mut self, first: u32) {
        self.job = Some(match self.index {
            Some(ix) => {
                let start = first.saturating_sub(WINDOW / 4).min(ix.count().saturating_sub(1));
                Job::Rows { ix, start, n: 0, letters: false }
            }
            None => {
                let start = first.saturating_sub(WINDOW / 4).min(self.count.saturating_sub(1));
                let k = (start / WINDOW).min(CHECKPOINTS as u32 - 1);
                Job::Win { cur: None, start, skip: start - k * WINDOW, n: 0 }
            }
        });
    }

    /// Rows `start + n..` of `ix` into the window until it is full, the
    /// chore's budget is used up (`audio::piece_left` before each card
    /// operation: the rows read, the first of which opens the index file,
    /// and each row's entry) or [`PIECE_ROWS`] are filled; returns the rows
    /// filled and whether the window is complete.
    #[inline(never)]
    fn fill_some(io: &Io<'_, '_>, ix: &Index, start: u32, mut n: u32, e: &mut DirEntry) -> (u32, bool) {
        let mut rows = [ROW0; 4];
        let n0 = n;
        while n < WINDOW {
            if n - n0 >= PIECE_ROWS || !crate::audio::piece_left() {
                return (n, false);
            }
            let want = ((WINDOW - n) as usize).min(rows.len());
            let got = match ix.rows(&mut io.store(), start + n, &mut rows[..want]) {
                Ok(g) if g > 0 => g,
                _ => break,
            };
            for r in &rows[..got] {
                if n - n0 >= PIECE_ROWS || !crate::audio::piece_left() {
                    return (n, false);
                }
                let found = io.with_vol(|vol, cx| spin(cx, vol.open_locator(r.loc, e)).is_ok());
                let row = if found {
                    Self::row(e)
                } else {
                    Row { name: Name::new("?"), kind: r.kind, size: r.size, file: fref(r.loc), ..Row::EMPTY }
                };
                critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
                n += 1;
            }
        }
        (n, true)
    }

    /// The index to serve a listing from: the current one, or (idle,
    /// small folder: `inline` is the scratch) one built now.
    #[inline(never)]
    fn resolve(
        io: &Io<'_, '_>,
        start: DirCursor,
        spec: Spec,
        sig: &Signature,
        inline: Option<&mut [u8]>,
        e: &mut DirEntry,
    ) -> Option<Index> {
        if let Ok(Some(ix)) = Index::open(&mut io.store(), start.dir_cluster(), spec)
            && ix.is_current(sig)
        {
            return Some(ix);
        }
        let mut b = Builder::new(inline?, start.dir_cluster(), spec, 0)?;
        let mut cur = start;
        let mut src = io.dir(start, &mut cur, e);
        // At most `INLINE_MAX` entries: a few steps.
        for _ in 0..32 {
            match b.step(&mut io.store(), &mut src, 256) {
                Ok(Progress::Done(ix)) => return Some(ix),
                Ok(_) => {}
                Err(_) => return None,
            }
        }
        None
    }

    /// Publish the sorted listing from `ix`: rows from near `first`;
    /// `None` (a listing just sorted): around the focused file if it is
    /// within the first `scan` rows, else from the top, and the letter
    /// table.
    #[inline(never)]
    fn publish_index(&self, io: &Io<'_, '_>, ix: &Index, first: Option<u32>, scan: u32, e: &mut DirEntry) {
        let count = ix.count();
        let letters = first.is_none();
        let first = first.or_else(|| self.focus_row(io, ix, scan)).unwrap_or(0);
        let start = first.saturating_sub(WINDOW / 4).min(count.saturating_sub(1));
        let n = Self::fill_rows(io, ix, start, e);
        if letters {
            Self::load_letters(io, ix, self.lgen);
        }
        let (dir, lgen) = (self.dir, self.lgen);
        critical_section::with(|cs| {
            let mut w = BROWSE.borrow(cs).borrow_mut();
            w.listing = Listing { dir, count, sorted: true, complete: true };
            w.lgen = lgen;
            w.first = start;
            w.n = n;
        });
        ready(tag::BROWSE);
    }

    /// The letter table of `ix` for listing generation `lgen` (true: it
    /// was looked at).
    #[inline(never)]
    fn load_letters(io: &Io<'_, '_>, ix: &Index, lgen: u32) -> bool {
        let mut t = LetterTable::EMPTY;
        if t.load(ix, &mut io.store()).is_err() {
            t = LetterTable::EMPTY;
        }
        critical_section::with(|cs| *LETTERS.borrow(cs).borrow_mut() = (lgen, t));
        true
    }

    /// Row of the focused file in `ix` (a pass over its rows; not for more
    /// than `scan` rows).
    #[inline(never)]
    fn focus_row(&self, io: &Io<'_, '_>, ix: &Index, scan: u32) -> Option<u32> {
        let f = critical_section::with(|cs| FOCUS.borrow(cs).get())?;
        if f.dir != self.dircl || ix.count() > scan {
            return None;
        }
        ix.position_of(&mut io.store(), loc(f)).ok()?
    }

    /// Rows `start..` of `ix` into the window (names through the
    /// locators); returns how many.
    #[inline(never)]
    fn fill_rows(io: &Io<'_, '_>, ix: &Index, start: u32, e: &mut DirEntry) -> u32 {
        let mut rows = [ROW0; 4];
        let mut n = 0u32;
        while n < WINDOW {
            let want = ((WINDOW - n) as usize).min(rows.len());
            let got = match ix.rows(&mut io.store(), start + n, &mut rows[..want]) {
                Ok(g) if g > 0 => g,
                _ => break,
            };
            for r in &rows[..got] {
                let found = io.with_vol(|vol, cx| spin(cx, vol.open_locator(r.loc, e)).is_ok());
                let row = if found {
                    Self::row(e)
                } else {
                    Row { name: Name::new("?"), kind: r.kind, size: r.size, file: fref(r.loc), ..Row::EMPTY }
                };
                critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
                n += 1;
            }
        }
        n
    }

    /// The listing's index file is (being) written: not an orphan.
    fn claim(&mut self, f: FileName) {
        if self.orphan == Some(f) {
            self.orphan = None;
        }
    }

    /// Build the index of the listing in the background (replacing any
    /// other build: the listing shown comes first).
    fn want_build(&mut self, start: DirCursor, spec: Spec) {
        let dircl = start.dir_cluster();
        if self.build.is_some_and(|b| b.dircl == dircl && b.spec == spec) {
            return;
        }
        if self.build.is_some_and(|b| b.st.is_some()) {
            self.abandon_build();
        }
        self.claim(FileName::index(dircl, spec));
        self.build = Some(Build { dircl, spec, start, cur: start, st: None });
    }

    // ------------------------------------------------- background build

    /// A build is waiting for the idle card task.
    #[must_use]
    pub fn wants_build(&self) -> bool {
        self.build.is_some()
    }

    /// Before a build starts: remove an abandoned build's files.
    #[inline(never)]
    pub fn clean_orphan(&mut self, io: &Io<'_, '_>) {
        if self.build.is_some_and(|b| b.st.is_some()) {
            return;
        }
        if let Some(f) = self.orphan.take() {
            let mut s = io.store();
            let _ = s.remove(f);
            let _ = s.remove(FileName::TEMP_A);
            let _ = s.remove(FileName::TEMP_B);
        }
    }

    /// The background build on `scratch` (started or resumed).
    pub fn resume_build<'s>(&mut self, scratch: &'s mut [u8]) -> Option<Builder<'s>> {
        let b = self.build.as_ref()?;
        let r = match b.st {
            Some(st) => st.resume(scratch),
            None => Builder::new(scratch, b.dircl, b.spec, 0),
        };
        if r.is_none() {
            self.abandon_build();
        }
        r
    }

    /// One step of the background build (bounded by [`BUILD_BUDGET`]).
    /// `None`: it failed.
    #[inline(never)]
    pub fn step_build(&mut self, io: &Io<'_, '_>, b: &mut Builder<'_>, e: &mut DirEntry) -> Option<Progress> {
        let bl = self.build.as_mut()?;
        let mut src = io.dir(bl.start, &mut bl.cur, e);
        b.step(&mut io.store(), &mut src, BUILD_BUDGET).ok()
    }

    /// Stop the background build here (its state is kept for the next
    /// slot); a build that cannot be suspended is given up.
    #[inline(never)]
    pub fn suspend_build(&mut self, io: &Io<'_, '_>, b: Builder<'_>) {
        match b.suspend(&mut io.store()) {
            Ok(st) => {
                if let Some(bl) = self.build.as_mut() {
                    bl.st = Some(st);
                }
            }
            Err(_) => self.abandon_build(),
        }
    }

    /// Give up the background build (its files go before the next one).
    pub fn abandon_build(&mut self) {
        if let Some(b) = self.build.take() {
            self.orphan = Some(FileName::index(b.dircl, b.spec));
        }
    }

    /// The background build finished: serve the listing from it if it is
    /// still the one shown.
    #[inline(never)]
    pub fn build_done(&mut self, io: &Io<'_, '_>, ix: Index, e: &mut DirEntry) {
        let Some(b) = self.build.take() else { return };
        if b.dircl == self.dircl && b.spec == Spec::from(self.filter) && self.index.is_none() {
            self.set_index(Some(ix));
            self.publish_index(io, &ix, None, u32::MAX, e);
        }
    }

    /// Before the card goes to the USB host: no index is trusted after it
    /// comes back; a build in progress is given up and its files removed.
    pub async fn card_leaving(&mut self, vol: &mut Vol) {
        crate::library::card_leaving(self);
        // The host may rename or replace queued files: no Up next title is
        // kept across the hand-over.
        self.titles_stale();
        self.job = None;
        self.set_index(None);
        // The host may change the folder's pictures.
        self.folder_art = None;
        self.dircl = u32::MAX;
        if self.build.is_some_and(|b| b.st.is_some()) {
            self.abandon_build();
        }
        self.build = None;
        if self.orphan.is_some() {
            with_io(vol, Mode::Write { alt: false }, |io| self.clean_orphan(io)).await;
        }
    }

    async fn window(&mut self, vol: &mut Vol, first: u32) {
        let mut e = DirEntry::new();
        if let Some(ix) = self.index {
            let e = &mut e;
            let mut at = self.index_at;
            with_index(vol, ix.file(), &mut at, |io| self.publish_index(io, &ix, Some(first), 0, e)).await;
            self.index_at = at;
            return;
        }
        let start = first.saturating_sub(WINDOW / 4).min(self.count.saturating_sub(1));
        let k = ((start / WINDOW) as usize).min(CHECKPOINTS - 1);
        let Some(mut cur) = Self::cursor(vol, self.entry).await else { return };
        if vol.seek_dir(&mut cur, self.checkpoints[k]).await.is_err() {
            return;
        }
        let mut skip = start - k as u32 * WINDOW;
        let mut n = 0u32;
        while n < WINDOW {
            match vol.read_dir(&mut cur, &mut e).await {
                Ok(true) => {}
                _ => break,
            }
            if !Self::matches(self.filter, &e) {
                continue;
            }
            if skip > 0 {
                skip -= 1;
                continue;
            }
            let row = Self::row(&e);
            critical_section::with(|cs| BROWSE.borrow(cs).borrow_mut().rows[n as usize] = row);
            n += 1;
        }
        critical_section::with(|cs| {
            let mut w = BROWSE.borrow(cs).borrow_mut();
            w.first = start;
            w.n = n;
        });
        ready(tag::BROWSE);
    }

    async fn text(&mut self, vol: &mut Vol, file: FileRef, offset: u32, buf: &mut [u8]) {
        let n = TEXT_WIN.min(buf.len());
        let buf = &mut buf[..n];
        let mut e = DirEntry::new();
        let mut got = 0usize;
        if vol.open_locator(loc(file), &mut e).await.is_ok()
            && let Ok(mut f) = vol.open_entry(&e)
        {
            f.set_position(u64::from(offset));
            while got < buf.len() {
                match vol.read(&mut f, &mut buf[got..]).await {
                    Ok(0) | Err(_) => break,
                    Ok(k) => got += k,
                }
            }
        }
        let text = &*buf;
        critical_section::with(|cs| {
            let mut w = TEXT.borrow(cs).borrow_mut();
            w.file = file;
            w.offset = offset;
            w.len = got as u32;
            w.buf[..got].copy_from_slice(&text[..got]);
        });
        ready(tag::TEXT);
    }

    /// Queued files may have changed on the card: a new queue generation,
    /// so no title is kept from the window ([`Server::keep_titles`]) and
    /// the UI asks for it again.
    fn titles_stale(&mut self) {
        self.qgen = self.qgen.wrapping_add(1);
        self.qjob = None;
        self.publish();
    }

    /// Slots of the Up next window at `first` that are queue positions
    /// (at most [`QWIN`]), as a mask.
    fn qwin_slots(&self, first: u32) -> u32 {
        let len = self.player.as_ref().map_or(0, |pl| pl.len().min(self.qlen));
        let n = len.saturating_sub(first).min(QWIN as u32);
        ((1u64 << n) - 1) as u32
    }

    /// The Up next window moves to `first`: the titles it holds for the
    /// same queue generation stay (moved to their slots) and the filled
    /// start of it is shown; returns the mask of the slots filled.
    fn keep_titles(&self, first: u32) -> u32 {
        let (qgen, slots) = (self.qgen, self.qwin_slots(first));
        critical_section::with(|cs| {
            let mut w = QWINDOW.borrow(cs).borrow_mut();
            let kept = if w.qgen == qgen {
                let (from, n) = (w.first, w.n);
                orpheus_player::queue::shift_window(&mut w.titles, from, n, first)
            } else {
                0
            };
            let mask = kept & slots;
            w.qgen = qgen;
            w.first = first;
            w.n = orpheus_player::queue::window_prefix(mask, QWIN as u32);
            mask
        })
    }

    /// Publish the Up next window at `first` with the slots of `mask`
    /// filled (the start of it without a gap is shown).
    fn publish_qwin(&self, first: u32, mask: u32) {
        let qgen = self.qgen;
        critical_section::with(|cs| {
            let mut w = QWINDOW.borrow(cs).borrow_mut();
            w.qgen = qgen;
            w.first = first;
            w.n = orpheus_player::queue::window_prefix(mask, QWIN as u32);
        });
        ready(tag::QUEUE);
    }

    /// One more title of the Up next window at `first` (`mask`: the slots
    /// filled so far): the first missing one, by its locator. Returns the
    /// new mask and whether titles are still missing. A title that takes
    /// 20 ms or more is logged with its card operations and code-line
    /// loads (`qtitle ms/io/ld A/B/C`), for the player check of AU-PERF.
    async fn queue_title(&mut self, vol: &mut Vol, first: u32, mask: u32) -> (u32, bool) {
        let todo = self.qwin_slots(first) & !mask;
        if todo == 0 {
            return (mask, false);
        }
        let i = todo.trailing_zeros();
        let item = self.player.as_ref().map_or(0, |pl| pl.item_at(first.saturating_add(i)));
        let (t0, io0, l0) = (atj2157_hal::time::now_us(), yoton_bsp::card::io_count(), page_loads());
        let title = match self.queue.get(item as usize) {
            Some(&l) => {
                let mut e = DirEntry::new();
                if vol.open_locator(l, &mut e).await.is_ok() {
                    Short::title(e.name())
                } else {
                    Short::new("?")
                }
            }
            None => Short::new("?"),
        };
        let us = (atj2157_hal::time::now_us() - t0) as u32;
        if us >= 20_000 {
            let mut l = atj2157_hal::log::LineBuf::new();
            // Short strings: they land in resident rodata (the stack's RAM).
            l.push_str("qtitle ms/io/ld ")
                .push_u32(us / 1000)
                .push_str("/")
                .push_u32(yoton_bsp::card::io_count().wrapping_sub(io0))
                .push_str("/")
                .push_u32(page_loads().wrapping_sub(l0));
            l.emit();
        }
        critical_section::with(|cs| {
            if let Some(t) = QWINDOW.borrow(cs).borrow_mut().titles.get_mut(i as usize) {
                *t = title;
            }
        });
        let mask = mask | 1 << i;
        (mask, todo & !(1 << i) != 0)
    }

    /// The Up next window at `first`, whole (idle: nothing plays).
    async fn queue_win(&mut self, vol: &mut Vol, first: u32) {
        let mut mask = self.keep_titles(first);
        loop {
            let (m, more) = self.queue_title(vol, first, mask).await;
            mask = m;
            if !more {
                break;
            }
        }
        self.publish_qwin(first, mask);
    }

    /// Fill the queue with the audio files of a folder: in the listing's
    /// order when it is the folder shown and its index is current, else in
    /// on-disk order. Returns the item of `start`, if found.
    pub async fn build_queue(
        &mut self,
        vol: &mut Vol,
        entry: Option<FileRef>,
        start: Option<FileRef>,
    ) -> Option<u32> {
        self.qgen = self.qgen.wrapping_add(1);
        let mut found = self.queue_from_disk(vol, entry, start).await;
        if let Some(ix) = self.index
            && entry == self.entry
        {
            let disk = self.qlen;
            let mut at = self.index_at;
            let q = with_index(vol, ix.file(), &mut at, |io| self.queue_from_index(io, &ix, start)).await;
            self.index_at = at;
            match q {
                Some((n, f)) if n == disk => found = f,
                // The index and the folder disagree: on-disk order.
                _ => found = self.queue_from_disk(vol, entry, start).await,
            }
        }
        self.queue_id = orpheus_player::image::queue_id(vol.serial(), &self.queue[..self.qlen as usize]);
        found
    }

    /// Shuffle-all walk progress for the status reply: entries read, and
    /// audio files seen | depth << 24.
    pub fn walk_progress() -> (u32, u32) {
        (
            WALK_STEPS.load(core::sync::atomic::Ordering::Relaxed),
            WALK_SEEN.load(core::sync::atomic::Ordering::Relaxed),
        )
    }

    /// Shuffle all: walk the card's visible folders (hidden ones such as
    /// `/.orpheus` skipped, depth ≤ 8, no recursion) and keep a uniform sample
    /// of up to `QUEUE` audio files (reservoir sampling).
    pub async fn build_all_queue(&mut self, vol: &mut Vol, seed: u32) {
        self.qgen = self.qgen.wrapping_add(1);
        self.qlen = 0;
        self.queue_id = 0;
        let mut stack: [Option<DirCursor>; 8] = [None; 8];
        stack[0] = Some(vol.root());
        let mut depth = 1usize;
        let mut e = DirEntry::new();
        let mut seen = 0u32;
        let mut rng = seed | 1;
        let mut steps = 0u32;
        let mut chunk_start = embassy_time::Instant::now();
        while depth > 0 {
            // The card task runs at RT priority: a long walk must give the
            // CPU back regularly (it starved thread mode into the watchdog).
            steps += 1;
            WALK_STEPS.store(steps, core::sync::atomic::Ordering::Relaxed);
            WALK_SEEN.store(seen | (depth as u32) << 24, core::sync::atomic::Ordering::Relaxed);
            // Also in two spare persistent words (survive a watchdog reset).
            // SAFETY: mailbox argument words 5/6, unused by the test harness.
            unsafe {
                core::ptr::write_volatile(0x0013_7FD4 as *mut u32, steps);
                core::ptr::write_volatile(0x0013_7FD8 as *mut u32, seen | (depth as u32) << 24);
            }
            // At most half the CPU: after every 8 entries, sleep as long as
            // they took (≥ 2 ms). A 1 ms pause per 16 entries still starved
            // the UI into the watchdog (2,061 entries read by then).
            if steps.is_multiple_of(8) {
                let took = embassy_time::Instant::now().saturating_duration_since(chunk_start);
                embassy_time::Timer::after(took.max(embassy_time::Duration::from_millis(2))).await;
                chunk_start = embassy_time::Instant::now();
            }
            let Some(cur) = stack[depth - 1].as_mut() else { break };
            match vol.read_dir(cur, &mut e).await {
                Ok(true) => {}
                _ => {
                    depth -= 1;
                    continue;
                }
            }
            let name = e.name();
            if name == "." || name == ".." {
                continue;
            }
            if e.is_dir() {
                if !is_hidden_name(name)
                    && e.attributes() & 0x06 == 0
                    && depth < stack.len()
                    && let Ok(c) = vol.dir_cursor(&e)
                {
                    stack[depth] = Some(c);
                    depth += 1;
                }
                continue;
            }
            if !Self::queueable(name, false, e.attributes()) {
                continue;
            }
            seen += 1;
            let l = e.locator();
            if (self.qlen as usize) < QUEUE {
                self.queue[self.qlen as usize] = l;
                self.qlen += 1;
            } else {
                rng ^= rng << 13;
                rng ^= rng >> 17;
                rng ^= rng << 5;
                let j = (rng % seen) as usize;
                if j < QUEUE {
                    self.queue[j] = l;
                }
            }
        }
        self.queue_id = orpheus_player::image::queue_id(vol.serial(), &self.queue[..self.qlen as usize]);
    }

    /// The queue item a file is, if it may be queued: visible audio files.
    fn queueable(name: &str, is_dir: bool, attr: u8) -> bool {
        !is_dir && kind_of(name, false) == EntryKind::Audio && !is_hidden_name(name) && attr & 0x06 == 0
    }

    async fn queue_from_disk(
        &mut self,
        vol: &mut Vol,
        entry: Option<FileRef>,
        start: Option<FileRef>,
    ) -> Option<u32> {
        self.qlen = 0;
        self.queue_id = 0;
        let mut found = None;
        let mut cur = Self::cursor(vol, entry).await?;
        let mut e = DirEntry::new();
        while (self.qlen as usize) < QUEUE {
            match vol.read_dir(&mut cur, &mut e).await {
                Ok(true) => {}
                _ => break,
            }
            if !Self::queueable(e.name(), e.is_dir(), e.attributes()) {
                continue;
            }
            let l = e.locator();
            if start.is_some_and(|s| loc(s) == l) {
                found = Some(self.qlen);
            }
            self.queue[self.qlen as usize] = l;
            self.qlen += 1;
        }
        found
    }

    /// The queue in the order of `ix`: (items, item of `start`); `None` if
    /// the index cannot be read.
    #[inline(never)]
    fn queue_from_index(
        &mut self,
        io: &Io<'_, '_>,
        ix: &Index,
        start: Option<FileRef>,
    ) -> Option<(u32, Option<u32>)> {
        let mut rows = [ROW0; 8];
        let (mut i, mut n, mut found) = (0u32, 0u32, None);
        while i < ix.count() && (n as usize) < QUEUE {
            let got = ix.rows(&mut io.store(), i, &mut rows).ok()?;
            if got == 0 {
                return None;
            }
            for r in &rows[..got] {
                if r.kind != EntryKind::Audio || r.hidden || n as usize >= QUEUE {
                    continue;
                }
                if start.is_some_and(|s| loc(s) == r.loc) {
                    found = Some(n);
                }
                self.queue[n as usize] = r.loc;
                n += 1;
            }
            i += got as u32;
        }
        self.qlen = n;
        Some((n, found))
    }

    // ------------------------------------------------------------ player

    /// Create the player from `player.bin` (`file`, possibly empty; once,
    /// after mount). Not inlined: the player is built by value here, not in
    /// the card task's frame.
    #[inline(never)]
    pub fn init_player(&mut self, file: &[u8]) {
        self.player = Some(Player::new(Config::default(), &mut self.store.load(file)));
    }

    /// Feed one event to the player; its actions come back as a [`Flow`].
    pub fn feed(&mut self, ev: Event) -> Flow {
        let Some(pl) = self.player.as_mut() else { return Flow::default() };
        let before = pl.sleep_timer();
        let mut out = Actions::default();
        pl.handle(ev, &mut self.store, &mut out);
        let after = pl.sleep_timer();
        if !matches!(ev, Event::Command(_))
            && ((before.0.is_some() && after.0.is_none()) || (before.1 && !after.1))
        {
            self.sleep_fired = self.sleep_fired.wrapping_add(1);
        }
        match (ev, pl.state()) {
            (Event::Opened { .. }, _) => self.can_resume = true,
            // The queue played to its end: nothing to resume.
            (_, State::Draining) => self.can_resume = false,
            _ => {}
        }
        if !matches!(ev, Event::Position { .. }) {
            self.publish();
        }
        Flow::of(&out)
    }

    /// [`Server::feed`] of `Event::Position` for the playback loop's every
    /// block (AU-PERF): the player's position step only. A position changes
    /// neither the sleep timer nor what is published (`feed` skips the
    /// publish for it), so nothing else of `feed` applies but the
    /// `Draining` rule.
    #[inline(never)]
    pub fn position(&mut self, frame: u64) -> Flow {
        let Some(pl) = self.player.as_mut() else { return Flow::default() };
        let mut out = Actions::default();
        pl.position(frame, &mut out);
        if pl.state() == State::Draining {
            self.can_resume = false;
        }
        if out.is_empty() { Flow::default() } else { Flow::of(&out) }
    }

    /// Publish the player's state to the snapshot (not the position: the
    /// playback loop does that per block).
    pub fn publish(&self) {
        let Some(pl) = self.player.as_ref() else { return };
        let cur = pl.current();
        let rate = u64::from(cur.map_or(0, |c| c.rate));
        let ms = move |f: u64| (f * 1000).checked_div(rate).unwrap_or(0) as u32;
        let (a, b) = pl.ab();
        let (left, after_track) = pl.sleep_timer();
        let (pos, len, shuffled) = (cur.map_or(0, |c| c.pos), pl.len(), pl.shuffled());
        let (fired, resume, qgen, bm_gen) = (self.sleep_fired, self.can_resume, self.qgen, self.bm_gen);
        player_update(|p| {
            p.queue_pos = pos;
            p.queue_len = len;
            p.shuffled = shuffled;
            p.a_ms = a.map(ms);
            p.b_ms = b.map(ms);
            p.sleep_min = left.map(|l| l.div_ceil(60_000) as u16).or(after_track.then_some(0));
            p.sleep_fired = fired;
            p.resume = resume;
            p.qgen = qgen;
            p.bm_gen = bm_gen;
        });
    }

    /// A player request (all but `PlayDir`, which needs the card first).
    pub fn play_req(&mut self, r: PlayReq) -> Flow {
        let Some(pl) = self.player.as_ref() else { return Flow::default() };
        let (state, cur, len, ab) = (pl.state(), pl.current(), pl.len(), pl.ab());
        match r {
            PlayReq::PlayDir { .. }
            | PlayReq::PlayLib { .. }
            | PlayReq::ShuffleAll
            | PlayReq::Shutdown
            | PlayReq::Edit(_) => Flow::default(),
            PlayReq::Cmd(c) => {
                match c {
                    Command::Repeat(rp) => self.repeat = rp,
                    Command::Shuffle(_) => self.qgen = self.qgen.wrapping_add(1),
                    _ => {}
                }
                let f = self.feed(cmd(c));
                if c == Command::AddBookmark && cur.is_some_and(|c| c.track.is_some()) {
                    self.bookmark_added();
                }
                f
            }
            PlayReq::JumpPos(p) if p < len => {
                let item = pl.item_at(p);
                self.feed(cmd(Command::JumpTo(item)))
            }
            PlayReq::JumpPos(_) => Flow::default(),
            PlayReq::Resume => match state {
                State::Paused => self.feed(cmd(Command::Play)),
                State::Stopped | State::Draining if cur.is_some() && self.can_resume => {
                    self.feed(cmd(Command::Play))
                }
                State::Stopped | State::Draining => self.resume_saved(),
                State::Opening | State::Playing => Flow::default(),
            },
            PlayReq::Stop => {
                let f = self.feed(cmd(Command::Stop));
                self.store.forget_resume();
                self.can_resume = false;
                self.publish();
                f
            }
            PlayReq::SetA => {
                self.feed(cmd(Command::AbClear));
                self.feed(cmd(Command::AbPoint))
            }
            PlayReq::SetB if ab.0.is_some() && ab.1.is_none() => self.feed(cmd(Command::AbPoint)),
            PlayReq::SetB => Flow::default(),
        }
    }

    /// Change the queue (`PlayReq::Edit`; `serial`: the card's). The queue
    /// is rewritten in play order (a shuffled one plays on unshuffled in
    /// the order it had), the player follows without interrupting the
    /// track (`Command::Requeue`), Up next is asked for again and
    /// `queue.bin` is rewritten at the next save. With nothing queued, Play
    /// next and Add to queue start a queue of that one file. An edit that
    /// does not apply (a full queue: the UI says so; a position that is
    /// not after the current track) changes nothing.
    #[inline(never)]
    pub fn edit(&mut self, e: QueueEdit, serial: u32) -> Flow {
        use orpheus_player::queue::{Edit, edit};
        let Some(pl) = self.player.as_ref() else { return Flow::default() };
        let len = pl.len().min(self.qlen);
        let Some(cur) = pl.current().filter(|_| len > 0) else {
            let (Edit::PlayNext(l) | Edit::Add(l)) = e else { return Flow::default() };
            self.queue[0] = l;
            self.set_queue_len(serial, 1);
            self.queue_dirty = true;
            return self.start_queue(Some(0), false, 0);
        };
        let order = pl.order();
        let Some(n) = edit(&mut self.queue, len as usize, cur.pos, order.as_ref(), e) else {
            return Flow::default();
        };
        self.set_queue_len(serial, n as u32);
        self.queue_dirty = true;
        let (id, len) = (self.queue_id, self.qlen);
        self.feed(cmd(Command::Requeue { id, len, pos: cur.pos }))
    }

    /// Play the queue just built: from item `start`, else (shuffled) a
    /// random item `rnd % len`, else the first.
    pub fn start_queue(&mut self, start: Option<u32>, shuffle: bool, rnd: u32) -> Flow {
        let (id, len) = (self.queue_id, self.qlen);
        if shuffle {
            // Off first: switching on picks a new order.
            self.feed(cmd(Command::Shuffle(false)));
        }
        self.feed(cmd(Command::Shuffle(shuffle)));
        let start = start.unwrap_or(if shuffle && len > 0 { rnd % len } else { 0 });
        self.feed(cmd(Command::PlayQueue { id, len, start }))
    }

    /// Continue at the record loaded at start-up.
    fn resume_saved(&mut self) -> Flow {
        let (qid, qlen, rp) = (self.queue_id, self.qlen, self.repeat);
        if !self.can_resume {
            return Flow::default();
        }
        let Some(pl) = self.player.as_mut() else { return Flow::default() };
        let mut out = Actions::default();
        if !pl.resume(qid, qlen, true, &mut out) {
            return Flow::default();
        }
        // The setting wins over the saved repeat mode.
        pl.handle(cmd(Command::Repeat(rp)), &mut self.store, &mut Actions::default());
        self.qgen = self.qgen.wrapping_add(1);
        self.publish();
        Flow::of(&out)
    }

    /// The player added a bookmark at the current track: mirror it in the
    /// bookmark window (oldest dropped when full, as the player does).
    fn bookmark_added(&mut self) {
        let Some(b) = self.player.as_ref().and_then(|p| p.bookmarks().last().copied()) else { return };
        let title = critical_section::with(|cs| Short::new(PLAYER.borrow(cs).borrow().title.as_str()));
        let row = BmRow { title, ms: bm_ms(b) };
        critical_section::with(|cs| {
            let mut w = BMS.borrow(cs).borrow_mut();
            if w.n as usize == BOOKMARKS {
                w.rows.copy_within(1.., 0);
                w.n -= 1;
            }
            let n = w.n as usize;
            w.rows[n] = row;
            w.n += 1;
        });
        self.bm_gen = self.bm_gen.wrapping_add(1);
        self.publish();
    }

    /// After start-up: titles of the saved bookmarks and of the resume
    /// point (one directory lookup each).
    pub async fn load_titles(&mut self, vol: &mut Vol) {
        let mut e = DirEntry::new();
        let n = self.player.as_ref().map_or(0, |p| p.bookmarks().len());
        for i in 0..n {
            let Some(b) = self.player.as_ref().and_then(|p| p.bookmarks().get(i).copied()) else { break };
            let found =
                vol.open_locator(b.track.locator(), &mut e).await.is_ok() && b.track.matches(e.name());
            let title = if found { Short::title(e.name()) } else { Short::new("?") };
            critical_section::with(|cs| BMS.borrow(cs).borrow_mut().rows[i] = BmRow { title, ms: bm_ms(b) });
        }
        critical_section::with(|cs| BMS.borrow(cs).borrow_mut().n = n as u32);
        self.bm_gen = self.bm_gen.wrapping_add(1);
        // The resume point: its track, else (the queue ended) its queue item.
        if let Some(r) = self.player.as_ref().and_then(Player::saved) {
            let at = match r.track {
                Some(t) => Some((t.locator(), Some(t))),
                None if r.queue_id == self.queue_id && r.item < self.qlen => {
                    Some((self.queue[r.item as usize], None))
                }
                None => None,
            };
            if let Some((l, t)) = at
                && vol.open_locator(l, &mut e).await.is_ok()
                && t.is_none_or(|t| t.matches(e.name()))
            {
                let title = Name::title(e.name());
                self.can_resume = true;
                player_update(|p| p.title = title);
            }
        }
        self.publish();
    }
}

impl Default for Server {
    fn default() -> Self {
        Self::new()
    }
}

// ------------------------------------------------------------- sound

/// The Sound settings the audio task applies to its DSP chain (published
/// by the UI when they change; `SOUND_GEN` tells the audio task to look).
#[derive(Clone, Copy, PartialEq, Eq, Default)]
pub struct Sound {
    /// Five EQ band gains, dB (presets resolved by the UI).
    pub eq: [i8; 5],
    /// Bass shelf, dB.
    pub bass: i8,
    /// −10 … +10.
    pub balance: i8,
    /// Off, low, medium, high.
    pub crossfeed: u8,
    /// Stereo, mono, swap.
    pub channels: u8,
    /// Off, track, album, auto.
    pub replaygain: u8,
}

pub static SOUND: Mutex<RefCell<Sound>> = Mutex::new(RefCell::new(Sound {
    eq: [0; 5],
    bass: 0,
    balance: 0,
    crossfeed: 0,
    channels: 0,
    replaygain: 0,
}));
pub static SOUND_GEN: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

/// Publish new Sound settings (no-op when unchanged).
pub fn publish_sound(s: Sound) {
    let changed = critical_section::with(|cs| {
        let mut cur = SOUND.borrow(cs).borrow_mut();
        let c = *cur != s;
        *cur = s;
        c
    });
    if changed {
        SOUND_GEN.fetch_add(1, core::sync::atomic::Ordering::Release);
    }
}

/// Apply the published Sound settings to a chain.
pub fn apply_sound(chain: &mut orpheus_dsp::Chain) {
    use orpheus_dsp::eq::{EQ5_FREQS, EQ5_Q100};
    use orpheus_dsp::{Band, ChannelMode, CrossfeedParams, RgMode, RgSettings};
    let s = critical_section::with(|cs| *SOUND.borrow(cs).borrow());
    let bands: [Band; 5] =
        core::array::from_fn(|i| Band::peaking(EQ5_FREQS[i], EQ5_Q100, i16::from(s.eq[i]) * 100));
    chain.set_eq(&bands);
    chain.set_bass(i16::from(s.bass) * 100);
    chain.set_balance(s.balance.saturating_mul(10));
    chain.set_crossfeed(match s.crossfeed {
        1 => Some(CrossfeedParams::DEFAULT),
        2 => Some(CrossfeedParams::CHU_MOY),
        3 => Some(CrossfeedParams::MEIER),
        _ => None,
    });
    chain.set_channel_mode(match s.channels {
        1 => ChannelMode::Mono,
        2 => ChannelMode::Swap,
        _ => ChannelMode::Stereo,
    });
    let mode = match s.replaygain {
        1 => RgMode::Track,
        2 | 3 => RgMode::Album,
        _ => RgMode::Off,
    };
    chain.set_replaygain(RgSettings { mode, ..RgSettings::default() });
}
