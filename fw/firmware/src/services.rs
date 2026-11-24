//! The firmware's implementation of the `orpheus-apps` service traits, from
//! state the tasks publish (battery, cable) and the settings cache.

use core::sync::atomic::Ordering;

use orpheus_apps::browse::letter_of;
use orpheus_apps::services::{
    self, BatteryInfo, BluetoothService, CardInfo, CardState, ClockService, DateTime, PlaybackService,
    Playing, PowerRequest, PowerService, SettingsService, StorageRequest, StorageService, SystemService,
    UsbMode, UsbService, UsbState,
};
use orpheus_apps::services::{
    Bookmark, BrowseFilter, BrowseService, DirId, Entry, FileOp, FileRef, Levels, LibList, LibListing,
    LibNow, LibRow, LibraryService, LibraryState, Listing, LyricLine, PicReq, PicSource, PicState,
    PictureService, PlayerCmd, PlayerService, PlayerState, QueueEntry, TextService, TrackInfo,
};
use orpheus_apps::services::{BtDevice, BtList, BtPhase, BtRequest};
use orpheus_apps::settings::{SettingId, SettingsCache, id};
use orpheus_apps::{App, Lang};
use orpheus_ui::event::Source;
use orpheus_ui::event::{BtState, Event, ServiceEvent};
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::Gfx;
use orpheus_ui::shell::System;
use orpheus_ui::widgets::bars::{MiniPlayer, Status};

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
    pub qwin: &'static mut media::QueueWin,
    pub bms: &'static mut media::BmWin,
    pub player: media::PlayerSnap,
    /// Up next has asked for queue rows from this far after the current
    /// track (the window then follows the track); the last window
    /// requested (first, queue generation).
    qwant: Option<u32>,
    qreq: (u32, u32),
    /// Last bookmark generation copied, sleep-timer expiries seen.
    bm_seen: u32,
    sleep_seen: u16,
    /// Folders entered from the root: id, own entry, name.
    path: [(DirId, FileRef, media::Name); 8],
    depth: usize,
    /// Text-viewer positions (RAM only for now).
    text_pos: [(FileRef, u32); 8],
    text_size: u32,
    /// The filter of the listing asked for, and the rows the screen last
    /// wanted (asked for again when the listing changes under them: it
    /// arrived, or became sorted).
    filter: BrowseFilter,
    wanted: Option<(u32, u32)>,
    /// Bluetooth as last published by the Bluetooth task; a newly
    /// connected device for the UI to save.
    bt: crate::bt::Snap,
    pub bt_to_save: Option<orpheus_bt::Addr>,
    /// Connected to a sink with absolute volume: the volume is the
    /// sink's (sent as command 0x26), the DAC stays at [`BT_DAC_REF`]
    /// (06 §4.2).
    bt_abs: bool,
    /// The picture asked for in the frame being drawn (panel box), lent
    /// to the card task after the frame (`ui`); the state of the last one.
    pub pic_want: core::cell::Cell<Option<(Rect, PicReq)>>,
    pub pic_state: core::cell::Cell<PicState>,
    /// The job out (its number): the UI draws nothing until it is back.
    pub pic_lent: Option<u32>,
    /// When it was sent (ms), to take it back if it never starts.
    pub pic_since: u64,
    pic_seq: u32,
    /// A picture was not asked for because something covered the screen:
    /// redraw once it is gone.
    pub pic_after_overlay: bool,
    /// The music library's state and generation last seen.
    lib_seen: (LibraryState, u32),
}

/// The volume sent to a connected sink (module scale 0–31): full scale,
/// the player's volume is applied at the DAC (see `apply_dac`).
const BT_SINK_LOCAL: u8 = 31;
/// "Reconnect automatically" (Settings › Bluetooth).
const BT_RECONNECT: SettingId = id::BT_RECONNECT;
/// DAC level (UI steps 0–30) while a Bluetooth sink with absolute
/// volume is connected (06 §4.2: a fixed reference, the sink scales):
/// step 30 = −3 dB, a little headroom below `dac::VOL_0DB`. Measured on
/// the player (lead, 2026-11-24, PA volume 7): the module's feed tracks
/// the DAC's digital volume exactly (−30 dB → −53.6 dBFS, 0 dB → −23.5
/// dBFS), so the tap is after the volume. Not capped by Volume limit:
/// the limit caps the sink's volume; wired headphones left plugged in
/// while streaming play at this level (U-BT-2, user check).
const BT_DAC_REF: u8 = 30;
/// "Tag code page" (Settings › Playback).
const TAG_CODE_PAGE: SettingId = SettingId(0x0208);

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
static UI_QWIN: static_cell::ConstStaticCell<media::QueueWin> =
    static_cell::ConstStaticCell::new(media::QueueWin::EMPTY);
