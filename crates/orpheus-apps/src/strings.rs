//! User-visible strings (05 §5.3): `tr!(lang, Id)`.
//!
//! English is built in ([`EN`]). A language pack is a `'static` byte blob
//! (`.ylang`, loaded by the firmware into a static arena from the card or
//! the NOR resource area, 02 §7) in this format, little-endian:
//!
//! ```text
//! "YLN1" | u16 count | u8 code_len | u8 name_len | code | name
//!        | count x (u16 offset, u16 len) | UTF-8 data
//! ```
//!
//! Entry `i` translates `Str` number `i`; `len == 0` (or anything invalid)
//! falls back to English, so a pack may be partial or older than the
//! firmware. Strings are referenced by number, so new ids are only ever
//! appended.

/// A language: English, or English plus a pack.
#[derive(Clone, Copy, Debug, Default)]
pub struct Lang {
    pack: Option<&'static [u8]>,
}

const MAGIC: &[u8; 4] = b"YLN1";

fn rd16(d: &[u8], o: usize) -> Option<u16> {
    Some(u16::from_le_bytes([*d.get(o)?, *d.get(o + 1)?]))
}

impl Lang {
    pub const ENGLISH: Lang = Lang { pack: None };

    /// Use a pack; `None` if the header is malformed.
    #[must_use]
    pub fn from_pack(pack: &'static [u8]) -> Option<Lang> {
        if pack.get(..4)? != MAGIC {
            return None;
        }
        let count = usize::from(rd16(pack, 4)?);
        let hdr = 8 + usize::from(*pack.get(6)?) + usize::from(*pack.get(7)?);
        if pack.len() < hdr + count * 4 {
            return None;
        }
        Some(Lang { pack: Some(pack) })
    }

    fn parts(p: &'static [u8]) -> Option<(usize, usize, usize)> {
        let count = usize::from(rd16(p, 4)?);
        let code_len = usize::from(*p.get(6)?);
        let name_len = usize::from(*p.get(7)?);
        let table = 8 + code_len + name_len;
        Some((count, table, table + count * 4))
    }

    /// ISO code of the pack (`"en"` without one).
    #[must_use]
    pub fn code(&self) -> &'static str {
        self.pack
            .and_then(|p| core::str::from_utf8(p.get(8..8 + usize::from(*p.get(6)?))?).ok())
            .unwrap_or("en")
    }

    /// Name of the language in itself (`"English"` without a pack).
    #[must_use]
    pub fn name(&self) -> &'static str {
        self.pack
            .and_then(|p| {
                let c = usize::from(*p.get(6)?);
                let n = usize::from(*p.get(7)?);
                core::str::from_utf8(p.get(8 + c..8 + c + n)?).ok()
            })
            .unwrap_or("English")
    }

    fn lookup(&self, i: usize) -> Option<&'static str> {
        let p = self.pack?;
        let (count, table, data) = Self::parts(p)?;
        if i >= count {
            return None;
        }
        let off = usize::from(rd16(p, table + 4 * i)?);
        let len = usize::from(rd16(p, table + 4 * i + 2)?);
        if len == 0 {
            return None;
        }
        core::str::from_utf8(p.get(data + off..data + off + len)?).ok()
    }

    /// The string for `s` in this language, English if the pack lacks it.
    #[must_use]
    pub fn get(&self, s: Str) -> &'static str {
        let i = s as usize;
        self.lookup(i).unwrap_or_else(|| EN.get(i).copied().unwrap_or(""))
    }
}

/// `tr!(lang, Id)`: the string `Str::Id` in `lang`.
#[macro_export]
macro_rules! tr {
    ($lang:expr, $id:ident) => {
        $lang.get($crate::strings::Str::$id)
    };
}

macro_rules! strings {
    ($($id:ident = $en:literal,)*) => {
        /// String ids. Append only: packs index by number.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        #[repr(u16)]
        pub enum Str { $($id,)* }

        /// English, indexed by `Str as usize`.
        pub static EN: &[&str] = &[$($en,)*];

        impl Str {
            /// All ids in order (pack builders, tests).
            pub const ALL: &'static [Str] = &[$(Str::$id,)*];
        }
    };
}

