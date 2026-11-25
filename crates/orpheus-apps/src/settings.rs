//! The settings model (05 §7.11): every group and item with its store key,
//! kind, range and default, and the RAM cache the firmware keeps in front
//! of `orpheus-settings`.
//!
//! Every stored value is an `i16` (toggle 0/1, choice index, range value,
//! bit mask, or a two-letter language code) encoded as 2 bytes
//! little-endian under its `SettingId` (the `orpheus-settings` u16 key,
//! `0x0101..=0x0CFF`: group in the high byte). Decoding validates against
//! the item's kind and falls back to the default, so a corrupt or
//! out-of-range record never reaches the UI. Ids are never reused.

use crate::strings::{Lang, Str};
use orpheus_ui::text::StrBuf;

/// A setting's store key.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub struct SettingId(pub u16);

/// How a choice is shown.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Choice {
    S(Str),
    /// Fixed text that is not translated (format names, patterns).
    Lit(&'static str),
    /// A duration in seconds ("5 s", "1 min").
    Secs(u16),
    /// Minutes ("15 min").
    Mins(u16),
    /// Playback speed in hundredths ("1.25×").
    Speed(u16),
}

/// Units of a range.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Unit {
    None,
    Db,
    /// Balance: "L 3", "0", "R 3".
    Balance,
    /// Minutes east of UTC shown as "+01:00".
    UtcMinutes,
    /// Seconds per day.
    PerDay,
    /// Centimetres (stride length).
    Cm,
    /// Kilograms (weight).
    Kg,
    /// Metres per second (wind).
    MetresPerSecond,
}

/// Things a row does instead of holding a value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Action {
    SetTime,
    SetDate,
    RebuildLibrary,
    ClearCaches,
    CheckCard,
    FormatCard,
    UpdateFirmware,
    StartOriginal,
    FactoryReset,
    Recovery,
    /// The developer rows (hidden until unlocked in About; PWR-5).
    Developer,
}

/// Read-only screens and values.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Info {
    Battery,
    CardInfo,
    About,
    Diagnostics,
    DeviceName,
    RecFolder,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    Toggle,
    Choice(&'static [Choice]),
    Range {
        min: i16,
        max: i16,
        step: i16,
        unit: Unit,
    },
    Action(Action),
    Info(Info),
    /// Two-letter code packed as `(c0 << 8) | c1`; 0 = English.
    Language,
    /// Bit `n` set = app `n` shown on Home (order of [`crate::App::ALL`]).
    HomeApps,
}

#[derive(Clone, Copy, Debug)]
pub struct Setting {
    pub id: SettingId,
    pub label: Str,
    pub kind: Kind,
    pub default: i16,
    /// A small setting kept in bits of another stored item (a "word"):
    /// `(word key, first bit, bits)`; `None`: stored on its own. Packing
    /// keeps the settings cache within its 128 slots and its RAM (the
    /// artillery game's 21 settings take four words, GAME-4/5).
    pub pack: Option<(SettingId, u8, u8)>,
}

impl Setting {
    /// Whether this item stores a value of its own (packed items live in
    /// their word's).
    #[must_use]
    pub const fn stored(&self) -> bool {
        !matches!(self.kind, Kind::Action(_) | Kind::Info(_)) && self.pack.is_none()
    }

    /// The same item kept in bits `shift .. shift + bits` of `word`.
    #[must_use]
    pub const fn packed(self, word: u16, shift: u8, bits: u8) -> Setting {
        Setting { pack: Some((SettingId(word), shift, bits)), ..self }
    }

    /// `v` if valid for this item, else the default.
    #[must_use]
    pub fn validate(&self, v: i16) -> i16 {
        let ok = match self.kind {
            Kind::Toggle => v == 0 || v == 1,
            Kind::Choice(c) => usize::try_from(v).is_ok_and(|i| i < c.len()),
            Kind::Range { min, max, step, .. } => {
                v >= min && v <= max && (i32::from(v) - i32::from(min)) % i32::from(step.max(1)) == 0
            }
            Kind::HomeApps => (0..1 << crate::App::ALL.len()).contains(&i32::from(v)),
            Kind::Language => true,
            Kind::Action(_) | Kind::Info(_) => false,
        };
        if ok { v } else { self.default }
    }

    /// The next value when stepped by `dir` (±1) in place: choices wrap,
    /// ranges clamp, toggles flip.
    #[must_use]
    pub fn step(&self, v: i16, dir: i16) -> i16 {
        match self.kind {
            Kind::Toggle => 1 - v.clamp(0, 1),
            Kind::Choice(c) => {
                let n = i16::try_from(c.len()).unwrap_or(1).max(1);
                (v + dir).rem_euclid(n)
            }
            Kind::Range { min, max, step, .. } => (v + dir * step).clamp(min, max),
            _ => v,
        }
    }
}

macro_rules! ch {
    ($($c:expr),* $(,)?) => { &[$($c),*] };
}

use Choice::{Lit, Mins, S, Secs, Speed};

pub(crate) const fn t(id: u16, label: Str, default: i16) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Toggle, default, pack: None }
}
pub(crate) const fn c(id: u16, label: Str, choices: &'static [Choice], default: i16) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Choice(choices), default, pack: None }
}
pub(crate) const fn r(
    id: u16,
    label: Str,
    min: i16,
    max: i16,
    step: i16,
    unit: Unit,
    default: i16,
) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Range { min, max, step, unit }, default, pack: None }
}
const fn a(id: u16, label: Str, act: Action) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Action(act), default: 0, pack: None }
}
const fn i(id: u16, label: Str, info: Info) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Info(info), default: 0, pack: None }
}

