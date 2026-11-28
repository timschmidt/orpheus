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
use orpheus_ui::text::StrBuf;
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

/// The battery logger (PWR-5; Settings › System › Developer).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DrainLog {
    pub on: bool,
    /// Auto-off is suppressed while it runs (or will be, when started).
    pub stay: bool,
    /// The file's number (`drain-<n>-….csv`; 0 until the card task chose
    /// it).
    pub file: u16,
    /// Rows on the card.
    pub rows: u32,
}

pub trait PowerService {
    fn battery(&self) -> BatteryInfo;
    /// Minutes left on the sleep timer, if running.
    fn sleep_timer_left(&self) -> Option<u16>;
    fn power(&mut self, req: PowerRequest);
    /// The developer rows are shown (unlocked in About; until restart).
    fn developer(&self) -> bool;
    fn show_developer(&mut self);
    fn drain_log(&self) -> DrainLog;
    /// Start (labelled with the Power mode) or stop the battery
    /// logger; `stay` suppresses auto-off while it runs.
    fn set_drain_log(&mut self, on: bool, stay: bool);
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

/// Seconds since 1970-01-01 of a UTC date and time (proleptic Gregorian;
/// the player's clock keeps UTC, 06 §5.3).
#[must_use]
pub fn unix_of(t: &DateTime) -> u64 {
    let m = i64::from(t.month);
    let y = i64::from(t.year) - i64::from(m <= 2);
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + i64::from(t.day) - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    let days = era * 146_097 + doe - 719_468;
    let s = days * 86_400 + i64::from(t.hour) * 3600 + i64::from(t.minute) * 60 + i64::from(t.second);
    u64::try_from(s).unwrap_or(0)
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
    /// Remove [`CACHE_DIRS`] (VER-13, 2026-11-27).
    ClearCaches,
    /// Read every folder and follow every file's clusters; writes nothing
    /// (`orpheus_fs::Volume::check`).
    CheckCard,
    /// Erase and create a file system (confirmed by the user;
    /// `orpheus_fs::format`: FAT32 to 32 GiB, exFAT above, in the card's
    /// own partition).
    Format,
}

/// What Settings › Storage › Clear caches removes: what the player makes
/// again from the card's files when it needs it (thumbnails and album art,
/// the folder indexes, the books' indexes and conversions). Never the
/// library, the queue, saves, bookmarks or anything outside `/.orpheus`.
pub const CACHE_DIRS: [&str; 2] = ["/.orpheus/cache", "/.orpheus/index"];

/// Folder levels Check card reads below the root (deeper folders are
/// counted, not read): a music tree is about four deep; each level is a
/// cursor in the card task's future (`.bss`).
pub const CHECK_DEPTH: usize = 8;

/// The last Settings › Storage card work (05 §7.11): `ServiceEvent::Other
/// { kind: other::STORAGE }` when it ends.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StorageOutcome {
    /// What was asked (`None`: nothing since start-up).
    pub req: Option<StorageRequest>,
    /// Still running.
    pub busy: bool,
    /// Ended without a card error.
    pub ok: bool,
    /// Check card: folders and files read, problems found, and whether it
    /// was stopped before its end (a request for the card came).
    pub dirs: u32,
    pub files: u32,
    pub problems: u32,
    pub stopped: bool,
}

pub trait StorageService {
    fn card(&self) -> CardState;
    fn card_info(&self) -> Option<CardInfo>;
    fn storage(&mut self, req: StorageRequest);
    fn storage_outcome(&self) -> StorageOutcome;
}

/// System › Diagnostics (05 §7.11, read-only): what the player reports
/// about itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Diagnostics {
    /// Seconds since start-up.
    pub uptime_s: u32,
    /// The core clock now (MHz).
    pub cpu_mhz: u16,
    /// Code and data lines loaded from flash since start-up (the pager).
    pub loads: u32,
    /// Audio underruns counted since start-up.
    pub underruns: u32,
    /// Stack bytes never used since start-up.
    pub stack_free: u32,
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

/// What the Bluetooth module is doing (06 §4.2), finer than the status
/// bar's [`BtState`].
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BtPhase {
    /// Bluetooth is off (enable line low).
    #[default]
    Off,
    /// Powering up and reading the paired list (or due to, shortly after
    /// boot).
    Starting,
    /// Up, not connected.
    Ready,
    /// Inquiry running (Pair new device).
    Scanning,
    Connecting,
    Connected,
    /// Connected and audio is playing.
    Streaming,
    /// The module stopped answering and was given up on after its
    /// bounded restarts; switching Bluetooth off and on tries again.
    NotResponding,
}

impl BtPhase {
    /// The module takes requests (scan, connect, forget).
    #[must_use]
    pub const fn is_up(self) -> bool {
        matches!(
            self,
            BtPhase::Ready
                | BtPhase::Scanning
                | BtPhase::Connecting
                | BtPhase::Connected
                | BtPhase::Streaming
        )
    }

    #[must_use]
    pub const fn is_connected(self) -> bool {
        matches!(self, BtPhase::Connected | BtPhase::Streaming)
    }
}

/// A device address as the module sends it: least-significant byte first
/// (HCI order, seen on the player). Compared and sent back as is.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BtAddr(pub [u8; 6]);

impl BtAddr {
    /// `XX:XX:XX:XX:XX:XX` as addresses are written (most-significant
    /// byte first, so the stored bytes reversed); shown for a device
    /// without a name, as the vendor does.
    pub fn push_text<const N: usize>(&self, out: &mut StrBuf<N>) {
        const HEX: &[u8; 16] = b"0123456789ABCDEF";
        for (i, b) in self.0.iter().rev().enumerate() {
            if i > 0 {
                out.push(':');
            }
            out.push(char::from(HEX[usize::from(b >> 4)]));
            out.push(char::from(HEX[usize::from(b & 15)]));
        }
    }
}

/// What a device is, from its class of device (inquiry results only).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BtKind {
    /// No class known (paired-list records carry none).
    #[default]
    Unknown,
    Headphones,
    Speaker,
    Phone,
    Computer,
    Other,
}

impl BtKind {
    /// From the three class-of-device bytes as the module sends them.
    /// The byte order is unknown (docs/hw/bluetooth.md §3.5), but the
    /// major class (bits 8–12) is in the middle byte either way; the
    /// minor class is taken from the first byte (HCI order, *inferred*):
    /// in the other order it would read the service bits, which only
    /// picks the speaker icon for an audio device instead of headphones.
    #[must_use]
    pub const fn from_cod(cod: [u8; 3]) -> BtKind {
        match cod[1] & 0x1F {
            0x01 => BtKind::Computer,
            0x02 => BtKind::Phone,
            0x04 => match (cod[0] >> 2) & 0x3F {
                // Loudspeaker, portable audio, car audio, set-top, HiFi.
                0x05 | 0x07 | 0x08 | 0x09 | 0x0A => BtKind::Speaker,
                _ => BtKind::Headphones,
            },
            _ => BtKind::Other,
        }
    }
}

/// Longest device name kept (bytes; the driver keeps as many).
pub const BT_NAME: usize = 24;
/// Devices per list (found or paired).
pub const BT_DEVICES: usize = 8;

/// A found or paired device, by value (no borrowed state: the firmware
/// copies it out of the Bluetooth task's published lists).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BtDevice {
    pub addr: BtAddr,
    pub kind: BtKind,
    /// The device the player is connected to (or connecting to).
    pub connected: bool,
    name: [u8; BT_NAME],
    name_len: u8,
}

impl BtDevice {
    pub const EMPTY: BtDevice = BtDevice {
        addr: BtAddr([0; 6]),
        kind: BtKind::Unknown,
        connected: false,
        name: [0; BT_NAME],
        name_len: 0,
    };

    /// `name` as the module sent it (normally UTF-8), cut to [`BT_NAME`].
    #[must_use]
    pub fn new(addr: BtAddr, kind: BtKind, name: &[u8], connected: bool) -> BtDevice {
        let mut d = BtDevice { addr, kind, connected, ..BtDevice::default() };
        // Without slicing: no panic site (resident in the firmware).
        for (o, c) in d.name.iter_mut().zip(name) {
            *o = *c;
        }
        d.name_len = name.len().min(BT_NAME) as u8;
        d
    }

    /// The name; `""` if none arrived (yet). A name cut inside a
    /// character ends before it.
    #[must_use]
    pub fn name(&self) -> &str {
        let b = self.name.get(..usize::from(self.name_len)).unwrap_or(&[]);
        match core::str::from_utf8(b) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(b.get(..e.valid_up_to()).unwrap_or(&[])).unwrap_or(""),
        }
    }

    /// The name, or the address in text form without one.
    pub fn push_label<const N: usize>(&self, out: &mut StrBuf<N>) {
        let n = self.name();
        if n.is_empty() {
            self.addr.push_text(out);
        } else {
            out.push_str_or_ellipsis(n);
        }
    }
}

/// The module's two device lists.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtList {
    /// Found by the last inquiry, in the order they answered.
    Found,
    /// Paired in the module.
    Paired,
}

