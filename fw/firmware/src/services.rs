//! The firmware's implementation of the `yoton-apps` service traits, from
//! state the tasks publish (battery, cable) and the settings cache.

use core::sync::atomic::Ordering;

use yoton_apps::browse::letter_of;
use yoton_apps::services::{
    self, BatteryInfo, BluetoothService, CardInfo, CardState, ClockService, DateTime, PlaybackService,
    Playing, PowerRequest, PowerService, SettingsService, StorageRequest, StorageService, SystemService,
    UsbMode, UsbService, UsbState,
};
use yoton_apps::services::{
    ArtSize, Bookmark, BrowseFilter, BrowseService, DirId, Entry, FileOp, FileRef, Levels, LibraryState,
    Listing, LyricLine, PlayerCmd, PlayerService, PlayerState, QueueEntry, TextService, TrackInfo,
};
use yoton_apps::settings::{SettingId, SettingsCache, id};
use yoton_apps::{App, Lang};
use yoton_ui::event::Source;
use yoton_ui::event::{BtState, Event, ServiceEvent};
use yoton_ui::shell::System;
use yoton_ui::widgets::bars::{MiniPlayer, Status};

use crate::{media, state};

/// Requests the UI made that other tasks carry out.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Action {
    Power(PowerRequest),
    Storage(StorageRequest),
    Usb(UsbMode),
}

pub struct Fw {
    pub settings: SettingsCache,
    pub volume: u8,
    pub locked: bool,
    pub pending: heapless_queue::Queue<Event, 8>,
    pub actions: heapless_queue::Queue<Action, 8>,
    /// Copies of the card task's windows (see `media`); in statics so `Fw`
    /// stays small (it is built on the stack).
    pub browse: &'static mut media::BrowseWin,
    pub text: &'static mut media::TextWin,
    pub player: media::PlayerSnap,
    /// Folders entered from the root: id, own entry, name.
    path: [(DirId, FileRef, media::Name); 8],
    depth: usize,
    /// Text-viewer positions (RAM only for now).
    text_pos: [(FileRef, u32); 8],
    text_size: u32,
}

/// A tiny fixed queue (no external dependency, no allocation).
pub mod heapless_queue {
    pub struct Queue<T: Copy, const N: usize> {
        buf: [Option<T>; N],
        len: usize,
    }
    impl<T: Copy, const N: usize> Queue<T, N> {
        #[must_use]
        pub const fn new() -> Self {
            Self { buf: [None; N], len: 0 }
        }
        pub fn push(&mut self, v: T) {
            if self.len < N {
                self.buf[self.len] = Some(v);
                self.len += 1;
            }
        }
        pub fn pop(&mut self) -> Option<T> {
            if self.len == 0 {
                return None;
            }
            let v = self.buf[0];
            self.buf.copy_within(1..self.len, 0);
            self.len -= 1;
            self.buf[self.len] = None;
            v
        }
    }
    impl<T: Copy, const N: usize> Default for Queue<T, N> {
        fn default() -> Self {
            Self::new()
        }
    }
}

static UI_BROWSE: static_cell::ConstStaticCell<media::BrowseWin> =
    static_cell::ConstStaticCell::new(media::BrowseWin::EMPTY);
static UI_TEXT: static_cell::ConstStaticCell<media::TextWin> =
    static_cell::ConstStaticCell::new(media::TextWin::EMPTY);

impl Fw {
    /// Once (takes the window statics).
    #[must_use]
    pub fn new() -> Self {
        Self {
            settings: SettingsCache::new(),
            volume: 12,
            locked: false,
            pending: heapless_queue::Queue::new(),
            actions: heapless_queue::Queue::new(),
            browse: UI_BROWSE.take(),
            text: UI_TEXT.take(),
            player: media::PlayerSnap::IDLE,
            path: [(DirId::ROOT, FileRef { dir: 0, index: 0 }, media::Name::EMPTY); 8],
            depth: 0,
            text_pos: [(FileRef { dir: u32::MAX, index: 0 }, 0); 8],
            text_size: 0,
        }
    }

    /// Copy the card task's window for `tag` (called before the UI gets
    /// `Event::Data`).
    pub fn sync(&mut self, tag: u16) {
        critical_section::with(|cs| match tag {
            media::tag::BROWSE => {
                let w = media::BROWSE.borrow(cs).borrow();
                // Only the folder this side asked for.
                if w.listing.dir == self.browse.listing.dir {
                    *self.browse = *w;
                }
            }
            media::tag::TEXT => *self.text = *media::TEXT.borrow(cs).borrow(),
            _ => {}
        });
    }