pub const EQ_PRESETS: &[Choice] = ch![
    S(Str::EqFlat),
    S(Str::EqRock),
    S(Str::EqPop),
    S(Str::EqJazz),
    S(Str::EqClassical),
    S(Str::EqElectronic),
    S(Str::EqHipHop),
    S(Str::EqBassBoost),
    S(Str::EqVocal),
    S(Str::EqCustom1),
    S(Str::EqCustom2),
];
pub const REPEAT: &[Choice] = ch![S(Str::Off), S(Str::RepeatOne), S(Str::RepeatAll)];
pub const SLEEP: &[Choice] =
    ch![S(Str::Off), Mins(15), Mins(30), Mins(45), Mins(60), Mins(90), S(Str::EndOfTrack)];
pub const OUTPUT: &[Choice] = ch![S(Str::Headphones), S(Str::AppBluetooth)];

/// Store keys used by code outside the Settings screens.
pub mod id {
    use super::SettingId;
    pub const EQUALISER: SettingId = SettingId(0x0101);
    pub const VOLUME_LIMIT: SettingId = SettingId(0x0107);
    pub const KEY_CLICK: SettingId = SettingId(0x0108);
    pub const SHUFFLE: SettingId = SettingId(0x0201);
    pub const REPEAT: SettingId = SettingId(0x0202);
    pub const BRIGHTNESS: SettingId = SettingId(0x0301);
    pub const BACKLIGHT: SettingId = SettingId(0x0302);
    pub const THEME: SettingId = SettingId(0x0304);
    pub const ACCENT: SettingId = SettingId(0x0305);
    pub const ANIMATIONS: SettingId = SettingId(0x0308);
    pub const ROTATE: SettingId = SettingId(0x0309);
    pub const CLOCK_IN_BAR: SettingId = SettingId(0x030A);
    pub const HOME_APPS: SettingId = SettingId(0x030B);
    /// Settings › Display › List roll-around (UI-8): 1 (default) every
    /// list rolls round at its ends, 0 the ends clamp.
    pub const LIST_ROLL: SettingId = SettingId(0x030C);
    pub const POWER_MODE: SettingId = SettingId(0x0401);
    pub const SLEEP_TIMER: SettingId = SettingId(0x0402);
    pub const KEYS_LOCKED: SettingId = SettingId(0x0404);
    pub const CLOCK_24H: SettingId = SettingId(0x0503);
    pub const DATE_FORMAT: SettingId = SettingId(0x0504);
    pub const UTC_OFFSET: SettingId = SettingId(0x0505);
    pub const BLUETOOTH: SettingId = SettingId(0x0601);
    pub const BT_RECONNECT: SettingId = SettingId(0x0603);
    pub const OUTPUT: SettingId = SettingId(0x0604);
    pub const USB_WHEN_CONNECTED: SettingId = SettingId(0x0901);
    /// The computer's keyboard layout (`orpheus_hid::Layout` order).
    pub const KBD_LAYOUT: SettingId = SettingId(0x0904);
    pub const KEYPAD_DIGITS: SettingId = SettingId(0x0905);
    pub const ENTER_AFTER_CODE: SettingId = SettingId(0x0906);
    pub const LANGUAGE: SettingId = SettingId(0x0A01);
    pub const FILES_SORT: SettingId = SettingId(0x0D01);
    pub const SHOW_HIDDEN: SettingId = SettingId(0x0D02);
    pub const NP_VIEW: SettingId = SettingId(0x0D03);
    /// Custom 1 bands 0..5, then Custom 2 bands 0..5.
    pub const EQ_BAND0: SettingId = SettingId(0x0D10);
    pub const SPEED: SettingId = SettingId(0x0206);
    /// Settings › System › Performance (D-4): 0 Normal, 1 Maximum.
    pub const PERFORMANCE: SettingId = SettingId(0x0C07);
    /// Settings › Radio (UI-5a): region (`orpheus_fm::Region` order),
    /// forced mono, seek sensitivity (low, medium, high).
    pub const RADIO_REGION: SettingId = SettingId(0x0701);
    pub const FORCE_MONO: SettingId = SettingId(0x0702);
    pub const SEEK_SENSITIVITY: SettingId = SettingId(0x0703);
    /// The radio's last frequency, in 10 kHz (UI-5a).
    pub const RADIO_FREQ: SettingId = SettingId(0x0D50);
    /// Steps (05 §7.7, UI-5c): daily goal, stride (cm), weight (kg),
    /// units (0 km, 1 mi), count in background (0 off, 1 while on),
    /// sensitivity (0 low, 1 medium, 2 high).
    pub const STEP_GOAL: SettingId = SettingId(0x0D20);
    pub const STRIDE: SettingId = SettingId(0x0D21);
    pub const WEIGHT: SettingId = SettingId(0x0D22);
    pub const STEP_UNITS: SettingId = SettingId(0x0D23);
    pub const STEP_BACKGROUND: SettingId = SettingId(0x0D24);
    pub const STEP_SENSITIVITY: SettingId = SettingId(0x0D25);
    /// Recorder (05 §7.5, UI-5b): source (Microphone, FM radio, Line-in),
    /// format (WAV, WAV ADPCM, FLAC), quality (16 / 44.1 / 48 kHz), gain
    /// mode (manual, automatic), manual input gain (dB from the source's
    /// default), voice activation (off, −50, −40, −30 dBFS), auto split
    /// (off, 15, 30, 60 min), monitor, low-cut.
    pub const REC_SOURCE: SettingId = SettingId(0x0801);
    pub const REC_FORMAT: SettingId = SettingId(0x0802);
    pub const REC_QUALITY: SettingId = SettingId(0x0803);
    pub const REC_GAIN_MODE: SettingId = SettingId(0x0804);
    pub const REC_VOX: SettingId = SettingId(0x0806);
    pub const REC_SPLIT: SettingId = SettingId(0x0807);
    pub const REC_MONITOR: SettingId = SettingId(0x0808);
    pub const REC_LOWCUT: SettingId = SettingId(0x0809);
    pub const REC_GAIN: SettingId = SettingId(0x080A);
    /// Alarm `i` (05 §7.8, UI-5d): `ALARM0 + 3i` minutes after midnight,
    /// `+ 1` repeat days (bit 0 Monday … bit 6 Sunday, 0 once), `+ 2` on.
    pub const ALARM0: SettingId = SettingId(0x0D30);
    /// Timer presets: the last five durations (seconds), newest first.
    pub const TIMER_PRESET0: SettingId = SettingId(0x0D40);
}

