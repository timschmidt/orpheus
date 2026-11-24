//! Host implementation of the `orpheus-apps` service traits: plain state with
//! scriptable fakes (`set:battery=8`, `set:card=absent`, `set:usb=host` …)
//! and a log of everything the screens asked for.

use std::cell::Cell;

use anyhow::{Result, bail};
use orpheus_apps::screens::{self, AppScreen};
use orpheus_apps::services::{
    self, BatteryInfo, BluetoothService, CardInfo, CardState, ClockService, DateTime, DrainLog, FsKind,
    PlaybackService, Playing, PowerRequest, PowerService, SettingsService, StorageRequest, StorageService,
    SystemService, UsbMode, UsbService, UsbState,
};
use orpheus_apps::services::{
    Bookmark, BrowseFilter, BrowseService, DirId, Entry, FileOp, FileRef, Levels, LibList, LibListing,
    LibNow, LibRow, LibraryService, LibraryState, Listing, LyricLine, PicReq, PicState, PictureService,
    PlayerCmd, PlayerService, PlayerState, QueueEntry, TextService, TrackInfo,
};
use orpheus_apps::services::{BtDevice, BtList, BtPhase, BtRequest, StepsInfo, StepsService, ToolsService};
use orpheus_apps::settings::{SettingId, SettingsCache, id, lang_code};
use orpheus_apps::tools::{self, Tools};
use orpheus_apps::{App, Lang};
use orpheus_power::charger::ChargeState;
use orpheus_ui::event::{BtState, Event, ServiceEvent, Source};
use orpheus_ui::geom::Rect;
use orpheus_ui::gfx::Gfx;
use orpheus_ui::input::KeyGate;
use orpheus_ui::nav::{Arg, ScreenId};
use orpheus_ui::shell::System;
use orpheus_ui::theme::{Accent, Palette, Theme};
use orpheus_ui::widgets::bars::{MiniPlayer, Status};

use crate::bt::FakeBt;
use crate::media::Media;
use crate::steps::FakeSteps;
use crate::{Host, packs};

/// Something the screens asked the services to do.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Logged {
    Power(PowerRequest),
    Storage(StorageRequest),
    Usb(UsbMode),
    SetTime(DateTime),
    FactoryReset,
}

pub struct HostServices {
    pub settings: SettingsCache,
    pub battery: BatteryInfo,
    pub sleep_left: Option<u16>,
    pub clock: DateTime,
    pub card: CardState,
    pub card_info: CardInfo,
    pub playing: Option<(&'static str, &'static str, Source, u16)>,
    pub volume: u8,
    pub usb: UsbState,
    pub bt: FakeBt,
    bt_seen: BtState,
    pub langs: Vec<Lang>,
    pub lang: Lang,
    pub dual_boot: bool,
    pub locked: bool,
    lock_request: bool,
    pub root: (ScreenId, Arg),
    pub log: Vec<Logged>,
    pub media: Media,
    events: Vec<Event>,
    picture: Cell<PicState>,
    /// The developer rows are shown (About × 5, or `set:developer=1`).
    pub developer: bool,
    /// The fake battery logger (PWR-5): `set:drain=on|off`, `set:drainrows=N`.
    pub drain: DrainLog,
    /// Files the fake logger has started (its next number).
    drain_files: u16,
    /// The fake accelerometer and the step history (`set:walk=on`,
    /// `set:steps=N`, `set:week=…`, `set:sensor=off`).
    pub steps: FakeSteps,
    /// Stopwatch, timer, alarms (UI-5d).
    pub tools: Tools,
    /// `set:alarmwake=1`: alarms can wake the player from off (PWR-3c).
    pub alarm_wake: bool,
    /// `set:clockvalid=0`: the RTC lost its time (U-RTC).
    pub clock_valid: bool,
    /// `set:clockrun=1`: the fake clock follows simulated time (alarms).
    pub clock_run: bool,
    clock_ms: u64,
    /// Simulated milliseconds (the shell's `Instant`).
    now_ms: u64,
    /// An alert lit the screen.
    wake: bool,
}