/// Bluetooth requests from the screens (none of them is a restricted
/// module command: CLAUDE.md rule 6).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BtRequest {
    /// Start an inquiry (empties the found list).
    Scan,
    StopScan,
    /// Connect (pairing if needed); a running inquiry is stopped first,
    /// another connection is closed first.
    Connect(BtAddr),
    CancelConnect,
    Disconnect,
    /// Remove a pairing from the module.
    Forget(BtAddr),
}

/// `ServiceEvent::Other { kind: other::BLUETOOTH, value }` values.
pub mod bt_event {
    /// A device list changed (found, name resolved, paired, forgotten).
    pub const LISTS: u32 = 1;
    /// [`super::BtPhase`] changed.
    pub const PHASE: u32 = 2;
    /// A connection attempt failed (or was cancelled).
    pub const CONNECT_FAILED: u32 = 3;
    /// A device connected (the user's request or a reconnect).
    pub const CONNECTED: u32 = 4;
    /// The connected device went away.
    pub const DISCONNECTED: u32 = 5;
}

/// The Bluetooth transmitter (06 §4, 05 §7.4). Lists are small (at most
/// [`BT_DEVICES`] each) and answered by value.
pub trait BluetoothService {
    fn bt(&self) -> BtState;
    /// Name of the connected (or last) device, `""` if none.
    fn bt_device(&self) -> &str;
    fn bt_phase(&self) -> BtPhase;
    fn bt_count(&self, list: BtList) -> usize;
    fn bt_entry(&self, list: BtList, i: usize) -> Option<BtDevice>;
    /// Carried out by the Bluetooth task; results arrive as
    /// `ServiceEvent::Other { kind: other::BLUETOOTH, .. }` and
    /// `ServiceEvent::Bluetooth`.
    fn bt_request(&mut self, r: BtRequest);
}

// ---------------------------------------------------------------- Radio

pub use orpheus_fm::presets::{MAX as PRESETS_MAX, Preset, Presets};
pub use orpheus_fm::region::Region;

/// What the radio is doing (05 §7.3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RadioPhase {
    /// The tuner is powered down.
    #[default]
    Off,
    Starting,
    Playing,
    Seeking,
    Scanning,
    /// The tuner did not answer.
    NotResponding,
}

impl RadioPhase {
    /// The radio is (or is about to be) heard.
    #[must_use]
    pub const fn is_on(self) -> bool {
        matches!(
            self,
            RadioPhase::Starting | RadioPhase::Playing | RadioPhase::Seeking | RadioPhase::Scanning
        )
    }
}

/// The radio's state, by value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RadioInfo {
    pub phase: RadioPhase,
    pub khz: u32,
    /// Received signal strength, 0–127 (about 1 dB per step).
    pub rssi: u8,
    pub stereo: bool,
    pub muted: bool,
    /// Earphones are plugged in: their cable is the antenna.
    pub antenna: bool,
    /// Auto-scan: progress 0–1000 and stations found so far.
    pub progress: u16,
    pub found: u8,
}

impl RadioInfo {
    /// Signal bars 0–3 for the screen.
    #[must_use]
    pub const fn bars(&self) -> u8 {
        match self.rssi {
            0..=19 => 0,
            20..=29 => 1,
            30..=39 => 2,
            _ => 3,
        }
    }
}

/// Radio requests from the screens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RadioRequest {
    /// Power the tuner and play (music stops).
    Start,
    /// Stop: the tuner is powered down.
    Stop,
    Tune(u32),
    /// One region step.
    Step {
        up: bool,
    },
    /// To the next station.
    Seek {
        up: bool,
    },
    /// Stop a seek or an auto-scan.
    Cancel,
    Mute(bool),
    /// Auto-scan: the stations found replace the presets when it ends.
    Scan,
}

/// Changes to the presets (the service saves them).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PresetOp {
    /// Save a frequency (named with the station's RDS name when it is the
    /// one playing).
    Save(u32),
    Remove(usize),
    MoveUp(usize),
    MoveDown(usize),
}

/// `ServiceEvent::Other { kind: other::RADIO, value }` values.
pub mod radio_event {
    /// [`super::RadioInfo`] changed (phase, frequency, signal, mute).
    pub const STATE: u32 = 1;
    /// The RDS name or text changed.
    pub const RDS: u32 = 2;
    /// An auto-scan ended (the presets were replaced if it found any).
    pub const SCANNED: u32 = 3;
    /// The presets changed.
    pub const PRESETS: u32 = 4;
}

/// The FM radio (05 §7.3, firmware `fm` task).
pub trait RadioService {
    fn radio(&self) -> RadioInfo;
    /// RDS programme service name, `""` if none.
    fn radio_ps(&self) -> &str;
    /// RDS radio text so far, `""` if none.
    fn radio_rt(&self) -> &str;
    fn radio_request(&mut self, r: RadioRequest);
    fn presets(&self) -> &Presets;
    /// Change the presets; false if it could not be done (list full).
    fn preset_op(&mut self, op: PresetOp) -> bool;
}

/// "101.90": a frequency in MHz with two decimals.
pub fn push_mhz<const N: usize>(khz: u32, out: &mut StrBuf<N>) {
    out.push_u32(khz / 1000);
    out.push('.');
    let frac = (khz % 1000) / 10;
    if frac < 10 {
        out.push('0');
    }
    out.push_u32(frac);
}

/// A preset's label: its RDS name as text.
pub fn push_preset_name<const N: usize>(p: &Preset, out: &mut StrBuf<N>) {
    let mut b = [0u8; 32];
    let n = orpheus_fm::rds::to_utf8(p.name(), &mut b);
    let s = core::str::from_utf8(b.get(..n).unwrap_or(&[])).unwrap_or("");
    out.push_str_or_ellipsis(s.trim());
}

/// The player's hardware as System › About shows it (the firmware's and the
/// simulator's [`SystemService::hardware`]): the model only, which leaves
/// the "Hardware" label room (05 §7.11).
pub const PLAYER_HARDWARE: &str = "YM03";

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
    /// The firmware slot boot stage 2 started (About; `None` when not
    /// started from a slot, e.g. a RAM test image).
    fn boot_slot(&self) -> Option<BootSlot> {
        None
    }
    /// System › Diagnostics.
    fn diagnostics(&self) -> Diagnostics;
}

/// Which firmware slot runs, and which ones stage 2 gave up on first
/// (T-6: a slot that fails three starts in a row is skipped).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BootSlot {
    /// Index: 0 = slot A, 1 = slot B, 2 = the old dual-boot slot.
    pub running: u8,
    /// Slots that failed to start before this one (bit `i` = index `i`).
    pub failed: u8,
}

impl BootSlot {
    /// Short name of slot index `i`.
    #[must_use]
    pub const fn name(i: u8) -> &'static str {
        match i {
            0 => "A",
            1 => "B",
            2 => "dual",
            _ => "?",
        }
    }
}

// ---------------------------------------------------------------- Steps

/// The pedometer as the Steps app sees it (05 §7.7, SENS-1).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StepsInfo {
    /// The accelerometer answered with its id.
    pub sensor: bool,
    /// Counting now (Count in background, or the app is open).
    pub counting: bool,
    /// Days are dated (the clock is valid); otherwise no history is kept.
    pub dated: bool,
}

/// `ServiceEvent::Other { kind: other::STEPS, value }`: today's count
/// changed (`value` = the count).
pub trait StepsService {
    fn steps_info(&self) -> StepsInfo;
    /// Steps `ago` days before today (0 = today, up to 6).
    fn steps_day(&self, ago: u8) -> u32;
    /// Steps › Reset today.
    fn steps_reset_today(&mut self);
}

// ---------------------------------------------------------------- Tools

/// `ServiceEvent::Other { kind: other::TOOLS, value }` values.
pub mod tools_event {
    /// An alert started ([`crate::tools::Tools::ringing`]).
    pub const ALERT: u32 = 1;
    /// The alert ended by itself (rang for a minute).
    pub const ALERT_ENDED: u32 = 2;
}

/// Stopwatch, timer and alarms (05 §7.8). The state is the service's, so
/// it runs while the app is left; the screens change it directly.
pub trait ToolsService {
    fn tools(&self) -> &crate::tools::Tools;
    fn tools_mut(&mut self) -> &mut crate::tools::Tools;
    /// An alarm can switch the player on from off (PWR-3c, U-PMU-3). Off
    /// until proven on the player: alarms ring only while it is on.
    fn alarm_wakes_from_off(&self) -> bool;
}

// ---------------------------------------------------------------- Authenticator

pub use orpheus_otp::Code;
pub use orpheus_otp::service::{ImportReport, State as VaultState, Work as VaultWork};

/// One account as the app shows it (never the secret).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct AuthEntry {
    pub issuer: StrBuf<32>,
    pub account: StrBuf<64>,
    /// TOTP period in seconds; 0 for HOTP.
    pub period: u32,
    pub digits: u8,
}

impl AuthEntry {
    /// The entry's labels and parameters (the secret stays behind).
    #[must_use]
    pub fn of(e: &orpheus_otp::Entry) -> AuthEntry {
        AuthEntry {
            issuer: StrBuf::from(e.issuer.as_str()),
            account: StrBuf::from(e.account.as_str()),
            period: match e.kind {
                orpheus_otp::Kind::Totp { period } => period,
                orpheus_otp::Kind::Hotp { .. } => 0,
            },
            digits: e.digits,
        }
    }

