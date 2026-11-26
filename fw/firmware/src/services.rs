//! The firmware's implementation of the `orpheus-apps` service traits, from
//! state the tasks publish (battery, cable) and the settings cache.

use core::sync::atomic::Ordering;

use orpheus_apps::browse::letter_of;
use orpheus_apps::services::{
    self, BatteryInfo, BluetoothService, CardInfo, CardState, ClockService, DateTime, FsKind,
    PlaybackService, Playing, PowerRequest, PowerService, SettingsService, StorageRequest, StorageService,
    SystemService, UsbMode, UsbService, UsbState,
};
use orpheus_apps::services::{
    AuthEntry, AuthOutcome, AuthRequest, AuthService, Code, VaultState, auth_event, other,
};
use orpheus_apps::services::{
    Bookmark, BrowseFilter, BrowseService, DirId, Entry, FileOp, FileRef, Levels, LibList, LibListing,
    LibNow, LibRow, LibraryService, LibraryState, Listing, LyricLine, PicReq, PicSource, PicState,
    PictureService, PlayerCmd, PlayerService, PlayerState, QueueEntry, TextService, TrackInfo,
};
use orpheus_apps::services::{BtDevice, BtList, BtPhase, BtRequest, StepsInfo, StepsService, ToolsService};
use orpheus_apps::services::{DriveInfo, DriveRequest, UsbDriveService, drive_event};
use orpheus_apps::services::{PresetOp, Presets, RadioInfo, RadioPhase, RadioRequest, RadioService, Region};
use orpheus_apps::services::{RecInfo, RecRequest, RecorderService, rec_settings};
use orpheus_apps::services::{UpdateFile, UpdateInfo, UpdateRequest, UpdateService};
use orpheus_apps::settings::{SettingId, SettingsCache, id};
use orpheus_apps::tools::Tools;
use orpheus_apps::{App, Lang};
use orpheus_ui::event::Source;
use orpheus_ui::event::{BtState, Event, ServiceEvent};
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::Gfx;
use orpheus_ui::shell::System;
use orpheus_ui::text::StrBuf;
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
    /// The radio (`crate::fm`) as last seen; its RDS name and text as
    /// UTF-8; the mini-player's title.
    radio: RadioInfo,
    /// The radio's change counters last seen (state, RDS, auto-scans).
    radio_seen: (u16, u16, u16),
    /// The RDS clock time the clock was last set from (minutes since 1970
    /// UTC; 0 none): Settings › Date & time › Set from RDS (VER-13).
    rds_set: u32,
    /// RDS name and text as UTF-8, the mini-player's title (SRAM3,
    /// `fm::RadioText`).
    radio_text: &'static mut crate::fm::RadioText,
    /// Since when (ms) the radio plays a frequency other than the one
    /// saved (saved after [`RADIO_FREQ_SAVE_MS`]).
    radio_freq_since: u64,
    /// The presets (a static, in their stored form); changed since saved.
    pub presets: &'static mut Presets,
    pub presets_dirty: bool,
    /// Stopwatch, timer, alarms (UI-5d).
    pub tools: Tools,
    /// An alert paused the music (resumed when it ends).
    pub alert_paused: bool,
    /// The RTC alarm as armed (hour, minute); the alarms changed since.
    pub rtc_alarm: Option<(u8, u8)>,
    pub alarms_changed: bool,
    /// The minute the RTC alarm was last looked at (once a minute is
    /// enough: the next alarm moves only then).
    last_alarm_minute: u8,
    /// The Authenticator (OTP-2, `crate::vault`): the service core in its
    /// static, the last outcome, an operation waiting for the scratch, the
    /// import file's phase and job number, the typing job awaited
    /// (`kbd::FINISHED` when it was queued).
    pub(crate) auth: &'static mut orpheus_otp::service::Authenticator<crate::vault::NorVault>,
    pub(crate) auth_outcome: orpheus_apps::services::AuthOutcome,
    pub(crate) auth_op: Option<crate::vault::AuthOp>,
    /// A key derivation asked for (run in `Ui::after` after one frame of
    /// "Unlocking…"); the flag: that frame was drawn.
    pub(crate) auth_pending: Option<(orpheus_apps::services::VaultWork, orpheus_apps::services::Pin, bool)>,
    pub(crate) otp_phase: crate::vault::OtpPhase,
    pub(crate) otp_seq: u32,
    pub(crate) kbd_seen: Option<u32>,
    /// The recorder as last seen (`crate::rec`, AU-13) and its file name.
    rec: RecInfo,
    rec_seq: u32,
    /// USB Drive: the export's change counter last seen, its file name.
    drive_gen: u32,
    drive_name: StrBuf<{ crate::drive::NAME_LEN }>,
    rec_name: StrBuf<40>,
    /// The Games app (`crate::games`): the hub and its card queue, in
    /// their static.
    pub games: &'static mut crate::games::GamesRam,
    /// Seeds handed out (two new games in the same microsecond differ).
    game_seeds: u32,
}