impl Default for HostServices {
    fn default() -> Self {
        HostServices {
            settings: SettingsCache::new(),
            battery: BatteryInfo { percent: 72, millivolts: 3912, charge: ChargeState::NotPresent },
            sleep_left: None,
            clock: DateTime { year: 2026, month: 11, day: 24, hour: 12, minute: 41, second: 0, weekday: 1 },
            card: CardState::Ready,
            card_info: CardInfo { capacity_mib: 243_712, free_mib: 51_380, fs: FsKind::ExFat },
            playing: None,
            volume: 12,
            usb: UsbState::Detached,
            bt: FakeBt::default(),
            bt_seen: BtState::Off,
            langs: vec![Lang::ENGLISH, packs::russian(), packs::japanese()],
            lang: Lang::ENGLISH,
            dual_boot: true,
            locked: false,
            lock_request: false,
            root: (screens::HOME, Arg(0)),
            log: Vec::new(),
            media: Media::new(),
            events: Vec::new(),
            picture: Cell::new(PicState::Nothing),
            developer: false,
            drain: DrainLog::default(),
            drain_files: 0,
            steps: FakeSteps::default(),
            tools: Tools::default(),
            alarm_wake: false,
            clock_valid: true,
            clock_run: false,
            clock_ms: 0,
            now_ms: 0,
            wake: false,
        }
    }
}

impl SettingsService for HostServices {
    fn setting(&self, sid: SettingId) -> i16 {
        self.settings.get(sid)
    }

    fn set_setting(&mut self, sid: SettingId, value: i16) {
        if self.settings.set(sid, value) {
            if sid == id::LANGUAGE {
                let v = self.settings.get(id::LANGUAGE);
                self.lang = self.langs.iter().copied().find(|l| lang_code(l.code()) == v).unwrap_or_default();
            }
            if sid == id::BLUETOOTH {
                self.bt.power(value != 0);
                self.bt_events();
            }
            if sid == id::BT_RECONNECT {
                self.bt.reconnect = value != 0;
            }
            if sid == id::SLEEP_TIMER {
                self.sleep_left = match value {
                    1..=5 => Some([15, 30, 45, 60, 90][usize::try_from(value - 1).unwrap_or(0)]),
                    _ => None,
                };
            }
            self.events.push(Event::Service(ServiceEvent::SettingChanged(sid.0)));
        }
        // The firmware would now write `take_dirty()` to the store.
        let _ = self.settings.take_dirty().count();
    }

    fn factory_reset(&mut self) {
        self.settings.reset();
        self.lang = Lang::ENGLISH;
        self.log.push(Logged::FactoryReset);
    }
}

impl PowerService for HostServices {
    fn battery(&self) -> BatteryInfo {
        self.battery
    }

    fn sleep_timer_left(&self) -> Option<u16> {
        self.sleep_left
    }

    fn power(&mut self, req: PowerRequest) {
        if req == PowerRequest::LockKeys {
            self.lock_request = true;
        }
        self.log.push(Logged::Power(req));
    }

    fn developer(&self) -> bool {
        self.developer
    }

    fn show_developer(&mut self) {
        self.developer = true;
    }

    fn drain_log(&self) -> DrainLog {
        self.drain
    }

    fn set_drain_log(&mut self, on: bool, stay: bool) {
        if on && !self.drain.on {
            self.drain_files += 1;
            self.drain = DrainLog { on: true, stay, file: self.drain_files, rows: 0 };
        }
        self.drain.on = on;
        self.drain.stay = stay;
    }
}

impl ClockService for HostServices {
    fn now(&self) -> DateTime {
        self.clock
    }

    fn set_now(&mut self, t: DateTime) {
        self.clock = t;
        self.log.push(Logged::SetTime(t));
    }

    fn clock_valid(&self) -> bool {
        self.clock_valid
    }
}

impl StorageService for HostServices {
    fn card(&self) -> CardState {
        self.card
    }