    #[must_use]
    pub const fn is_hotp(&self) -> bool {
        self.period == 0
    }

    /// The row's name: the issuer, or the account without one.
    #[must_use]
    pub fn name(&self) -> &str {
        if self.issuer.is_empty() { self.account.as_str() } else { self.issuer.as_str() }
    }
}

/// PIN digits as typed (ASCII), wiped when dropped.
#[derive(Clone, PartialEq, Eq, Default)]
pub struct Pin {
    d: [u8; orpheus_otp::service::MAX_PIN],
    n: u8,
}

impl core::fmt::Debug for Pin {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.write_str("Pin")
    }
}

impl Drop for Pin {
    fn drop(&mut self) {
        use zeroize::Zeroize;
        self.d.zeroize();
    }
}

impl Pin {
    /// Add a digit (0..=9); `false` when full.
    pub fn push(&mut self, digit: u8) -> bool {
        match self.d.get_mut(usize::from(self.n)) {
            Some(slot) if digit < 10 => {
                *slot = b'0' + digit;
                self.n += 1;
                true
            }
            _ => false,
        }
    }

    pub fn pop(&mut self) {
        if self.n > 0 {
            self.n -= 1;
            if let Some(d) = self.d.get_mut(usize::from(self.n)) {
                *d = 0;
            }
        }
    }

    pub fn clear(&mut self) {
        use zeroize::Zeroize;
        self.d.zeroize();
        self.n = 0;
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.n)
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.n == 0
    }

    /// Long enough to submit.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.len() >= orpheus_otp::service::MIN_PIN
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.d[..usize::from(self.n)]
    }
}

/// What the app asks of the authenticator service. Results arrive as
/// `ServiceEvent::Other { kind: other::AUTH, value }` ([`auth_event`]) with
/// details in [`AuthService::auth_outcome`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AuthRequest {
    /// A key was pressed in the app (restarts the 60-s relock timer).
    Touch,
    Unlock(Pin),
    Create(Pin),
    ChangePin(Pin),
    Lock,
    /// Import `/.orpheus/otp-import.txt` from the card.
    Import,
    NextCode(u8),
    Delete(u8),
    /// Exchange entries `i` and `i + 1`.
    SwapNext(u8),
    /// Type the current code of an entry on the USB keyboard.
    Type(u8),
    /// The user's answer to a provisioning request.
    Provision(bool),
    Wipe,
}

/// `ServiceEvent::Other { kind: other::AUTH, value }` values.
pub mod auth_event {
    /// The vault's state changed (unlocked, created, locked, wiped…).
    pub const STATE: u32 = 1;
    pub const WRONG_PIN: u32 = 2;
    /// The entries changed (import, delete, move, provisioning, next code).
    pub const ENTRIES: u32 = 3;
    /// A request finished: see `AuthService::auth_outcome`.
    pub const OUTCOME: u32 = 4;
    /// A provisioning request waits for the user (`provision_pending`).
    pub const PROVISION: u32 = 5;
}

/// The result of the last request that has one.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum AuthOutcome {
    #[default]
    None,
    Imported {
        report: ImportReport,
        /// The file was wiped and deleted.
        deleted: bool,
    },
    NoImportFile,
    ImportTooLarge,
    /// Typing started; `Typed` follows.
    Typing,
    Typed,
    TypeFailed,
    NoComputer,
    /// HOTP without a code yet, or TOTP without a clock.
    NoCode,
    Added,
    AlreadyThere,
    PinChanged,
    /// The vault could not be written or read.
    Failed,
}

pub trait AuthService {
    fn vault(&self) -> VaultState;
    /// Seconds since 1970 UTC while the clock is valid (TOTP needs it).
    fn unix_time(&self) -> Option<u64>;
    /// Entry `i` of the unlocked vault (decrypted on demand).
    fn auth_entry(&mut self, i: usize) -> Option<AuthEntry>;
    /// The code of entry `i`: TOTP now, HOTP the one last made.
    fn auth_code(&mut self, i: usize) -> Option<Code>;
    fn auth_request(&mut self, r: AuthRequest);
    fn auth_outcome(&self) -> AuthOutcome;
    /// A provisioning request waiting for confirmation.
    fn provision_pending(&self) -> Option<AuthEntry>;
}

// ---------------------------------------------------------------- Recorder

pub use orpheus_rec::{Format as RecFormat, Quality as RecQuality, Source as RecSource};

/// What the recorder is doing (05 §7.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RecPhase {
    /// Not capturing.
    #[default]
    Off,
    /// Capturing for the meters (the app is open), not writing.
    Armed,
    Recording,
    Paused,
    /// Voice activation waits for sound (recording, the timer stopped).
    Waiting,
    /// The rest is written and the file closed.
    Saving,
}

impl RecPhase {
    /// A file is open.
    #[must_use]
    pub const fn is_recording(self) -> bool {
        matches!(self, RecPhase::Recording | RecPhase::Paused | RecPhase::Waiting | RecPhase::Saving)
    }
}

/// Whether a key press clicks (05 §3, Settings › Sound › Key click 0x0108):
/// the setting on, and the microphone not live (05 §7.5: "the key click is
/// forced off while the microphone is live": the recorder capturing from
/// it, for its meters or a recording).
#[must_use]
pub fn key_click(setting: i16, rec: &RecInfo) -> bool {
    setting != 0 && !(rec.phase != RecPhase::Off && rec.source == RecSource::Mic)
}

/// Whether a held key clicks again when it reaches its hold delay (A11Y-2,
/// 05 §7.14 "key-press confirmation sound"): Key click = Presses and holds
/// (2), under the same microphone rule as [`key_click`].
#[must_use]
pub fn hold_click(setting: i16, rec: &RecInfo) -> bool {
    setting == 2 && key_click(setting, rec)
}

/// Why a recording ended by itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecEnd {
    /// Saved: stopped by the user (or the time limit).
    Saved,
    /// The card had 1 MiB left; the file is saved.
    CardFull,
    /// Writing failed (card removed?).
    CardError,
    /// No card, or the folder could not be made.
    NoCard,
    /// The source is not there (no tuner, no line input).
    NoSource,
}

/// The recorder's state, by value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct RecInfo {
    pub phase: RecPhase,
    pub source: RecSource,
    pub format: RecFormat,
    pub rate: u32,
    pub channels: u8,
    /// Length of the file so far.
    pub elapsed_ms: u32,
    /// Meters: bar and held peak per channel in dBFS (−60…0), clip.
    pub bar: [i8; 2],
    pub hold: [i8; 2],
    pub clip: bool,
    /// Gain shown in dB: manual, the input gain; automatic, the AGC's.
    pub gain_db: i8,
    pub auto_gain: bool,
    /// Recording time left on the card (seconds; `None` unknown).
    pub left_s: Option<u32>,
    /// Blocks the card was too slow for (dropped, marked in the file).
    pub drops: u32,
    pub marks: u16,
    /// The station when recording the radio.
    pub fm_khz: u32,
    /// How the last recording ended (until the next starts).
    pub ended: Option<RecEnd>,
}

/// Requests from the Recorder app.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum RecRequest {
    /// Capture for the meters with the current settings (the app is
    /// shown); again after a setting changed.
    Arm,
    /// Stop capturing (the app is left).
    Disarm,
    /// Start a recording with the current settings.
    Start,
    Pause,
    Resume,
    /// Stop and save.
    Stop,
    /// A mark here (a new file if Auto split… is on mark: not built).
    Mark,
    /// Manual input gain one step up or down.
    Gain(i8),
}

/// `ServiceEvent::Other { kind: other::RECORDER, value }` values.
pub mod rec_event {
    /// [`super::RecInfo`] changed (phase, levels, time).
    pub const STATE: u32 = 1;
    /// A recording ended ([`super::RecInfo::ended`] says how).
    pub const ENDED: u32 = 2;
}

/// The recorder (05 §7.5, firmware card task in recording mode).
pub trait RecorderService {
    fn recorder(&self) -> RecInfo;
    fn rec_request(&mut self, r: RecRequest);
    /// The current or last recording's file name (`""` none yet).
    fn rec_name(&self) -> &str;
    /// The recordings folder, if there is one.
    fn rec_dir(&mut self) -> Option<DirId>;
}

// ---------------------------------------------------------------- Update

/// Where Settings › System › Update firmware stands (UPD-1, 02 §2.5).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UpdPhase {
    #[default]
    Idle,
    /// Looking for `.ofw` files on the card.
    Searching,
    /// The search ended; [`UpdateInfo::files`] were found.
    Listed,
    /// Reading the whole file to check its SHA-256 (nothing written yet).
    Checking,
    /// Writing the inactive firmware slot.
    Writing,
    /// Reading it back.
    Verifying,
    /// Installed: the player restarts into it.
    Done,
    /// Stopped ([`UpdateInfo::error`]); the installed firmware is unchanged.
    Failed,
}

/// Why an update stopped, as the UI words it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum UpdError {
    #[default]
    None,
    /// The file fails a check (header, SHA-256, sizes).
    Damaged,
    /// Built for another layout, or the player runs the dual-boot layout.
    WrongPlayer,
    /// Below 30 % without the cable.
    Battery,
    /// The card could not be read.
    Card,
    /// Erase, program or read-back failed.
    Write,
    /// An update over USB is running.
    Busy,
}