strings! {
    // Apps (Home, 05 §6.2).
    AppMusic = "Music",
    AppFiles = "Files",
    AppRadio = "Radio",
    AppBluetooth = "Bluetooth",
    AppRecorder = "Recorder",
    AppPictures = "Pictures",
    AppSteps = "Steps",
    AppStopwatch = "Stopwatch",
    AppAuthenticator = "Authenticator",
    AppUsbDrive = "USB Drive",
    AppSettings = "Settings",
    // Settings groups (05 §7.11).
    GSound = "Sound",
    GPlayback = "Playback",
    GDisplay = "Display",
    GPower = "Power",
    GDateTime = "Date and time",
    GRecording = "Recording",
    GUsb = "USB",
    GLanguage = "Language",
    GStorage = "Storage",
    GSystem = "System",
    // Sound.
    Equaliser = "Equaliser",
    Balance = "Balance",
    Bass = "Bass",
    Crossfeed = "Crossfeed",
    ChannelMode = "Channel mode",
    ReplayGain = "ReplayGain",
    VolumeLimit = "Volume limit",
    KeyClick = "Key click",
    FadeOnPause = "Fade on pause",
    EqFlat = "Flat",
    EqRock = "Rock",
    EqPop = "Pop",
    EqJazz = "Jazz",
    EqClassical = "Classical",
    EqElectronic = "Electronic",
    EqHipHop = "Hip-hop",
    EqBassBoost = "Bass boost",
    EqVocal = "Vocal",
    EqCustom1 = "Custom 1",
    EqCustom2 = "Custom 2",
    Off = "Off",
    On = "On",
    Low = "Low",
    Medium = "Medium",
    High = "High",
    Stereo = "Stereo",
    Mono = "Mono",
    SwapLr = "Swap left/right",
    Track = "Track",
    Album = "Album",
    Auto = "Auto",
    // Playback.
    Shuffle = "Shuffle",
    Repeat = "Repeat",
    RepeatOne = "One",
    RepeatAll = "All",
    Gapless = "Gapless",
    ResumeAtStart = "Resume at start-up",
    SeekStep = "Seek step",
    Speed = "Speed",
    AutoBookmark = "Bookmark long files",
    TagCodePage = "Tag code page",
    CpWestern = "Western (1252)",
    CpCyrillic = "Cyrillic (1251)",
    CpCentral = "Central European (1250)",
    CpJapanese = "Japanese (Shift-JIS)",
    CpChineseS = "Chinese (GBK)",
    CpChineseT = "Chinese (Big5)",
    CpKorean = "Korean (EUC-KR)",
    // Display.
    Brightness = "Brightness",
    BacklightTimeout = "Backlight",
    LockDelay = "Auto-lock",
    Theme = "Theme",
    ThemeDark = "Dark",
    ThemeLight = "Light",
    ThemeAmber = "Black and amber",
    Accent = "Accent colour",
    Blue = "Blue",
    Green = "Green",
    Pink = "Pink",
    Red = "Red",
    Grey = "Grey",
    Wallpaper = "Wallpaper",
    None_ = "None",
    ScreenSaver = "Screen saver",
    Clock = "Clock",
    Animations = "Animations",
    Rotate = "Rotate 180°",
    ClockInBar = "Clock in status bar",
    HomeScreen = "Home screen",
    Always = "Always on",
    Immediately = "At once",
    Never = "Never",
    // Power.
    PowerMode = "Power mode",
    BatterySaver = "Battery saver",
    HighPerformance = "High performance",
    SleepTimer = "Sleep timer",
    EndOfTrack = "End of track",
    AutoOff = "Auto power-off",
    KeysLocked = "Keys when locked",
    KeysVolume = "Volume",
    KeysTransport = "Volume and playback",
    Battery = "Battery",
    // Date and time.
    SetTime = "Set time",
    SetDate = "Set date",
    Clock24 = "24-hour clock",
    DateFormat = "Date format",
    UtcOffset = "Time zone (UTC)",
    Correction = "Clock correction",
    SetFromRds = "Set from radio (RDS)",
    // Bluetooth.
    DeviceName = "Device name",
    Reconnect = "Reconnect automatically",
    // Radio.
    Region = "Region",
    Europe = "Europe",
    UnitedStates = "United States",
    World = "World",
    Japan = "Japan",
    Wide = "Wide band",
    ForceMono = "Force mono",
    SeekSensitivity = "Seek sensitivity",
    // Recording.
    RecSource = "Source",
    Microphone = "Microphone",
    FmRadio = "FM radio",
    LineIn = "Line-in",
    RecFormat = "Format",
    Quality = "Quality",
    QVoice = "Voice (16 kHz mono)",
    QStandard = "Standard (44.1 kHz mono)",
    QHigh = "High (44.1 kHz stereo)",
    GainMode = "Gain",
    Manual = "Manual",
    Folder = "Folder",
    // USB.
    WhenConnected = "When connected",
    Ask = "Ask",
    ChargeOnly = "Charge only",
    FileTransfer = "File transfer",
    DiskImage = "Present disk image…",
    Keyboard = "Keyboard",
    UsbAudio = "USB audio",
    LastImage = "Last disk image",
    ReadOnlyCard = "Read-only card reader",
    DevChannel = "Developer channel",
    // Storage.
    CardInfo = "Card info",
    RebuildLibrary = "Rebuild library",
    ClearCaches = "Clear caches",
    CheckCard = "Check card",
    FormatCard = "Format card…",
    Capacity = "Capacity",
    Free = "Free",
    FileSystem = "File system",
    // System.
    About = "About",
    UpdateFirmware = "Update firmware…",
    Diagnostics = "Diagnostics",
    StartOriginal = "Start original firmware",
    FactoryReset = "Factory reset…",
    RecoveryMode = "Recovery mode",
    Version = "Version",
    Build = "Build",
    Serial = "Serial number",
    Hardware = "Hardware",
    Licences = "Licences",
    Voltage = "Voltage",
    State = "State",
    Level = "Level",
    // Power menu, lock, charging, boot.
    PowerOff = "Power off",
    LockKeysOnly = "Lock keys",
    Restart = "Restart",
    PressPowerToUnlock = "Press Power to unlock",
    Charging = "Charging",
    Charged = "Charged",
    NotCharging = "Not charging",
    NoBattery = "No battery",
    ChargerFault = "Charger fault",
    UsbConnected = "USB connected",
    PressPowerToStart = "Press Power to start",
    Goodbye = "Goodbye",
    // Quick panel.
    QuickSettings = "Quick settings",
    NowPlaying = "Now Playing",
    Lock = "Lock",
    Output = "Output",
    Headphones = "Headphones",
    // Notices and dialogs.
    NoCard = "No card",
    NoCardAction = "Insert a microSD card.",
    CardUnreadable = "The card cannot be read",
    CardUnreadableAction = "Check it in Settings › Storage.",
    NotYet = "Not available yet",
    NotYetAction = "This arrives in a later version.",
    BatteryLow = "Battery low",
    BatteryLowAction = "Connect the charger soon.",
    CardRemoved = "Card removed",
    ComputerConnected = "PC connected",
    AlwaysDoThis = "Always do this",
    Cancel = "Cancel",
    Ok = "OK",
    Reset = "Reset",
    Format = "Format",
    Start = "Start",
    Rebuild = "Rebuild",
    Clear = "Clear",
    FactoryResetQ = "Reset all settings?",
    FactoryResetText = "Settings return to their defaults. The card is not touched.",
    FormatQ = "Format the card?",
    FormatText = "Everything on the card will be erased.",
    RebuildQ = "Rebuild the library?",
    RebuildText = "The card is scanned again in the background.",
    ClearQ = "Clear caches?",
    ClearText = "Thumbnails and folder indexes are rebuilt when needed.",
    StartOriginalQ = "Start the original firmware?",
    StartOriginalText = "The player restarts into the stock firmware.",
    RecoveryQ = "Restart in recovery mode?",
    RecoveryText = "For updates from a computer (ADFU).",
    Done = "Done",
    Started = "Started",
    BatteryPercent = "Battery",
    Hours = "Hours",
    Minutes = "Minutes",
    Day = "Day",
    Month = "Month",
    Year = "Year",
    HintChange = "◂ ▸ change · OK done",
    HintOpen = "OK open",
    HintQuick = "◂ ▸ change",
    Seconds = "s",
    MinutesShort = "min",
    PerDay = "s/day",
    Db = "dB",
    SortBy = "Sort",
    SortName = "Name",
    SortDate = "Date",
    SortSize = "Size",
    ShowHidden = "Show hidden",
    NpView = "View",
    ViewStandard = "Standard",
    ViewBigArt = "Big art",
    ViewLyrics = "Lyrics",
    ViewUpNext = "Up next",
    ViewDetails = "Details",
    ViewLevels = "Levels",
    CardRoot = "Card",
    PlayFolder = "Play folder",
    ShuffleFolder = "Shuffle folder",
    PlayNext = "Play next",
    AddToQueue = "Add to queue",
    Properties = "Properties",
    DeleteMenu = "Delete…",
    DeleteQ = "Delete this file?",
    DeleteBtn = "Delete",
    Deleted = "Deleted",
    AddedToQueue = "Added to the queue",
    PlayingNext = "Plays next",
    Name = "Name",
    Type = "Type",
    Size = "Size",
    Modified = "Modified",
    KindFolder = "Folder",
    KindAudio = "Audio",
    KindPlaylist = "Playlist",
    KindImage = "Picture",
    KindText = "Text",
    KindDiskImage = "Disk image",
    KindFirmware = "Firmware",
    KindOtp = "Authenticator import",
    KindOther = "File",
    EmptyFolder = "Empty folder",
    CannotOpen = "Cannot open this file",
    CannotOpenAction = "No app on the player reads this type.",
    ResumeTitle = "Resume",
    ShuffleAll = "Shuffle all",
    Artists = "Artists",
    Albums = "Albums",
    Songs = "Songs",
    Genres = "Genres",
    Playlists = "Playlists",
    Folders = "Folders",
    RecentlyAdded = "Recently added",
    Bookmarks = "Bookmarks",
    BuildingLibrary = "Building library…",
    FilesCount = "files",
    NoBookmarks = "No bookmarks yet",
    NothingPlaying = "Nothing playing",
    NothingPlayingAction = "Choose music in Folders or Shuffle all.",
    NoLyrics = "No lyrics",
    AddFavourites = "Add to Favourites",
    AddedFavourites = "Added to Favourites",
    BookmarkHere = "Bookmark here",
    BookmarkAdded = "Bookmark added",
    AbRepeat = "A–B repeat",
    SetA = "Set A",
    SetB = "Set B",
    ClearAb = "Clear",
    GoToFolder = "Go to folder",
    DeleteFile = "Delete file…",
    EqualiserDots = "Equaliser…",
    SleepTimerDots = "Sleep timer…",
    RemoveFromQueue = "Remove from queue",
    MoveUp = "Move up",
    MoveDown = "Move down",
    Codec = "Codec",
    Bitrate = "Bitrate",
    SampleRate = "Sample rate",
    Channels = "Channels",
    FileLabel = "File",
    Cpu = "Decoder CPU",
    Buffer = "Buffer",
    PreAmp = "Pre-amp",
    Kbps = "kbit/s",
    HintEq = "◂ ▸ band · OK preset",
    HintText = "◂ ▸ page",
    Mins = "min",
    // Pictures (05 §7.6).
    PicLoading = "Loading…",
    PicBusy = "Stop playback to view pictures",
    PicBusyAction = "The picture decoder shares its memory with the music.",
    PicFailed = "Cannot show this picture",
    PicUnsupported = "This kind of picture is not supported.",
    PicCorrupt = "The file is damaged or incomplete.",
    PicTooLarge = "The picture is too large for the player.",
    PicIo = "The card could not be read.",
    Slideshow = "Slideshow",
    SlideInterval = "Slideshow interval",
    RotateLeft = "Rotate left",
    RotateRight = "Rotate right",
    FillScreen = "Fill the screen",
    NoPictures = "No pictures here",
    // Music library (05 §7.1).
    UnknownArtist = "Unknown artist",
    UnknownAlbum = "Unknown album",
    UnknownGenre = "Unknown genre",
    GoToAlbum = "Go to album",
    GoToArtist = "Go to artist",
    PlayItem = "Play",
    PlayAll = "Play all",
    NoMusic = "No music found",
    // Queue edits (AU-5).
    QueueFull = "The queue is full",
}