    /// Copy the player snapshot (every UI loop).
    pub fn sync_player(&mut self) {
        self.player = critical_section::with(|cs| *media::PLAYER.borrow(cs).borrow());
    }

    /// The directory entry of `dir`, from the current window or the path
    /// of folders entered. Keeps the path in step.
    fn entry_of(&mut self, dir: DirId) -> Option<FileRef> {
        if dir == DirId::ROOT {
            self.depth = 0;
            return None;
        }
        if let Some(i) = self.path[..self.depth].iter().position(|p| p.0 == dir) {
            self.depth = i + 1;
            return Some(self.path[i].1);
        }
        let w = &*self.browse;
        let row = w.rows[..w.n as usize].iter().find(|r| r.child == dir)?;
        let (file, name) = (row.file, row.name);
        if self.depth == self.path.len() {
            self.path.copy_within(1.., 0);
            self.depth -= 1;
        }
        self.path[self.depth] = (dir, file, name);
        self.depth += 1;
        Some(file)
    }
}

impl SettingsService for Fw {
    fn setting(&self, sid: SettingId) -> i16 {
        self.settings.get(sid)
    }
    fn set_setting(&mut self, sid: SettingId, value: i16) {
        if self.settings.set(sid, value) {
            self.pending.push(Event::Service(ServiceEvent::SettingChanged(sid.0)));
        }
    }
    fn factory_reset(&mut self) {
        self.settings.reset();
    }
}

impl PowerService for Fw {
    fn battery(&self) -> BatteryInfo {
        let s = state::snapshot();
        BatteryInfo { percent: s.percent, millivolts: s.millivolts, charge: s.charge }
    }
    fn sleep_timer_left(&self) -> Option<u16> {
        None
    }
    fn power(&mut self, req: PowerRequest) {
        if req == PowerRequest::LockKeys {
            self.locked = true;
        }
        self.actions.push(Action::Power(req));
    }
}

impl ClockService for Fw {
    fn now(&self) -> DateTime {
        let n = atj2157_hal::rtc_clock::now();
        // 2000-01-01 was a Saturday (weekday 5 with Monday = 0).
        let days = atj2157_hal::rtc_clock::epoch2000(n) / 86_400;
        DateTime {
            year: n.year,
            month: n.month,
            day: n.day,
            hour: n.hour,
            minute: n.minute,
            second: n.second,
            weekday: ((days + 5) % 7) as u8,
        }
    }
    fn set_now(&mut self, t: DateTime) {
        let _ = atj2157_hal::rtc_clock::set(atj2157_hal::rtc_clock::DateTime {
            year: t.year,
            month: t.month,
            day: t.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
        });
    }
    fn clock_valid(&self) -> bool {
        atj2157_hal::rtc_clock::running() && atj2157_hal::rtc_clock::now().year >= 2026
    }
}

impl StorageService for Fw {
    fn card(&self) -> CardState {
        if state::CARD_READY.load(Ordering::Relaxed) { CardState::Ready } else { CardState::Absent }
    }
    fn card_info(&self) -> Option<CardInfo> {
        None
    }
    fn storage(&mut self, req: StorageRequest) {
        self.actions.push(Action::Storage(req));
    }
}

impl PlaybackService for Fw {
    fn playing(&self) -> Option<Playing<'_>> {
        let p = &self.player;
        if p.source == Source::Idle {
            return None;
        }
        let progress = if p.duration_ms > 0 {
            (u64::from(p.position_ms) * 1000 / u64::from(p.duration_ms)).min(1000) as u16
        } else {
            0
        };
        Some(Playing { source: p.source, title: p.title.as_str(), artist: "", progress })
    }
    fn volume_step(&mut self, delta: i8) -> (u8, u8) {
        let max = u8::try_from(self.settings.get(id::VOLUME_LIMIT)).unwrap_or(30);
        let v = i16::from(self.volume) + i16::from(delta);
        self.volume = u8::try_from(v.clamp(0, i16::from(max))).unwrap_or(0);
        apply_volume(self.volume);
        (self.volume, max)
    }
}