const SOUND: &[Setting] = &[
    c(0x0101, Str::Equaliser, EQ_PRESETS, 0),
    r(0x0102, Str::Balance, -10, 10, 1, Unit::Balance, 0),
    r(0x0103, Str::Bass, -6, 6, 1, Unit::Db, 0),
    c(0x0104, Str::Crossfeed, ch![S(Str::Off), S(Str::Low), S(Str::Medium), S(Str::High)], 0),
    c(0x0105, Str::ChannelMode, ch![S(Str::Stereo), S(Str::Mono), S(Str::SwapLr)], 0),
    c(0x0106, Str::ReplayGain, ch![S(Str::Off), S(Str::Track), S(Str::Album), S(Str::Auto)], 0),
    r(0x0107, Str::VolumeLimit, 10, 30, 1, Unit::None, 30),
    t(0x0108, Str::KeyClick, 0),
    t(0x0109, Str::FadeOnPause, 1),
];

const PLAYBACK: &[Setting] = &[
    t(0x0201, Str::Shuffle, 0),
    c(0x0202, Str::Repeat, REPEAT, 0),
    t(0x0203, Str::Gapless, 1),
    t(0x0204, Str::ResumeAtStart, 1),
    c(0x0205, Str::SeekStep, ch![Secs(2), Secs(5), Secs(10), Secs(30)], 1),
    c(
        0x0206,
        Str::Speed,
        ch![Speed(50), Speed(75), Speed(90), Speed(100), Speed(110), Speed(125), Speed(150), Speed(200)],
        3,
    ),
    t(0x0207, Str::AutoBookmark, 1),
    c(
        0x0208,
        Str::TagCodePage,
        ch![
            S(Str::CpWestern),
            S(Str::CpCentral),
            S(Str::CpCyrillic),
            S(Str::CpJapanese),
            S(Str::CpChineseS),
            S(Str::CpChineseT),
            S(Str::CpKorean)
        ],
        0,
    ),
];

/// The Home-screen mask with every app shown (`App::ALL`'s bits).
pub const HOME_ALL: i16 = (1 << crate::App::COUNT) - 1;

const DISPLAY: &[Setting] = &[
    r(0x0301, Str::Brightness, 1, 16, 1, Unit::None, 10),
    c(
        0x0302,
        Str::BacklightTimeout,
        ch![Secs(5), Secs(10), Secs(15), Secs(30), Secs(60), Secs(120), S(Str::Always)],
        2,
    ),
    c(0x0303, Str::LockDelay, ch![S(Str::Immediately), Secs(5), Secs(30), Secs(60), S(Str::Never)], 0),
    c(0x0304, Str::Theme, ch![S(Str::ThemeDark), S(Str::ThemeLight), S(Str::ThemeAmber)], 0),
    c(0x0305, Str::Accent, ch![S(Str::Blue), S(Str::Green), S(Str::Pink), S(Str::Red), S(Str::Grey)], 0),
    c(0x0306, Str::Wallpaper, ch![S(Str::None_)], 0),
    c(0x0307, Str::ScreenSaver, ch![S(Str::Off), S(Str::Clock)], 0),
    t(0x0308, Str::Animations, 1),
    t(0x0309, Str::Rotate, 0),
    t(0x030A, Str::ClockInBar, 1),
    // UI-8: on by default (the owner's requirement); shown before Home
    // screen as in 05 §7.11.
    t(0x030C, Str::ListRollAround, 1),
    // Every app (Games, bit 11, with the `games` feature: on the player since
    // GAME-2's integration).
    Setting {
        id: SettingId(0x030B),
        label: Str::HomeScreen,
        kind: Kind::HomeApps,
        default: HOME_ALL,
        pack: None,
    },
];

const POWER: &[Setting] = &[
    c(0x0401, Str::PowerMode, ch![S(Str::Auto), S(Str::BatterySaver), S(Str::HighPerformance)], 0),
    c(0x0402, Str::SleepTimer, SLEEP, 0),
    c(0x0403, Str::AutoOff, ch![S(Str::Off), Mins(5), Mins(10), Mins(15), Mins(30), Mins(60)], 3),
    c(0x0404, Str::KeysLocked, ch![S(Str::None_), S(Str::KeysVolume), S(Str::KeysTransport)], 1),
    i(0x0405, Str::Battery, Info::Battery),
];

const DATE_TIME: &[Setting] = &[
    a(0x0501, Str::SetTime, Action::SetTime),
    a(0x0502, Str::SetDate, Action::SetDate),
    t(0x0503, Str::Clock24, 1),
    c(0x0504, Str::DateFormat, ch![Lit("DD.MM.YYYY"), Lit("MM/DD/YYYY"), Lit("YYYY-MM-DD")], 0),
    r(0x0505, Str::UtcOffset, -720, 840, 15, Unit::UtcMinutes, 0),
    r(0x0506, Str::Correction, -60, 60, 1, Unit::PerDay, 0),
    t(0x0507, Str::SetFromRds, 0),
];

const BLUETOOTH: &[Setting] = &[
    t(0x0601, Str::AppBluetooth, 0),
    i(0x0602, Str::DeviceName, Info::DeviceName),
    t(0x0603, Str::Reconnect, 1),
    c(0x0604, Str::Output, OUTPUT, 0),
];

