//! Service traits the screens talk to (05 §5.1). All methods are
//! synchronous and cheap: the firmware's UI task answers them from state it
//! caches from the owning tasks, and turns the `&mut self` requests into
//! channel messages. Changes arrive at the screens as
//! `Event::Service(ServiceEvent::…)`.

use yoton_power::charger::ChargeState;
use yoton_ui::event::{BtState, Source};
use yoton_ui::shell::System;
use yoton_ui::theme::{Accent, Palette, Theme};
use yoton_ui::widgets::bars::{MiniPlayer, Status};

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
        clock: (s.setting(id::CLOCK_IN_BAR) != 0).then(|| {
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
