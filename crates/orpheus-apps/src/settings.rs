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
}

impl Setting {
    /// Whether this item stores a value.
    #[must_use]
    pub const fn stored(&self) -> bool {
        !matches!(self.kind, Kind::Action(_) | Kind::Info(_))
    }

    /// `v` if valid for this item, else the default.
    #[must_use]
    pub fn validate(&self, v: i16) -> i16 {
        let ok = match self.kind {
            Kind::Toggle => v == 0 || v == 1,
            Kind::Choice(c) => usize::try_from(v).is_ok_and(|i| i < c.len()),
            Kind::Range { min, max, step, .. } => v >= min && v <= max && (v - min) % step.max(1) == 0,
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

const fn t(id: u16, label: Str, default: i16) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Toggle, default }
}
const fn c(id: u16, label: Str, choices: &'static [Choice], default: i16) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Choice(choices), default }
}
const fn r(id: u16, label: Str, min: i16, max: i16, step: i16, unit: Unit, default: i16) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Range { min, max, step, unit }, default }
}
const fn a(id: u16, label: Str, act: Action) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Action(act), default: 0 }
}
const fn i(id: u16, label: Str, info: Info) -> Setting {
    Setting { id: SettingId(id), label, kind: Kind::Info(info), default: 0 }
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
    pub const LANGUAGE: SettingId = SettingId(0x0A01);
    pub const FILES_SORT: SettingId = SettingId(0x0D01);
    pub const SHOW_HIDDEN: SettingId = SettingId(0x0D02);
    pub const NP_VIEW: SettingId = SettingId(0x0D03);
    /// Custom 1 bands 0..5, then Custom 2 bands 0..5.
    pub const EQ_BAND0: SettingId = SettingId(0x0D10);
    pub const SPEED: SettingId = SettingId(0x0206);
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
    Setting { id: SettingId(0x030B), label: Str::HomeScreen, kind: Kind::HomeApps, default: 0x07FF },
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
    c(0x0802, Str::RecFormat, ch![Lit("WAV"), Lit("WAV ADPCM"), Lit("FLAC"), Lit("MP3"), Lit("Opus")], 0),
    c(0x0803, Str::Quality, ch![S(Str::QVoice), S(Str::QStandard), S(Str::QHigh)], 1),
    c(0x0804, Str::GainMode, ch![S(Str::Manual), S(Str::Auto)], 1),
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
];

const LANGUAGE: &[Setting] =
    &[Setting { id: SettingId(0x0A01), label: Str::GLanguage, kind: Kind::Language, default: 0 }];

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

/// Every item, group by group.
pub fn all() -> impl Iterator<Item = &'static Setting> {
    GROUPS.iter().flat_map(|g| g.items.iter()).chain(APP_ITEMS.iter())
}

/// Look an item up by key.
#[must_use]
pub fn def(id: SettingId) -> Option<&'static Setting> {
    all().find(|s| s.id == id)
}

/// Number of stored items.
pub const STORED: usize = 64;

/// Position among stored items (cache index).
#[must_use]
pub fn index(id: SettingId) -> Option<usize> {
    all().filter(|s| s.stored()).position(|s| s.id == id)
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
        match index(id).and_then(|i| self.vals.get(i)) {
            Some(v) => *v,
            None => def(id).map_or(0, |d| d.default),
        }
    }

    /// Set a value (validated); returns true if it changed.
    pub fn set(&mut self, id: SettingId, v: i16) -> bool {
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
    extern crate std;
    use super::*;
    use std::collections::BTreeSet;
    use std::vec::Vec;

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