/// A frequency is saved as the radio's last one after playing this long
/// (not at every step: each save is a NOR write).
const RADIO_FREQ_SAVE_MS: u64 = 5000;

/// The radio presets (332 B, `.bss`: no initial image in the resident
/// copy; SRAM3 has no room left beside the logger, the step counter and
/// the radio).
static mut UI_PRESETS: core::mem::MaybeUninit<Presets> = core::mem::MaybeUninit::uninit();

/// Once, from `Fw::new`.
fn ui_presets() -> &'static mut Presets {
    // SAFETY: called once (`Fw::new` runs once, in the UI task), so this is
    // the only reference to the static.
    unsafe { (*(&raw mut UI_PRESETS)).write(Presets::new()) }
}

/// Alarms can switch the player on from off: PWR-3c (U-PMU-3) has not
/// shown that the RTC alarm wakes this board, nor how a start knows it was
/// one; until then alarms ring only while the player is on, and power-off
/// does not arm the alarm wake (`pmu::power_off(false)`).
pub const ALARM_WAKE_FROM_OFF: bool = false;

/// The volume sent to a connected sink (module scale 0–31): full scale,
/// the player's volume is applied at the DAC (see `apply_dac`).
const BT_SINK_LOCAL: u8 = orpheus_bt::proto::SINK_VOLUME_SENT;
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
        let presets = ui_presets();
        let radio_text = crate::fm::ui_text();
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
            radio: RadioInfo {
                phase: RadioPhase::Off,
                khz: 87_500,
                rssi: 0,
                stereo: false,
                muted: false,
                antenna: false,
                progress: 0,
                found: 0,
            },
            radio_seen: (0, 0, 0),
            rds_set: 0,
            radio_text,
            radio_freq_since: 0,
            presets,
            presets_dirty: false,
            tools: Tools::default(),
            alert_paused: false,
            rtc_alarm: None,
            alarms_changed: true,
            last_alarm_minute: 0xFF,
            auth: crate::vault::init(),
            auth_outcome: orpheus_apps::services::AuthOutcome::None,
            auth_op: None,
            auth_pending: None,
            otp_phase: crate::vault::OtpPhase::Idle,
            otp_seq: 0,
            kbd_seen: None,
            games: crate::games::ram(),
            game_seeds: 0,
            rec: RecInfo::default(),
            rec_seq: 0,
            rec_name: StrBuf::new(),
            drive_gen: 0,
            drive_name: StrBuf::new(),
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
    #[inline(never)]
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
        if tag == media::tag::OTP {
            self.otp_sync();
        }
        if tag == crate::games::TAG {
            self.games_poll(now_ms());
        }
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
    #[inline(never)]
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
    #[inline(never)]
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
        let _ = BT_DAC_REF;
        apply_volume(orpheus_bt::proto::dac_level(self.volume, self.bt_abs));
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

    /// The radio plays (or is starting, seeking, scanning).
    #[must_use]
    pub fn radio_on(&self) -> bool {
        self.radio.phase.is_on()
    }

    /// After the settings are loaded: the radio's settings and volume.
    pub fn radio_boot(&mut self) {
        crate::fm::request(orpheus_fm::Req::Config(self.radio_config()));
        crate::fm::request(orpheus_fm::Req::Volume(self.volume));
    }

    fn radio_config(&self) -> orpheus_fm::radio::Config {
        orpheus_fm::radio::Config {
            region: Region::from_index(self.settings.get(id::RADIO_REGION)),
            mono: self.settings.get(id::FORCE_MONO) != 0,
            sensitivity: u8::try_from(self.settings.get(id::SEEK_SENSITIVITY)).unwrap_or(1),
            ..orpheus_fm::radio::Config::DEFAULT
        }
    }

    /// Settings › Radio goes to the radio when it changes.
    fn setting_to_radio(&self, sid: SettingId) {
        if [id::RADIO_REGION, id::FORCE_MONO, id::SEEK_SENSITIVITY].contains(&sid) {
            crate::fm::request(orpheus_fm::Req::Config(self.radio_config()));
        }
    }

    /// Copy the radio's state (every UI loop, and after a request): changes
    /// become service events; new RDS text is converted to UTF-8; a
    /// finished auto-scan replaces the presets; a frequency played for a
    /// while becomes the one the radio starts at.
    /// The recorder's state for the UI (`crate::rec::view`).
    fn rec_info(&self) -> RecInfo {
        let v = crate::rec::view();
        let s = rec_settings(self);
        RecInfo {
            phase: v.phase,
            source: s.source,
            format: s.params.format,
            rate: s.params.rate,
            channels: s.params.channels,
            elapsed_ms: v.elapsed_ms,
            bar: [v.meters[0], v.meters[1]],
            hold: [v.meters[2], v.meters[3]],
            clip: v.clip,
            gain_db: if s.chain.agc { i8::try_from(v.agc_db10 / 10).unwrap_or(0) } else { s.gain_db },
            auto_gain: s.chain.agc,
            left_s: v.left_s,
            drops: v.dropouts,
            marks: u16::try_from(v.marks).unwrap_or(u16::MAX),
            fm_khz: if s.source == orpheus_apps::services::RecSource::Fm {
                v.fm_khz.max(self.radio.khz)
            } else {
                0
            },
            ended: v.ended,
        }
    }

    /// Copy the recorder's state (every UI loop): changes become service
    /// events; a recording that ended says how.
    #[inline(never)]
    pub fn sync_rec(&mut self) {
        use orpheus_apps::services::rec_event;
        let seq = crate::rec::view().seq;
        let info = self.rec_info();
        if seq == self.rec_seq && info == self.rec {
            return;
        }
        let other = |v| Event::Service(ServiceEvent::Other { kind: services::other::RECORDER, value: v });
        if info.ended.is_some()
            && (self.rec.ended != info.ended || info.phase != self.rec.phase)
            && !info.phase.is_recording()
            && self.rec.phase.is_recording()
        {
            self.pending.push(other(rec_event::ENDED));
        }
        if info.phase.is_recording() != self.rec.phase.is_recording() {
            let src = if info.phase.is_recording() { Source::Recording } else { Source::Idle };
            self.pending.push(Event::Service(ServiceEvent::Playback(src)));
        }
        crate::rec::file_name(&mut self.rec_name);
        self.pending.push(other(rec_event::STATE));
        self.rec = info;
        self.rec_seq = seq;
    }

    /// USB Drive: an event when the export's phase or host changed.
    #[inline(never)]
    pub fn sync_drive(&mut self) {
        let g = crate::drive::GEN.load(Ordering::Relaxed);
        if g != self.drive_gen {
            self.drive_gen = g;
            crate::drive::name(&mut self.drive_name);
            self.pending.push(Event::Service(ServiceEvent::Other {
                kind: services::other::USB_DRIVE,
                value: drive_event::STATE,
            }));
        }
    }

    /// Settings › Date & time › Set from RDS (VER-13): a station's clock
    /// time sets the RTC (in the Time zone), once per new minute.
    #[inline(never)]
    fn rds_clock(&mut self, utc: u64) {
        let min = (utc / 60) as u32;
        if min == self.rds_set {
            return;
        }
        // Not in a test session: a test never moves the owner's clock.
        if let Some(t) = services::rds_clock(&*self, utc).filter(|_| !crate::ui::session_on()) {
            self.rds_set = min;
            self.set_now(t);
            atj2157_hal::log::line(b"clock: set from RDS");
        }
    }

    pub fn sync_radio(&mut self, now_ms: u64) {
        use orpheus_apps::services::radio_event;
        use orpheus_fm::Phase;
        let (seen_seq, seen_rds, seen_scan) = self.radio_seen;
        let mut ps = [0u8; 8];
        let mut ps_len = 0;
        let mut ct = None;
        let text = &mut *self.radio_text;
        let i = crate::fm::with(|r| {
            let i = r.info();
            if i.rds_seq != seen_rds {
                ct = r.rds().clock();
                let mut b = [0u8; 96];
                let n = orpheus_fm::rds::to_utf8(r.rds().rt(), &mut b);
                text.rt = StrBuf::from(core::str::from_utf8(b.get(..n).unwrap_or(&[])).unwrap_or(""));
                let n = orpheus_fm::rds::to_utf8(r.rds().ps(), &mut b);
                text.ps = StrBuf::from(core::str::from_utf8(b.get(..n).unwrap_or(&[])).unwrap_or(""));
                for (o, &c) in ps.iter_mut().zip(r.rds().ps()) {
                    *o = c;
                    ps_len += 1;
                }
            }
            if i.scan_seq != seen_scan && !r.scan_results().is_empty() {
                self.presets.replace(r.scan_results().iter().map(orpheus_fm::radio::Station::khz));
                self.presets_dirty = true;
            }
            i
        });
        let phase = match i.phase {
            Phase::Off | Phase::Stopping => RadioPhase::Off,
            Phase::Starting => RadioPhase::Starting,
            Phase::Playing => RadioPhase::Playing,
            Phase::Seeking => RadioPhase::Seeking,
            Phase::Scanning => RadioPhase::Scanning,
            Phase::Failed => RadioPhase::NotResponding,
        };
        let info = RadioInfo {
            phase,
            khz: i.khz,
            rssi: i.rssi,
            stereo: i.stereo,
            muted: i.muted,
            antenna: state::EARPHONES.load(Ordering::Relaxed),
            progress: i.progress,
            found: i.found,
        };
        let other = |v| Event::Service(ServiceEvent::Other { kind: services::other::RADIO, value: v });
        if i.rds_seq != seen_rds {
            // A preset without a name takes the station's.
            if ps_len > 0
                && let Some(p) = self.presets.find(i.khz)
                && self.presets.get(p).is_some_and(|x| x.name().is_empty())
                && self.presets.set_name(p, ps.get(..ps_len).unwrap_or(&[]))
            {
                self.presets_dirty = true;
            }
            self.pending.push(other(radio_event::RDS));
        }
        if let Some(utc) = ct {
            self.rds_clock(utc);
        }
        if i.scan_seq != seen_scan {
            self.pending.push(other(radio_event::SCANNED));
        }
        if i.seq != seen_seq || i.rds_seq != seen_rds || info != self.radio {
            let t = &mut self.radio_text;
            t.title.clear();
            if phase.is_on() {
                t.title.push_str("FM ");
                services::push_mhz(i.khz, &mut t.title);
                if !t.ps.is_empty() {
                    t.title.push_str(" · ");
                    let ps = t.ps;
                    t.title.push_str(ps.as_str());
                }
            }
            self.pending.push(other(radio_event::STATE));
        }
        if phase.is_on() != self.radio.phase.is_on() {
            let src = if phase.is_on() { Source::Radio } else { Source::Idle };
            self.pending.push(Event::Service(ServiceEvent::Playback(src)));
        }
        // The last frequency, once it has played a while.
        let k = i16::try_from(i.khz / 10).unwrap_or(8750);
        if phase == RadioPhase::Playing && self.settings.get(id::RADIO_FREQ) != k {
            if self.radio_freq_since == 0 {
                self.radio_freq_since = now_ms.max(1);
            } else if now_ms.saturating_sub(self.radio_freq_since) >= RADIO_FREQ_SAVE_MS {
                self.settings.set(id::RADIO_FREQ, k);
                self.radio_freq_since = 0;
            }
        } else {
            self.radio_freq_since = 0;
        }
        self.radio = info;
        self.radio_seen = (i.seq, i.rds_seq, i.scan_seq);
    }

    /// After the settings are loaded: what the player follows from them.
    pub fn init_player(&mut self) {
        self.setting_to_library(TAG_CODE_PAGE);
        // A sleep timer does not survive a restart.
        if self.settings.get(id::SLEEP_TIMER) != 0 {
            self.settings.set(id::SLEEP_TIMER, 0);
        }
        self.setting_to_player(id::REPEAT);
        // Gapless and Auto-bookmark long files are on in a new player: only
        // an "off" needs saying (the request queue holds four).
        for sid in [id::GAPLESS, id::AUTO_BOOKMARK] {
            if self.settings.get(sid) == 0 {
                self.setting_to_player(sid);
            }
        }
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
            id::GAPLESS => Command::Gapless(v != 0),
            id::AUTO_BOOKMARK => Command::AutoBookmark(v != 0),
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
        let Some(row) = w.rows[..w.n as usize].iter().find(|r| r.child == dir) else {
            // Recorder › Recordings… opens its folder without browsing to it.
            if dir.0 == crate::rec::rec_dir()
                && let Some((d, index)) = crate::rec::rec_entry()
            {
                self.depth = 0;
                return Some(FileRef { dir: d, index });
            }
            return None;
        };
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
        // VER-7: `ui test-look on` (RAM only, the store keeps its values).
        if let Some(v) = crate::ui::test_look(sid) {
            return v;
        }
        self.settings.get(sid)
    }
    fn set_setting(&mut self, sid: SettingId, value: i16) {
        if self.settings.set(sid, value) {
            if (id::ALARM0.0..id::ALARM0.0 + 3 * orpheus_apps::tools::ALARMS as u16).contains(&sid.0) {
                self.alarms_changed = true;
            }
            self.pending.push(Event::Service(ServiceEvent::SettingChanged(sid.0)));
            self.setting_to_player(sid);
            self.setting_to_bt(sid);
            self.setting_to_library(sid);
            self.setting_to_radio(sid);
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
        if crate::usbdev::CARD_WITH_USB.load(Ordering::Relaxed) {
            CardState::UsbOwned
        } else if state::CARD_READY.load(Ordering::Relaxed) {
            CardState::Ready
        } else {
            CardState::Absent
        }
    }
    fn card_info(&self) -> Option<CardInfo> {
        // From the card task (`audio::publish_card`, `count_free`); the
        // free space reads 0 for the moment it is still being counted.
        let mib = state::CARD_MIB.load(Ordering::Relaxed);
        if self.card() != CardState::Ready || mib == 0 {
            return None;
        }
        let free = state::CARD_FREE_MIB.load(Ordering::Relaxed);
        Some(CardInfo {
            capacity_mib: mib,
            free_mib: if free == state::FREE_UNKNOWN { 0 } else { free },
            fs: match state::CARD_FS.load(Ordering::Relaxed) {
                state::CARD_FS_FAT32 => FsKind::Fat32,
                state::CARD_FS_EXFAT => FsKind::ExFat,
                _ => FsKind::Other,
            },
        })
    }
    fn storage(&mut self, req: StorageRequest) {
        self.actions.push(Action::Storage(req));
    }
}