const RADIO: &[Setting] = &[
    c(
        0x0701,
        Str::Region,
        ch![S(Str::Europe), S(Str::UnitedStates), S(Str::World), S(Str::Japan), S(Str::Wide)],
        0,
    ),
    t(0x0702, Str::ForceMono, 0),
    c(0x0703, Str::SeekSensitivity, ch![S(Str::Low), S(Str::Medium), S(Str::High)], 1),
];

const RECORDING: &[Setting] = &[
    c(0x0801, Str::RecSource, ch![S(Str::Microphone), S(Str::FmRadio), S(Str::LineIn)], 0),
    // The formats built (AU-13); MP3 and Opus are not.
    c(0x0802, Str::RecFormat, ch![Lit("WAV"), Lit("WAV ADPCM"), Lit("FLAC")], 0),
    c(0x0803, Str::Quality, ch![S(Str::QVoice), S(Str::QStandard), S(Str::QHigh)], 1),
    c(0x0804, Str::GainMode, ch![S(Str::Manual), S(Str::Auto)], 1),
    r(0x080A, Str::InputGain, -12, 12, 1, Unit::Db, 0),
    c(0x0806, Str::VoiceActivation, ch![S(Str::Off), Lit("−50 dB"), Lit("−40 dB"), Lit("−30 dB")], 0),
    c(0x0807, Str::AutoSplit, ch![S(Str::Off), Mins(15), Mins(30), Mins(60)], 0),
    t(0x0808, Str::Monitor, 0),
    t(0x0809, Str::LowCut, 0),
    i(0x0805, Str::Folder, Info::RecFolder),
];

const USB: &[Setting] = &[
    c(
        0x0901,
        Str::WhenConnected,
        ch![S(Str::Ask), S(Str::ChargeOnly), S(Str::FileTransfer), S(Str::Keyboard), S(Str::LastImage)],
        0,
    ),
    t(0x0902, Str::ReadOnlyCard, 0),
    t(0x0903, Str::DevChannel, 1),
    // USB-5 / OTP-2: the computer's layout for typed codes, digits from
    // the keypad (layouts whose top row needs Shift), Enter after a code.
    c(0x0904, Str::KbdLayout, ch![S(Str::KbdUs), S(Str::KbdUk), S(Str::KbdDe), S(Str::KbdFr)], 0),
    t(0x0905, Str::KeypadDigits, 0),
    t(0x0906, Str::EnterAfterCode, 1),
];

const LANGUAGE: &[Setting] =
    &[Setting { id: SettingId(0x0A01), label: Str::GLanguage, kind: Kind::Language, default: 0, pack: None }];

const STORAGE: &[Setting] = &[
    i(0x0B01, Str::CardInfo, Info::CardInfo),
    a(0x0B02, Str::RebuildLibrary, Action::RebuildLibrary),
    a(0x0B03, Str::ClearCaches, Action::ClearCaches),
    a(0x0B04, Str::CheckCard, Action::CheckCard),
    a(0x0B05, Str::FormatCard, Action::FormatCard),
];

const SYSTEM: &[Setting] = &[
    i(0x0C01, Str::About, Info::About),
    a(0x0C02, Str::UpdateFirmware, Action::UpdateFirmware),
    i(0x0C03, Str::Diagnostics, Info::Diagnostics),
    a(0x0C04, Str::StartOriginal, Action::StartOriginal),
    a(0x0C05, Str::FactoryReset, Action::FactoryReset),
    a(0x0C06, Str::RecoveryMode, Action::Recovery),
    c(0x0C07, Str::Performance, ch![S(Str::PerfNormal), S(Str::PerfMaximum)], 0),
    a(0x0C08, Str::Developer, Action::Developer),
];

/// Now Playing views (05 §7.1), in Up/Down order.
pub const NP_VIEWS: &[Choice] = ch![
    S(Str::ViewStandard),
    S(Str::ViewBigArt),
    S(Str::ViewLyrics),
    S(Str::ViewUpNext),
    S(Str::ViewDetails),
    S(Str::ViewLevels)
];

/// Equaliser band centres (05 §7.1), Hz.
pub const EQ_BANDS_HZ: [u32; 5] = [60, 230, 910, 3600, 14000];

/// Gains in dB of the built-in presets (EQ_PRESETS order, without the two
/// custom curves, which are settings).
pub const EQ_GAINS: [[i8; 5]; 9] = [
    [0, 0, 0, 0, 0],
    [5, 3, -1, 3, 5],
    [-1, 2, 4, 2, -1],
    [3, 2, -1, 2, 4],
    [4, 3, 0, 2, 3],
    [5, 2, 0, 2, 5],
    [5, 4, 0, 1, 3],
    [7, 4, 0, 0, 0],
    [-2, 0, 3, 4, 1],
];

/// Steps › Units.
pub const STEP_UNITS: &[Choice] = ch![S(Str::Kilometres), S(Str::Miles)];
/// Steps › Count in background.
pub const STEP_BACKGROUND: &[Choice] = ch![S(Str::Off), S(Str::WhileOn)];
/// Steps › Sensitivity.
pub const STEP_SENSITIVITY: &[Choice] = ch![S(Str::Low), S(Str::Medium), S(Str::High)];