/// Number of strings.
pub const COUNT: usize = Str::ALL.len();

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    fn build(code: &str, name: &str, entries: &[(Str, &str)]) -> Vec<u8> {
        let mut v = Vec::from(&MAGIC[..]);
        v.extend((COUNT as u16).to_le_bytes());
        v.push(code.len() as u8);
        v.push(name.len() as u8);
        v.extend(code.as_bytes());
        v.extend(name.as_bytes());
        let mut data: Vec<u8> = Vec::new();
        let mut table: Vec<u8> = Vec::new();
        for s in Str::ALL {
            match entries.iter().find(|(id, _)| id == s) {
                Some((_, t)) => {
                    table.extend((data.len() as u16).to_le_bytes());
                    table.extend((t.len() as u16).to_le_bytes());
                    data.extend(t.as_bytes());
                }
                None => table.extend([0u8; 4]),
            }
        }
        v.extend(table);
        v.extend(data);
        v
    }

    #[test]
    fn english_and_pack() {
        assert_eq!(tr!(Lang::ENGLISH, AppMusic), "Music");
        assert_eq!(EN.len(), COUNT);
        let pack: &'static [u8] =
            std::boxed::Box::leak(build("ru", "Русский", &[(Str::AppMusic, "Музыка")]).into_boxed_slice());
        let l = Lang::from_pack(pack).unwrap();
        assert_eq!(tr!(l, AppMusic), "Музыка");
        assert_eq!(tr!(l, AppFiles), "Files", "missing entries fall back to English");
        assert_eq!((l.code(), l.name()), ("ru", "Русский"));
        assert!(Lang::from_pack(b"nope").is_none());
        let truncated: &'static [u8] = std::boxed::Box::leak(pack[..20].to_vec().into_boxed_slice());
        assert!(Lang::from_pack(truncated).is_none());
    }
}