impl From<orpheus_update::Error> for UpdError {
    fn from(e: orpheus_update::Error) -> UpdError {
        use orpheus_update::Error as E;
        match e {
            E::Layout | E::NotFull | E::NoSlot | E::Size => UpdError::WrongPlayer,
            E::Battery => UpdError::Battery,
            E::Busy => UpdError::Busy,
            E::Refused | E::Flash | E::Verify => UpdError::Write,
            E::Io => UpdError::Card,
            _ => UpdError::Damaged,
        }
    }
}

/// The update service's state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct UpdateInfo {
    pub phase: UpdPhase,
    /// Files found by the last search (at most [`UPDATE_FILES`]).
    pub files: u8,
    /// Progress of [`UpdPhase::Checking`], or of writing and verifying
    /// together, in per mille.
    pub permille: u16,
    pub error: UpdError,
    /// Bumped on every change (the screen redraws when it moves).
    pub seq: u32,
}

/// Most update files a search lists.
pub const UPDATE_FILES: usize = 3;

/// One update file found on the card.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct UpdateFile<'a> {
    /// File name (no folder).
    pub name: &'a str,
    pub version: &'a str,
    /// Image bytes.
    pub size: u32,
    /// The running firmware is this build.
    pub installed: bool,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UpdateRequest {
    /// Look for `/orpheus-*.ofw` and `/.orpheus/update/*.ofw`.
    Scan,
    /// Load the change list of file `i`.
    Details(u8),
    /// Check, write and verify file `i`, then switch slots.
    Install(u8),
    /// Stop (only before writing starts; afterwards finishing is safer).
    Cancel,
}

/// Settings › System › Update firmware.
pub trait UpdateService {
    fn update(&self) -> UpdateInfo;
    fn update_file(&self, i: usize) -> Option<UpdateFile<'_>>;
    /// The change list of the file last asked for with
    /// [`UpdateRequest::Details`] (`""` until it arrived or if it has none).
    fn update_changes(&self) -> &str;
    fn update_request(&mut self, r: UpdateRequest);
}

/// Settings › Recording › Source as a source.
#[must_use]
pub const fn rec_source(v: i16) -> RecSource {
    // Value 2 (Line-in) was retired 2026-11-27: no line input on the
    // board; the developer command can still try `rec start line`.
    match v {
        1 => RecSource::Fm,
        _ => RecSource::Mic,
    }
}

/// What Settings › Recording asks for, as the recorder's settings (both
/// service implementations start recordings from this).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct RecSettings {
    pub source: RecSource,
    pub params: orpheus_rec::Params,
    pub chain: orpheus_rec::chain::ChainConfig,
    /// Manual input gain, dB from the source's default (−12…12).
    pub gain_db: i8,
    pub split_min: u32,
    pub monitor: bool,
}

/// Read the recorder's settings.
pub fn rec_settings<S: SettingsService + ?Sized>(s: &S) -> RecSettings {
    let source = rec_source(s.setting(id::REC_SOURCE));
    let format = RecFormat::from_index(s.setting(id::REC_FORMAT));
    let rate = RecQuality::from_index(s.setting(id::REC_QUALITY)).rate();
    let vox = orpheus_rec::vox::THRESHOLDS_DB.get(usize::try_from(s.setting(id::REC_VOX)).unwrap_or(0));
    RecSettings {
        source,
        params: orpheus_rec::Params { rate, channels: source.channels(), format, level: 1 },
        chain: orpheus_rec::chain::ChainConfig {
            lowcut: s.setting(id::REC_LOWCUT) != 0,
            agc: s.setting(id::REC_GAIN_MODE) != 0,
            vox_db: vox.copied().unwrap_or(0),
        },
        gain_db: i8::try_from(s.setting(id::REC_GAIN).clamp(-12, 12)).unwrap_or(0),
        split_min: [0, 15, 30, 60]
            .get(usize::try_from(s.setting(id::REC_SPLIT)).unwrap_or(0))
            .copied()
            .unwrap_or(0),
        monitor: s.setting(id::REC_MONITOR) != 0,
    }
}

/// "5 h 12 min", "47 min" (recording time left).
pub fn push_left<const N: usize>(secs: u32, lang: Lang, out: &mut StrBuf<N>) {
    let h = secs / 3600;
    let m = secs / 60 % 60;
    if h > 0 {
        out.push_u32(h);
        out.push_str(" h ");
    }
    out.push_u32(m);
    out.push(' ');
    out.push_str(lang.get(crate::strings::Str::Mins));
}

// --------------------------------------------------------------- USB Drive

pub use orpheus_usb::msc::image::{Medium as DriveMedium, Refusal as DriveRefusal};

/// What the USB Drive export is doing (05 §7.10, 06 §3).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum DrivePhase {
    /// Nothing presented.
    #[default]
    Off,
    /// The file is being mapped; the card goes to USB.
    Preparing,
    /// The image is presented ([`DriveInfo::host`]: a computer uses it).
    Attached,
    /// Refused or failed ([`DriveInfo::refusal`] says why); the card
    /// stayed with the player.
    Failed,
}

impl DrivePhase {
    /// The card is (or is about to be) with the computer.
    #[must_use]
    pub const fn is_active(self) -> bool {
        matches!(self, DrivePhase::Preparing | DrivePhase::Attached)
    }
}

/// The export, by value.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct DriveInfo {
    pub phase: DrivePhase,
    pub medium: DriveMedium,
    pub read_only: bool,
    /// The image file's size.
    pub size: u64,
    pub bytes_read: u64,
    pub bytes_written: u64,
    /// A computer has configured the device.
    pub host: bool,
    pub refusal: Option<DriveRefusal>,
    /// Runs of the file on the card (diagnostics).
    pub runs: u16,
}

/// An image to present: a file of the current listing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DriveRequest {
    pub file: FileRef,
    pub medium: DriveMedium,
    /// Let the computer write (honoured only for a disk image in the test
    /// folder in test mode; the service checks again).
    pub write: bool,
}

/// `ServiceEvent::Other { kind: other::USB_DRIVE, value }` values.
pub mod drive_event {
    /// [`super::DriveInfo`] changed (phase, host, counters).
    pub const STATE: u32 = 1;
}

/// The USB Drive app's back end (firmware: card task + USB task).
pub trait UsbDriveService {
    /// `/Images` and `/.orpheus/images` (the test images), where present.
    fn drive_dirs(&mut self) -> (Option<DirId>, Option<DirId>);
    /// Test mode: a disk image in the test folder may be presented
    /// writable (CLAUDE.md rule 5).
    fn drive_test_mode(&self) -> bool;
    /// Present the image; the card is the computer's until [`Self::drive_stop`].
    fn drive_start(&mut self, req: DriveRequest, name: &str);
    /// Detach and give the card back to the player.
    fn drive_stop(&mut self);
    fn drive(&self) -> DriveInfo;
    /// The presented (or last) image's file name.
    fn drive_name(&self) -> &str;
}

/// The Games app's service (M14) with the `games` feature; without it, a
/// part every type has.
#[cfg(feature = "games")]
pub use crate::games::GamesService as GamesPart;
#[cfg(not(feature = "games"))]
pub trait GamesPart {}
#[cfg(not(feature = "games"))]
impl<T: ?Sized> GamesPart for T {}

/// The Books app's service (M15) with the `books` feature; without it, a
/// part every type has.
#[cfg(feature = "books")]
pub use crate::books::BooksService as BooksPart;
#[cfg(not(feature = "books"))]
pub trait BooksPart {}
#[cfg(not(feature = "books"))]
impl<T: ?Sized> BooksPart for T {}

/// The Sequencer's service (M17) with the `seq` feature; without it, a
/// part every type has.
#[cfg(feature = "seq")]
pub use crate::seq::SeqService as SeqPart;
#[cfg(not(feature = "seq"))]
pub trait SeqPart {}
#[cfg(not(feature = "seq"))]
impl<T: ?Sized> SeqPart for T {}

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
    + RadioService
    + SystemService
    + StepsService
    + ToolsService
    + AuthService
    + RecorderService
    + UsbDriveService
    + UpdateService
    + GamesPart
    + BooksPart
    + SeqPart
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
        + RadioService
        + SystemService
        + StepsService
        + ToolsService
        + AuthService
        + RecorderService
        + UsbDriveService
        + UpdateService
        + GamesPart
        + BooksPart
        + SeqPart
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
        + ToolsService
        + ?Sized,
{
    let b = s.battery();
    let tools = s.tools();
    // The bell: an alarm set, a timer running or an alarm snoozed; the dot:
    // the stopwatch running (05 §7.8: they run in the background).
    let alarm = tools.timer.running()
        || tools.snoozed()
        || (0..crate::tools::ALARMS).any(|i| crate::tools::alarm(s, i).on);
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
        alarm,
        activity: tools.stopwatch.running(),
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
    let theme = Theme::from_setting(s.setting(id::THEME));
    let accent = match s.setting(id::ACCENT) {
        1 => Accent::Green,
        2 => Accent::Pink,
        3 => Accent::Red,
        4 => Accent::Grey,
        _ => Accent::Blue,
    };
    theme.palette(accent)
}