/// Settings that belong to apps rather than to a Settings group (stored
/// like the others; edited in the apps).
pub const APP_ITEMS: &[Setting] = &[
    c(0x0D01, Str::SortBy, ch![S(Str::SortName), S(Str::SortDate), S(Str::SortSize)], 0),
    t(0x0D02, Str::ShowHidden, 0),
    c(0x0D03, Str::NpView, NP_VIEWS, 0),
    r(0x0D10, Str::EqCustom1, -12, 12, 1, Unit::Db, 0),
    r(0x0D11, Str::EqCustom1, -12, 12, 1, Unit::Db, 0),
    r(0x0D12, Str::EqCustom1, -12, 12, 1, Unit::Db, 0),
    r(0x0D13, Str::EqCustom1, -12, 12, 1, Unit::Db, 0),
    r(0x0D14, Str::EqCustom1, -12, 12, 1, Unit::Db, 0),
    r(0x0D15, Str::EqCustom2, -12, 12, 1, Unit::Db, 0),
    r(0x0D16, Str::EqCustom2, -12, 12, 1, Unit::Db, 0),
    r(0x0D17, Str::EqCustom2, -12, 12, 1, Unit::Db, 0),
    r(0x0D18, Str::EqCustom2, -12, 12, 1, Unit::Db, 0),
    r(0x0D19, Str::EqCustom2, -12, 12, 1, Unit::Db, 0),
    r(0x0D50, Str::AppRadio, 6500, 10800, 5, Unit::None, 8750),
    // Steps (UI-5c).
    r(0x0D20, Str::DailyGoal, 1000, 30000, 500, Unit::None, 10000),
    r(0x0D21, Str::StrideLength, 30, 150, 1, Unit::Cm, 70),
    r(0x0D22, Str::Weight, 30, 200, 1, Unit::Kg, 70),
    c(0x0D23, Str::Units, STEP_UNITS, 0),
    c(0x0D24, Str::CountInBackground, STEP_BACKGROUND, 1),
    c(0x0D25, Str::Sensitivity, STEP_SENSITIVITY, 1),
    // Alarms (UI-5d): time (minutes), repeat days, on.
    r(0x0D30, Str::TabAlarm, 0, 1439, 1, Unit::None, 7 * 60),
    r(0x0D31, Str::RepeatDays, 0, 127, 1, Unit::None, 0x1F),
    t(0x0D32, Str::TabAlarm, 0),
    r(0x0D33, Str::TabAlarm, 0, 1439, 1, Unit::None, 7 * 60 + 30),
    r(0x0D34, Str::RepeatDays, 0, 127, 1, Unit::None, 0x1F),
    t(0x0D35, Str::TabAlarm, 0),
    r(0x0D36, Str::TabAlarm, 0, 1439, 1, Unit::None, 9 * 60),
    r(0x0D37, Str::RepeatDays, 0, 127, 1, Unit::None, 0x60),
    t(0x0D38, Str::TabAlarm, 0),
    r(0x0D39, Str::TabAlarm, 0, 1439, 1, Unit::None, 12 * 60),
    r(0x0D3A, Str::RepeatDays, 0, 127, 1, Unit::None, 0),
    t(0x0D3B, Str::TabAlarm, 0),
    // Timer presets (UI-5d): seconds, newest first.
    r(0x0D40, Str::TabTimer, 0, 32400, 1, Unit::None, 60),
    r(0x0D41, Str::TabTimer, 0, 32400, 1, Unit::None, 180),
    r(0x0D42, Str::TabTimer, 0, 32400, 1, Unit::None, 300),
    r(0x0D43, Str::TabTimer, 0, 32400, 1, Unit::None, 600),
    r(0x0D44, Str::TabTimer, 0, 32400, 1, Unit::None, 1500),
];

/// The five band gains (dB) of equaliser preset `preset` (an
/// `EQUALISER` value), reading the custom curves from `get`.
pub fn eq_gains(preset: i16, get: impl Fn(SettingId) -> i16) -> [i8; 5] {
    let p = usize::try_from(preset).unwrap_or(0);
    match EQ_GAINS.get(p) {
        Some(g) => *g,
        None => {
            let base = if p == 9 { 0 } else { 5 };
            let mut g = [0i8; 5];
            for (i, v) in g.iter_mut().enumerate() {
                let key = id::EQ_BAND0.0 + base + u16::try_from(i).unwrap_or(0);
                *v = i8::try_from(get(SettingId(key))).unwrap_or(0);
            }
            g
        }
    }
}

/// A Settings group.
#[derive(Clone, Copy, Debug)]
pub struct Group {
    pub label: Str,
    pub items: &'static [Setting],
}

/// The twelve groups in the order of 05 §7.11.
pub static GROUPS: [Group; 12] = [
    Group { label: Str::GSound, items: SOUND },
    Group { label: Str::GPlayback, items: PLAYBACK },
    Group { label: Str::GDisplay, items: DISPLAY },
    Group { label: Str::GPower, items: POWER },
    Group { label: Str::GDateTime, items: DATE_TIME },
    Group { label: Str::AppBluetooth, items: BLUETOOTH },
    Group { label: Str::AppRadio, items: RADIO },
    Group { label: Str::GRecording, items: RECORDING },
    Group { label: Str::GUsb, items: USB },
    Group { label: Str::GLanguage, items: LANGUAGE },
    Group { label: Str::GStorage, items: STORAGE },
    Group { label: Str::GSystem, items: SYSTEM },
];

/// Every item, group by group, then the apps' own and (with the `games`
/// feature) the games' (M14): the lists of [`LISTS`], so one copy of each
/// table serves the iteration and the lookups.
pub fn all() -> impl Iterator<Item = &'static Setting> {
    LISTS.iter().flat_map(|l| l.iter())
}

/// The item lists by the high byte of their keys: groups `0x01..=0x0C`
/// in [`GROUPS`]' order, the apps' items `0x0D`, the games' `0x0E` (a test
/// holds every key to its list's byte). Plain slices, so a lookup touches
/// one list and no closure.
static LISTS: [&[Setting]; 14] = {
    #[cfg(feature = "games")]
    let games: &'static [Setting] = &crate::games::SETTINGS;
    #[cfg(not(feature = "games"))]
    let games: &'static [Setting] = &[];
    [
        SOUND, PLAYBACK, DISPLAY, POWER, DATE_TIME, BLUETOOTH, RADIO, RECORDING, USB, LANGUAGE, STORAGE,
        SYSTEM, APP_ITEMS, games,
    ]
};