    fn card_info(&self) -> Option<CardInfo> {
        (self.card == CardState::Ready).then_some(self.card_info)
    }

    fn storage(&mut self, req: StorageRequest) {
        self.log.push(Logged::Storage(req));
    }
}

impl PlaybackService for HostServices {
    fn playing(&self) -> Option<Playing<'_>> {
        if let Some((title, artist, playing, progress)) = self.media.title_artist() {
            let source = if playing { Source::Playing } else { Source::Paused };
            return Some(Playing { source, title, artist, progress });
        }
        self.playing.map(|(title, artist, source, progress)| Playing { source, title, artist, progress })
    }

    fn volume_step(&mut self, delta: i8) -> (u8, u8) {
        let max = u8::try_from(self.settings.get(id::VOLUME_LIMIT)).unwrap_or(30);
        let v = i16::from(self.volume) + i16::from(delta);
        self.volume = u8::try_from(v.clamp(0, i16::from(max))).unwrap_or(0);
        (self.volume, max)
    }
}

impl UsbService for HostServices {
    fn usb(&self) -> UsbState {
        self.usb
    }

    fn select_usb(&mut self, mode: UsbMode) {
        self.log.push(Logged::Usb(mode));
    }
}

impl BluetoothService for HostServices {
    fn bt(&self) -> BtState {
        self.bt.state()
    }

    fn bt_device(&self) -> &str {
        self.bt.device_name()
    }

    fn bt_phase(&self) -> BtPhase {
        self.bt.phase()
    }

    fn bt_count(&self, list: BtList) -> usize {
        self.bt.count(list)
    }

    fn bt_entry(&self, list: BtList, i: usize) -> Option<BtDevice> {
        self.bt.entry(list, i)
    }

    fn bt_request(&mut self, r: BtRequest) {
        self.bt.request(r);
        self.bt_events();
    }
}

impl StepsService for HostServices {
    fn steps_info(&self) -> StepsInfo {
        let background = self.settings.get(id::STEP_BACKGROUND) != 0;
        StepsInfo { sensor: self.steps.sensor, counting: background, dated: self.clock_valid }
    }

    fn steps_day(&self, ago: u8) -> u32 {
        self.steps.history.steps_ago(usize::from(ago))
    }

    fn steps_reset_today(&mut self) {
        self.steps.history.reset_today();
        self.events.push(Event::Service(ServiceEvent::Other { kind: services::other::STEPS, value: 0 }));
    }
}

impl ToolsService for HostServices {
    fn tools(&self) -> &Tools {
        &self.tools
    }

    fn tools_mut(&mut self) -> &mut Tools {
        &mut self.tools
    }

    fn alarm_wakes_from_off(&self) -> bool {
        self.alarm_wake
    }
}

impl SystemService for HostServices {
    fn lang(&self) -> Lang {
        self.lang
    }

    fn language_count(&self) -> usize {
        self.langs.len()
    }

    fn language(&self, i: usize) -> Lang {
        self.langs.get(i).copied().unwrap_or_default()
    }

    fn version(&self) -> &str {
        "0.1.0-sim"
    }

    fn build_id(&self) -> &str {
        "a1b2c3d"
    }

    fn serial(&self) -> &str {
        "5F3A91C2"
    }

    fn hardware(&self) -> &str {
        "YM03"
    }

    fn app_available(&self, _app: App) -> bool {
        true
    }

    fn dual_boot(&self) -> bool {
        self.dual_boot
    }

    fn usb_audio_built(&self) -> bool {
        false
    }

    fn keys_locked(&self) -> bool {
        self.locked
    }
}

impl System for HostServices {
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

/// Sample "now playing" per language.
const PLAYING: [(&str, &str); 3] =
    [("Hoppípolla", "Sigur Rós"), ("Звезда по имени Солнце", "Кино"), ("夜に駆ける", "YOASOBI")];

impl Host for HostServices {
    type Screens = AppScreen;

    fn palette(&self, _theme: Theme, _accent: Accent) -> Palette {
        services::palette(self)
    }