/// Settings › Accessibility › Text size (A11Y-2): what
/// `orpheus_ui::shell::System::text_size` answers.
pub fn text_size<S: SettingsService + ?Sized>(s: &S) -> orpheus_ui::font::TextSize {
    orpheus_ui::font::TextSize::from_setting(s.setting(id::TEXT_SIZE))
}

/// Settings › Accessibility › Hold delay and Repeat speed (A11Y-2): the
/// key mapper's timing (`orpheus_ui::input::Timing::accessible`).
pub fn key_timing<S: SettingsService + ?Sized>(s: &S) -> orpheus_ui::input::Timing {
    let ms = usize::try_from(s.setting(id::HOLD_DELAY))
        .ok()
        .and_then(|i| crate::settings::HOLD_DELAY_MS.get(i))
        .copied()
        .unwrap_or(500);
    let repeat = u8::try_from(s.setting(id::REPEAT_SPEED)).unwrap_or(0);
    orpheus_ui::input::Timing::accessible(ms, repeat)
}

/// Settings › Accessibility › the screen reader (A11Y-1, 05 §7.14): what
/// the reader and the speech service follow
/// (`orpheus_ui::a11y::Config`).
#[cfg(feature = "screen-reader")]
#[must_use]
pub fn reader_config<S: SettingsService + ?Sized>(s: &S) -> orpheus_ui::a11y::Config {
    use orpheus_ui::a11y::{Config, Verbosity, WhereKeys};
    let pitch = usize::try_from(s.setting(id::READER_PITCH))
        .ok()
        .and_then(|i| crate::settings::READER_PITCH_SEMITONES.get(i))
        .copied()
        .unwrap_or(0);
    Config {
        on: s.setting(id::READER) != 0,
        rate: u8::try_from(s.setting(id::SPEECH_RATE)).unwrap_or(4).clamp(1, 40),
        voice: u8::try_from(s.setting(id::READER_VOICE)).unwrap_or(0),
        pitch,
        verbosity: Verbosity::from_index(s.setting(id::VERBOSITY)),
        earcons: s.setting(id::EARCONS) != 0,
        where_keys: WhereKeys::from_index(s.setting(id::WHERE_KEYS)),
        dark_keys: s.setting(id::DARK_KEYS) != 0,
        volume: u8::try_from(s.setting(id::SPEECH_VOLUME)).unwrap_or(0),
        music: u8::try_from(s.setting(id::MUSIC_WHILE_SPEAKING)).unwrap_or(0),
    }
}

/// Settings › Accessibility › Music while speaking › Paused (TTS-3, 05
/// §7.14 "music ducking or pausing while speaking"): the player paused
/// while the screen reader speaks over it and resumed once it has been
/// quiet for [`SpeechPause::QUIET_MS`]. The firmware's UI task and the
/// simulator run it every loop with what the speech service says.
///
/// Only music the reader paused is resumed. Music the listener starts or
/// resumes (a song chosen, OK on Now Playing) is never paused for the
/// speech that answers that key: it plays under the speech until the
/// speech is quiet, and the next utterance may pause it again.
#[cfg(feature = "screen-reader")]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct SpeechPause {
    state: PauseState,
    /// The player's source at the last step.
    last: Option<Source>,
    /// When the speech was last heard (ms).
    heard_ms: u64,
    /// When the pause was asked for, and whether the player has shown it
    /// (it pauses after its fade: until then it still says Playing).
    asked_ms: u64,
    seen_paused: bool,
}

#[cfg(feature = "screen-reader")]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
enum PauseState {
    #[default]
    Idle,
    /// The reader paused the music.
    Holding,
    /// The listener started the music during speech: left playing until
    /// the speech is quiet.
    Overridden,
}

#[cfg(feature = "screen-reader")]
impl SpeechPause {
    /// Quiet this long before the music resumes (the reader's utterances
    /// follow each other with short gaps).
    pub const QUIET_MS: u64 = 400;
    /// The player answers a pause within this (its fade is 60 ms).
    pub const ASK_MS: u64 = 1500;

    #[must_use]
    pub const fn new() -> SpeechPause {
        SpeechPause { state: PauseState::Idle, last: None, heard_ms: 0, asked_ms: 0, seen_paused: false }
    }

    /// The reader holds the music paused.
    #[must_use]
    pub fn holding(&self) -> bool {
        self.state == PauseState::Holding
    }

    /// One step: `pause` (the reader on and Music while speaking Paused),
    /// `speaking` (speech sounds or waits), `source` (the player's). The
    /// command to send, if any (always [`PlayerCmd::PlayPause`]).
    pub fn step(
        &mut self,
        pause: bool,
        speaking: bool,
        source: Option<Source>,
        now_ms: u64,
    ) -> Option<PlayerCmd> {
        let src = source.unwrap_or(Source::Idle);
        let last = self.last.replace(src);
        if speaking {
            self.heard_ms = now_ms;
        }
        let quiet = !speaking && now_ms.saturating_sub(self.heard_ms) >= Self::QUIET_MS;
        // Music the listener started or resumed (not by this policy).
        let started = src == Source::Playing && last != Some(Source::Playing) && last.is_some();
        match self.state {
            PauseState::Idle if !pause => None,
            PauseState::Idle if started && speaking => {
                self.state = PauseState::Overridden;
                None
            }
            PauseState::Idle if speaking && src == Source::Playing => {
                self.state = PauseState::Holding;
                self.asked_ms = now_ms;
                self.seen_paused = false;
                Some(PlayerCmd::PlayPause)
            }
            PauseState::Idle => None,
            PauseState::Holding => match src {
                // Not paused yet (the fade, the card task's answer); after
                // `ASK_MS` the pause is taken as lost.
                Source::Playing if !self.seen_paused => {
                    if now_ms.saturating_sub(self.asked_ms) >= Self::ASK_MS {
                        self.state = PauseState::Overridden;
                    }
                    None
                }
                // Resumed by the listener: theirs from now on.
                Source::Playing => {
                    self.state = PauseState::Overridden;
                    None
                }
                Source::Paused if !self.seen_paused && !(quiet || !pause) => {
                    self.seen_paused = true;
                    None
                }
                Source::Paused if quiet || !pause => {
                    self.state = PauseState::Idle;
                    Some(PlayerCmd::PlayPause)
                }
                Source::Paused => None,
                // Stopped, another source: nothing to resume.
                _ => {
                    self.state = PauseState::Idle;
                    None
                }
            },
            PauseState::Overridden => {
                if quiet || !pause {
                    self.state = PauseState::Idle;
                }
                None
            }
        }
    }
}

/// Settings → Display → List roll-around (UI-8): what
/// `orpheus_ui::shell::System::list_roll_around` answers, so every list
/// follows the one stored setting.
pub fn list_roll_around<S: SettingsService + ?Sized>(s: &S) -> bool {
    s.setting(id::LIST_ROLL) != 0
}

// ------------------------------------------------ Display, Power, USB (VER-13)

/// Settings › Display › Brightness (0x030D) on the backlight: `None` the
/// panel dark, else the stock duty of the level (U-PWM-3). `lit`: the
/// screen on or the screen saver on the panel; `ringing`: an alert rings,
/// which flashes the backlight (bright, the stock maximum 15, and dim
/// every 500 ms; the level does not matter then). The firmware's UI task
/// and the simulator both take the backlight from here.
pub fn backlight<S: SettingsService + ?Sized>(
    s: &S,
    lit: bool,
    ringing: bool,
    now_ms: u64,
) -> Option<orpheus_power::backlight::Duty> {
    use orpheus_power::backlight as bl;
    let flash = if ringing { Some(bl::flash_phase(now_ms)) } else { None };
    bl::light(s.setting(id::BRIGHTNESS), lit, flash)
}

/// Settings › Display › Backlight timeout (0x0302), Lock delay (0x0303)
/// and Screen saver (0x0307) as the screen timers take them
/// (`orpheus_ui::input::ScreenTimer`, 05 §6.3).
pub fn screen_timeouts<S: SettingsService + ?Sized>(s: &S) -> orpheus_ui::input::Timeouts {
    const OFF_S: [u32; 6] = [5, 10, 15, 30, 60, 120];
    const LOCK_S: [u32; 4] = [0, 5, 30, 60];
    let pick = |v: i16, t: &[u32]| usize::try_from(v).ok().and_then(|i| t.get(i)).map(|s| s * 1000);
    orpheus_ui::input::Timeouts {
        screen_off_ms: pick(s.setting(id::BACKLIGHT), &OFF_S),
        lock_ms: pick(s.setting(id::LOCK_DELAY), &LOCK_S),
        saver: s.setting(id::SCREEN_SAVER) == 1,
    }
}

/// A host test's hold on the screen timers (VER-13). Under the firmware's
/// test look (`ui test-look on`: the suite, the fuzzers, the soak), and in
/// the simulator standing in for the player, the backlight timeout and the
/// lock delay do not run: a test's keys must never be swallowed by a
/// screen that went off, nor the player left locked (CLAUDE.md rule 4). A
/// test session that sets one of them, or the screen saver, to another
/// value than its default (`settings test-set`) runs the timers with the
/// session's values; setting it back to its default holds them again.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TimerHold {
    armed: u8,
}