/// Stored items before each list (the cache index of its first stored
/// item), computed at build time.
const FIRST_STORED: [u16; 14] = {
    let mut out = [0u16; 14];
    let mut n = 0u16;
    let mut l = 0;
    while l < 14 {
        out[l] = n;
        let list = LISTS[l];
        let mut i = 0;
        while i < list.len() {
            if list[i].stored() {
                n += 1;
            }
            i += 1;
        }
        l += 1;
    }
    out
};

/// The list a key belongs to, by its high byte.
#[inline(always)]
fn list_of(id: SettingId) -> Option<usize> {
    let hi = (id.0 >> 8) as usize;
    (1..=14).contains(&hi).then(|| hi - 1)
}

/// Look an item up by key.
///
/// It searches only the key's own list (by the key's high byte) with a
/// plain loop. 2026-11-25 on the player: the iterator form
/// (`all().find(..)`) called its closure out of line for every item, and
/// that closure and its caller shared a code-cache line (idle Home 1.7 s per
/// UI loop, 2,939 line loads/s, keys 3.3 s each); and a scan of all ~150
/// items reads 3.6 KB of tables in several rodata lines on every lookup
/// (a third of all line loads while switching games).
#[must_use]
#[inline(never)]
pub fn def(id: SettingId) -> Option<&'static Setting> {
    let list = LISTS[list_of(id)?];
    let mut i = 0;
    while i < list.len() {
        if list[i].id == id {
            return Some(&list[i]);
        }
        i += 1;
    }
    None
}

/// Number of stored items (the games' settings with the `games` feature).
#[cfg(not(feature = "games"))]
pub const STORED: usize = 98;
#[cfg(feature = "games")]
pub const STORED: usize = 98 + crate::games::STORED_SETTINGS;

/// Position among stored items (cache index).
///
/// The key's list only, for the reasons given at [`def`].
#[must_use]
#[inline(never)]
pub fn index(id: SettingId) -> Option<usize> {
    let l = list_of(id)?;
    let list = LISTS[l];
    let mut n = FIRST_STORED[l] as usize;
    let mut i = 0;
    while i < list.len() {
        let s = &list[i];
        if s.stored() {
            if s.id == id {
                return Some(n);
            }
            n += 1;
        }
        i += 1;
    }
    None
}

/// Value to store bytes.
#[must_use]
pub const fn encode(v: i16) -> [u8; 2] {
    v.to_le_bytes()
}

/// Store bytes to a validated value (default if the record is bad).
#[must_use]
pub fn decode(id: SettingId, bytes: &[u8]) -> Option<i16> {
    let d = def(id)?;
    let v = match bytes {
        [a, b] => i16::from_le_bytes([*a, *b]),
        _ => d.default,
    };
    Some(d.validate(v))
}

/// Two-letter language code to a stored value.
#[must_use]
pub fn lang_code(code: &str) -> i16 {
    match code.as_bytes() {
        [a, b] if code != "en" => i16::from_be_bytes([*a, *b]),
        _ => 0,
    }
}

/// The display text of value `v` of `s` (for `Choice` and `Range`).
pub fn format_value<const N: usize>(s: &Setting, v: i16, lang: Lang, out: &mut StrBuf<N>) {
    out.clear();
    match s.kind {
        Kind::Choice(cs) => {
            if let Some(ch) = usize::try_from(v).ok().and_then(|i| cs.get(i)) {
                format_choice(*ch, lang, out);
            }
        }
        Kind::Range { unit, .. } => match unit {
            Unit::None => {
                out.push_i32(i32::from(v));
            }
            Unit::Db => {
                if v > 0 {
                    out.push('+');
                }
                out.push_i32(i32::from(v));
                out.push(' ');
                out.push_str(lang.get(Str::Db));
            }
            Unit::Balance => {
                if v < 0 {
                    out.push_str("L ");
                } else if v > 0 {
                    out.push_str("R ");
                }
                out.push_u32(u32::from(v.unsigned_abs()));
            }
            Unit::UtcMinutes => {
                out.push(if v < 0 { '−' } else { '+' });
                let m = u32::from(v.unsigned_abs());
                out.push_u32_padded(m / 60, 2);
                out.push(':');
                out.push_u32_padded(m % 60, 2);
            }
            Unit::PerDay => {
                if v > 0 {
                    out.push('+');
                }
                out.push_i32(i32::from(v));
                out.push(' ');
                out.push_str(lang.get(Str::PerDay));
            }
            Unit::Cm | Unit::Kg => {
                out.push_i32(i32::from(v));
                out.push(' ');
                out.push_str(lang.get(if unit == Unit::Cm { Str::Cm } else { Str::Kg }));
            }
            Unit::MetresPerSecond => {
                out.push_i32(i32::from(v));
                out.push_str(" m/s");
            }
        },
        _ => {}
    }
}

pub fn format_choice<const N: usize>(ch: Choice, lang: Lang, out: &mut StrBuf<N>) {
    match ch {
        Choice::S(s) => {
            out.push_str(lang.get(s));
        }
        Choice::Lit(l) => {
            out.push_str(l);
        }
        Choice::Secs(n) if n >= 60 && n % 60 == 0 => {
            out.push_u32(u32::from(n / 60));
            out.push(' ');
            out.push_str(lang.get(Str::MinutesShort));
        }
        Choice::Secs(n) => {
            out.push_u32(u32::from(n));
            out.push(' ');
            out.push_str(lang.get(Str::Seconds));
        }
        Choice::Mins(n) => {
            out.push_u32(u32::from(n));
            out.push(' ');
            out.push_str(lang.get(Str::MinutesShort));
        }
        Choice::Speed(h) => {
            out.push_u32(u32::from(h / 100));
            out.push('.');
            let f = h % 100;
            if f % 10 == 0 {
                out.push_u32(u32::from(f / 10));
            } else {
                out.push_u32_padded(u32::from(f), 2);
            }
            out.push('×');
        }
    }
}