impl PlaybackService for Fw {
    fn playing(&self) -> Option<Playing<'_>> {
        // Recording (AU-13): the status bar's red dot; no playback.
        if self.rec.phase.is_recording() {
            return Some(Playing {
                source: Source::Recording,
                title: self.rec_name.as_str(),
                artist: "",
                progress: 0,
            });
        }
        // The radio plays only while the music is stopped (05 §7.3).
        if self.radio.phase.is_on() {
            return Some(Playing {
                source: Source::Radio,
                title: self.radio_text.title.as_str(),
                artist: "",
                progress: 0,
            });
        }
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
        // The radio's volume: tuner volume and amplifier gain pairs.
        crate::fm::request(orpheus_fm::Req::Volume(self.volume));
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
    fn file_op(&mut self, op: FileOp) -> bool {
        // Writes only inside /.orpheus/ (CLAUDE.md rule 5) until FS-3;
        // the one exception is deleting a recording in /Recordings (UI-5b,
        // checked again by the card task).
        match op {
            FileOp::Delete(f) => {
                let dir = crate::rec::rec_dir();
                let ok = dir != 0 && f.dir == dir && media::try_request(media::Req::Delete(f));
                if ok {
                    // The screen browses again at once: the card task serves
                    // that after the delete (one queue), so the cached listing
                    // must not answer it (2026-11-25: the deleted recording
                    // stayed in the list).
                    self.browse.listing.complete = false;
                }
                ok
            }
        }
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

impl UsbDriveService for Fw {
    fn drive_dirs(&mut self) -> (Option<DirId>, Option<DirId>) {
        let (u, t) = crate::drive::dirs();
        ((u != 0).then_some(DirId(u)), (t != 0).then_some(DirId(t)))
    }
    fn drive_test_mode(&self) -> bool {
        // Test mode for the app: the developer rows are shown (About × 5).
        PowerService::developer(self)
    }
    fn drive_start(&mut self, req: DriveRequest, name: &str) {
        let w = crate::drive::Want {
            src: crate::drive::Source::File(req.file),
            req: orpheus_usb::msc::image::Request { medium: req.medium, write: req.write },
            test_mode: self.drive_test_mode(),
        };
        if crate::drive::want(w, name) {
            self.actions.push(Action::Usb(UsbMode::DiskImage));
        }
    }
    fn drive_stop(&mut self) {
        if crate::drive::info().phase.is_active() {
            crate::usbdev::MODE.signal(crate::usbdev::Personality::Dev);
        } else {
            crate::drive::clear_failed();
        }
    }
    fn drive(&self) -> DriveInfo {
        crate::drive::info()
    }
    fn drive_name(&self) -> &str {
        self.drive_name.as_str()
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

/// Settings › System › Update firmware (UPD-1): `crate::update`.
impl UpdateService for Fw {
    fn update(&self) -> UpdateInfo {
        crate::update::info()
    }
    fn update_file(&self, i: usize) -> Option<UpdateFile<'_>> {
        crate::update::file(i)
    }
    fn update_changes(&self) -> &str {
        crate::update::changes()
    }
    fn update_request(&mut self, r: UpdateRequest) {
        crate::update::request(r);
    }
}

impl RecorderService for Fw {
    fn recorder(&self) -> RecInfo {
        // Only the Recorder app asks: the meters run while it does.
        crate::rec::keepalive();
        self.rec_info()
    }
    fn rec_request(&mut self, r: RecRequest) {
        use crate::rec::Cmd;
        let s = rec_settings(self);
        let c = match r {
            RecRequest::Arm => Cmd::Arm(s),
            RecRequest::Disarm => Cmd::Disarm,
            RecRequest::Start => {
                // No playback while recording (05 §7.5).
                if matches!(self.player.source, Source::Playing | Source::Paused) {
                    PlayerService::command(self, PlayerCmd::Stop);
                }
                let fm = if self.radio.phase == RadioPhase::Playing { self.radio.khz } else { 0 };
                Cmd::Start { s, fm_khz: fm, dev: None }
            }
            RecRequest::Pause => Cmd::Pause,
            RecRequest::Resume => Cmd::Resume,
            RecRequest::Stop => Cmd::Stop,
            RecRequest::Mark => Cmd::Mark,
            RecRequest::Gain(_) => Cmd::Gain(s.gain_db),
        };
        crate::rec::request(c);
    }
    fn rec_name(&self) -> &str {
        self.rec_name.as_str()
    }
    fn rec_dir(&mut self) -> Option<DirId> {
        let d = crate::rec::rec_dir();
        (d != 0).then_some(DirId(d))
    }
    fn rec_has_line_in(&self) -> bool {
        // No line input on the YM03 that anyone has found (U-AUD-4); the
        // developer command can still try `rec start line`.
        false
    }
}

impl RadioService for Fw {
    fn radio(&self) -> RadioInfo {
        self.radio
    }
    fn radio_ps(&self) -> &str {
        self.radio_text.ps.as_str()
    }
    fn radio_rt(&self) -> &str {
        self.radio_text.rt.as_str()
    }
    fn radio_request(&mut self, r: RadioRequest) {
        use orpheus_fm::Req;
        let starts = matches!(r, RadioRequest::Start | RadioRequest::Tune(_));
        // Starting the radio stops the music (the fm task waits for the
        // DAC to stop before it switches the bypass on).
        if starts && matches!(self.player.source, Source::Playing | Source::Paused) {
            PlayerService::command(self, PlayerCmd::Stop);
        }
        let req = match r {
            // The radio starts where it was last.
            RadioRequest::Start => {
                Req::Tune(u32::try_from(self.settings.get(id::RADIO_FREQ)).unwrap_or(8750) * 10)
            }
            RadioRequest::Stop => Req::Off,
            RadioRequest::Tune(k) => Req::Tune(k),
            RadioRequest::Step { up } => Req::Step(up),
            RadioRequest::Seek { up } => Req::Seek(up),
            RadioRequest::Cancel => Req::Cancel,
            RadioRequest::Mute(m) => Req::Mute(m),
            RadioRequest::Scan => Req::Scan,
        };
        crate::fm::request(req);
        self.sync_radio(embassy_time::Instant::now().as_millis());
    }
    fn presets(&self) -> &Presets {
        self.presets
    }
    fn preset_op(&mut self, op: PresetOp) -> bool {
        let ok = match op {
            PresetOp::Save(khz) => {
                // Named after the station when it is the one playing.
                let mut name = [0u8; 8];
                let mut n = 0;
                crate::fm::with(|r| {
                    if r.info().khz == khz {
                        for (o, &c) in name.iter_mut().zip(r.rds().ps()) {
                            *o = c;
                            n += 1;
                        }
                    }
                });
                self.presets.save(khz, name.get(..n).unwrap_or(&[])).is_some()
            }
            PresetOp::Remove(i) => self.presets.remove(i),
            PresetOp::MoveUp(i) => i > 0 && self.presets.swap(i, i - 1),
            PresetOp::MoveDown(i) => self.presets.swap(i, i + 1),
        };
        if ok {
            self.presets_dirty = true;
            self.pending.push(Event::Service(ServiceEvent::Other {
                kind: services::other::RADIO,
                value: services::radio_event::PRESETS,
            }));
        }
        ok
    }
}

impl StepsService for Fw {
    fn steps_info(&self) -> StepsInfo {
        crate::sensor::info()
    }
    fn steps_day(&self, ago: u8) -> u32 {
        crate::sensor::day(ago)
    }
    fn steps_reset_today(&mut self) {
        crate::sensor::reset_today();
    }
}

impl ToolsService for Fw {
    fn tools(&self) -> &Tools {
        &self.tools
    }
    fn tools_mut(&mut self) -> &mut Tools {
        &mut self.tools
    }
    fn alarm_wakes_from_off(&self) -> bool {
        ALARM_WAKE_FROM_OFF
    }
}

impl Fw {
    /// Each UI loop (UI-5c, SENS-1): the Steps app's state and settings to
    /// the sensor task; a changed count as an event.
    #[inline(never)]
    pub fn sync_steps(&mut self, app_open: bool) {
        crate::sensor::set_ui(
            app_open,
            self.settings.get(id::STEP_BACKGROUND) != 0,
            u8::try_from(self.settings.get(id::STEP_SENSITIVITY)).unwrap_or(1),
        );
        if crate::sensor::take_changed() {
            let v = crate::sensor::day(0);
            self.pending.push(Event::Service(ServiceEvent::Other { kind: services::other::STEPS, value: v }));
        }
    }

    /// Each UI loop (UI-5d): the timer and the alarms. Returns the
    /// `tools_event` sent, if any (the UI lights the screen for an alert).
    #[inline(never)]
    pub fn sync_tools(&mut self, now_ms: u64) -> Option<u32> {
        let e = orpheus_apps::tools::poll_service(self, now_ms);
        if let Some(v) = e {
            self.pending.push(Event::Service(ServiceEvent::Other { kind: services::other::TOOLS, value: v }));
        }
        // The alert's sound: the music pauses for it and resumes after.
        let ringing = self.tools.ringing().is_some();
        let sounding = crate::audio::ALERT.load(Ordering::Relaxed);
        if ringing && !sounding {
            if self.player.source == Source::Playing {
                self.alert_paused = true;
                self.command(PlayerCmd::PlayPause);
            }
            crate::audio::ALERT.store(true, Ordering::Relaxed);
            let _ = media::try_request(media::Req::Alert);
        } else if !ringing && sounding {
            crate::audio::ALERT.store(false, Ordering::Relaxed);
            if core::mem::take(&mut self.alert_paused) && self.player.source == Source::Paused {
                self.command(PlayerCmd::PlayPause);
            }
        }
        self.arm_rtc_alarm();
        e
    }

    /// The RTC alarm at the next alarm's time (re-armed when the alarms or
    /// the minute change; the alarm wake from off needs it, PWR-3c). While
    /// the player is on the alarms ring from `Tools::poll`, which compares
    /// the RTC's minute.
    fn arm_rtc_alarm(&mut self) {
        let minute = atj2157_hal::rtc_clock::now().minute;
        if !core::mem::take(&mut self.alarms_changed) && self.last_alarm_minute == minute {
            return;
        }
        self.last_alarm_minute = minute;
        let next = if self.clock_valid() {
            orpheus_apps::tools::next_alarm(&orpheus_apps::tools::alarms(self), &self.now())
        } else {
            None
        };
        let want = next.map(|(_, h, m)| (h, m));
        if want != self.rtc_alarm {
            match want {
                Some((h, m)) => atj2157_hal::rtc_clock::set_alarm(h, m, 0),
                None => atj2157_hal::rtc_clock::clear_alarm(),
            }
            self.rtc_alarm = want;
        }
    }
}

/// `alarm status` (UI-5d, RTC-1's alarm): the RTC alarm registers and the
/// alarm the UI armed.
pub fn alarm_status() {
    let (enabled, pending, h, m, s) = atj2157_hal::rtc_clock::alarm_state();
    let t = atj2157_hal::rtc_clock::now();
    crate::audio::reply(|w| {
        w.s(b"alarm rtc=").u(u64::from(h)).s(b":").u(u64::from(m)).s(b":").u(u64::from(s));
        w.s(b" enabled=").u(u64::from(enabled)).s(b" pending=").u(u64::from(pending));
        w.s(b" now=").u(u64::from(t.hour)).s(b":").u(u64::from(t.minute)).s(b":").u(u64::from(t.second));
        w.s(b" wake_from_off=").u(u64::from(ALARM_WAKE_FROM_OFF));
    });
}

impl Fw {
    /// The games' card queue (every UI loop, on its data tag, after each
    /// request): the screens hear `other::GAMES` when the hub changed.
    #[inline(never)]
    pub fn games_poll(&mut self, now_ms: u64) {
        if self.games.pump(now_ms) {
            self.pending.push(Event::Service(ServiceEvent::Other {
                kind: orpheus_apps::services::other::GAMES,
                value: 0,
            }));
        }
    }

    /// Games operations are queued or out (power-off and USB hand-over
    /// wait for them).
    #[must_use]
    pub fn games_busy(&self) -> bool {
        self.games.busy()
    }
}

impl orpheus_apps::games::GamesService for Fw {
    fn games(&self) -> &orpheus_apps::games::engine::Hub {
        &self.games.hub
    }

    fn games_mut(&mut self) -> &mut orpheus_apps::games::engine::Hub {
        &mut self.games.hub
    }

    fn games_request(&mut self, r: orpheus_apps::games::GamesRequest) {
        self.games.io.request(&mut self.games.hub, r);
        self.games_poll(now_ms());
    }

    /// The uptime in microseconds and the RTC's seconds since 2000, mixed.
    fn game_seed(&mut self) -> u64 {
        self.game_seeds = self.game_seeds.wrapping_add(1);
        let rtc = atj2157_hal::rtc_clock::epoch2000(atj2157_hal::rtc_clock::now());
        let up = atj2157_hal::time::now_us() ^ u64::from(self.game_seeds) << 48;
        orpheus_apps::games::seed_from(up, u64::from(rtc))
    }

    /// The screen as the UI task last switched it (`drain::Shared`).
    fn display_on(&self) -> bool {
        crate::drain::shared().screen_on.load(Ordering::Relaxed)
    }
}

impl AuthService for Fw {
    fn vault(&self) -> VaultState {
        match &self.auth_pending {
            Some((work, ..)) => VaultState::Working { work: *work, percent: 0 },
            None => self.auth.state(now_ms()),
        }
    }
    fn unix_time(&self) -> Option<u64> {
        crate::vault::unix_now()
    }
    // Not inlined: in the Authenticator screen's row loop its future and
    // the decoded entry sat in the frame above the code's HMAC (VER-10).
    #[inline(never)]
    fn auth_entry(&mut self, i: usize) -> Option<AuthEntry> {
        embassy_futures::block_on(self.auth.entry(i)).ok().map(|e| AuthEntry::of(&e))
    }
    fn auth_code(&mut self, i: usize) -> Option<Code> {
        self.auth_code_now(i, crate::vault::unix_now())
    }
    fn auth_request(&mut self, r: AuthRequest) {
        use crate::vault::AuthOp;
        let now = now_ms();
        let op = match r {
            AuthRequest::Touch => {
                self.auth.touch(now);
                None
            }
            AuthRequest::Unlock(pin) => {
                self.auth_pending = Some((orpheus_apps::services::VaultWork::Unlock, pin, false));
                self.pending.push(Event::Service(ServiceEvent::Other {
                    kind: other::AUTH,
                    value: auth_event::STATE,
                }));
                None
            }
            AuthRequest::Create(pin) => {
                self.auth_pending = Some((orpheus_apps::services::VaultWork::Create, pin, false));
                self.pending.push(Event::Service(ServiceEvent::Other {
                    kind: other::AUTH,
                    value: auth_event::STATE,
                }));
                None
            }
            AuthRequest::ChangePin(pin) => {
                self.auth_pending = Some((orpheus_apps::services::VaultWork::ChangePin, pin, false));
                self.pending.push(Event::Service(ServiceEvent::Other {
                    kind: other::AUTH,
                    value: auth_event::STATE,
                }));
                None
            }
            AuthRequest::Lock => {
                self.auth.lock();
                self.pending.push(Event::Service(ServiceEvent::Other {
                    kind: other::AUTH,
                    value: auth_event::STATE,
                }));
                None
            }
            AuthRequest::Type(i) => {
                self.auth_type(i);
                None
            }
            AuthRequest::Import => Some(AuthOp::Import),
            AuthRequest::NextCode(i) => Some(AuthOp::NextCode(i)),
            AuthRequest::Delete(i) => Some(AuthOp::Delete(i)),
            AuthRequest::SwapNext(i) => Some(AuthOp::SwapNext(i)),
            AuthRequest::Provision(true) => Some(AuthOp::AddProvision),
            AuthRequest::Provision(false) => {
                crate::vault::reject_provision();
                None
            }
            AuthRequest::Wipe => Some(AuthOp::Wipe),
        };
        if op.is_some() {
            self.auth_op = op;
        }
    }
    fn auth_outcome(&self) -> AuthOutcome {
        self.auth_outcome
    }
    fn provision_pending(&self) -> Option<AuthEntry> {
        crate::vault::provision_pending()
    }
}

fn now_ms() -> u64 {
    embassy_time::Instant::now().as_millis()
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
        // As the simulator (05 §7.11, System › About): the model fits the row; the
        // longer "Yoton YM03 (ATJ2157)" shortened both value and label.
        services::PLAYER_HARDWARE
    }
    fn app_available(&self, app: App) -> bool {
        // The transmitter module is fitted on every YM03 (06 §4.1); a
        // module that does not answer shows "Not responding" in the app.
        // The tuner answers at 0x10 on every YM03 seen (01-hardware §9); a
        // tuner that does not shows "Tuner not responding" in the app.
        // Steps and Stopwatch: SENS-1, UI-5c, UI-5d (Steps says so when the
        // sensor does not answer). Recorder (AU-13) and USB Drive (USB-6)
        // are built and verified on the player (2026-11-24).
        // The list itself: `App::on_player` (host-tested).
        app.on_player()
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
    fn boot_slot(&self) -> Option<orpheus_apps::services::BootSlot> {
        // T-6: stage 2's boot-info word for this start.
        let i = atj2157_hal::persist::boot_info()?;
        Some(orpheus_apps::services::BootSlot { running: i.slot()?, failed: i.tripped() })
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
    fn list_roll_around(&self) -> bool {
        services::list_roll_around(self)
    }
    /// Settings › Sound › Key click (VER-13): a click into the sound
    /// output, mixed into the music while it plays, else sounded alone by
    /// the card task (not over the radio, whose sound bypasses the DAC).
    fn key_click(&mut self) {
        if orpheus_apps::services::key_click(self.settings.get(id::KEY_CLICK), &self.recorder())
            && !self.radio.phase.is_on()
        {
            crate::audio::click(self.player.source != orpheus_ui::event::Source::Playing);
        }
    }
    /// Key click = Presses and holds (A11Y-2): a second click when a held
    /// key reaches its hold delay.
    fn key_hold_click(&mut self) {
        if orpheus_apps::services::hold_click(self.settings.get(id::KEY_CLICK), &self.recorder())
            && !self.radio.phase.is_on()
        {
            crate::audio::click(self.player.source != orpheus_ui::event::Source::Playing);
        }
    }
    fn animations(&self) -> bool {
        services::animations(self)
    }
    fn text_size(&self) -> orpheus_ui::font::TextSize {
        services::text_size(self)
    }
    // Not inlined: a player command's frame in the UI task's poll frame.
    #[inline(never)]
    fn transport(&mut self, k: orpheus_ui::event::Key, p: orpheus_ui::event::KeyPhase) {
        services::locked_transport(self, k, p);
    }
    fn saver_clock(&self) -> Option<(u8, u8)> {
        services::saver_clock(self)
    }
}