impl TimerHold {
    #[must_use]
    pub const fn from_bits(armed: u8) -> TimerHold {
        TimerHold { armed }
    }

    #[must_use]
    pub const fn bits(self) -> u8 {
        self.armed
    }

    /// `settings test-set` of `sid` to `value` (other settings: no change).
    pub fn test_set(&mut self, sid: SettingId, value: i16) {
        let bit = match sid {
            id::BACKLIGHT => 1,
            id::LOCK_DELAY => 2,
            id::SCREEN_SAVER => 4,
            _ => return,
        };
        if crate::settings::def(sid).is_some_and(|d| d.default == value) {
            self.armed &= !bit;
        } else {
            self.armed |= bit;
        }
    }

    /// The timers under the hold: none unless a session armed them.
    pub fn timeouts<S: SettingsService + ?Sized>(self, s: &S) -> orpheus_ui::input::Timeouts {
        if self.armed == 0 { orpheus_ui::input::Timeouts::HELD } else { screen_timeouts(s) }
    }
}

/// Settings › Power › Keys when locked (0x0404) for the key gate.
pub fn locked_keys<S: SettingsService + ?Sized>(s: &S) -> orpheus_ui::input::LockedKeys {
    orpheus_ui::input::KeyGate::locked_keys(s.setting(id::KEYS_LOCKED))
}

/// Settings › Display › Rotate (0x0309): the panel turned by 180° (05 §4
/// "Rotation": the picture and Up/Down, Prev/Next).
pub fn rotated<S: SettingsService + ?Sized>(s: &S) -> bool {
    s.setting(id::ROTATE) != 0
}

/// Settings › Display › Animations (0x0308), off in Battery saver (02
/// §5.4): what `orpheus_ui::shell::System::animations` answers.
pub fn animations<S: SettingsService + ?Sized>(s: &S) -> bool {
    s.setting(id::ANIMATIONS) != 0 && power_mode(s) != orpheus_power::governor::PowerMode::Saver
}

/// Settings › Power › Power mode (0x0401) for the clock governor.
pub fn power_mode<S: SettingsService + ?Sized>(s: &S) -> orpheus_power::governor::PowerMode {
    orpheus_power::governor::PowerMode::from_setting(s.setting(id::POWER_MODE))
}

/// The screen saver's clock (`orpheus_ui::shell::System::saver_clock`):
/// hours as the 24-hour setting wants them, minutes; none while the clock
/// is not set.
pub fn saver_clock<S: SettingsService + ClockService + ?Sized>(s: &S) -> Option<(u8, u8)> {
    s.clock_valid().then(|| {
        let t = s.now();
        let h = if s.setting(id::CLOCK_24H) != 0 { t.hour } else { (t.hour + 11) % 12 + 1 };
        (h, t.minute)
    })
}

/// A transport key while the keys are locked or the screen is off, with
/// Keys when locked = Volume and transport (05 §3): a tap of OK plays or
/// pauses, Next and Prev change the track. Only while something is
/// queued; holds and repeats do nothing.
pub fn locked_transport<S: PlayerService + ?Sized>(
    s: &mut S,
    k: orpheus_ui::event::Key,
    p: orpheus_ui::event::KeyPhase,
) {
    use orpheus_ui::event::{Key, KeyPhase};
    if p != (KeyPhase::Up { held: false }) || s.player().source == Source::Idle {
        return;
    }
    match k {
        Key::Ok => s.command(PlayerCmd::PlayPause),
        Key::Next => s.command(PlayerCmd::Next),
        Key::Prev => s.command(PlayerCmd::Prev),
        _ => {}
    }
}

/// Settings › Date & time › Time zone (0x0505), in seconds east of UTC.
pub fn utc_offset_s<S: SettingsService + ?Sized>(s: &S) -> i32 {
    i32::from(s.setting(id::UTC_OFFSET)) * 60
}

/// The player's clock keeps local time (what every screen shows); UTC
/// (TOTP codes, 06 §5.3) is that less the UTC offset.
#[must_use]
pub fn utc_of_local(local_unix: u64, offset_s: i32) -> u64 {
    local_unix.saturating_add_signed(-i64::from(offset_s))
}

/// The local date and time of `unix` seconds (the inverse of [`unix_of`];
/// weekday 0 = Monday).
#[must_use]
pub fn date_of(unix: u64) -> DateTime {
    let days = i64::try_from(unix / 86_400).unwrap_or(0);
    let secs = unix % 86_400;
    // Days to the civil date (proleptic Gregorian, eras of 400 years).
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    DateTime {
        year: u16::try_from(year).unwrap_or(0),
        month: month as u8,
        day: day as u8,
        hour: (secs / 3600) as u8,
        minute: ((secs / 60) % 60) as u8,
        second: (secs % 60) as u8,
        weekday: ((days + 3).rem_euclid(7)) as u8,
    }
}

/// Settings › Date & time › Set from RDS (0x0507, 06 §5.3): the local time
/// to set the clock to from a station's clock time (`utc` seconds, RDS
/// group 4A: `orpheus_fm::rds::Rds::clock`), with the Time zone setting;
/// `None` while the setting is off.
pub fn rds_clock<S: SettingsService + ?Sized>(s: &S, utc: u64) -> Option<DateTime> {
    (s.setting(id::SET_FROM_RDS) != 0).then(|| date_of(utc.saturating_add_signed(i64::from(utc_offset_s(s)))))
}

/// Settings › USB › Read-only card (0x0902): the card reader presents the
/// card write-protected (SCSI MODE SENSE WP, writes refused:
/// `orpheus_usb::msc::scsi::Unit::read_only`).
pub fn usb_card_read_only<S: SettingsService + ?Sized>(s: &S) -> bool {
    s.setting(id::READ_ONLY_CARD) != 0
}

/// Settings › USB › Developer channel (0x0903) as offered on USB: on unless
/// the setting is off and nothing holds it on (`held`: a host test or a
/// test session is running, which needs the channel; the RAM-only session
/// never turns it off).
pub fn developer_channel<S: SettingsService + ?Sized>(s: &S, held: bool) -> bool {
    held || s.setting(id::DEV_CHANNEL) != 0
}

/// Settings › Date & time › Correction (0x0506, seconds per day; 06 §5.3):
/// the whole seconds to add to the clock as time passes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct ClockDrift {
    /// Milliseconds × seconds per day not yet applied.
    acc: i64,
}

impl ClockDrift {
    const DAY_MS: i64 = 86_400_000;

    #[must_use]
    pub const fn new() -> ClockDrift {
        ClockDrift { acc: 0 }
    }

    /// `elapsed_ms` of running at `per_day` seconds a day: the seconds to
    /// add to the clock now (negative: hold it back).
    pub fn step(&mut self, elapsed_ms: u64, per_day: i16) -> i32 {
        let ms = i64::try_from(elapsed_ms.min(Self::DAY_MS as u64)).unwrap_or(0);
        self.acc += ms * i64::from(per_day);
        let whole = self.acc / Self::DAY_MS;
        self.acc -= whole * Self::DAY_MS;
        i32::try_from(whole).unwrap_or(0)
    }
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
    /// Bluetooth: `value` is one of [`super::bt_event`].
    pub const BLUETOOTH: u16 = 2;
    /// Radio: `value` is one of [`super::radio_event`].
    pub const RADIO: u16 = 3;
    /// Steps: today's count changed (`value`).
    pub const STEPS: u16 = 4;
    /// Stopwatch, timer, alarms: `value` is one of [`super::tools_event`].
    pub const TOOLS: u16 = 5;
    /// Authenticator: `value` is one of [`super::auth_event`].
    pub const AUTH: u16 = 6;
    /// Recorder: `value` is one of [`super::rec_event`].
    pub const RECORDER: u16 = 8;
    /// USB Drive: `value` is one of [`super::drive_event`].
    pub const USB_DRIVE: u16 = 7;
    /// Games: a card operation was answered (the hub changed: a save file
    /// was read, the scores arrived).
    pub const GAMES: u16 = 9;
    /// Books: card work changed the library or the open book.
    pub const BOOKS: u16 = 10;
    /// Sequencer: a card request was answered (the list, the open project).
    pub const SEQ: u16 = 11;
    /// Settings › Storage: card work ended (`StorageService::storage_outcome`).
    pub const STORAGE: u16 = 12;
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

/// The bit rate Now Playing › Info shows (kbit/s, rounded): the decoder's
/// average when it reports one (MP3 frame header or Xing/VBRI tag, FLAC
/// stream info, …), else the file's size over its duration (tags
/// included: a little high for files with large embedded art); 0 when
/// neither is known.
#[must_use]
pub fn average_kbps(reported_bps: u32, size: u64, duration_ms: u32) -> u16 {
    let bps = if reported_bps > 0 {
        u64::from(reported_bps)
    } else if duration_ms > 0 {
        size.saturating_mul(8000) / u64::from(duration_ms)
    } else {
        0
    };
    u16::try_from(bps.saturating_add(500) / 1000).unwrap_or(u16::MAX)
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

/// The longest play queue: Play next and Add to queue do nothing beyond
/// it (the screens say "The queue is full").
pub const QUEUE_MAX: u32 = 256;

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
    /// The folder Pictures opens at (05 §7.6): the first of
    /// [`crate::browse::PICTURE_DIRS`] on the card, [`DirId::ROOT`] when
    /// there is none. The firmware looks them up when the card mounts and
    /// when it comes back from the computer or a format.
    fn pictures_dir(&mut self) -> DirId;
}

#[cfg(test)]
mod bt_tests {
    use super::*;