    fn appearance(&mut self, theme: Theme, accent: Accent) -> Vec<Event> {
        let t = match theme {
            Theme::Dark => 0,
            Theme::Light => 1,
            Theme::Amber => 2,
        };
        let a = match accent {
            Accent::Blue => 0,
            Accent::Green => 1,
            Accent::Pink => 2,
            Accent::Red => 3,
            Accent::Grey => 4,
        };
        self.set_setting(id::THEME, t);
        self.set_setting(id::ACCENT, a);
        core::mem::take(&mut self.events)
    }

    fn set_lang(&mut self, lang: &str) -> Result<bool> {
        let code = match lang {
            "latin" | "en" => "en",
            "cyrillic" | "ru" => "ru",
            "cjk" | "ja" => "ja",
            _ => bail!("lang: latin|en, cyrillic|ru, cjk|ja"),
        };
        self.set_setting(id::LANGUAGE, lang_code(code));
        self.retitle();
        Ok(false)
    }

    fn set(&mut self, kv: &str) -> Result<Vec<Event>> {
        let Some((k, v)) = kv.split_once('=') else { bail!("set:{kv}: expected key=value") };
        let on = matches!(v, "1" | "on" | "true" | "yes");
        let mut evs = Vec::new();
        match k {
            "battery" => {
                self.battery.percent = v.parse()?;
                self.battery.millivolts = 3300 + u16::from(self.battery.percent) * 9;
                evs.push(Event::Service(ServiceEvent::Battery {
                    percent: self.battery.percent,
                    charging: self.battery.charging(),
                }));
            }
            "charge" => {
                self.battery.charge = match v {
                    "none" => ChargeState::NotPresent,
                    "charging" => ChargeState::Charging,
                    "full" => ChargeState::Full,
                    "fault" => ChargeState::Fault,
                    _ => bail!("charge: none, charging, full, fault"),
                };
                evs.push(Event::Service(ServiceEvent::Battery {
                    percent: self.battery.percent,
                    charging: self.battery.charging(),
                }));
            }
            "card" => {
                self.card = match v {
                    "absent" => CardState::Absent,
                    "ready" => CardState::Ready,
                    "unreadable" => CardState::Unreadable,
                    _ => bail!("card: absent, ready, unreadable"),
                };
                evs.push(Event::Service(if self.card == CardState::Absent {
                    ServiceEvent::CardRemoved
                } else {
                    ServiceEvent::CardInserted
                }));
            }
            "usb" => {
                self.usb = match v {
                    "off" => UsbState::Detached,
                    "charger" => UsbState::Charger,
                    "host" => UsbState::Host,
                    _ => bail!("usb: off, charger, host"),
                };
                evs.push(Event::Service(ServiceEvent::Usb { host: self.usb == UsbState::Host }));
            }
            "bt" => {
                let p = match v {
                    "off" => BtPhase::Off,
                    "starting" => BtPhase::Starting,
                    "on" => BtPhase::Ready,
                    "scanning" => BtPhase::Scanning,
                    "connecting" => BtPhase::Connecting,
                    "connected" => BtPhase::Connected,
                    "streaming" => BtPhase::Streaming,
                    "dead" => BtPhase::NotResponding,
                    _ => bail!("bt: off, starting, on, scanning, connecting, connected, streaming, dead"),
                };
                self.bt.force(p);
                self.settings.set(id::BLUETOOTH, i16::from(p != BtPhase::Off));
                self.bt_events();
            }
            "developer" => self.developer = on,
            "drain" => self.set_drain_log(on, self.drain.stay),
            "drainrows" => self.drain.rows = v.parse()?,
            "btfound" => {
                self.bt.force_found(v.parse()?);
                self.bt_events();
            }
            "playing" => {
                self.playing = on.then(|| {
                    let (t, a) = PLAYING[self.lang_index()];
                    (t, a, Source::Playing, 460)
                });
                evs.push(Event::Service(ServiceEvent::Playback(if on {
                    Source::Playing
                } else {
                    Source::Idle
                })));
            }
            "clock" => {
                let mut it = v.split(':');
                self.clock.hour = it.next().unwrap_or("12").parse()?;
                self.clock.minute = it.next().unwrap_or("41").parse()?;
                self.clock.second = it.next().unwrap_or("0").parse()?;
                self.clock_ms = 0;
                evs.push(Event::Service(ServiceEvent::Minute));
            }
            "weekday" => self.clock.weekday = v.parse::<u8>()? % 7,
            "clockvalid" => self.clock_valid = on,
            "clockrun" => self.clock_run = on,
            "alarmwake" => self.alarm_wake = on,
            "walk" => self.steps.walking = on,
            "sensor" => self.steps.sensor = on,
            "steps" => {
                self.steps.set_today(v.parse()?);
                evs.push(Event::Service(ServiceEvent::Other { kind: services::other::STEPS, value: 0 }));
            }
            "week" => {
                let counts: Vec<u32> = v.split('/').map(str::parse).collect::<Result<_, _>>()?;
                let today = self.today().unwrap_or(0);
                self.steps.set_week(&counts, today);
                evs.push(Event::Service(ServiceEvent::Other { kind: services::other::STEPS, value: 0 }));
            }
            "library" => {
                self.media.set_library(match v {
                    "ready" => LibraryState::Ready,
                    "missing" => LibraryState::Missing,
                    _ => LibraryState::Building { files: 1287 },
                });
                evs.push(Event::Service(ServiceEvent::Other { kind: services::other::LIBRARY, value: 0 }));
            }
            "queue" => {
                // Start an album: takk, kino, yoasobi, podcast.
                let path = match v {
                    "kino" => "/Music/Кино/Звезда по имени Солнце",
                    "yoasobi" => "/Music/YOASOBI/THE BOOK",
                    "podcast" => "/Music/Podcasts",
                    _ => "/Music/Sigur Rós/Takk…",
                };
                self.media.learn(path);
                if let Some(dir) = self.media.dir_id(path) {
                    PlayerService::command(self, PlayerCmd::PlayDir { dir, start: None, shuffle: false });
                }
            }
            "track" => {
                let n: u32 = v.parse()?;
                PlayerService::command(self, PlayerCmd::Jump(n));
            }
            "pos" => {
                let ms: u32 = v.parse()?;
                PlayerService::command(self, PlayerCmd::SeekTo(ms));
            }
            "pause" => {
                if on && self.media.state().source == Source::Playing {
                    PlayerService::command(self, PlayerCmd::PlayPause);
                }
            }
            "resume" => {
                self.media.learn("/Music/Sigur Rós/Takk…");
                self.media.set_resume("/Music/Sigur Rós/Takk…/02 Hoppípolla.mp3");
            }
            "bookmark" => {
                PlayerService::command(self, PlayerCmd::Bookmark);
            }
            "locked" => {
                self.locked = on;
                self.lock_request = on;
            }
            "dual" => self.dual_boot = on,
            "volume" => self.volume = v.parse()?,
            _ => {
                // `set:0x0304=1`: any setting by key.
                let key = k.strip_prefix("0x").map(|h| u16::from_str_radix(h, 16));
                match key {
                    Some(Ok(key)) => self.set_setting(SettingId(key), v.parse()?),
                    _ => bail!("set: unknown key `{k}`"),
                }
            }
        }
        evs.append(&mut self.events);
        Ok(evs)
    }