static UI_BMS: static_cell::ConstStaticCell<media::BmWin> =
    static_cell::ConstStaticCell::new(media::BmWin::EMPTY);

/// Sleep-timer choices (`settings::SLEEP`): minutes; 6 = end of track.
const SLEEP_MIN: [u32; 6] = [0, 15, 30, 45, 60, 90];

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
            qwin: UI_QWIN.take(),
            bms: UI_BMS.take(),
            player: media::PlayerSnap::IDLE,
            qwant: None,
            qreq: (u32::MAX, u32::MAX),
            bm_seen: 0,
            sleep_seen: 0,
            path: [(DirId::ROOT, FileRef { dir: 0, index: 0 }, media::Name::EMPTY); 8],
            depth: 0,
            text_pos: [(FileRef { dir: u32::MAX, index: 0 }, 0); 8],
            text_size: 0,
            filter: BrowseFilter {
                sort: services::SortBy::Name,
                hidden: false,
                audio_only: false,
                images_only: false,
            },
            wanted: None,
            bt: crate::bt::Snap::OFF,
            bt_to_save: None,
            bt_abs: false,
            pic_want: core::cell::Cell::new(None),
            pic_state: core::cell::Cell::new(PicState::Nothing),
            pic_lent: None,
            pic_since: 0,
            pic_seq: 0,
            pic_after_overlay: false,
            lib_seen: (LibraryState::Missing, 0),
        }
    }

    /// Lend the frame buffer `fb` for the picture asked for in the frame
    /// just drawn (if any). False if it could not be sent (the screen is
    /// told it was dropped).
    pub fn lend_picture(&mut self, fb: &mut [u16], now_ms: u64) {
        let Some((r, req)) = self.pic_want.take() else { return };
        let fit = |v: i16| u8::try_from(v).ok();
        let job = (|| {
            Some(media::PicJob {
                seq: 0,
                fb: fb.as_mut_ptr() as usize,
                x: fit(r.x)?,
                y: fit(r.y)?,
                w: fit(r.w)?,
                h: fit(r.h)?,
                req,
            })
        })();
        let Some(mut job) = job else {
            self.picture_dropped();
            return;
        };
        self.pic_seq = (self.pic_seq + 1) & !media::PIC_RUNNING;
        if self.pic_seq == 0 {
            self.pic_seq = 1;
        }
        job.seq = self.pic_seq;
        media::PIC_CANCEL.store(false, Ordering::Relaxed);
        media::PIC_SLOT.store(job.seq, Ordering::Release);
        if media::try_request(media::Req::Picture(job)) {
            self.pic_lent = Some(job.seq);
            self.pic_since = now_ms;
        } else {
            media::PIC_SLOT.store(0, Ordering::Release);
            self.picture_dropped();
        }
    }

    /// Take the job back if the card task has not started it (a key, or
    /// it waited too long); else ask it to stop early. True if the frame
    /// buffer is back.
    pub fn reclaim_picture(&mut self) -> bool {
        let Some(seq) = self.pic_lent else { return true };
        if media::PIC_SLOT.compare_exchange(seq, 0, Ordering::AcqRel, Ordering::Acquire).is_ok() {
            self.pic_lent = None;
            self.picture_dropped();
            return true;
        }
        media::PIC_CANCEL.store(true, Ordering::Relaxed);
        false
    }

    /// Every UI loop while a picture job is out: finish it if the card task
    /// is done (its data-ready tag may have been lost in a full channel);
    /// take it back if it has waited 3 s without starting (the card task is
    /// busy elsewhere, e.g. the card is with USB).
    pub fn poll_picture(&mut self, now_ms: u64) {
        let Some(seq) = self.pic_lent else { return };
        let slot = media::PIC_SLOT.load(Ordering::Acquire);
        if slot == 0 {
            self.sync(media::tag::PICTURE);
            self.pending.push(Event::Data(orpheus_ui::event::DataReady {
                tag: media::tag::PICTURE,
                first: 0,
                count: 0,
            }));
        } else if slot == seq && now_ms.saturating_sub(self.pic_since) > 3000 {
            self.reclaim_picture();
        }
    }

    /// The picture asked for will not come: the screen hears `Pending`
    /// with `Event::Data(PICTURE)` and asks again.
    fn picture_dropped(&mut self) {
        self.pic_state.set(PicState::Pending);
        self.pending.push(Event::Data(orpheus_ui::event::DataReady {
            tag: media::tag::PICTURE,
            first: 0,
            count: 0,
        }));
    }

    /// Copy the card task's window for `tag` (called before the UI gets
    /// `Event::Data`).
    pub fn sync(&mut self, tag: u16) {
        let again = critical_section::with(|cs| match tag {
            media::tag::BROWSE | media::tag::LIBRARY => {
                let w = media::BROWSE.borrow(cs).borrow();
                // Only the folder this side asked for.
                if w.listing.dir != self.browse.listing.dir {
                    return None;
                }
                let changed = w.listing != self.browse.listing || w.lgen != self.browse.lgen;
                *self.browse = *w;
                self.wanted.filter(|_| changed)
            }
            media::tag::TEXT => {
                *self.text = *media::TEXT.borrow(cs).borrow();
                None
            }
            media::tag::QUEUE => {
                *self.qwin = *media::QWINDOW.borrow(cs).borrow();
                None
            }
            media::tag::PICTURE => {
                let (seq, st) = media::PIC_RESULT.borrow(cs).get();
                if self.pic_lent == Some(seq) {
                    self.pic_lent = None;
                    self.pic_state.set(st);
                }
                None
            }
            _ => None,
        });
        // The listing arrived or was re-sorted: the rows the screen shows
        // may not be in the new window.
        if let Some((first, count)) = again {
            self.want(first, count);
        }
    }

    /// The letter table of the current listing, when it is sorted and the
    /// table fits ([`media::LETTERS`]).
    fn with_letters<R>(
        &self,
        f: impl FnOnce(&orpheus_index::LetterTable<{ media::LETTER_RUNS }>) -> R,
    ) -> Option<R> {
        let w = &*self.browse;
        if !w.listing.sorted {
            return None;
        }
        critical_section::with(|cs| {
            let l = media::LETTERS.borrow(cs).borrow();
            (l.0 == w.lgen && l.1.is_loaded()).then(|| f(&l.1))
        })
    }

    /// Copy the player snapshot (every UI loop), and the windows that
    /// follow it.
    pub fn sync_player(&mut self) {
        self.player = critical_section::with(|cs| *media::PLAYER.borrow(cs).borrow());
        let p = self.player;
        if p.bm_gen != self.bm_seen {
            self.bm_seen = p.bm_gen;
            *self.bms = critical_section::with(|cs| *media::BMS.borrow(cs).borrow());
        }
        if p.sleep_fired != self.sleep_seen {
            // The sleep timer ran out: the setting goes back to Off.
            self.sleep_seen = p.sleep_fired;
            if self.settings.set(id::SLEEP_TIMER, 0) {
                self.pending.push(Event::Service(ServiceEvent::SettingChanged(id::SLEEP_TIMER.0)));
            }
        }
        if let Some(off) = self.qwant {
            self.want_rows(p.queue_pos.saturating_add(off), 7);
        }
        let lib = (p.lib_state, p.lib_gen);
        if lib != self.lib_seen {
            if lib.1 != self.lib_seen.1 && crate::library::dir_list(self.browse.listing.dir).is_some() {
                // Another library: the list shown is asked for again.
                self.browse.listing.complete = false;
            }
            self.lib_seen = lib;
            self.pending
                .push(Event::Service(ServiceEvent::Other { kind: services::other::LIBRARY, value: 0 }));
        }
    }

    /// Ask for the Up-next window at `first` unless it is loaded (or was
    /// just asked for).
    fn want_rows(&mut self, first: u32, count: u32) {
        let p = &self.player;
        let last = first.saturating_add(count.max(1) - 1).min(p.queue_len.saturating_sub(1));
        let have = first >= p.queue_len
            || (self.qwin.title(p.qgen, first).is_some() && self.qwin.title(p.qgen, last).is_some());
        if !have && self.qreq != (first, p.qgen) {
            self.qreq = (first, p.qgen);
            media::request(media::Req::QueueWin { first });
        }
    }

    /// After the settings are loaded: the Bluetooth task gets them. The
    /// module stays off in a host test whatever the setting says.
    pub fn bt_boot(&mut self, last: Option<orpheus_bt::Addr>) {
        crate::bt::VOLUME.store(BT_SINK_LOCAL, Ordering::Relaxed);
        let on = self.settings.get(id::BLUETOOTH) != 0 && atj2157_hal::persist::test_request().is_none();
        let reconnect = self.settings.get(BT_RECONNECT) != 0;
        crate::bt::request(crate::bt::Req::Boot { on, reconnect, last });
    }

    /// Copy the Bluetooth task's snapshot (every UI loop); changes become
    /// service events, the sink's volume takes over the volume keys.
    pub fn sync_bt(&mut self) {
        use orpheus_apps::services::bt_event;
        let n = critical_section::with(|cs| *crate::bt::SNAP.borrow(cs).borrow());
        let o = self.bt;
        if n.state != o.state {
            self.pending.push(Event::Service(ServiceEvent::Bluetooth(n.state)));
        }
        if n.last_gen != o.last_gen {
            self.bt_to_save = n.last;
        }
        let other = |v| Event::Service(ServiceEvent::Other { kind: services::other::BLUETOOTH, value: v });
        for (changed, v) in [
            (n.lists_gen != o.lists_gen, bt_event::LISTS),
            (n.phase != o.phase, bt_event::PHASE),
            (n.fails != o.fails, bt_event::CONNECT_FAILED),
            (n.connects != o.connects, bt_event::CONNECTED),
            (n.drops != o.drops, bt_event::DISCONNECTED),
        ] {
            if changed {
                self.pending.push(other(v));
            }
        }
        // Sink volume reports do not move the player's volume: the module
        // echoes the value it was sent as a "sink volume" (measured
        // 2026-11-24), so following them would jump to full scale. A
        // headset's own buttons set its own level.
        self.bt_abs = false;
        self.bt = n;
    }

    /// The player's volume (0–30): the DAC (unless a sink with absolute
    /// volume owns it) and the Bluetooth task (which sends it to a
    /// connected sink, and again at every connection).
    pub fn set_volume(&mut self, v: u8) {
        self.volume = v;
        self.apply_dac();
    }

    /// Volume policy (06 §4.2): while connected to a sink that takes
    /// absolute volume, the sink scales (it gets the player's volume as
    /// command 0x26) and the DAC plays at the fixed reference level;
    /// otherwise (not connected, the sink said it has no absolute volume,
    /// or 0x26 was never acknowledged) the DAC plays at the player's
    /// volume.
    ///
    /// As built (2026-11-24): the DAC always plays at the player's volume
    /// and the sink is sent full scale ([`BT_SINK_LOCAL`]). Neither the
    /// module's acknowledgement of 0x26 nor its volume reports show
    /// whether the sink scales (a BlueZ sink without absolute volume is
    /// acknowledged, answers 0x25 with 128, and the module echoes the sent
    /// value back), so the fixed-reference scheme would leave the volume
    /// keys without effect on such sinks. Revisit with real headsets.
    fn apply_dac(&self) {
        let _ = (self.bt_abs, BT_DAC_REF);
        apply_volume(self.volume);
    }

    /// Bluetooth settings go to the Bluetooth task when they change.
    fn setting_to_bt(&self, sid: SettingId) {
        let on = self.settings.get(sid) != 0;
        match sid {
            id::BLUETOOTH => crate::bt::request(crate::bt::Req::Power(on)),
            BT_RECONNECT => crate::bt::request(crate::bt::Req::Reconnect(on)),
            _ => {}
        }
    }

    /// After the settings are loaded: what the player follows from them.
    pub fn init_player(&mut self) {
        self.setting_to_library(TAG_CODE_PAGE);
        // A sleep timer does not survive a restart.
        if self.settings.get(id::SLEEP_TIMER) != 0 {
            self.settings.set(id::SLEEP_TIMER, 0);
        }
        self.setting_to_player(id::REPEAT);
    }

    /// The tag code page goes to the library builder.
    fn setting_to_library(&self, sid: SettingId) {
        if sid == TAG_CODE_PAGE {
            let v = u8::try_from(self.settings.get(sid)).unwrap_or(0);
            crate::library::CODE_PAGE.store(v, Ordering::Relaxed);
        }
    }

    /// Player settings go to the player when they change.
    fn setting_to_player(&self, sid: SettingId) {
        use orpheus_player::{Command, Repeat};
        let v = self.settings.get(sid);
        let c = match sid {
            id::SHUFFLE => Command::Shuffle(v != 0),
            id::REPEAT => Command::Repeat(match v {
                1 => Repeat::One,
                2 => Repeat::All,
                _ => Repeat::Off,
            }),
            id::SLEEP_TIMER => match usize::try_from(v) {
                Ok(6) => Command::SleepAfterTrack,
                Ok(i) if i < SLEEP_MIN.len() => Command::SleepAfterMs(SLEEP_MIN[i] * 60_000),
                _ => Command::SleepAfterMs(0),
            },
            _ => return,
        };
        media::play(media::PlayReq::Cmd(c));
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
            self.setting_to_player(sid);
            self.setting_to_bt(sid);
            self.setting_to_library(sid);
        }
    }
    fn factory_reset(&mut self) {
        self.settings.reset();
        self.setting_to_bt(id::BLUETOOTH);
    }
}