/// UI volume 0..=30 to the DAC's digital volume: 1.5 dB per step below
/// −3 dB at 30, muted at 0.
pub fn apply_volume(v: u8) {
    let code = if v == 0 {
        0
    } else {
        atj2157_hal::dac::VOL_0DB.saturating_sub(8 + 4 * (30u8.saturating_sub(v.min(30))))
    };
    atj2157_hal::dac::set_volume(code, code);
}

impl BrowseService for Fw {
    fn browse(&mut self, dir: DirId, filter: BrowseFilter) -> Listing {
        let entry = self.entry_of(dir);
        if self.browse.listing.dir == dir && self.browse.listing.complete {
            return self.browse.listing;
        }
        *self.browse = media::BrowseWin::EMPTY;
        self.browse.listing = Listing { dir, count: 0, sorted: false, complete: false };
        media::request(media::Req::Browse { dir, entry, filter });
        self.browse.listing
    }
    fn listing(&self) -> Listing {
        self.browse.listing
    }
    fn entry(&self, i: u32) -> Option<Entry<'_>> {
        let r = self.browse.row(i)?;
        Some(Entry {
            name: r.name.as_str(),
            kind: r.kind,
            size: r.size,
            modified: r.modified,
            file: r.file,
            child: r.child,
        })
    }
    fn want(&mut self, first: u32, count: u32) {
        let w = &*self.browse;
        // Rows past the end of the listing are never coming: clamp, or a
        // short folder would be asked for again on every draw.
        let end = first.saturating_add(count).min(w.listing.count);
        if !w.listing.complete || first >= w.listing.count || (first >= w.first && end <= w.first + w.n) {
            return;
        }
        media::request(media::Req::Want { first });
    }
    fn position(&self, f: FileRef) -> Option<u32> {
        let w = &*self.browse;
        w.rows[..w.n as usize].iter().position(|r| r.file == f).map(|i| w.first + i as u32)
    }
    fn letter(&self, i: u32) -> Option<char> {
        letter_of(self.browse.row(i)?.name.as_str())
    }
    fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        // Within the window only until the directory index exists.
        let w = &*self.browse;
        let cur = self.letter(from)?;
        let mut i = from;
        loop {
            i = if forward { i.checked_add(1)? } else { i.checked_sub(1)? };
            let c = letter_of(w.row(i)?.name.as_str())?;
            if c != cur {
                return Some((i, c));
            }
        }
    }
    fn dir_name(&self, dir: DirId) -> &str {
        self.path[..self.depth].iter().find(|p| p.0 == dir).map_or("", |p| p.2.as_str())
    }
    fn file_op(&mut self, _op: FileOp) -> bool {
        // Writes only inside /.yoton/ (CLAUDE.md rule 5) until FS-3.
        false
    }
}

impl TextService for Fw {
    fn text_open(&mut self, f: FileRef) -> Option<u32> {
        let w = &*self.browse;
        let size = w.rows[..w.n as usize].iter().find(|r| r.file == f)?.size;
        self.text_size = u32::try_from(size).unwrap_or(u32::MAX);
        *self.text = media::TextWin::EMPTY;
        self.text.file = f;
        media::request(media::Req::Text { file: f, offset: 0 });
        Some(self.text_size)
    }
    fn text_window(&self) -> (u32, &[u8]) {
        if self.text.offset == u32::MAX {
            return (0, &[]);
        }
        (self.text.offset, &self.text.buf[..self.text.len as usize])
    }
    fn want_text(&mut self, offset: u32) {
        let w = &*self.text;
        let end = w.offset.saturating_add(w.len);
        let have = w.offset != u32::MAX
            && offset >= w.offset
            && (offset.saturating_add(600) <= end || end >= self.text_size);
        if !have {
            media::request(media::Req::Text { file: w.file, offset: offset.saturating_sub(512) / 512 * 512 });
        }
    }
    fn text_position(&self, f: FileRef) -> u32 {
        self.text_pos.iter().find(|p| p.0 == f).map_or(0, |p| p.1)
    }
    fn set_text_position(&mut self, f: FileRef, offset: u32) {
        let i = self.text_pos.iter().position(|p| p.0 == f).unwrap_or(self.text_pos.len() - 1);
        self.text_pos.copy_within(0..i, 1);
        self.text_pos[0] = (f, offset);
    }
}