    fn screen_by_name(name: &str) -> Option<ScreenId> {
        Some(match name {
            "Home" => screens::HOME,
            "Music" => screens::MUSIC,
            "Files" => screens::FILES,
            "NowPlaying" => screens::NOW_PLAYING,
            "Equaliser" => screens::EQUALISER,
            "Boot" => screens::BOOT,
            "Settings" => screens::SETTINGS,
            "PowerMenu" => screens::POWER_MENU,
            "QuickPanel" => screens::QUICK_PANEL,
            "Lock" => screens::LOCK,
            "Charging" => screens::CHARGING,
            "UsbConnect" => screens::USB_CONNECT,
            "Goodbye" => screens::GOODBYE,
            "HomeApps" => screens::HOME_APPS,
            "Bluetooth" => screens::BLUETOOTH,
            "BtScan" => screens::BT_SCAN,
            "BtPaired" => screens::BT_PAIRED,
            "Steps" => screens::STEPS,
            "Stopwatch" => screens::STOPWATCH,
            _ => return None,
        })
    }

    fn root() -> (ScreenId, Arg) {
        (screens::HOME, Arg(0))
    }

    fn after_step(&mut self, gate: &mut KeyGate) {
        // What the firmware's power task does with the requests.
        for l in &self.log {
            if let Logged::Power(PowerRequest::LockNoticeDone) = l {
                // The lock notice is over: if the keys were locked with the
                // screen off, it goes off again.
                if gate.locked {
                    gate.screen_on = false;
                }
            }
        }
        self.log.retain(|l| *l != Logged::Power(PowerRequest::LockNoticeDone));
        if core::mem::take(&mut self.lock_request) {
            gate.locked = true;
        }
        // An alert lights the screen (the keys stay as they were).
        if core::mem::take(&mut self.wake) {
            gate.screen_on = true;
        }
        self.locked = gate.locked;
    }