impl PowerService for Fw {
    fn battery(&self) -> BatteryInfo {
        let s = state::snapshot();
        BatteryInfo { percent: s.percent, millivolts: s.millivolts, charge: s.charge }
    }
    fn sleep_timer_left(&self) -> Option<u16> {
        self.player.sleep_min
    }
    fn power(&mut self, req: PowerRequest) {
        if req == PowerRequest::LockKeys {
            self.locked = true;
        }
        self.actions.push(Action::Power(req));
    }
    // Developer rows and the battery logger (PWR-5): state in
    // `crate::drain::Shared` (SRAM3), not in `Fw`.
    fn developer(&self) -> bool {
        crate::drain::shared().developer.load(Ordering::Relaxed)
    }
    fn show_developer(&mut self) {
        crate::drain::shared().developer.store(true, Ordering::Relaxed);
    }
    fn drain_log(&self) -> orpheus_apps::services::DrainLog {
        crate::drain::info()
    }
    fn set_drain_log(&mut self, on: bool, stay: bool) {
        crate::drain::shared().stay_pref.store(stay, Ordering::Relaxed);
        let d = crate::drain::info();
        if on && !d.on {
            // Labelled by the Performance setting: one file per run.
            let max = self.settings.get(id::PERFORMANCE) == 1;
            crate::drain::start(if max { b"max" } else { b"normal" }, stay);
        } else if !on && d.on {
            crate::drain::stop();
        } else {
            crate::drain::set_stay(stay);
        }
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
        self.set_volume(u8::try_from(v.clamp(0, i16::from(max))).unwrap_or(0));
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
        if self.browse.listing.dir == dir && self.browse.listing.complete && self.filter == filter {
            return self.browse.listing;
        }
        if self.browse.listing.dir != dir {
            // Another folder: nothing focused in it yet.
            media::set_focus(None);
        }
        self.filter = filter;
        self.wanted = None;
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
        self.wanted = Some((first, count));
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
    fn focus_hint(&mut self, f: FileRef) {
        media::set_focus(Some(f));
    }
    fn letter(&self, i: u32) -> Option<char> {
        if let Some(l) = self.with_letters(|t| t.letter(i)) {
            return l;
        }
        letter_of(self.browse.row(i)?.name.as_str())
    }
    fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        // From the index's letter runs when the listing is sorted and they
        // fit in RAM; else within the loaded window.
        if let Some(j) = self.with_letters(|t| t.seek_letter(from, forward)) {
            return j;
        }
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
        // Writes only inside /.orpheus/ (CLAUDE.md rule 5) until FS-3.
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
            a_ms: p.a_ms,
            b_ms: p.b_ms,
            buffer_pct: 0,
            cpu_pct: 0,
            art: p.art,
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
            bitrate_kbps: p.kbps,
            gain_cdb: None,
        })
    }
    fn queue_entry(&self, i: u32) -> Option<QueueEntry<'_>> {
        let title = self.qwin.title(self.player.qgen, i)?;
        Some(QueueEntry { title, artist: "", duration_ms: 0 })
    }
    fn want_queue(&mut self, first: u32, count: u32) {
        self.qwant = Some(first.saturating_sub(self.player.queue_pos));
        self.want_rows(first, count);
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
        let p = &self.player;
        (p.source == Source::Idle && p.resume && !p.title.is_empty()).then(|| p.title.as_str())
    }
    fn bookmark_count(&self) -> u32 {
        self.bms.n
    }
    fn bookmark(&self, i: u32) -> Option<Bookmark<'_>> {
        let r = self.bms.rows[..self.bms.n as usize].get(i as usize)?;
        Some(Bookmark { title: r.title.as_str(), position_ms: r.ms })
    }
    fn library(&self) -> LibraryState {
        self.player.lib_state
    }
    fn command(&mut self, c: PlayerCmd) {
        use media::PlayReq;
        use orpheus_fs::Locator;
        use orpheus_player::Command;
        use orpheus_player::queue::Edit;
        let r = match c {
            PlayerCmd::PlayPause => PlayReq::Cmd(Command::Toggle),
            PlayerCmd::Stop => PlayReq::Stop,
            PlayerCmd::Next => PlayReq::Cmd(Command::Next),
            PlayerCmd::Prev => PlayReq::Cmd(Command::Prev),
            PlayerCmd::SeekTo(ms) => PlayReq::Cmd(Command::SeekMs(ms)),
            PlayerCmd::SeekBy(ms) => PlayReq::Cmd(Command::SeekByMs(ms)),
            PlayerCmd::Jump(pos) => PlayReq::JumpPos(pos),
            PlayerCmd::PlayDir { dir, start, shuffle } => {
                let entry = self.entry_of(dir);
                let shuffle = shuffle || self.settings.get(id::SHUFFLE) != 0;
                PlayReq::PlayDir { entry, start, shuffle }
            }
            PlayerCmd::SetA => PlayReq::SetA,
            PlayerCmd::SetB => PlayReq::SetB,
            PlayerCmd::ClearAb => PlayReq::Cmd(Command::AbClear),
            PlayerCmd::Bookmark => PlayReq::Cmd(Command::AddBookmark),
            PlayerCmd::PlayBookmark(i) => {
                PlayReq::Cmd(Command::GotoBookmark(u8::try_from(i).unwrap_or(u8::MAX)))
            }
            PlayerCmd::Resume => PlayReq::Resume,
            PlayerCmd::ShuffleAll => PlayReq::ShuffleAll,
            PlayerCmd::PlayLib { list, start, shuffle } => {
                let shuffle = shuffle || self.settings.get(id::SHUFFLE) != 0;
                PlayReq::PlayLib { list, start, shuffle }
            }
            // Up next and a file's menu edit the queue (`Server::edit`).
            PlayerCmd::Remove(p) => PlayReq::Edit(Edit::Remove(p)),
            PlayerCmd::MoveUp(p) => PlayReq::Edit(Edit::MoveUp(p)),
            PlayerCmd::MoveDown(p) => PlayReq::Edit(Edit::MoveDown(p)),
            PlayerCmd::PlayNext(f) => PlayReq::Edit(Edit::PlayNext(Locator { dir: f.dir, index: f.index })),
            PlayerCmd::AddToQueue(f) => PlayReq::Edit(Edit::Add(Locator { dir: f.dir, index: f.index })),
            // Favourites need playlists (later package).
            PlayerCmd::Favourite => return,
        };
        media::play(r);
    }
}

