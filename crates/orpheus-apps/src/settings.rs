//! The settings model (05 §7.11): every group and item with its store key,
//! kind, range and default, and the RAM cache the firmware keeps in front
//! of `orpheus-settings`.
//!
//! Every stored value is an `i16` (toggle 0/1, choice index, range value,
//! bit mask, or a two-letter language code) encoded as 2 bytes
//! little-endian under its `SettingId` (the `orpheus-settings` u16 key,
//! `0x0101..=0x11FF`: group or list in the high byte). Decoding validates against
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
    /// Quarters shown as a speed ("1.25×"): the screen reader's speech
    /// rate (A11Y-1).
    #[cfg(feature = "screen-reader")]
    Quarters,
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
    /// Bit `n` set = app `n` hidden from Home (order of
    /// [`crate::App::ALL`]): an app added later is shown until hidden.
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
    /// saves store keys (each takes an index slot and a 16-byte record,
    /// [`STORE_KEYS`]) and cache RAM (the artillery game's 21 settings take
    /// four words, GAME-4/5).
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
        // In i32: a range reaching the i16 limits (the packed accessibility
        // and screen-reader words) overflowed at its ends (VER-3,
        // 2026-11-27).
        let (v32, d32) = (i32::from(v), i32::from(dir));
        match self.kind {
            Kind::Toggle => 1 - v.clamp(0, 1),
            Kind::Choice(c) => {
                let n = i32::try_from(c.len()).unwrap_or(1).max(1);
                (v32 + d32).rem_euclid(n) as i16
            }
            Kind::Range { min, max, step, .. } => {
                (v32 + d32 * i32::from(step)).clamp(i32::from(min), i32::from(max)) as i16
            }
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

/// Keys of settings that were removed or whose stored meaning changed:
/// never used again (a store written by an older firmware may still hold
/// them; they are skipped when it loads).
///
/// * `0x0301`: Brightness 1–16 (default 10) until 2026-11-26; it drove
///   nothing (the firmware lit the panel at a fixed duty 12). Now
///   [`id::BRIGHTNESS`] (`0x030D`), the stock levels.
/// * `0x0C07`: Settings › System › Performance (Normal / Maximum, D-4)
///   until 2026-11-26 (PWR-5): the governor now reaches 288 MHz by itself
///   when the load needs it, and Power mode (`0x0401`) is the only
///   setting it follows.
/// * `0x030B`: Settings › Display › Home screen as the apps *shown*
///   (default every app) until 2026-11-27 (BOOK-1/2b): a mask saved when
///   Home had twelve apps hid every app added after it (Books, bit 12, on
///   the owner's player). Now [`id::HOME_HIDDEN`] (`0x030E`), the apps
///   hidden: a new app is shown until hidden.
pub const RETIRED: &[u16] = &[0x0301, 0x0C07, 0x030B];

/// Settings-store key of the last connected Bluetooth device (6 bytes, as
/// the module sends the address; the firmware's `bt::LAST_KEY`).
pub const BT_LAST_KEY: u16 = 0x06F0;

/// Store keys the firmware keeps beside the stored settings: the
/// Bluetooth address, the radio presets ([`orpheus_fm::presets::STORE_KEY`])
/// and the [`RETIRED`] keys an older firmware may have left.
pub const OTHER_STORE_KEYS: usize = 2 + RETIRED.len();

/// Store keys in use at most: every stored setting and the others.
pub const STORE_KEYS: usize = STORED + OTHER_STORE_KEYS;

/// Free store keys required above [`STORE_KEYS`] (the settings store's
/// index holds 192 since VER-4 2026-11-26; with every app's settings 128
/// of its 128 were in use). The firmware asserts it at build time and
/// `regress_full_feature_settings_tree_fits_with_headroom` with every
/// feature.
pub const MIN_FREE_STORE_KEYS: usize = 32;

/// Store keys used by code outside the Settings screens.
pub mod id {
    use super::SettingId;
    pub const EQUALISER: SettingId = SettingId(0x0101);
    pub const VOLUME_LIMIT: SettingId = SettingId(0x0107);
    /// Settings › Sound › Key click: 0 off, 1 presses, 2 presses and holds
    /// (A11Y-2: a second click when a held key reaches its hold delay).
    pub const KEY_CLICK: SettingId = SettingId(0x0108);
    pub const FADE_ON_PAUSE: SettingId = SettingId(0x0109);
    pub const SHUFFLE: SettingId = SettingId(0x0201);
    pub const REPEAT: SettingId = SettingId(0x0202);
    pub const GAPLESS: SettingId = SettingId(0x0203);
    pub const RESUME_AT_START: SettingId = SettingId(0x0204);
    pub const SEEK_STEP: SettingId = SettingId(0x0205);
    pub const AUTO_BOOKMARK: SettingId = SettingId(0x0207);
    /// Settings › Display › Brightness: the stock backlight level 1–11
    /// (default 6; `orpheus_power::backlight`, U-PWM-3). Key `0x030D`
    /// since 2026-11-26: `0x0301` held a 1–16 value that drove nothing
    /// and is retired ([`RETIRED`]), so an old store reads the stock
    /// default instead of a level of another scale.
    pub const BRIGHTNESS: SettingId = SettingId(0x030D);
    pub const BACKLIGHT: SettingId = SettingId(0x0302);
    pub const LOCK_DELAY: SettingId = SettingId(0x0303);
    pub const THEME: SettingId = SettingId(0x0304);
    pub const ACCENT: SettingId = SettingId(0x0305);
    pub const SCREEN_SAVER: SettingId = SettingId(0x0307);
    pub const ANIMATIONS: SettingId = SettingId(0x0308);
    pub const ROTATE: SettingId = SettingId(0x0309);
    pub const CLOCK_IN_BAR: SettingId = SettingId(0x030A);
    /// Settings › Display › Home screen: the apps hidden from Home (bit
    /// `n`: `App::ALL[n]`; 0, the default, shows every app). `0x030B` (the
    /// apps shown) is retired ([`RETIRED`]).
    pub const HOME_HIDDEN: SettingId = SettingId(0x030E);
    /// Settings › Display › List roll-around (UI-8): 1 (default) every
    /// list rolls round at its ends, 0 the ends clamp.
    pub const LIST_ROLL: SettingId = SettingId(0x030C);
    pub const POWER_MODE: SettingId = SettingId(0x0401);
    pub const SLEEP_TIMER: SettingId = SettingId(0x0402);
    pub const KEYS_LOCKED: SettingId = SettingId(0x0404);
    pub const CLOCK_24H: SettingId = SettingId(0x0503);
    pub const DATE_FORMAT: SettingId = SettingId(0x0504);
    pub const UTC_OFFSET: SettingId = SettingId(0x0505);
    /// Clock drift correction, seconds per day (-60 ..= 60).
    pub const CLOCK_CORRECTION: SettingId = SettingId(0x0506);
    pub const SET_FROM_RDS: SettingId = SettingId(0x0507);
    pub const BLUETOOTH: SettingId = SettingId(0x0601);
    pub const BT_RECONNECT: SettingId = SettingId(0x0603);
    pub const OUTPUT: SettingId = SettingId(0x0604);
    pub const USB_WHEN_CONNECTED: SettingId = SettingId(0x0901);
    pub const READ_ONLY_CARD: SettingId = SettingId(0x0902);
    pub const DEV_CHANNEL: SettingId = SettingId(0x0903);
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
    /// Settings › Accessibility (A11Y-2, 05 §7.14), packed into
    /// [`A11Y_WORD`]: text size (0 normal, 1 large, 2 extra large), hold
    /// delay (index of [`super::HOLD_DELAY_MS`]), repeat speed (0 normal,
    /// 1 slow, 2 slower).
    pub const TEXT_SIZE: SettingId = SettingId(0x1001);
    pub const HOLD_DELAY: SettingId = SettingId(0x1002);
    pub const REPEAT_SPEED: SettingId = SettingId(0x1003);
    /// The word the accessibility settings are packed into (one cache
    /// slot: the cache holds at most 128).
    pub const A11Y_WORD: SettingId = SettingId(0x0D60);
    /// The screen reader (A11Y-1, 05 §7.14; feature `screen-reader`): on,
    /// speech rate (quarters 1–40, stored on its own), voice, pitch,
    /// verbosity, earcons, the where-am-I keys and keys with the screen
    /// off, the others packed into [`READER_WORD`].
    #[cfg(feature = "screen-reader")]
    pub const READER: SettingId = SettingId(0x1004);
    #[cfg(feature = "screen-reader")]
    pub const SPEECH_RATE: SettingId = SettingId(0x1005);
    #[cfg(feature = "screen-reader")]
    pub const READER_VOICE: SettingId = SettingId(0x1006);
    #[cfg(feature = "screen-reader")]
    pub const READER_PITCH: SettingId = SettingId(0x1007);
    #[cfg(feature = "screen-reader")]
    pub const VERBOSITY: SettingId = SettingId(0x1008);
    #[cfg(feature = "screen-reader")]
    pub const EARCONS: SettingId = SettingId(0x1009);
    #[cfg(feature = "screen-reader")]
    pub const WHERE_KEYS: SettingId = SettingId(0x100A);
    #[cfg(feature = "screen-reader")]
    pub const DARK_KEYS: SettingId = SettingId(0x100B);
    /// TTS-3: Speech volume (Normal, −6, −12, −18 dB) and Music while
    /// speaking (Lowered, Paused, Unchanged), in [`READER_WORD`] too.
    #[cfg(feature = "screen-reader")]
    pub const SPEECH_VOLUME: SettingId = SettingId(0x100C);
    #[cfg(feature = "screen-reader")]
    pub const MUSIC_WHILE_SPEAKING: SettingId = SettingId(0x100D);
    #[cfg(feature = "screen-reader")]
    pub const READER_WORD: SettingId = SettingId(0x0D61);
}

/// The screen reader's pitch choices in semitones (A11Y-1): Lower, Low,
/// Normal, High, Higher.
#[cfg(feature = "screen-reader")]
pub const READER_PITCH_SEMITONES: [i8; 5] = [-4, -2, 0, 2, 4];

/// The default of the screen reader's word 0x0D61: every field's default
/// (off, Neutral, pitch Normal, verbosity Normal, earcons on, Back + OK,
/// keys with the screen off on, speech at Normal volume, the music
/// lowered while speaking).
#[cfg(feature = "screen-reader")]
const READER_WORD_DEFAULT: i16 = (2 << 3) | (1 << 6) | (1 << 8) | (1 << 9) | (1 << 11);

/// Settings › Accessibility › Hold delay choices, ms (A11Y-2).
pub const HOLD_DELAY_MS: [u32; 5] = [500, 750, 1000, 1500, 2000];

const SOUND: &[Setting] = &[
    c(0x0101, Str::Equaliser, EQ_PRESETS, 0),
    r(0x0102, Str::Balance, -10, 10, 1, Unit::Balance, 0),
    r(0x0103, Str::Bass, -6, 6, 1, Unit::Db, 0),
    c(0x0104, Str::Crossfeed, ch![S(Str::Off), S(Str::Low), S(Str::Medium), S(Str::High)], 0),
    c(0x0105, Str::ChannelMode, ch![S(Str::Stereo), S(Str::Mono), S(Str::SwapLr)], 0),
    c(0x0106, Str::ReplayGain, ch![S(Str::Off), S(Str::Track), S(Str::Album), S(Str::Auto)], 0),
    r(0x0107, Str::VolumeLimit, 10, 30, 1, Unit::None, 30),
    c(0x0108, Str::KeyClick, ch![S(Str::Off), S(Str::ClickPresses), S(Str::ClickHolds)], 0),
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
    // The stock levels (U-PWM-3); 0x0301 (1–16) is retired.
    r(
        0x030D,
        Str::Brightness,
        orpheus_power::backlight::MIN_LEVEL,
        orpheus_power::backlight::MAX_LEVEL,
        1,
        Unit::None,
        orpheus_power::backlight::DEFAULT_LEVEL,
    ),
    c(
        0x0302,
        Str::BacklightTimeout,
        ch![Secs(5), Secs(10), Secs(15), Secs(30), Secs(60), Secs(120), S(Str::Always)],
        2,
    ),
    c(0x0303, Str::LockDelay, ch![S(Str::Immediately), Secs(5), Secs(30), Secs(60), S(Str::Never)], 0),
    c(
        0x0304,
        Str::Theme,
        ch![
            S(Str::ThemeDark),
            S(Str::ThemeLight),
            S(Str::ThemeAmber),
            S(Str::ThemeContrastDark),
            S(Str::ThemeContrastLight)
        ],
        0,
    ),
    c(0x0305, Str::Accent, ch![S(Str::Blue), S(Str::Green), S(Str::Pink), S(Str::Red), S(Str::Grey)], 0),
    c(0x0306, Str::Wallpaper, ch![S(Str::None_)], 0),
    c(0x0307, Str::ScreenSaver, ch![S(Str::Off), S(Str::Clock)], 0),
    t(0x0308, Str::Animations, 1),
    t(0x0309, Str::Rotate, 0),
    t(0x030A, Str::ClockInBar, 1),
    // UI-8: on by default (the owner's requirement); shown before Home
    // screen as in 05 §7.11.
    t(0x030C, Str::ListRollAround, 1),
    // No app hidden: every app on Home, also those added after a choice
    // was saved (BOOK-1/2b).
    Setting { id: SettingId(0x030E), label: Str::HomeScreen, kind: Kind::HomeApps, default: 0, pack: None },
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
    // Line-in (value 2) retired 2026-11-27: the YM03 has no line input
    // (01 §2, U-AUD-4); a stored 2 reads as the default, Microphone.
    c(0x0801, Str::RecSource, ch![S(Str::Microphone), S(Str::FmRadio)], 0),
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

/// Settings › Accessibility (A11Y-2, 05 §7.14 "vision and motor
/// settings"): packed into one word ([`id::A11Y_WORD`], in [`APP_ITEMS`]).
/// High contrast is two Themes (Display), reduced motion is Animations off
/// (Display), the key confirmation sound is Key click (Sound).
const ACCESSIBILITY: &[Setting] = &[
    c(0x1001, Str::TextSize, ch![S(Str::TextNormal), S(Str::TextLarge), S(Str::TextExtraLarge)], 0)
        .packed(0x0D60, 0, 2),
    c(0x1002, Str::HoldDelay, ch![Lit("0.5 s"), Lit("0.75 s"), Lit("1 s"), Lit("1.5 s"), Lit("2 s")], 0)
        .packed(0x0D60, 2, 3),
    c(0x1003, Str::RepeatSpeed, ch![S(Str::RepeatNormal), S(Str::RepeatSlow), S(Str::RepeatSlower)], 0)
        .packed(0x0D60, 5, 2),
    // A11Y-1: the screen reader (feature `screen-reader`).
    #[cfg(feature = "screen-reader")]
    t(0x1004, Str::ScreenReader, 0).packed(0x0D61, 0, 1),
    // 0.25×–10× in 0.25× steps (owner 2026-11-25; 04 §8): 40 values.
    #[cfg(feature = "screen-reader")]
    r(0x1005, Str::SpeechRate, 1, 40, 1, Unit::Quarters, 4),
    #[cfg(feature = "screen-reader")]
    c(0x1006, Str::Voice, ch![S(Str::VoiceNeutral), S(Str::Low), S(Str::VoiceBright)], 0)
        .packed(0x0D61, 1, 2),
    #[cfg(feature = "screen-reader")]
    c(
        0x1007,
        Str::Pitch,
        ch![S(Str::PitchLower), S(Str::Low), S(Str::TextNormal), S(Str::High), S(Str::PitchHigher)],
        2,
    )
    .packed(0x0D61, 3, 3),
    #[cfg(feature = "screen-reader")]
    c(0x1008, Str::Verbosity, ch![S(Str::VerbBrief), S(Str::TextNormal), S(Str::VerbDetailed)], 1)
        .packed(0x0D61, 6, 2),
    #[cfg(feature = "screen-reader")]
    t(0x1009, Str::Earcons, 1).packed(0x0D61, 8, 1),
    #[cfg(feature = "screen-reader")]
    c(0x100A, Str::WhereAmIKeys, ch![S(Str::Off), Lit("Back + OK"), Lit("Prev + Next")], 1)
        .packed(0x0D61, 9, 2),
    #[cfg(feature = "screen-reader")]
    t(0x100B, Str::DarkKeys, 1).packed(0x0D61, 11, 1),
    // TTS-3 (05 §7.14 "speech volume relative to music, music ducking or
    // pausing while speaking"): the speech's gain (orpheus-tts
    // `mix::SPEECH_DB`) and what the music does while speech sounds.
    #[cfg(feature = "screen-reader")]
    c(0x100C, Str::SpeechVolume, ch![S(Str::TextNormal), Lit("−6 dB"), Lit("−12 dB"), Lit("−18 dB")], 0)
        .packed(0x0D61, 12, 2),
    #[cfg(feature = "screen-reader")]
    c(
        0x100D,
        Str::MusicWhileSpeaking,
        ch![S(Str::MusicLowered), S(Str::MusicPaused), S(Str::MusicUnchanged)],
        0,
    )
    .packed(0x0D61, 14, 2),
];

const SYSTEM: &[Setting] = &[
    i(0x0C01, Str::About, Info::About),
    a(0x0C02, Str::UpdateFirmware, Action::UpdateFirmware),
    i(0x0C03, Str::Diagnostics, Info::Diagnostics),
    a(0x0C04, Str::StartOriginal, Action::StartOriginal),
    a(0x0C05, Str::FactoryReset, Action::FactoryReset),
    a(0x0C06, Str::RecoveryMode, Action::Recovery),
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
    // The accessibility settings' word (A11Y-2): every field's default is 0.
    r(0x0D60, Str::GAccessibility, i16::MIN, i16::MAX, 1, Unit::None, 0),
    // The screen reader's word (A11Y-1).
    #[cfg(feature = "screen-reader")]
    r(0x0D61, Str::ScreenReader, i16::MIN, i16::MAX, 1, Unit::None, READER_WORD_DEFAULT),
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

/// The twelve groups in the order of 05 §7.11, then Accessibility
/// (A11Y-2, 05 §7.14).
pub static GROUPS: [Group; 13] = [
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
    Group { label: Str::GAccessibility, items: ACCESSIBILITY },
];

/// Every item, group by group, then the apps' own and (with the `games`
/// feature) the games' (M14): the lists of [`LISTS`], so one copy of each
/// table serves the iteration and the lookups.
pub fn all() -> impl Iterator<Item = &'static Setting> {
    LISTS.iter().flat_map(|l| l.iter())
}

/// The item lists by the high byte of their keys: groups `0x01..=0x0C`
/// in [`GROUPS`]' order, the apps' items `0x0D`, the games' `0x0E`, the
/// reading settings `0x0F` (Books), Accessibility `0x10`, the Sequencer's
/// `0x11` (with the `seq` feature only: the firmware's tables stay as
/// they were) (a test holds every key to its list's byte). Plain slices,
/// so a lookup touches one list and no closure.
static LISTS: [&[Setting]; LIST_COUNT] = {
    #[cfg(feature = "games")]
    let games: &'static [Setting] = &crate::games::SETTINGS;
    #[cfg(not(feature = "games"))]
    let games: &'static [Setting] = &[];
    #[cfg(feature = "books")]
    let books: &'static [Setting] = &crate::books::SETTINGS;
    #[cfg(not(feature = "books"))]
    let books: &'static [Setting] = &[];
    [
        SOUND,
        PLAYBACK,
        DISPLAY,
        POWER,
        DATE_TIME,
        BLUETOOTH,
        RADIO,
        RECORDING,
        USB,
        LANGUAGE,
        STORAGE,
        SYSTEM,
        APP_ITEMS,
        games,
        books,
        ACCESSIBILITY,
        #[cfg(feature = "seq")]
        &crate::seq::SETTINGS,
    ]
};

/// Number of lists (high bytes `0x01..=LIST_COUNT`).
const LIST_COUNT: usize = 16 + cfg!(feature = "seq") as usize;

/// Stored items before each list (the cache index of its first stored
/// item), computed at build time.
const FIRST_STORED: [u16; LIST_COUNT] = {
    let mut out = [0u16; LIST_COUNT];
    let mut n = 0u16;
    let mut l = 0;
    while l < LIST_COUNT {
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
    (1..=LIST_COUNT).contains(&hi).then(|| hi - 1)
}

/// Settings kept (stored, with their key and value) but offered in no
/// menu: Speed (0x0206) until the time stretch it needs exists (04 §5.1;
/// owner 2026-11-26: "take speed off the menu for now"); Output (0x0604)
/// because the YM03 has no route to choose: the Bluetooth module takes
/// its audio after the headphone amplifier (U-BT-2), so the jack and a
/// connected sink always carry the same sound and neither can be muted
/// alone (VER-13, 2026-11-27).
#[must_use]
pub const fn hidden(id: SettingId) -> bool {
    id.0 == 0x0206 || id.0 == 0x0604
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

/// Number of stored items (the games' settings with the `games` feature,
/// the reading settings' word with `books`, the Sequencer's word with
/// `seq`, the screen reader's speech rate and word with `screen-reader`).
pub const STORED: usize = 98 + GAMES_STORED + BOOKS_STORED + SEQ_STORED + READER_STORED;
#[cfg(feature = "screen-reader")]
const READER_STORED: usize = 2;
#[cfg(not(feature = "screen-reader"))]
const READER_STORED: usize = 0;
#[cfg(feature = "seq")]
const SEQ_STORED: usize = crate::seq::STORED_SETTINGS;
#[cfg(not(feature = "seq"))]
const SEQ_STORED: usize = 0;
#[cfg(feature = "games")]
const GAMES_STORED: usize = crate::games::STORED_SETTINGS;
#[cfg(not(feature = "games"))]
const GAMES_STORED: usize = 0;
#[cfg(feature = "books")]
const BOOKS_STORED: usize = crate::books::STORED_SETTINGS;
#[cfg(not(feature = "books"))]
const BOOKS_STORED: usize = 0;

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
            #[cfg(feature = "screen-reader")]
            // Saturating: any i16 may be shown (VER-3, 2026-11-27).
            Unit::Quarters => format_choice(Choice::Speed(v.unsigned_abs().saturating_mul(25)), lang, out),
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
    /// One bit per stored item (was a `u128`, full at 128 items, SEQ-2).
    dirty: [u32; DIRTY_WORDS],
}

const DIRTY_WORDS: usize = STORED.div_ceil(32);

/// Whether bit `i` of `d` is set.
fn bit(d: &[u32; DIRTY_WORDS], i: usize) -> bool {
    d.get(i / 32).is_some_and(|w| w & (1 << (i % 32)) != 0)
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
        SettingsCache { vals, dirty: [0; DIRTY_WORDS] }
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
                if let Some(w) = self.dirty.get_mut(i / 32) {
                    *w |= 1 << (i % 32);
                }
                true
            }
            _ => false,
        }
    }

    /// Back to defaults; every item is marked for writing.
    pub fn reset(&mut self) {
        *self = SettingsCache::new();
        // Every stored item, whatever their number.
        for i in 0..STORED {
            self.dirty[i / 32] |= 1 << (i % 32);
        }
    }

    /// Pending writes as (store key, bytes); clears the marks.
    pub fn take_dirty(&mut self) -> impl Iterator<Item = (u16, [u8; 2])> + '_ {
        let d = core::mem::take(&mut self.dirty);
        all().filter(|s| s.stored()).enumerate().filter(move |(i, _)| bit(&d, *i)).map(|(i, s)| {
            let v = self.vals.get(i).copied().unwrap_or(s.default);
            (s.id.0, encode(v))
        })
    }

    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.dirty.iter().any(|&w| w != 0)
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
        // The twelve groups are lists 1..=12; Accessibility (A11Y-2) is
        // list 0x10, after the apps', games' and Books' lists.
        for (g, grp) in GROUPS.iter().enumerate() {
            let l = if g < 12 { g } else { 15 };
            assert!(core::ptr::eq(grp.items, LISTS[l]), "group {g}");
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

    /// A11Y-1: the screen reader's settings follow A11Y-2's in the group;
    /// the rate is stored on its own, the others are bits of the word
    /// 0x0D61 (its default every field's default), set and read without
    /// touching each other.
    #[test]
    #[cfg(feature = "screen-reader")]
    fn reader_packed() {
        // A11Y-1: the screen reader's rows after A11Y-2's, the rate stored
        // on its own, the rest in the word 0x0D61 whose default is every
        // field's default; each field set alone keeps the others.
        let grp = GROUPS.iter().find(|g| g.label == Str::GAccessibility).unwrap();
        let ids: Vec<u16> = grp.items.iter().map(|s| s.id.0).collect();
        assert_eq!(
            ids,
            [
                0x1001, 0x1002, 0x1003, 0x1004, 0x1005, 0x1006, 0x1007, 0x1008, 0x1009, 0x100A, 0x100B,
                0x100C, 0x100D
            ]
        );
        let mut c = SettingsCache::new();
        let packed: Vec<&Setting> =
            grp.items.iter().filter(|s| s.pack.is_some_and(|p| p.0 == id::READER_WORD)).collect();
        assert_eq!(packed.len(), 9);
        for s in &packed {
            assert_eq!(c.get(s.id), s.default, "{:#06x}", s.id.0);
        }
        assert_eq!(c.get(id::SPEECH_RATE), 4, "1×");
        assert!(def(id::SPEECH_RATE).is_some_and(Setting::stored));
        let before: Vec<i16> = packed.iter().map(|s| c.get(s.id)).collect();
        for (k, s) in packed.iter().enumerate() {
            let v = s.step(c.get(s.id), 1);
            assert!(c.set(s.id, v));
            for (j, t) in packed.iter().enumerate() {
                let want = if j == k { v } else { before[j] };
                assert_eq!(c.get(t.id), want, "{:#06x} after {:#06x}", t.id.0, s.id.0);
            }
            c.set(s.id, before[k]);
        }
        assert_eq!(c.get(id::READER_WORD), READER_WORD_DEFAULT);
        // The rate: 0.25× to 10× in 0.25× steps, out of range the default.
        assert!(c.set(id::SPEECH_RATE, 40));
        assert_eq!(c.get(id::SPEECH_RATE), 40);
        c.set(id::SPEECH_RATE, 41);
        assert_eq!(c.get(id::SPEECH_RATE), 4);
        let d = def(id::SPEECH_RATE).unwrap();
        let mut b = StrBuf::<12>::new();
        format_value(d, 1, Lang::ENGLISH, &mut b);
        assert_eq!(b.as_str(), "0.25×");
        format_value(d, 40, Lang::ENGLISH, &mut b);
        assert_eq!(b.as_str(), "10.0×");
    }

    /// A11Y-2: Text size, Hold delay and Key repeat live in bits 0–1, 2–4
    /// and 5–6 of the word 0x0D60 (default 0: every field's default), are
    /// set and read through it without touching each other, and the group
    /// shows them in this order.
    #[test]
    fn accessibility_packed() {
        let grp = GROUPS.iter().find(|g| g.label == Str::GAccessibility).unwrap();
        // The screen reader's rows (A11Y-1, feature `screen-reader`) follow.
        let items: Vec<&Setting> = grp.items.iter().filter(|s| s.id.0 <= 0x1003).collect();
        let ids: Vec<u16> = items.iter().map(|s| s.id.0).collect();
        assert_eq!(ids, [0x1001, 0x1002, 0x1003]);
        assert_eq!(grp.items.len(), if cfg!(feature = "screen-reader") { 13 } else { 3 });
        for s in items {
            assert_eq!(s.pack.map(|p| p.0), Some(id::A11Y_WORD), "{:#06x}", s.id.0);
            assert_eq!(s.default, 0);
        }
        assert_eq!(def(id::A11Y_WORD).unwrap().default, 0);
        let mut c = SettingsCache::new();
        assert!(c.set(id::TEXT_SIZE, 2));
        assert!(c.set(id::HOLD_DELAY, 4));
        assert!(c.set(id::REPEAT_SPEED, 1));
        assert_eq!(c.get(id::A11Y_WORD), 2 | (4 << 2) | (1 << 5));
        assert_eq!((c.get(id::TEXT_SIZE), c.get(id::HOLD_DELAY), c.get(id::REPEAT_SPEED)), (2, 4, 1));
        c.set(id::HOLD_DELAY, 7);
        assert_eq!(c.get(id::HOLD_DELAY), 0, "out of range: the default");
        assert_eq!((c.get(id::TEXT_SIZE), c.get(id::REPEAT_SPEED)), (2, 1), "the others untouched");
        assert_eq!(HOLD_DELAY_MS.len(), 5);
        let written: Vec<u16> = c.take_dirty().map(|(k, _)| k).collect();
        assert_eq!(written, [id::A11Y_WORD.0], "one record");
    }

    /// Regression: the M7 merge gave the radio frequency the step goal's
    /// store key 0x0D20 (2026-11-24; the radio's moved to 0x0D50): every
    /// table id is unique (with `id_tests` for the named ids).
    #[test]
    fn table_is_consistent() {
        let mut ids = BTreeSet::new();
        for (g, grp) in GROUPS.iter().enumerate() {
            for s in grp.items {
                assert!(ids.insert(s.id), "duplicate id {:#06x}", s.id.0);
                let byte = if g < 12 { g + 1 } else { 0x10 };
                assert_eq!(usize::from(s.id.0 >> 8), byte, "id {:#06x} in group {g}", s.id.0);
                assert_eq!(s.validate(s.default), s.default, "default of {:#06x} invalid", s.id.0);
            }
        }
        assert_eq!(all().filter(|s| s.stored()).count(), STORED);
        const { assert!(STORE_KEYS + MIN_FREE_STORE_KEYS <= orpheus_settings::MAX_KEYS) };
    }

    /// 2026-11-26 (SEQ-2): with every app's settings (games, Salvo,
    /// Bricks, Books, Sequencer) the settings used 128 of the store's 128
    /// index keys, and with the Bluetooth address, the radio presets and
    /// the retired Brightness key 131 keys were needed: after a factory
    /// reset the store refused the last writes (`TooManyKeys`, ignored by
    /// the firmware: those values reverted at the next start), and the
    /// firmware alone needed 129. The index now holds
    /// [`orpheus_settings::MAX_KEYS`] (192).
    ///
    /// The real store, written as the firmware writes it after a factory
    /// reset on a player that had an older firmware, keeps every value, has
    /// at least [`MIN_FREE_STORE_KEYS`] keys free, takes that many new
    /// two-byte settings, and still compacts with the presets rewritten.
    #[test]
    fn regress_full_feature_settings_tree_fits_with_headroom() {
        use embassy_futures::block_on;
        use orpheus_settings::{MAX_KEYS, MAX_VALUE, Nor, SECTOR, Store};

        struct Ram(Vec<u8>);
        impl Nor for Ram {
            type Error = ();
            async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), ()> {
                buf.copy_from_slice(&self.0[off as usize..off as usize + buf.len()]);
                Ok(())
            }
            async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
                for (m, d) in self.0[off as usize..].iter_mut().zip(data) {
                    *m &= *d;
                }
                Ok(())
            }
            async fn erase(&mut self, off: u32) -> Result<(), ()> {
                self.0[off as usize..(off + SECTOR) as usize].fill(0xFF);
                Ok(())
            }
        }

        // With every feature (as `cargo test --workspace` builds it: the
        // simulator turns them all on) this is the full tree.
        #[cfg(all(
            feature = "games",
            feature = "salvo",
            feature = "bricks",
            feature = "books",
            feature = "seq"
        ))]
        const {
            // 128 when the bug was found; Performance (0x0C07) retired since
            // (PWR-5): the store keys it needed are the same (RETIRED).
            assert!(STORED >= 127 && STORE_KEYS >= 131);
        };
        let free = MAX_KEYS - STORE_KEYS;
        assert!(free >= MIN_FREE_STORE_KEYS, "{STORE_KEYS} of {MAX_KEYS} store keys: {free} free");

        let presets = [0x5Au8; orpheus_fm::presets::BYTES];
        let mut s = block_on(Store::<Ram, 4>::mount(Ram(std::vec![0xFF; 4 * SECTOR as usize]))).unwrap();
        for &k in RETIRED {
            block_on(s.set(k, &encode(10))).unwrap();
        }
        let mut c = SettingsCache::new();
        c.reset();
        let written: Vec<(u16, [u8; 2])> = c.take_dirty().collect();
        assert_eq!(written.len(), STORED);
        for (k, v) in &written {
            block_on(s.set(*k, v)).unwrap();
        }
        block_on(s.set(BT_LAST_KEY, &[1, 2, 3, 4, 5, 6])).unwrap();
        block_on(s.set(orpheus_fm::presets::STORE_KEY, &presets)).unwrap();
        assert_eq!(s.indexed(), STORE_KEYS);
        assert!(MAX_KEYS - s.indexed() >= MIN_FREE_STORE_KEYS);

        // The free keys are usable: new settings, then every value and the
        // presets rewritten through several compactions.
        let new: Vec<u16> = (0..MIN_FREE_STORE_KEYS as u16).map(|i| 0x1F00 + i).collect();
        for &k in &new {
            block_on(s.set(k, &encode(k as i16))).unwrap();
        }
        let erases = s.erases;
        for round in 0..6i16 {
            for (k, _) in &written {
                block_on(s.set(*k, &encode(round))).unwrap();
            }
            block_on(s.set(orpheus_fm::presets::STORE_KEY, &presets[..presets.len() - round as usize]))
                .unwrap();
        }
        assert!(s.erases >= erases + 3, "compacted: {} erases", s.erases - erases);

        let mut s = block_on(Store::<Ram, 4>::mount(s.into_inner())).unwrap();
        let get = |s: &mut Store<Ram, 4>, k: u16| {
            let mut b = [0u8; MAX_VALUE];
            block_on(s.get(k, &mut b)).unwrap().map(|n| b[..n].to_vec())
        };
        for (k, _) in &written {
            assert_eq!(get(&mut s, *k), Some(encode(5).to_vec()), "{k:#06x}");
        }
        for &k in &new {
            assert_eq!(get(&mut s, k), Some(encode(k as i16).to_vec()));
        }
        assert_eq!(get(&mut s, BT_LAST_KEY), Some(std::vec![1, 2, 3, 4, 5, 6]));
        assert_eq!(get(&mut s, orpheus_fm::presets::STORE_KEY).map(|v| v.len()), Some(presets.len() - 5));
        assert_eq!(s.indexed(), STORE_KEYS + MIN_FREE_STORE_KEYS);
    }

    /// The cache's write marks cover every stored item, however many
    /// (2026-11-26: a `u128`, full at 128): a reset marks all, each
    /// item's own change marks only it.
    #[test]
    fn dirty_marks_cover_every_stored_item() {
        let mut c = SettingsCache::new();
        assert!(!c.is_dirty());
        c.reset();
        let all_keys: Vec<u16> = c.take_dirty().map(|(k, _)| k).collect();
        assert_eq!(all_keys.len(), STORED);
        assert!(!c.is_dirty());
        let stored: Vec<&Setting> = all().filter(|s| s.stored()).collect();
        for s in [stored[0], stored[31], stored[32], stored[STORED - 1]] {
            let mut c = SettingsCache::new();
            let v =
                (s.default..s.default.saturating_add(400)).find(|&v| v != s.default && s.validate(v) == v);
            let Some(v) =
                v.or_else(|| (s.default.saturating_sub(400)..s.default).find(|&v| s.validate(v) == v))
            else {
                continue;
            };
            assert!(c.set(s.id, v), "{:#06x}", s.id.0);
            let w: Vec<(u16, [u8; 2])> = c.take_dirty().collect();
            assert_eq!(w, [(s.id.0, encode(v))]);
        }
    }

    /// Salvo's settings live in bits of four stored words (GAME-4/5): each
    /// reads and writes under its own key, validated, without touching the
    /// others; the words' defaults hold every default; fields never
    /// overlap.
    #[cfg(feature = "salvo")]
    #[test]
    fn packed_settings() {
        let mut c = SettingsCache::new();
        // The games' (the reading settings of Books are packed too: their
        // own test, `books::tests::settings_table`).
        let packed: Vec<&Setting> = all().filter(|s| s.pack.is_some() && s.id.0 >> 8 == 0x0E).collect();
        assert_eq!(packed.len(), 22 + if cfg!(feature = "bricks") { 7 } else { 0 });
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

    /// Bricks' seven settings live in one stored word (GAME-6): fields
    /// apart, the defaults in the word, each read and written on its own.
    #[cfg(feature = "bricks")]
    #[test]
    fn bricks_packed_settings() {
        let mut c = SettingsCache::new();
        let packed: Vec<&Setting> =
            all().filter(|s| s.pack.is_some_and(|(w, _, _)| w == SettingId(0x0EB0))).collect();
        assert_eq!(packed.len(), 7);
        let mut used = 0u16;
        for s in &packed {
            let (_, shift, bits) = s.pack.unwrap();
            let mask = (((1u32 << bits) - 1) << shift) as u16;
            assert_eq!(used & mask, 0, "{:#06x} overlaps", s.id.0);
            used |= mask;
            assert_eq!(c.get(s.id), s.default);
            let max = match s.kind {
                Kind::Toggle => 1,
                Kind::Choice(ch) => ch.len() as i32 - 1,
                Kind::Range { max, .. } => i32::from(max),
                _ => 0,
            };
            assert!(max < 1 << bits, "{:#06x} does not fit {bits} bits", s.id.0);
        }
        assert_eq!(c.get(SettingId(0x0EB0)), 1 | 1 << 4 | 1 << 6 | 1 << 8 | 1 << 10 | 2 << 11 | 1 << 13);
        let level = SettingId(0x0EA1);
        assert!(c.set(level, 10));
        assert_eq!((c.get(level), c.get(SettingId(0x0EA2))), (10, 1));
        assert!(c.set(level, 11), "out of range: the default");
        assert_eq!(c.get(level), 1);
        let w: Vec<_> = c.take_dirty().collect();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].0, 0x0EB0);
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
        assert_eq!(d.get(id::BRIGHTNESS), 6, "short record: default");
        d.load(id::UTC_OFFSET.0, &encode(7));
        assert_eq!(d.get(id::UTC_OFFSET), 0, "not on the 15-minute grid");
        d.reset();
        assert_eq!(d.take_dirty().count(), STORED);
    }

    /// Regression (DOC-2 finding, 2026-11-27): Settings › Recording ›
    /// Source offered "Line-in", but the YM03 has no line input (01 §2,
    /// U-AUD-4), so choosing it recorded nothing useful. The value is
    /// retired: two choices, and a store that holds 2 reads as Microphone.
    #[test]
    fn regress_rec_source_line_in_retired() {
        let d = def(id::REC_SOURCE).unwrap();
        let Kind::Choice(cs) = d.kind else { panic!("Source is a choice") };
        assert_eq!(cs.len(), 2, "Microphone and FM radio only");
        let mut c = SettingsCache::new();
        c.load(id::REC_SOURCE.0, &encode(2));
        assert_eq!(c.get(id::REC_SOURCE), 0, "a stored Line-in reads as Microphone");
        assert!(!c.set(id::REC_SOURCE, 2), "Line-in cannot be chosen");
        assert_eq!(crate::services::rec_source(2), crate::services::RecSource::Mic);
        c.load(id::REC_SOURCE.0, &encode(1));
        assert_eq!(c.get(id::REC_SOURCE), 1, "FM radio stays");
    }

    /// Display › Brightness offers the stock levels 1–11, default 6
    /// (U-PWM-3), and every value it can hold is a stock duty.
    #[test]
    fn brightness_is_the_stock_levels() {
        let d = def(id::BRIGHTNESS).unwrap();
        assert!(matches!(d.kind, Kind::Range { min: 1, max: 11, step: 1, .. }), "{:?}", d.kind);
        assert_eq!(d.default, 6);
        assert_eq!(d.label, Str::Brightness);
        let display: Vec<u16> = DISPLAY.iter().map(|s| s.id.0).collect();
        assert_eq!(display[0], id::BRIGHTNESS.0, "still the first Display row");
        for v in i16::MIN..=i16::MAX {
            let lvl = d.validate(v);
            assert!((1..=11).contains(&lvl), "{v} → {lvl}");
            let duty = orpheus_power::backlight::Duty::of_level(lvl).get();
            assert!(orpheus_power::backlight::LEVEL_DUTY[1..].contains(&duty));
        }
    }

    /// A store of the firmware before 2026-11-26 holds Brightness under
    /// `0x0301` on a 1–16 scale (default 10; it lit nothing). That key is
    /// retired: the old record is skipped and Brightness reads the stock
    /// default 6 (duty 8), whatever the old value; a retired key is no
    /// setting and none reuses it.
    #[test]
    fn old_brightness_record_reads_the_stock_default() {
        for old in [1i16, 6, 10, 11, 12, 16, -1] {
            let mut c = SettingsCache::new();
            c.load(0x0301, &encode(old));
            assert_eq!(c.get(id::BRIGHTNESS), 6, "old 0x0301={old}");
            assert!(!c.is_dirty(), "nothing written");
        }
        // Beside a new record, the new one wins whatever the order.
        let mut c = SettingsCache::new();
        c.load(id::BRIGHTNESS.0, &encode(9));
        c.load(0x0301, &encode(16));
        assert_eq!(c.get(id::BRIGHTNESS), 9);
        for k in RETIRED {
            assert!(def(SettingId(*k)).is_none(), "{k:#06x} reused");
            assert!(all().all(|s| s.id.0 != *k && s.pack.is_none_or(|p| p.0.0 != *k)), "{k:#06x}");
        }
    }

    #[test]
    fn list_roll_around_is_a_display_toggle_on_by_default() {
        let s = def(id::LIST_ROLL).unwrap();
        assert_eq!((s.kind, s.default, s.label), (Kind::Toggle, 1, Str::ListRollAround));
        let display = &GROUPS[2];
        assert_eq!(display.label, Str::GDisplay);
        let pos = display.items.iter().position(|s| s.id == id::LIST_ROLL).unwrap();
        assert_eq!(display.items.get(pos + 1).map(|s| s.id), Some(id::HOME_HIDDEN));
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

/// VER-13 (07 §5.12): persistence, defaults, out-of-range values and
/// stores from an older firmware, for every setting of each kind (toggle,
/// choice, range, language, Home-screen mask, packed fields and words).
/// "Reboot" is the firmware's path: the cache's dirty records (what it
/// writes to the `orpheus-settings` store, whose own tests cover the NOR
/// side and power loss) loaded into a fresh cache.
#[cfg(test)]
mod persistence {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// The settings of a kind (`packed`: the fields kept in a word and the
    /// words themselves).
    fn of(kind: &str) -> Vec<&'static Setting> {
        let words: Vec<SettingId> = all().filter_map(|s| s.pack.map(|p| p.0)).collect();
        let v: Vec<&'static Setting> = all()
            .filter(|s| match (kind, s.kind) {
                ("packed", _) => s.pack.is_some() || words.contains(&s.id),
                (_, Kind::Action(_) | Kind::Info(_)) => false,
                _ if words.contains(&s.id) => false,
                ("toggle", Kind::Toggle) | ("choice", Kind::Choice(_)) | ("range", Kind::Range { .. }) => {
                    true
                }
                ("language", Kind::Language) | ("homeapps", Kind::HomeApps) => true,
                _ => false,
            })
            .collect();
        assert!(!v.is_empty(), "no {kind} setting");
        v
    }

    /// Those with a value other than the default (Wallpaper has one).
    fn changeable(kind: &str) -> Vec<&'static Setting> {
        of(kind).into_iter().filter(|s| other(s) != s.default).collect()
    }

    /// A valid value other than the default.
    fn other(s: &Setting) -> i16 {
        match s.kind {
            Kind::Toggle => 1 - s.default,
            Kind::Choice(c) => (s.default + 1) % i16::try_from(c.len()).unwrap(),
            Kind::Range { min, max, step, .. } => {
                if s.default <= max - step {
                    s.default + step
                } else {
                    min
                }
            }
            Kind::Language => lang_code("ru"),
            Kind::HomeApps => s.default | 1,
            Kind::Action(_) | Kind::Info(_) => s.default,
        }
    }

    /// Values a store can hold that the setting cannot take.
    fn invalid(s: &Setting) -> Vec<i16> {
        match s.kind {
            Kind::Toggle => std::vec![2, -1, i16::MAX],
            Kind::Choice(c) => std::vec![i16::try_from(c.len()).unwrap(), -1, i16::MAX],
            Kind::Range { min, max, step, .. } => {
                let mut v = std::vec![];
                if min > i16::MIN {
                    v.push(min - 1);
                }
                if max < i16::MAX {
                    v.push(max + 1);
                }
                if step > 1 {
                    v.push(min + 1);
                }
                v
            }
            Kind::HomeApps => std::vec![-1, (1i32 << crate::App::COUNT) as i16],
            _ => std::vec![],
        }
    }

    /// Set to another value, written and read back after a reboot; the
    /// other settings keep their defaults.
    fn persist(kind: &str) {
        for s in changeable(kind) {
            let mut c = SettingsCache::new();
            let v = other(s);
            assert!(c.set(s.id, v), "{:#06x} set", s.id.0);
            let w: Vec<_> = c.take_dirty().collect();
            assert_eq!(w.len(), 1, "{:#06x}: one record written", s.id.0);
            let mut after = SettingsCache::new();
            for (k, b) in &w {
                after.load(*k, b);
            }
            assert_eq!(after.get(s.id), v, "{:#06x} after a reboot", s.id.0);
            for t in all().filter(|t| t.stored() && t.id != s.id && Some(t.id) != s.pack.map(|p| p.0)) {
                assert_eq!(after.get(t.id), t.default, "{:#06x} untouched by {:#06x}", t.id.0, s.id.0);
            }
        }
    }

    /// A fresh store gives the default; a factory reset brings it back
    /// and writes it.
    fn default(kind: &str) {
        for s in of(kind) {
            let mut c = SettingsCache::new();
            assert_eq!(c.get(s.id), s.default, "{:#06x}", s.id.0);
            c.set(s.id, other(s));
            c.reset();
            assert_eq!(c.get(s.id), s.default, "{:#06x} after a factory reset", s.id.0);
            let key = s.pack.map_or(s.id, |p| p.0);
            assert!(c.take_dirty().any(|(k, _)| k == key.0), "{:#06x}: the reset is written", s.id.0);
        }
    }

    /// An out-of-range stored value (or one set) gives the default (the
    /// cache validates; it does not clamp to the nearest end, so a damaged
    /// record never yields an extreme).
    fn clamp(kind: &str) {
        for s in of(kind) {
            for bad in invalid(s) {
                let mut c = SettingsCache::new();
                if s.pack.is_none() {
                    c.load(s.id.0, &encode(bad));
                    assert_eq!(c.get(s.id), s.default, "{:#06x} loaded {bad}", s.id.0);
                }
                c.set(s.id, other(s));
                c.set(s.id, bad);
                assert_eq!(c.get(s.id), s.default, "{:#06x} set {bad}", s.id.0);
            }
        }
    }

    /// A store written by an older firmware: records this one does not
    /// know are skipped, a record of another length is the default, and
    /// a setting the old store lacks is the default; the known records
    /// still load.
    fn migrate(kind: &str) {
        for s in changeable(kind) {
            let key = s.pack.map_or(s.id, |p| p.0);
            let mut old = SettingsCache::new();
            old.set(s.id, other(s));
            let rec: Vec<_> = old.take_dirty().collect();
            let mut c = SettingsCache::new();
            // Keys of no setting (removed items, the old key ranges).
            for k in [0x0000u16, 0x00FF, 0x0110, 0x0D2F, 0x0EFF, 0x10FF, 0x11FF, 0x1201, 0xFFFF] {
                assert!(def(SettingId(k)).is_none());
                c.load(k, &encode(1));
            }
            for (k, b) in &rec {
                c.load(*k, b);
            }
            assert_eq!(c.get(s.id), other(s), "{:#06x} beside unknown keys", s.id.0);
            for bytes in [&[1u8][..], &[1, 0, 0, 0], &[]] {
                let mut c = SettingsCache::new();
                c.load(key.0, bytes);
                assert_eq!(c.get(s.id), s.default, "{:#06x}: a {}-byte record", s.id.0, bytes.len());
            }
            assert_eq!(SettingsCache::new().get(s.id), s.default, "{:#06x} missing from the store", s.id.0);
        }
    }

    #[test]
    fn persist_toggle() {
        persist("toggle");
    }
    #[test]
    fn persist_choice() {
        persist("choice");
    }
    #[test]
    fn persist_range() {
        persist("range");
    }
    #[test]
    fn persist_language() {
        persist("language");
    }
    #[test]
    fn persist_homeapps() {
        persist("homeapps");
    }
    #[cfg(any(feature = "salvo", feature = "bricks", feature = "books", feature = "seq"))]
    #[test]
    fn persist_packed() {
        persist("packed");
    }
    #[test]
    fn default_toggle() {
        default("toggle");
    }
    #[test]
    fn default_choice() {
        default("choice");
    }
    #[test]
    fn default_range() {
        default("range");
    }
    #[test]
    fn default_language() {
        default("language");
    }
    #[test]
    fn default_homeapps() {
        default("homeapps");
    }
    #[cfg(any(feature = "salvo", feature = "bricks", feature = "books", feature = "seq"))]
    #[test]
    fn default_packed() {
        default("packed");
    }
    #[test]
    fn clamp_toggle() {
        clamp("toggle");
    }
    #[test]
    fn clamp_choice() {
        clamp("choice");
    }
    #[test]
    fn clamp_range() {
        clamp("range");
    }
    /// Every two-letter code is a valid language value (a code without a
    /// pack shows English: the `Lang` lookup), so there is nothing to
    /// clamp; a record of another length still gives the default.
    #[test]
    fn clamp_language() {
        for s in of("language") {
            let mut c = SettingsCache::new();
            c.load(s.id.0, &encode(lang_code("xx")));
            assert_eq!(c.get(s.id), lang_code("xx"));
            c.load(s.id.0, &[7]);
            assert_eq!(c.get(s.id), s.default);
        }
    }
    #[test]
    fn clamp_homeapps() {
        clamp("homeapps");
    }
    #[cfg(any(feature = "salvo", feature = "bricks", feature = "books", feature = "seq"))]
    #[test]
    fn clamp_packed() {
        clamp("packed");
    }
    #[test]
    fn migrate_toggle() {
        migrate("toggle");
    }
    #[test]
    fn migrate_choice() {
        migrate("choice");
    }
    #[test]
    fn migrate_range() {
        migrate("range");
    }
    #[test]
    fn migrate_language() {
        migrate("language");
    }
    #[test]
    fn migrate_homeapps() {
        migrate("homeapps");
    }
    #[cfg(any(feature = "salvo", feature = "bricks", feature = "books", feature = "seq"))]
    #[test]
    fn migrate_packed() {
        migrate("packed");
    }
}