    fn take_events(&mut self) -> Vec<Event> {
        let mut e = core::mem::take(&mut self.events);
        e.extend(self.media.take_events());
        e
    }

    fn advance(&mut self, ms: u64) {
        let repeat = self.settings.get(id::REPEAT);
        self.media.advance(ms, repeat);
        self.bt.advance(ms);
        self.bt_events();
        self.now_ms += ms;
        if self.clock_run {
            self.tick_clock(ms);
        }
        let today = self.today();
        self.steps.advance(ms, today);
        if self.steps.take_changed() {
            let v = self.steps.history.today();
            self.events.push(Event::Service(ServiceEvent::Other { kind: services::other::STEPS, value: v }));
        }
        if let Some(e) = tools::poll_service(self, self.now_ms) {
            if e == services::tools_event::ALERT {
                self.wake = true;
            }
            self.events.push(Event::Service(ServiceEvent::Other { kind: services::other::TOOLS, value: e }));
        }
    }
}

impl BrowseService for HostServices {
    fn browse(&mut self, dir: DirId, filter: BrowseFilter) -> Listing {
        self.media.browse(dir, filter)
    }
    fn listing(&self) -> Listing {
        self.media.listing()
    }
    fn entry(&self, i: u32) -> Option<Entry<'_>> {
        self.media.entry(i)
    }
    fn want(&mut self, first: u32, count: u32) {
        self.media.want(first, count);
    }
    fn position(&self, f: FileRef) -> Option<u32> {
        self.media.position(f)
    }
    fn letter(&self, i: u32) -> Option<char> {
        self.media.letter(i)
    }
    fn seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        self.media.seek_letter(from, forward)
    }
    fn dir_name(&self, dir: DirId) -> &str {
        self.media.dir_name(dir)
    }
    fn file_op(&mut self, op: FileOp) -> bool {
        match op {
            FileOp::Delete(f) => self.media.delete(f),
        }
    }
}

impl TextService for HostServices {
    fn text_open(&mut self, f: FileRef) -> Option<u32> {
        self.media.text_open(f)
    }
    fn text_window(&self) -> (u32, &[u8]) {
        self.media.text_window()
    }
    fn want_text(&mut self, offset: u32) {
        self.media.want_text(offset);
    }
    fn text_position(&self, f: FileRef) -> u32 {
        self.media.text_position(f)
    }
    fn set_text_position(&mut self, f: FileRef, offset: u32) {
        self.media.set_text_position(f, offset);
    }
}