/// Library lists share the browse window (`crate::library`).
impl LibraryService for Fw {
    fn lib_open(&mut self, list: LibList) -> LibListing {
        let dir = crate::library::list_dir(list);
        let w = &*self.browse;
        if w.listing.dir == dir && w.listing.complete {
            return LibListing { list, count: w.listing.count, complete: true };
        }
        if w.listing.dir != dir {
            media::set_focus(None);
        }
        self.wanted = None;
        *self.browse = media::BrowseWin::EMPTY;
        self.browse.listing = Listing { dir, count: 0, sorted: true, complete: false };
        media::request(media::Req::LibList { list });
        LibListing { list, count: 0, complete: false }
    }
    fn lib_listing(&self) -> LibListing {
        let l = self.browse.listing;
        match crate::library::dir_list(l.dir) {
            Some(list) => LibListing { list, count: l.count, complete: l.complete },
            None => LibListing { list: LibList::Artists, count: 0, complete: false },
        }
    }
    fn lib_row(&self, i: u32) -> Option<LibRow<'_>> {
        crate::library::dir_list(self.browse.listing.dir)?;
        let r = self.browse.row(i)?;
        Some(LibRow {
            kind: crate::library::kind_of_code(r.file.dir),
            id: r.file.index,
            name: r.name.as_str(),
            duration_ms: u32::try_from(r.size).unwrap_or(0),
        })
    }
    fn lib_want(&mut self, first: u32, count: u32) {
        BrowseService::want(self, first, count);
    }
    fn lib_position(&self, id: u32) -> Option<u32> {
        crate::library::dir_list(self.browse.listing.dir)?;
        let w = &*self.browse;
        w.rows[..w.n as usize].iter().position(|r| r.file.index == id).map(|i| w.first + i as u32)
    }
    fn lib_letter(&self, i: u32) -> Option<char> {
        BrowseService::letter(self, i)
    }
    fn lib_seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        BrowseService::seek_letter(self, from, forward)
    }
    fn lib_now(&self) -> Option<LibNow> {
        let p = &self.player;
        if p.source == Source::Idle || p.lib_state != LibraryState::Ready {
            return None;
        }
        p.lib_now
    }
}