/// The RAM side of the settings store: validated values of every stored
/// item and which ones still need writing. The firmware loads it from
/// `orpheus-settings` at boot (`load`), serves `get`/`set` to the UI, and
/// writes `dirty()` entries back (debounced).
#[derive(Clone, Debug)]
pub struct SettingsCache {
    vals: [i16; STORED],
    dirty: u128,
}

impl Default for SettingsCache {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsCache {
    /// All defaults, nothing dirty.
    #[must_use]
    pub fn new() -> SettingsCache {
        let mut vals = [0; STORED];
        for (slot, s) in vals.iter_mut().zip(all().filter(|s| s.stored())) {
            *slot = s.default;
        }
        SettingsCache { vals, dirty: 0 }
    }

    /// Take a record read from the store (unknown keys are ignored).
    pub fn load(&mut self, key: u16, bytes: &[u8]) {
        let id = SettingId(key);
        if let (Some(i), Some(v)) = (index(id), decode(id, bytes))
            && let Some(slot) = self.vals.get_mut(i)
        {
            *slot = v;
        }
    }

    #[must_use]
    pub fn get(&self, id: SettingId) -> i16 {
        if let Some(d) = def(id)
            && let Some((word, shift, bits)) = d.pack
        {
            let w = self.get(word) as u16;
            return d.validate(((w >> shift) & ((1u16 << bits) - 1)) as i16);
        }
        match index(id).and_then(|i| self.vals.get(i)) {
            Some(v) => *v,
            None => def(id).map_or(0, |d| d.default),
        }
    }

    /// Set a value (validated); returns true if it changed.
    pub fn set(&mut self, id: SettingId, v: i16) -> bool {
        if let Some(d) = def(id)
            && let Some((word, shift, bits)) = d.pack
        {
            let mask = ((1u16 << bits) - 1) << shift;
            let w = self.get(word) as u16;
            let nw = (w & !mask) | (((d.validate(v) as u16) << shift) & mask);
            return self.set(word, nw as i16);
        }
        let (Some(i), Some(d)) = (index(id), def(id)) else { return false };
        let v = d.validate(v);
        match self.vals.get_mut(i) {
            Some(slot) if *slot != v => {
                *slot = v;
                self.dirty |= 1u128 << i;
                true
            }
            _ => false,
        }
    }

    /// Back to defaults; every item is marked for writing.
    pub fn reset(&mut self) {
        *self = SettingsCache::new();
        self.dirty = (1u128 << STORED) - 1;
    }

    /// Pending writes as (store key, bytes); clears the marks.
    pub fn take_dirty(&mut self) -> impl Iterator<Item = (u16, [u8; 2])> + '_ {
        let d = core::mem::take(&mut self.dirty);
        all().filter(|s| s.stored()).enumerate().filter(move |(i, _)| d & (1u128 << i) != 0).map(|(i, s)| {
            let v = self.vals.get(i).copied().unwrap_or(s.default);
            (s.id.0, encode(v))
        })
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty != 0
    }
}

#[cfg(test)]
mod tests {
    /// Every key's high byte names its list, and the lists are `all()`
    /// in order (the lookups rely on both).
    #[test]
    fn keys_are_grouped_by_their_high_byte() {
        let mut flat = LISTS.iter().flat_map(|l| l.iter());
        for s in all() {
            assert!(core::ptr::eq(flat.next().unwrap(), s));
        }
        assert!(flat.next().is_none());
        for (l, list) in LISTS.iter().enumerate() {
            for s in *list {
                assert_eq!(usize::from(s.id.0 >> 8), l + 1, "key {:#06x}", s.id.0);
            }
        }
        for (g, list) in GROUPS.iter().zip(LISTS) {
            assert!(core::ptr::eq(g.items, list));
        }
    }

    /// The lookups give exactly what the iterator definitions give, for
    /// every item and for keys that are not items (2026-11-25: `def` and
    /// `index` became per-list loops; the closure form thrashed the
    /// player's code cache, idle Home 1.7 s per UI loop).
    #[test]
    fn regress_settings_lookup_loops_match_the_iterator_form() {
        let stored: Vec<SettingId> = all().filter(|s| s.stored()).map(|s| s.id).collect();
        assert_eq!(stored.len(), STORED);
        for s in all() {
            assert!(core::ptr::eq(def(s.id).unwrap(), all().find(|t| t.id == s.id).unwrap()));
            assert_eq!(index(s.id), stored.iter().position(|&k| k == s.id));
        }
        for k in [0u16, 1, 0x7FFF, 0xFFFF, 0x0DFF] {
            let id = SettingId(k);
            assert_eq!(def(id).is_some(), all().any(|t| t.id == id));
            assert_eq!(index(id), stored.iter().position(|&x| x == id));
        }
    }

    extern crate std;
    use super::*;
    use std::collections::BTreeSet;
    use std::vec::Vec;

    /// Regression: the M7 merge gave the radio frequency the step goal's
    /// store key 0x0D20 (2026-11-24; the radio's moved to 0x0D50): every
    /// table id is unique (with `id_tests` for the named ids).
    #[test]
    fn table_is_consistent() {
        let mut ids = BTreeSet::new();
        for (g, grp) in GROUPS.iter().enumerate() {
            for s in grp.items {
                assert!(ids.insert(s.id), "duplicate id {:#06x}", s.id.0);
                assert_eq!(usize::from(s.id.0 >> 8), g + 1, "id {:#06x} in group {g}", s.id.0);
                assert_eq!(s.validate(s.default), s.default, "default of {:#06x} invalid", s.id.0);
            }
        }
        assert_eq!(all().filter(|s| s.stored()).count(), STORED);
        const { assert!(STORED <= 128) };
    }