impl PlayerService for HostServices {
    fn player(&self) -> PlayerState {
        self.media.state()
    }
    fn track(&self) -> Option<TrackInfo<'_>> {
        self.media.track()
    }
    fn queue_entry(&self, i: u32) -> Option<QueueEntry<'_>> {
        self.media.queue_entry(i)
    }
    fn want_queue(&mut self, _first: u32, _count: u32) {}
    fn lyrics_count(&self) -> u32 {
        self.media.lyrics_count()
    }
    fn lyric(&self, i: u32) -> Option<LyricLine<'_>> {
        self.media.lyric(i)
    }
    fn levels(&self) -> Levels {
        self.media.levels()
    }
    fn resume_title(&self) -> Option<&str> {
        self.media.resume_title()
    }
    fn bookmark_count(&self) -> u32 {
        self.media.bookmark_count()
    }
    fn bookmark(&self, i: u32) -> Option<Bookmark<'_>> {
        self.media.bookmark(i)
    }
    fn library(&self) -> LibraryState {
        self.media.library
    }
    fn command(&mut self, c: PlayerCmd) {
        let repeat = self.settings.get(id::REPEAT);
        self.media.command(c, repeat);
    }
}

impl LibraryService for HostServices {
    fn lib_open(&mut self, list: LibList) -> LibListing {
        self.media.lib_open(list)
    }
    fn lib_listing(&self) -> LibListing {
        self.media.lib_listing()
    }
    fn lib_row(&self, i: u32) -> Option<LibRow<'_>> {
        self.media.lib_row(i)
    }
    fn lib_want(&mut self, _first: u32, _count: u32) {}
    fn lib_position(&self, id: u32) -> Option<u32> {
        self.media.lib_position(id)
    }
    fn lib_letter(&self, i: u32) -> Option<char> {
        self.media.lib_letter(i)
    }
    fn lib_seek_letter(&self, from: u32, forward: bool) -> Option<(u32, char)> {
        self.media.lib_seek_letter(from, forward)
    }
    fn lib_now(&self) -> Option<LibNow> {
        self.media.lib_now()
    }
}

impl PictureService for HostServices {
    fn show_picture(&self, g: &mut Gfx<'_>, r: Rect, req: PicReq) -> PicState {
        let st = self.media.show_picture(g, r, req);
        self.picture.set(st);
        st
    }
    fn picture_state(&self) -> PicState {
        self.picture.get()
    }
}

impl HostServices {
    /// The fake module's events as service events (and the status bar's
    /// state when it changed).
    fn bt_events(&mut self) {
        for v in self.bt.take_events() {
            self.events
                .push(Event::Service(ServiceEvent::Other { kind: services::other::BLUETOOTH, value: v }));
        }
        let st = self.bt.state();
        if st != self.bt_seen {
            self.bt_seen = st;
            self.events.push(Event::Service(ServiceEvent::Bluetooth(st)));
        }
    }

    /// Today's day number while the fake clock is valid.
    fn today(&self) -> Option<u32> {
        self.clock_valid.then(|| orpheus_steps::day_number(self.clock.year, self.clock.month, self.clock.day))
    }

    /// The fake clock follows simulated time (`set:clockrun=1`); days do
    /// not change month (enough for the alarm tests).
    fn tick_clock(&mut self, ms: u64) {
        self.clock_ms += ms;
        while self.clock_ms >= 1000 {
            self.clock_ms -= 1000;
            let c = &mut self.clock;
            c.second += 1;
            if c.second == 60 {
                c.second = 0;
                c.minute += 1;
                self.events.push(Event::Service(ServiceEvent::Minute));
            }
            if c.minute == 60 {
                c.minute = 0;
                c.hour += 1;
            }
            if c.hour == 24 {
                c.hour = 0;
                c.day += 1;
                c.weekday = (c.weekday + 1) % 7;
            }
        }
    }

    fn lang_index(&self) -> usize {
        match self.lang.code() {
            "ru" => 1,
            "ja" => 2,
            _ => 0,
        }
    }

    fn retitle(&mut self) {
        let (t, a) = PLAYING[self.lang_index()];
        if let Some(p) = self.playing.as_mut() {
            (p.0, p.1) = (t, a);
        }
    }
}