    /// Settings › Sound › Key click (VER-13): off no click; on a click,
    /// except while the microphone is live (armed or recording), not while
    /// the radio or the line input is recorded.
    #[test]
    fn key_click_follows_the_setting_and_the_microphone() {
        let rec = |phase, source| RecInfo { phase, source, ..RecInfo::default() };
        let idle = RecInfo::default();
        assert!(!key_click(0, &idle));
        assert!(key_click(1, &idle));
        for phase in
            [RecPhase::Armed, RecPhase::Recording, RecPhase::Paused, RecPhase::Waiting, RecPhase::Saving]
        {
            assert!(!key_click(1, &rec(phase, RecSource::Mic)), "{phase:?} from the microphone");
            assert!(key_click(1, &rec(phase, RecSource::Fm)), "{phase:?} from the radio");
            assert!(key_click(1, &rec(phase, RecSource::Line)), "{phase:?} from the line input");
            assert!(!key_click(0, &rec(phase, RecSource::Fm)));
        }
    }

    #[test]
    fn unix_time_of_dates() {
        let d = |year, month, day, hour, minute, second| DateTime {
            year,
            month,
            day,
            hour,
            minute,
            second,
            weekday: 0,
        };
        assert_eq!(unix_of(&d(1970, 1, 1, 0, 0, 0)), 0);
        assert_eq!(unix_of(&d(2000, 1, 1, 0, 0, 0)), 946_684_800);
        assert_eq!(unix_of(&d(2024, 2, 29, 23, 59, 59)), 1_709_251_199);
        assert_eq!(unix_of(&d(2026, 11, 24, 12, 41, 0)), 1_795_524_060);
    }

    /// Regression: Now Playing › Info said "Bitrate 0 kbit/s" on the player
    /// (AU-5, 2026-11-24: the firmware always passed 0); `average_kbps`
    /// falls back to the file's size over its duration.
    #[test]
    fn bitrate_shown() {
        // The decoder's figure first.
        assert_eq!(average_kbps(192_000, 5_000_000, 200_000), 192);
        assert_eq!(average_kbps(127_600, 0, 0), 128);
        // Otherwise size over duration: 4.8 MB in 200 s = 192 kbit/s.
        assert_eq!(average_kbps(0, 4_800_000, 200_000), 192);
        // 48 kHz VBR MP3 without a tag: 3 MB over 2:05.
        assert_eq!(average_kbps(0, 3_000_000, 125_000), 192);
        // Nothing known.
        assert_eq!(average_kbps(0, 4_800_000, 0), 0);
        assert_eq!(average_kbps(0, u64::MAX, 1), u16::MAX);
    }

    #[test]
    fn kind_from_class_of_device() {
        // Headphones (minor 6), wearable headset (1), loudspeaker (5), car
        // audio (8), phone, computer, uncategorised.
        assert_eq!(BtKind::from_cod([0x18, 0x04, 0x24]), BtKind::Headphones);
        assert_eq!(BtKind::from_cod([0x04, 0x04, 0x20]), BtKind::Headphones);
        assert_eq!(BtKind::from_cod([0x14, 0x04, 0x24]), BtKind::Speaker);
        assert_eq!(BtKind::from_cod([0x20, 0x04, 0x24]), BtKind::Speaker);
        assert_eq!(BtKind::from_cod([0x0C, 0x02, 0x5A]), BtKind::Phone);
        assert_eq!(BtKind::from_cod([0x0C, 0x01, 0x1C]), BtKind::Computer);
        assert_eq!(BtKind::from_cod([0x00, 0x1F, 0x00]), BtKind::Other);
        // The other byte order keeps the major class.
        assert!(matches!(BtKind::from_cod([0x24, 0x04, 0x18]), BtKind::Headphones | BtKind::Speaker));
    }

    #[test]
    fn device_names_are_cut_on_characters() {
        let a = BtAddr([0x00, 0xE0, 0x4C, 0x12, 0x34, 0xAB]);
        // Longer than 24 bytes; byte 24 falls inside a two-byte character.
        let n = "Наушники Соня ж".as_bytes();
        assert!(n.len() > BT_NAME);
        let d = BtDevice::new(a, BtKind::Unknown, n, false);
        assert_eq!(d.name(), "Наушники Сон");
        let mut t = StrBuf::<24>::new();
        BtDevice::new(a, BtKind::Unknown, b"", false).push_label(&mut t);
        assert_eq!(t.as_str(), "AB:34:12:4C:E0:00");
        let bad = BtDevice::new(a, BtKind::Unknown, b"ok\xFFno", false);
        assert_eq!(bad.name(), "ok");
    }
}

/// TTS-3: Music while speaking › Paused.
#[cfg(all(test, feature = "screen-reader"))]
mod speech_pause {
    use super::*;
    use orpheus_ui::event::Source::{Idle, Paused, Playing};

    #[test]
    fn pauses_while_speaking_and_resumes_when_quiet() {
        let mut p = SpeechPause::new();
        assert_eq!(p.step(true, false, Some(Playing), 0), None);
        assert_eq!(p.step(true, true, Some(Playing), 10), Some(PlayerCmd::PlayPause));
        assert!(p.holding());
        assert_eq!(p.step(true, true, Some(Paused), 500), None);
        // Quiet, but not for long enough yet.
        assert_eq!(p.step(true, false, Some(Paused), 600), None);
        assert_eq!(p.step(true, false, Some(Paused), 899), None);
        assert_eq!(p.step(true, false, Some(Paused), 900), Some(PlayerCmd::PlayPause));
        assert!(!p.holding());
        assert_eq!(p.step(true, false, Some(Playing), 950), None);
    }

    #[test]
    fn music_the_listener_starts_is_not_paused_for_its_answer() {
        let mut p = SpeechPause::new();
        assert_eq!(p.step(true, false, Some(Paused), 0), None);
        // OK on Now Playing: playing again, the reader says so.
        assert_eq!(p.step(true, true, Some(Playing), 100), None);
        assert_eq!(p.step(true, true, Some(Playing), 600), None);
        assert_eq!(p.step(true, false, Some(Playing), 700), None);
        // Quiet: the next utterance pauses it.
        assert_eq!(p.step(true, false, Some(Playing), 1200), None);
        assert_eq!(p.step(true, true, Some(Playing), 1300), Some(PlayerCmd::PlayPause));
        // Still playing while the player fades: held.
        assert_eq!(p.step(true, true, Some(Playing), 1350), None);
        assert!(p.holding());
        assert_eq!(p.step(true, true, Some(Paused), 1400), None);
        // Resumed by the listener while held: left playing.
        assert_eq!(p.step(true, true, Some(Playing), 1450), None);
        assert!(!p.holding());
        assert_eq!(p.step(true, false, Some(Playing), 2000), None);
    }

    #[test]
    fn other_modes_and_stops_leave_the_player() {
        let mut p = SpeechPause::new();
        assert_eq!(p.step(false, true, Some(Playing), 0), None);
        assert_eq!(p.step(true, true, Some(Paused), 10), None);
        assert_eq!(p.step(true, true, None, 20), None);
        let mut p = SpeechPause::new();
        assert_eq!(p.step(true, false, Some(Playing), 0), None);
        assert_eq!(p.step(true, true, Some(Playing), 10), Some(PlayerCmd::PlayPause));
        // Stopped while held: nothing to resume.
        assert_eq!(p.step(true, false, Some(Idle), 2000), None);
        assert!(!p.holding());
        // A pause the player never carried out is given up.
        let mut p = SpeechPause::new();
        p.step(true, false, Some(Playing), 0);
        p.step(true, true, Some(Playing), 10);
        assert_eq!(p.step(true, true, Some(Playing), 1509), None);
        assert!(p.holding());
        assert_eq!(p.step(true, true, Some(Playing), 1510), None);
        assert!(!p.holding());
        // The setting changed while held: resumed at once.
        let mut p = SpeechPause::new();
        p.step(true, false, Some(Playing), 0);
        p.step(true, true, Some(Playing), 10);
        assert_eq!(p.step(false, true, Some(Paused), 20), Some(PlayerCmd::PlayPause));
    }
}

/// VER-13 (owner's "100 % setting → effect"): the Display, Power, Date &
/// time and USB settings as the code that applies them takes them.
#[cfg(test)]
mod setting_effects {
    extern crate std;
    use super::*;
    use crate::settings::SettingsCache;
    use orpheus_ui::input::{LockedKeys, Timeouts};

    struct S(SettingsCache);
    impl SettingsService for S {
        fn setting(&self, id: SettingId) -> i16 {
            self.0.get(id)
        }
        fn set_setting(&mut self, id: SettingId, v: i16) {
            self.0.set(id, v);
        }
        fn factory_reset(&mut self) {}
    }