impl PictureService for Fw {
    fn show_picture(&self, g: &mut Gfx<'_>, r: Rect, req: PicReq) -> PicState {
        let s = g.to_screen(r);
        let card = state::CARD_READY.load(Ordering::Relaxed);
        let st = match req.src {
            _ if s.w <= 0 || s.h <= 0 => PicState::Nothing,
            PicSource::File(_) if !card => PicState::Failed(services::PicError::Io),
            // The decoder memory is the audio decoder's while a track is
            // loaded.
            PicSource::File(_) if self.player.source != Source::Idle => PicState::Busy,
            PicSource::Art(_) if self.player.art == 0 || !card => PicState::Nothing,
            _ => {
                self.pic_want.set(Some((s, req)));
                PicState::Pending
            }
        };
        self.pic_state.set(st);
        st
    }
    fn picture_state(&self) -> PicState {
        self.pic_state.get()
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
        self.bt.state
    }
    fn bt_device(&self) -> &str {
        self.bt.name()
    }
    fn bt_phase(&self) -> BtPhase {
        self.bt.phase
    }
    fn bt_count(&self, list: BtList) -> usize {
        critical_section::with(|cs| crate::bt::LISTS.borrow(cs).borrow().count(list))
    }
    fn bt_entry(&self, list: BtList, i: usize) -> Option<BtDevice> {
        critical_section::with(|cs| crate::bt::LISTS.borrow(cs).borrow().get(list, i))
    }
    fn bt_request(&mut self, r: BtRequest) {
        use crate::bt::Req;
        let addr = |a: orpheus_apps::services::BtAddr| orpheus_bt::Addr(a.0);
        crate::bt::request(match r {
            BtRequest::Scan => Req::Scan,
            BtRequest::StopScan => Req::StopScan,
            BtRequest::Connect(a) => Req::ConnectTo(addr(a)),
            BtRequest::CancelConnect => Req::CancelConnect,
            BtRequest::Disconnect => Req::Disconnect,
            BtRequest::Forget(a) => Req::Forget(addr(a)),
        });
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
        // The transmitter module is fitted on every YM03 (06 §4.1); a
        // module that does not answer shows "Not responding" in the app.
        matches!(app, App::Settings | App::Files | App::Music | App::Pictures | App::Bluetooth)
    }
    fn dual_boot(&self) -> bool {
        // "Start original firmware" only exists in the dual-boot layout; in
        // the full layout (decision D-2) the stock firmware is erased.
        !atj2157_hal::nor::full_layout()
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