impl PlayerService for Fw {
    fn player(&self) -> PlayerState {
        let p = &self.player;
        PlayerState {
            source: p.source,
            position_ms: p.position_ms,
            duration_ms: p.duration_ms,
            queue_pos: p.queue_pos,
            queue_len: p.queue_len,
            a_ms: None,
            b_ms: None,
            buffer_pct: 0,
            cpu_pct: 0,
        }
    }
    fn track(&self) -> Option<TrackInfo<'_>> {
        let p = &self.player;
        if p.source == Source::Idle {
            return None;
        }
        Some(TrackInfo {
            title: p.title.as_str(),
            artist: "",
            album: "",
            file: p.file,
            path: "",
            size: p.size,
            codec: p.codec,
            sample_rate: p.rate,
            bits: p.bits,
            channels: p.channels,
            bitrate_kbps: 0,
            gain_cdb: None,
        })
    }
    fn queue_entry(&self, _i: u32) -> Option<QueueEntry<'_>> {
        None
    }
    fn want_queue(&mut self, _first: u32, _count: u32) {}
    fn art(&self, _size: ArtSize) -> Option<&[u16]> {
        None
    }
    fn lyrics_count(&self) -> u32 {
        0
    }
    fn lyric(&self, _i: u32) -> Option<LyricLine<'_>> {
        None
    }
    fn levels(&self) -> Levels {
        Levels::default()
    }
    fn resume_title(&self) -> Option<&str> {
        None
    }
    fn bookmark_count(&self) -> u32 {
        0
    }
    fn bookmark(&self, _i: u32) -> Option<Bookmark<'_>> {
        None
    }
    fn library(&self) -> LibraryState {
        LibraryState::Missing
    }
    fn command(&mut self, c: PlayerCmd) {
        use crate::audio::PAUSED;
        match c {
            PlayerCmd::PlayPause => {
                if self.player.source != Source::Idle {
                    PAUSED.store(!PAUSED.load(Ordering::Relaxed), Ordering::Relaxed);
                }
            }
            PlayerCmd::Stop => {
                PAUSED.store(false, Ordering::Relaxed);
                media::request(media::Req::Stop);
            }
            PlayerCmd::Next => media::request(media::Req::Skip(1)),
            PlayerCmd::Prev => media::request(media::Req::Skip(-1)),
            PlayerCmd::Jump(i) => media::request(media::Req::Jump(i)),
            PlayerCmd::PlayDir { dir, start, shuffle: _ } => {
                let entry = self.entry_of(dir);
                PAUSED.store(false, Ordering::Relaxed);
                media::request(media::Req::PlayDir { entry, start });
            }
            // Seeking, queue edits, A-B, bookmarks, favourites, shuffle-all
            // and resume come with the yoton-player integration.
            _ => {}
        }
    }
}

impl UsbService for Fw {
    fn usb(&self) -> UsbState {
        if state::snapshot().cable { UsbState::Host } else { UsbState::Detached }
    }
    fn select_usb(&mut self, mode: UsbMode) {
        self.actions.push(Action::Usb(mode));
    }
}

impl BluetoothService for Fw {
    fn bt(&self) -> BtState {
        BtState::Off
    }
    fn bt_device(&self) -> &str {
        ""
    }
}

impl SystemService for Fw {
    fn lang(&self) -> Lang {
        Lang::ENGLISH
    }
    fn language_count(&self) -> usize {
        1
    }
    fn language(&self, _i: usize) -> Lang {
        Lang::ENGLISH
    }
    fn version(&self) -> &str {
        env!("CARGO_PKG_VERSION")
    }
    fn build_id(&self) -> &str {
        "dev"
    }
    fn serial(&self) -> &str {
        "YM03"
    }
    fn hardware(&self) -> &str {
        "Yoton YM03 (ATJ2157)"
    }
    fn app_available(&self, app: App) -> bool {
        matches!(app, App::Settings | App::Files | App::Music)
    }
    fn dual_boot(&self) -> bool {
        true
    }
    fn usb_audio_built(&self) -> bool {
        false
    }
    fn keys_locked(&self) -> bool {
        self.locked
    }
}

impl System for Fw {
    fn status(&self) -> Status {
        services::status(self)
    }
    fn volume_step(&mut self, delta: i8) -> (u8, u8) {
        PlaybackService::volume_step(self, delta)
    }
    fn mini_player(&self) -> Option<MiniPlayer<'_>> {
        services::mini_player(self)
    }
}