    /// Salvo's settings live in bits of four stored words (GAME-4/5): each
    /// reads and writes under its own key, validated, without touching the
    /// others; the words' defaults hold every default; fields never
    /// overlap.
    #[cfg(feature = "salvo")]
    #[test]
    fn packed_settings() {
        let mut c = SettingsCache::new();
        let packed: Vec<&Setting> = all().filter(|s| s.pack.is_some()).collect();
        assert_eq!(packed.len(), 22);
        let mut used = std::collections::BTreeMap::<u16, u16>::new();
        for s in &packed {
            let (w, shift, bits) = s.pack.unwrap();
            assert!(def(w).is_some_and(Setting::stored), "{:#06x}: its word is stored", s.id.0);
            assert!(u32::from(shift) + u32::from(bits) <= 16);
            let mask = (((1u32 << bits) - 1) << shift) as u16;
            let u = used.entry(w.0).or_default();
            assert_eq!(*u & mask, 0, "{:#06x} overlaps", s.id.0);
            *u |= mask;
            assert_eq!(c.get(s.id), s.default, "{:#06x} default", s.id.0);
            // Every valid value fits its bits.
            let max = match s.kind {
                Kind::Toggle => 1,
                Kind::Choice(ch) => ch.len() as i32 - 1,
                Kind::Range { max, .. } => i32::from(max),
                _ => 0,
            };
            assert!(max < 1 << bits, "{:#06x} does not fit {bits} bits", s.id.0);
        }
        assert_eq!(
            [0x0E80, 0x0E81, 0x0E82, 0x0E83].map(|w| c.get(SettingId(w))),
            [21056, 160, 4634, 61],
            "the manifest's defaults"
        );
        let before: Vec<i16> = packed.iter().map(|s| c.get(s.id)).collect();
        let gravity = SettingId(0x0E6D);
        assert!(c.set(gravity, 3));
        assert_eq!(c.get(gravity), 3);
        assert!(!c.set(gravity, 3), "unchanged");
        assert!(c.set(gravity, 9), "invalid: the default");
        assert_eq!(c.get(gravity), 2);
        assert!(c.set(gravity, 3));
        for (s, b) in packed.iter().zip(&before) {
            if s.id != gravity {
                assert_eq!(c.get(s.id), *b, "{:#06x} untouched", s.id.0);
            }
        }
        let w: Vec<_> = c.take_dirty().collect();
        assert_eq!(w.len(), 1, "one word written");
        assert_eq!(w[0].0, 0x0E82);
        // A word read from the store gives its fields.
        let mut d = SettingsCache::new();
        d.load(w[0].0, &w[0].1);
        assert_eq!(d.get(gravity), 3);
        let seed = SettingId(0x0E6B);
        assert!(c.set(seed, 99));
        assert_eq!(c.get(seed), 99);
        assert_eq!(c.get(SettingId(0x0E6C)), 2, "the next field is intact");
    }

    #[test]
    fn cache_roundtrip_and_validation() {
        let mut c = SettingsCache::new();
        assert_eq!(c.get(id::THEME), 0);
        assert!(c.set(id::THEME, 1));
        assert!(!c.set(id::THEME, 1));
        assert!(c.set(id::THEME, 9), "out of range falls back to the default");
        assert_eq!(c.get(id::THEME), 0);
        c.set(id::BRIGHTNESS, 4);
        let w: Vec<_> = c.take_dirty().collect();
        assert_eq!(w.len(), 2);
        assert!(!c.is_dirty());
        let mut d = SettingsCache::new();
        for (k, b) in &w {
            d.load(*k, b);
        }
        assert_eq!(d.get(id::BRIGHTNESS), 4);
        d.load(id::BRIGHTNESS.0, &[0xFF]);
        assert_eq!(d.get(id::BRIGHTNESS), 10, "short record: default");
        d.load(id::UTC_OFFSET.0, &encode(7));
        assert_eq!(d.get(id::UTC_OFFSET), 0, "not on the 15-minute grid");
        d.reset();
        assert_eq!(d.take_dirty().count(), STORED);
    }

    #[test]
    fn list_roll_around_is_a_display_toggle_on_by_default() {
        let s = def(id::LIST_ROLL).unwrap();
        assert_eq!((s.kind, s.default, s.label), (Kind::Toggle, 1, Str::ListRollAround));
        let display = &GROUPS[2];
        assert_eq!(display.label, Str::GDisplay);
        let pos = display.items.iter().position(|s| s.id == id::LIST_ROLL).unwrap();
        assert_eq!(display.items.get(pos + 1).map(|s| s.id), Some(id::HOME_APPS));
        let mut c = SettingsCache::new();
        assert_eq!(c.get(id::LIST_ROLL), 1);
        assert!(c.set(id::LIST_ROLL, 0));
        let w: Vec<_> = c.take_dirty().collect();
        assert_eq!(w, [(0x030C, encode(0))]);
        let mut d = SettingsCache::new();
        d.load(0x030C, &w[0].1);
        assert_eq!(d.get(id::LIST_ROLL), 0, "persisted off");
    }

    #[test]
    fn formatting() {
        let mut b = StrBuf::<40>::new();
        let l = Lang::ENGLISH;
        format_value(def(SettingId(0x0505)).unwrap(), -330, l, &mut b);
        assert_eq!(b.as_str(), "−05:30");
        format_value(def(SettingId(0x0206)).unwrap(), 5, l, &mut b);
        assert_eq!(b.as_str(), "1.25×");
        format_value(def(SettingId(0x0206)).unwrap(), 3, l, &mut b);
        assert_eq!(b.as_str(), "1.0×");
        format_value(def(SettingId(0x0302)).unwrap(), 4, l, &mut b);
        assert_eq!(b.as_str(), "1 min");
        format_value(def(SettingId(0x0102)).unwrap(), -3, l, &mut b);
        assert_eq!(b.as_str(), "L 3");
        let d = def(SettingId(0x0202)).unwrap();
        assert_eq!(d.step(2, 1), 0);
        assert_eq!(d.step(0, -1), 2);
        assert_eq!(lang_code("ru"), i16::from_be_bytes(*b"ru"));
    }
}