    fn with(sets: &[(SettingId, i16)]) -> S {
        let mut s = S(SettingsCache::new());
        for &(k, v) in sets {
            assert!(s.0.set(k, v) || s.0.get(k) == v, "{k:?}={v}");
        }
        s
    }

    /// Backlight timeout (0x0302), Lock delay (0x0303), Screen saver
    /// (0x0307): every value; the defaults are 15 s, Immediately, Off.
    #[test]
    fn screen_timer_values() {
        let d = screen_timeouts(&with(&[]));
        assert_eq!(d, Timeouts { screen_off_ms: Some(15_000), lock_ms: Some(0), saver: false });
        let off = [Some(5_000), Some(10_000), Some(15_000), Some(30_000), Some(60_000), Some(120_000), None];
        for (v, want) in off.iter().enumerate() {
            let t = screen_timeouts(&with(&[(id::BACKLIGHT, v as i16)]));
            assert_eq!(t.screen_off_ms, *want, "backlight {v}");
            assert_eq!((t.lock_ms, t.saver), (d.lock_ms, d.saver), "only the backlight timeout changes");
        }
        let lock = [Some(0), Some(5_000), Some(30_000), Some(60_000), None];
        for (v, want) in lock.iter().enumerate() {
            let t = screen_timeouts(&with(&[(id::LOCK_DELAY, v as i16)]));
            assert_eq!(t.lock_ms, *want, "lock {v}");
            assert_eq!((t.screen_off_ms, t.saver), (d.screen_off_ms, d.saver));
        }
        assert!(screen_timeouts(&with(&[(id::SCREEN_SAVER, 1)])).saver);
    }

    /// Brightness (0x030D): every level lights the panel at its stock
    /// duty (1 → 3 … 10 → 13, 11 → 15; default 6 → 8); dark when the
    /// panel is off; an alert flashes 15 / 3 whatever the level. No other
    /// setting moves the backlight.
    #[test]
    fn brightness_values() {
        use orpheus_power::backlight::{Duty, LEVEL_DUTY};
        let duty = |s: &S, lit, ring, t| backlight(s, lit, ring, t).map(Duty::get);
        assert_eq!(duty(&with(&[]), true, false, 0), Some(8), "default level 6");
        for lvl in 1..=11i16 {
            let s = with(&[(id::BRIGHTNESS, lvl)]);
            assert_eq!(duty(&s, true, false, 0), Some(LEVEL_DUTY[lvl as usize]), "level {lvl}");
            assert_eq!(duty(&s, false, false, 0), None, "level {lvl}, panel dark");
            assert_eq!(duty(&s, true, true, 100), Some(15), "level {lvl}, alert bright");
            assert_eq!(duty(&s, true, true, 600), Some(3), "level {lvl}, alert dim");
        }
        for d in crate::settings::all().filter(|d| d.stored() && d.id != id::BRIGHTNESS) {
            let other = d.step(d.default, 1);
            let s = with(&[(d.id, other)]);
            assert_eq!(duty(&s, true, false, 0), Some(8), "{:#06x} moved the backlight", d.id.0);
        }
    }

    /// A host test holds the timers until its session sets one to another
    /// value than the default; the default holds them again.
    #[test]
    fn timer_hold() {
        let s = with(&[(id::BACKLIGHT, 0), (id::LOCK_DELAY, 1)]);
        let mut h = TimerHold::default();
        assert_eq!(h.timeouts(&s), Timeouts::HELD);
        h.test_set(id::THEME, 1);
        assert_eq!(h.timeouts(&s), Timeouts::HELD);
        h.test_set(id::BACKLIGHT, 0);
        assert_eq!(h.timeouts(&s), screen_timeouts(&s));
        h.test_set(id::LOCK_DELAY, 1);
        h.test_set(id::BACKLIGHT, 2);
        assert_eq!(h.timeouts(&s), screen_timeouts(&s), "Lock delay still armed");
        h.test_set(id::LOCK_DELAY, 0);
        assert_eq!(h.timeouts(&s), Timeouts::HELD);
        assert_eq!(TimerHold::from_bits(h.bits()), h);
    }

    /// Keys when locked (0x0404), Rotate (0x0309), Animations (0x0308) with
    /// Power mode (0x0401): Battery saver turns animations off.
    #[test]
    fn keys_rotation_animations_power_mode() {
        use orpheus_power::governor::PowerMode;
        assert_eq!(locked_keys(&with(&[])), LockedKeys::Volume);
        assert_eq!(locked_keys(&with(&[(id::KEYS_LOCKED, 0)])), LockedKeys::None);
        assert_eq!(locked_keys(&with(&[(id::KEYS_LOCKED, 2)])), LockedKeys::VolumeTransport);
        assert!(!rotated(&with(&[])));
        assert!(rotated(&with(&[(id::ROTATE, 1)])));
        assert!(animations(&with(&[])));
        assert!(!animations(&with(&[(id::ANIMATIONS, 0)])));
        assert_eq!(power_mode(&with(&[])), PowerMode::Auto);
        assert_eq!(power_mode(&with(&[(id::POWER_MODE, 1)])), PowerMode::Saver);
        assert_eq!(power_mode(&with(&[(id::POWER_MODE, 2)])), PowerMode::High);
        assert!(!animations(&with(&[(id::POWER_MODE, 1)])), "Battery saver: no animations");
        assert!(animations(&with(&[(id::POWER_MODE, 2)])));
    }

    /// Time zone (0x0505): the clock keeps local time; UTC is that less the
    /// offset (Berlin in winter: 13:00 local is 12:00 UTC).
    #[test]
    fn utc_offset() {
        assert_eq!(utc_offset_s(&with(&[])), 0);
        assert_eq!(utc_offset_s(&with(&[(id::UTC_OFFSET, 60)])), 3600);
        assert_eq!(utc_offset_s(&with(&[(id::UTC_OFFSET, -720)])), -43_200);
        assert_eq!(utc_offset_s(&with(&[(id::UTC_OFFSET, 840)])), 50_400);
        let local = 1_795_524_060; // 2026-11-24 12:41
        assert_eq!(utc_of_local(local, 0), local);
        assert_eq!(utc_of_local(local, 3600), local - 3600);
        assert_eq!(utc_of_local(local, -43_200), local + 43_200);
        assert_eq!(utc_of_local(10, 3600), 0, "never before 1970");
    }

    /// Set from RDS (0x0507): off by default (the clock is never set from
    /// the air); on, a station's clock time in the player's time zone.
    #[test]
    fn set_from_rds() {
        let utc = 1_795_595_400; // 2026-11-25 08:30 UTC, a Wednesday
        assert_eq!(rds_clock(&with(&[]), utc), None);
        let t = rds_clock(&with(&[(id::SET_FROM_RDS, 1)]), utc).unwrap();
        assert_eq!(
            (t.year, t.month, t.day, t.hour, t.minute, t.second, t.weekday),
            (2026, 11, 25, 8, 30, 0, 2)
        );
        let t = rds_clock(&with(&[(id::SET_FROM_RDS, 1), (id::UTC_OFFSET, 60)]), utc).unwrap();
        assert_eq!((t.hour, t.minute), (9, 30));
        let t = rds_clock(&with(&[(id::SET_FROM_RDS, 1), (id::UTC_OFFSET, -720)]), utc).unwrap();
        assert_eq!((t.day, t.hour), (24, 20));
        // date_of is unix_of's inverse.
        for u in [0u64, 946_684_800, 1_709_251_199, 1_795_524_060, 4_102_444_799] {
            assert_eq!(unix_of(&date_of(u)), u, "{u}");
        }
        assert_eq!(date_of(0).weekday, 3, "1970-01-01 was a Thursday");
    }

    /// Correction (0x0506): +60 s a day adds a second every 24 minutes,
    /// -60 holds one back, 0 never changes the clock; the remainder carries
    /// over between steps of any size.
    #[test]
    fn clock_drift_correction() {
        let run = |per_day: i16, step_ms: u64, total_ms: u64| {
            let mut d = ClockDrift::new();
            let mut n = 0i32;
            let mut t = 0;
            while t < total_ms {
                n += d.step(step_ms, per_day);
                t += step_ms;
            }
            n
        };
        assert_eq!(run(0, 1000, 86_400_000), 0);
        assert_eq!(run(60, 1000, 86_400_000), 60);
        assert_eq!(run(-60, 1000, 86_400_000), -60);
        assert_eq!(run(60, 1000, 1_440_000), 1, "one second after 24 minutes");
        assert_eq!(run(60, 1000, 1_439_000), 0);
        assert_eq!(run(1, 7_777, 86_400_000 * 3), 3);
        assert_eq!(run(-17, 250, 86_400_000), -17);
    }

    /// Read-only card (0x0902), Developer channel (0x0903): the developer
    /// channel stays on while a test holds it, whatever the setting.
    #[test]
    fn usb_settings() {
        assert!(!usb_card_read_only(&with(&[])));
        assert!(usb_card_read_only(&with(&[(id::READ_ONLY_CARD, 1)])));
        assert!(developer_channel(&with(&[]), false));
        assert!(!developer_channel(&with(&[(id::DEV_CHANNEL, 0)]), false));
        assert!(developer_channel(&with(&[(id::DEV_CHANNEL, 0)]), true), "a test holds it on");
    }
}
