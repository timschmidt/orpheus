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
    ($($(#[$m:meta])* $id:ident = $en:literal,)*) => {
        /// String ids. Append only: packs index by number.
        #[derive(Clone, Copy, PartialEq, Eq, Debug)]
        #[repr(u16)]
        pub enum Str { $($(#[$m])* $id,)* }

        /// English, indexed by `Str as usize`.
        pub static EN: &[&str] = &[$($(#[$m])* $en,)*];

        impl Str {
            /// All ids in order (pack builders, tests).
            pub const ALL: &'static [Str] = &[$($(#[$m])* Str::$id,)*];
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
    QVoice = "Voice (16 kHz)",
    QStandard = "Standard (44.1 kHz)",
    QHigh = "High (48 kHz)",
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
    // Bluetooth app (05 §7.4).
    BtStarting = "Starting…",
    BtNotConnected = "Not connected",
    BtSearching = "Searching…",
    BtConnecting = "Connecting…",
    BtConnected = "Connected",
    BtStreaming = "Sending audio",
    BtNotResponding = "Not responding",
    BtPairNew = "Pair new device…",
    BtPaired = "Paired devices",
    BtNoPaired = "No paired devices",
    BtAvailable = "Devices found",
    BtNoneFound = "No devices found",
    BtSearchAgain = "Search again",
    BtConnect = "Connect",
    BtDisconnect = "Disconnect",
    BtDisconnectQ = "Disconnect?",
    BtForget = "Forget…",
    BtForgetQ = "Forget this device?",
    BtForgetText = "Pair it again to use it.",
    BtForgetBtn = "Forget",
    BtFailed = "Could not connect",
    BtFailedText = "Switch the device on and put it in pairing mode.",
    BtTryAgain = "Retry",
    BtTurnOn = "Turn Bluetooth on first",
    BtDisconnected = "Disconnected",
    BtDisconnectFirst = "Disconnect first",
    HintBtScan = "OK connect",
    HintBtConnect = "Back cancel",
    // Queue edits (AU-5).
    QueueFull = "The queue is full",
    // The developer rows (PWR-5).
    Developer = "Developer",
    BatteryLog = "Battery log",
    KeepAwake = "Keep awake",
    LogRows = "Rows written",
    DeveloperOn = "Developer on",
    // Radio (UI-5a).
    RadioOff = "Radio off",
    RadioOkToPlay = "OK to play",
    RadioSeeking = "Seeking…",
    RadioScanning = "Scanning…",
    RadioFound = "found",
    RadioNoTuner = "Tuner not responding",
    RadioAntenna = "Plug in earphones: the cable is the antenna",
    RadioMuted = "Muted",
    SavePreset = "Save preset",
    RemovePreset = "Remove preset",
    PresetsDots = "Presets…",
    Presets = "Presets",
    NoPresets = "No presets",
    AutoScan = "Auto-scan",
    AutoScanQ = "Replace presets?",
    AutoScanText = "Auto-scan finds the stations and replaces the list.",
    ScanBtn = "Scan",
    StationsFound = "stations found",
    NoStations = "No stations found",
    PresetSaved = "Saved",
    PresetsFull = "The presets are full",
    StopRadio = "Stop radio",
    HintRadio = "◂ ▸ tune · OK mute",
    HintRadioScan = "Back stop",
    // Steps (UI-5c).
    TabToday = "Today",
    TabWeek = "Week",
    Goal = "Goal",
    GoalReached = "Goal reached",
    Km = "km",
    Mi = "mi",
    Kcal = "kcal",
    StepsUnit = "steps",
    DailyGoal = "Daily goal",
    StrideLength = "Stride length",
    Weight = "Weight",
    Units = "Units",
    Kilometres = "Kilometres",
    Miles = "Miles",
    CountInBackground = "Count in background",
    WhileOn = "While on",
    Sensitivity = "Sensitivity",
    ResetToday = "Reset today…",
    ResetTodayQ = "Reset today?",
    ResetBtn = "Reset",
    NoSensor = "No motion sensor",
    NoHistory = "No clock: no history",
    CountingOff = "Counts only here",
    Cm = "cm",
    Kg = "kg",
    DayMo = "Mo",
    DayTu = "Tu",
    DayWe = "We",
    DayTh = "Th",
    DayFr = "Fr",
    DaySa = "Sa",
    DaySu = "Su",
    Monday = "Monday",
    Tuesday = "Tuesday",
    Wednesday = "Wednesday",
    Thursday = "Thursday",
    Friday = "Friday",
    Saturday = "Saturday",
    Sunday = "Sunday",
    // Stopwatch, Timer, Alarm (UI-5d).
    TabTimer = "Timer",
    TabAlarm = "Alarm",
    Lap = "Lap",
    HintSwIdle = "OK start",
    HintSwRunning = "OK stop · Next lap",
    HintSwStopped = "OK start · Next reset",
    ResetStopwatchQ = "Reset stopwatch?",
    HintTimer = "OK start · Next reset",
    HintTimerRunning = "OK pause · Next reset",
    SetDuration = "Set duration…",
    TimesUp = "Time's up",
    Snooze = "Snooze",
    HintAlert = "OK stop · Next snooze",
    HintAlertTimer = "OK stop",
    AlarmTime = "Time…",
    RepeatDays = "Repeat…",
    Once = "Once",
    EveryDay = "Daily",
    Weekdays = "Mo–Fr",
    Weekend = "Sa–Su",
    AlarmOnlyOn = "Rings only while on",
    SecondsField = "Seconds",
    Duration = "Duration",
    ResetTodayText = "Today's count goes back to 0.",
    // USB keyboard (USB-5) and the Authenticator (OTP-2, 05 §7.9).
    KbdLayout = "Keyboard layout",
    KbdUs = "US",
    KbdUk = "UK",
    KbdDe = "German",
    KbdFr = "French",
    KeypadDigits = "Digits from keypad",
    EnterAfterCode = "Enter after code",
    AuthNewPin = "Choose a PIN",
    AuthConfirmPin = "Enter the PIN again",
    AuthEnterPin = "Enter PIN",
    AuthNewPinChange = "New PIN",
    AuthPinsDiffer = "The PINs differ",
    AuthWrongPin = "Wrong PIN",
    AuthWait = "Wait",
    AuthUnlocking = "Unlocking…",
    AuthSettingUp = "Setting up…",
    AuthChangingPin = "Changing PIN…",
    AuthPinWarning = "A PIN stops casual use, not someone with a cable.",
    HintPinPad = "OK key · Back delete",
    HintPinDone = "Hold OK: done",
    AuthNoAccounts = "No accounts yet",
    AuthNoAccountsAction = "Copy otpauth:// lines into /.orpheus/otp-import.txt on the card, then Menu › Import.",
    AuthClockNotSet = "Clock not set",
    AuthClockNotSetAction = "Set the time from a computer or in Settings.",
    HintAuthList = "OK show code",
    HintAuthCode = "OK type the code",
    HintAuthHotp = "Menu › Next code",
    AuthTyping = "Typing…",
    AuthTyped = "Typed",
    AuthTypeFailed = "Could not type the code",
    AuthNoComputer = "Connect a computer",
    AuthNoCode = "Make a code first",
    AuthImport = "Import from card",
    AuthNextCode = "Next code",
    AuthTypeCode = "Type the code",
    AuthDeleteQ = "Delete this account?",
    AuthDeleteText = "Its codes are gone for good.",
    AuthTimeCheck = "Time check",
    AuthLock = "Lock",
    AuthChangePin = "Change PIN",
    AuthWipe = "Wipe vault…",
    AuthWipeQ = "Wipe the vault?",
    AuthWipeText = "Every account is deleted.",
    WipeBtn = "Wipe",
    AuthFileDeleted = "File deleted",
    AuthFileKept = "File kept",
    AuthAdded = "added",
    AuthSkipped = "already there",
    AuthBadLines = "not understood",
    AuthVaultFull = "vault full",
    AuthOldCopies = "The card may keep old copies.",
    AuthNoImportFile = "No import file",
    AuthImportTooLarge = "Import file too large",
    AuthAddQ = "Add this account?",
    AddBtn = "Add",
    AuthAlreadyThere = "Already in the vault",
    AuthDamaged = "Vault damaged",
    AuthDamagedAction = "Wipe it and add the accounts again.",
    AuthUnsupported = "Vault too new",
    AuthFailed = "The vault could not be read or written",
    AuthLocked = "Locked",
    AuthUtc = "UTC",
    AuthPinChanged = "PIN changed",
    AuthPinTitle = "About the PIN",
    // Recorder (UI-5b, 05 §7.5).
    InputGain = "Input gain",
    VoiceActivation = "Voice activation",
    AutoSplit = "Auto split",
    Monitor = "Monitor in headphones",
    LowCut = "Low-cut filter",
    RecReady = "Ready",
    RecRecording = "Recording",
    RecPaused = "Paused",
    RecWaiting = "Waiting for sound",
    RecSaving = "Saving…",
    RecLeft = "left",
    HintRecIdle = "OK record",
    HintRecording = "OK pause · Next mark",
    HintRecPaused = "OK resume · Next mark",
    StopRecQ = "Stop recording?",
    StopRecText = "The recording is saved.",
    StopBtn = "Stop",
    RecordingsDots = "Recordings…",
    Recordings = "Recordings",
    NoRecordings = "No recordings yet",
    RecSaved = "Saved",
    RecCardFull = "Card full · recording saved",
    RecCardError = "Card error · recording stopped",
    RecNoSource = "No input from this source",
    RecMark = "Mark",
    RecStopFirst = "Stop the recording first",
    AutoGain = "Automatic gain",
    RecDropped = "dropped",
    // UI-8: Settings › Display.
    ListRollAround = "List roll-around",
    // USB Drive (USB-6, 05 §7.10).
    DriveImages = "Images",
    DriveTestImages = "Test images",
    DriveChoose = "Choose file…",
    DriveHowTo = "Copy .iso or .img files to the Images folder on the card.",
    DriveNoFolder = "No Images folder on the card",
    DriveNoImages = "No disk images here",
    DriveMode = "Mode",
    DriveCd = "CD-ROM",
    DriveRemovable = "Disk",
    DriveFixed = "Fixed disk",
    DriveReadOnly = "Read-only",
    DriveReadWrite = "Read/write",
    DriveForcedRo = "Always read-only",
    DriveConnect = "Connect",
    DrivePreparing = "Preparing…",
    DriveWaiting = "Connect a computer",
    DriveInUse = "Connected",
    DriveRead = "Read",
    DriveWritten = "Written",
    DrivePresenting = "Presenting",
    DriveFailed = "Not presented",
    DriveFragmented = "The file is too fragmented. Copy it to the card again.",
    DriveTooSmall = "The file is too small for this mode.",
    DriveTooLarge = "The file is too large.",
    DriveIncomplete = "The file is not completely written.",
    DriveCardError = "The card could not be read.",
    DriveNotFound = "The file is no longer there.",
    DriveNotAllowed = "Only test images may be written.",
    DriveStopQ = "Stop the drive?",
    DriveStopText = "Eject it on the computer first.",
    DriveStop = "Stop",
    HintDriveStatus = "Back disconnects",
    // Update firmware (UPD-1, 02 §2.5).
    UpdTitle = "Update firmware",
    UpdSearching = "Looking for updates…",
    UpdNone = "No update file found",
    UpdNoneAction = "Copy orpheus-<version>.ofw to the card's top folder.",
    UpdInstalledVersion = "Installed",
    UpdThisOne = "installed",
    UpdInstallQ = "Install this version?",
    UpdInstallText = "The player restarts when done. Settings and music stay.",
    UpdInstall = "Install",
    UpdChecking = "Checking the file…",
    UpdWriting = "Writing…",
    UpdVerifying = "Verifying…",
    UpdKeepOn = "Do not switch off",
    UpdDone = "Update installed",
    UpdRestarting = "Restarting…",
    UpdFailed = "Update not installed",
    UpdUnchanged = "The installed firmware is unchanged.",
    UpdDamaged = "The file is damaged.",
    UpdWrongPlayer = "This file does not fit this player.",
    UpdBattery = "Battery below 30 %",
    UpdCard = "The card could not be read.",
    UpdWrite = "Writing the flash failed.",
    UpdBusy = "Another update is running.",
    UpdNoChanges = "No change list.",
    HintUpdInstall = "OK install",
    // T-6: About, the slot stage 2 started.
    FwSlot = "Firmware slot",
    SlotFailed = "Failed to start",
    // M14 Games (GAME-0/1/3). Built with and without the `games` feature
    // (the firmware has it since GAME-2's integration), so the numbers do
    // not depend on it; new strings are appended after the games' as
    // everywhere else.
    AppGames = "Games",
    GameBlocks = "Blocks",
    Game4096 = "4096",
    GameContinue = "Continue",
    GameResume = "Resume",
    GameNew = "New game",
    GameHowToPlay = "How to play",
    GameQuit = "Quit",
    GamePaused = "Paused",
    GameOkToContinue = "OK to continue",
    GameOkToStart = "OK to start",
    GameOver = "Game over",
    GameTimeUp = "Time is up",
    GameAllLines = "40 lines done",
    GameGoalReached = "Goal reached",
    GameOkKeepGoing = "OK to keep going",
    GameOkNewGame = "OK for a new game",
    GameNewBest = "New best",
    GameScore = "Score",
    GameBest = "Best",
    GameLines = "Lines",
    GameTime = "Time",
    GameNext = "Next",
    GameHold = "Hold",
    GameNewQ = "Start a new game?",
    GameNewText = "The game in progress ends.",
    GameNextGame = "From the next game",
    GamePlayed = "Played",
    GameMode = "Mode",
    GameEndless = "Endless",
    GameFortyLines = "40 lines",
    GameTwoMinutes = "2 minutes",
    GameStartLevel = "Start level",
    GamePieces = "Pieces",
    GameBag = "Bag of 7",
    GameRandom = "Random",
    GameGhost = "Ghost piece",
    GamePreviews = "Previews",
    GameHoldPiece = "Hold piece",
    GameLockDelay = "Lock delay",
    GameAutoShift = "Shift",
    GameRepeat = "Repeat",
    GameKicks = "Wall kicks",
    GameBoard = "Board",
    GameGoalTile = "Goal tile",
    GameChanceOfFour = "Chance of 4",
    GameUndoSteps = "Undo steps",
    // How to play, a string per paragraph (`games::Game::help`).
    HelpBlocks = "Pieces of four squares fall into the well. Fill a row from wall to wall and it goes; clear several rows with one piece for more points. The game ends when the stack reaches the top.",
    HelpBlocks2 = "Prev and Next move the piece, Up turns it clockwise and OK the other way. Down drops it faster; hold Down to drop it at once. Menu puts the piece aside and brings back the one kept there.",
    HelpBlocks3 = "Every ten rows the level rises and the pieces fall faster. Modes: Endless; 40 lines, as fast as you can; 2 minutes, as many points as you can.",
    HelpBlocks4 = "In Settings, Shift is how long Prev or Next is held before the piece runs, and Repeat how fast it runs then.",
    HelpBlocks5 = "Back pauses the game.",
    Help4096 = "Up, Down, Prev and Next slide every tile as far as it goes. Two equal tiles that meet join into one of twice the value, which is added to the score. After each slide a new tile appears: a 2, now and then a 4.",
    Help40962 = "Make a 4096 tile to win (Goal tile sets another), then keep going if you like. The game ends when no slide can move a tile.",
    Help40963 = "OK takes back the last slide, as many times as Undo steps allows. Back pauses the game.",
    // M14 Solitaire (GAME-2) and the games' card files.
    GameSolitaire = "Solitaire",
    GameDraw = "Draw",
    GameDrawOne = "One card",
    GameDrawThree = "Three cards",
    GamePasses = "Passes",
    GameUnlimited = "Unlimited",
    GameScoring = "Scoring",
    GameStandard = "Standard",
    GameCasino = "Casino",
    GameNoScore = "None",
    GameAutoMove = "Auto-move",
    GameAutoComplete = "Auto-complete",
    GameTimer = "Show timer",
    GameWinnable = "Winnable only",
    GameWon = "You won",
    GameNoMoves = "No more moves",
    GameBank = "Bank",
    GameMoves = "Moves",
    HelpSolitaire = "Build four piles, one per suit, from the ace up to the king. In the seven columns, cards go on a card one higher of the other colour; only a king goes into an empty column. A column's face-down card is turned up when it is uncovered.",
    HelpSolitaire2 = "Prev and Next move between the piles, Up and Down choose how many cards of a column; Up at a column's top goes to the row above, Down there to the columns. OK picks cards up and puts them down; OK on the same pile puts them back.",
    HelpSolitaire3 = "OK on the stock turns cards over (one or three, see Draw); when it is empty, OK turns the waste back as often as Passes allows.",
    HelpSolitaire4 = "Menu sends cards to the suit piles: the card under the cursor, or every card that can go. Hold OK to take back a move.",
    HelpSolitaire5 = "Standard scoring: 10 for each card on a suit pile, 5 for a card turned up or played from the waste, minus 15 for one taken back, a pass costs points; a win adds a bonus for time. Casino: a deal costs 52, each card on a suit pile earns 5, and the bank carries over. Back pauses the game.",
    // M14 Salvo, the artillery game (GAME-4/5).
    GameSalvo = "Salvo",
    GamePlayers = "Players",
    GamePlayer = "Player",
    GamePlayer1 = "Player 1",
    GamePlayer2 = "Player 2",
    GamePlayer3 = "Player 3",
    GamePlayer4 = "Player 4",
    GameHuman = "Human",
    GameNovice = "Novice",
    GameEasy = "Easy",
    GameHard = "Hard",
    GameExpert = "Expert",
    GameRounds = "Rounds",
    GameTurnTimer = "Turn timer",
    GameTerrain = "Terrain",
    GameHills = "Hills",
    GameMountains = "Mountains",
    GameCanyon = "Canyon",
    GameIslands = "Islands",
    GamePlateaus = "Plateaus",
    GameMixed = "Mixed",
    GameRoughness = "Roughness",
    GameRelief = "Height range",
    GameTerrainSeed = "Terrain seed",
    GameWorldWidth = "World width",
    GameGravity = "Gravity",
    GameMoon = "Moon",
    GameMars = "Mars",
    GameEarth = "Earth",
    GameJupiter = "Jupiter",
    GameWind = "Wind",
    GameLight = "Light",
    GameStrong = "Strong",
    GameCustom = "Custom",
    GameCustomWind = "Custom wind",
    GameGusts = "Gusts",
    GameDrag = "Air drag",
    GameRealistic = "Realistic",
    GameHeavy = "Heavy",
    GameTankHealth = "Tank health",
    GameFallDamage = "Fall damage",
    GameWeapons = "Weapons",
    GameShellsOnly = "Shells only",
    GameAllWeapons = "All seven",
    GameShop = "Shop",
    GameAimGuide = "Aim guide",
    GameLastShot = "Last shot",
    GamePreview = "Preview",
    GameRules = "Rules",
    WeaponShell = "Shell",
    WeaponHeavy = "Heavy shell",
    WeaponCluster = "Cluster",
    WeaponRoller = "Roller",
    WeaponDigger = "Digger",
    WeaponDirt = "Dirt bomb",
    WeaponSmoke = "Smoke",
    GamePassTo = "Pass to",
    GameRound = "Round",
    GameWinsRound = "wins the round",
    GameWinsGame = "wins the game",
    GameNobody = "Nobody is left",
    GameTie = "A tie",
    GameCredits = "Credits",
    GameThinking = "thinking",
    HelpSalvo = "Two to four tanks take turns to shoot at each other over hills, mountains, a canyon, islands or plateaus. The last tank standing wins the round; points come from damage dealt (a point each), tanks destroyed (50) and rounds won (100).",
    HelpSalvo2 = "Up and Down turn the barrel, Prev and Next set the power (hold to go faster). OK fires. Menu opens the weapon list. Hold OK for the whole world at once. During a shot OK speeds it up, and Prev and Next look around.",
    HelpSalvo3 = "Shells fly as in real air: gravity of the world you chose, drag that grows with speed, and a wind (the arrow at the top) that pushes light shells more than heavy ones. Gusts change it during the flight.",
    HelpSalvo4 = "Weapons: the shell; the heavy shell (bigger blast, less drift); the cluster (splits into three at the top); the roller (rolls downhill and goes off where it stops); the digger (cuts a trench); the dirt bomb (a mound of earth for cover); smoke (shows the wind).",
    HelpSalvo5 = "Ground blown away lets the earth above fall in; tanks fall and slide, and a long fall hurts. In the shop between rounds, credits buy weapons. Back pauses the game.",
    // M14 Bricks, the brick-breaking game (GAME-6).
    GameBricks = "Bricks",
    GameBallSpeed = "Ball",
    GameSlow = "Slow",
    GameNormal = "Normal",
    GameFast = "Fast",
    GamePaddleSize = "Paddle",
    GameSmall = "Small",
    GameLarge = "Large",
    GameLives = "Lives",
    GamePowerUps = "Power-ups",
    GameLevelSet = "Levels",
    GameHandMade = "Hand-made",
    GameGenerated = "Generated",
    GamePaddleSpeed = "Steering",
    GameLevelCleared = "Level cleared",
    GameOkToLaunch = "OK to launch",
    HelpBricks = "Keep the ball in play with the paddle and break every brick to clear the level. Bricks take one, two or three hits and fade as they wear; grey ones never break. A brick scores 10 for each hit it took, a cleared level 100.",
    HelpBricks2 = "Prev and Next (or Down and Up) move the paddle, faster while held. OK launches the ball. Where the ball meets the paddle sets its way: the middle sends it nearly straight up, the ends off to the side. The ball speeds up a little as the level goes on.",
    HelpBricks3 = "Some bricks drop a capsule: catch it with the paddle. A bar widens the paddle, S slows the ball, three dots split it into three, C lets the paddle hold the ball (OK lets it go), + is a life. Back pauses the game.",
    HelpBricks4 = "The first eight levels are hand-made; after them (or from the start, with the Levels setting) they are generated, fuller and harder as you go.",
    // M15 Books (BOOK-1). Built with and without the `books` feature (not
    // in the firmware yet), so the numbers do not depend on it.
    AppBooks = "Books",
    BooksSearching = "Looking for books",
    BooksNone = "No books found",
    BooksNoneFolder = "Put .txt, .md, .fb2 or .epub files in the Books folder.",
    BooksNoneCard = "No .txt, .md, .fb2 or .epub files on the card.",
    BooksSearchAgain = "Search again",
    BooksPreparing = "Preparing the book",
    BooksLayingOut = "Laying out pages",
    BooksFailed = "The book could not be read.",
    BooksContents = "Contents",
    BooksNoContents = "No contents",
    BooksBookmarks = "Bookmarks",
    BooksNoBookmarks = "No bookmarks",
    BooksAddBookmark = "Add bookmark",
    BooksBookmarkAdded = "Bookmark added",
    BooksBookmarksFull = "Bookmarks are full",
    BooksDeleteBookmark = "Delete bookmark",
    BooksGoTo = "Go to",
    BooksPage = "Page",
    BooksOf = "of",
    BooksEnd = "End of the book",
    BooksNew = "New",
    BooksFontSize = "Font size",
    BooksSmall = "Small",
    BooksLarge = "Large",
    BooksExtraLarge = "Extra large",
    BooksMargins = "Margins",
    BooksNarrow = "Narrow",
    BooksNormal = "Normal",
    BooksWide = "Wide",
    BooksLineSpacing = "Spacing",
    BooksTight = "Tight",
    BooksRelaxed = "Relaxed",
    BooksJustify = "Justify",
    BooksPalette = "Palette",
    BooksSepia = "Sepia",
    BooksAutoTurn = "Auto page turn",
    BooksReadAloud = "Read aloud",
    BooksFindIn = "Find books in",
    BooksFolder = "Books folder",
    BooksWholeCard = "Whole card",
    BooksHintGoTo = "◀▶ 10 % · ▲▼ 1 %",
    // Accessibility (A11Y-2, 05 §7.14).
    GAccessibility = "Accessibility",
    TextSize = "Text size",
    TextNormal = "Normal",
    TextLarge = "Large",
    TextExtraLarge = "Extra large",
    HoldDelay = "Hold delay",
    RepeatSpeed = "Key repeat",
    RepeatNormal = "Normal",
    RepeatSlow = "Slow",
    RepeatSlower = "Slower",
    ClickPresses = "Presses",
    ClickHolds = "Presses and holds",
    ThemeContrastDark = "White on black",
    ThemeContrastLight = "Black on white",
    // M17 Sequencer (SEQ-1). Built with and without the `seq` feature (not
    // in the firmware yet), so the numbers do not depend on it.
    AppSequencer = "Sequencer",
    SeqNewProject = "New project",
    SeqPlay = "Play",
    SeqStop = "Stop",
    SeqPlaying = "Playing",
    SeqTransport = "Transport",
    SeqTempo = "Tempo",
    SeqSwing = "Swing",
    SeqTapTempo = "Tap tempo",
    SeqPlayMode = "Play mode",
    SeqPattern = "Pattern",
    SeqSong = "Song",
    SeqLength = "Length",
    SeqSteps = "steps",
    SeqKit = "Kit",
    SeqMixer = "Mixer",
    SeqTrack = "Track",
    SeqSound = "Sound",
    SeqMute = "Mute",
    SeqSolo = "Solo",
    SeqStep = "Step",
    SeqStepOn = "Step on",
    SeqVelocity = "Velocity",
    SeqChance = "Chance",
    SeqRatchet = "Ratchet",
    SeqDelay = "Delay",
    SeqAccent = "Accent",
    SeqNextPattern = "Next pattern",
    SeqPrevPattern = "Previous pattern",
    SeqCopyNext = "Copy to next pattern",
    SeqClearPattern = "Clear pattern",
    SeqSave = "Save",
    SeqSaved = "Saved",
    SeqSaveFailed = "Could not save",
    SeqOpenFailed = "The project could not be read",
    SeqDelete = "Delete project",
    SeqDeleted = "Project deleted",
    SeqHintMixer = "OK mute · hold: solo",
    SeqHintValues = "◀▶ change · OK on/off",
    // SEQ-2: behind the `seq` feature (the firmware, which has no
    // Sequencer yet, cannot spare the read-only data: its stack margin is
    // within one 256-byte step of the budget). They are the last strings:
    // strings added later go above this block, so the number of every
    // string the firmware has never depends on the feature.
    #[cfg(feature = "seq")]
    SeqKeys = "Keys",
    #[cfg(feature = "seq")]
    SeqLive = "Live",
    #[cfg(feature = "seq")]
    SeqSongEdit = "Song",
    #[cfg(feature = "seq")]
    SeqExportWav = "Export WAV",
    #[cfg(feature = "seq")]
    SeqExportMidi = "Export MIDI",
    #[cfg(feature = "seq")]
    SeqExported = "Exported",
    #[cfg(feature = "seq")]
    SeqExportFailed = "Could not export",
    #[cfg(feature = "seq")]
    SeqNote = "Note",
    #[cfg(feature = "seq")]
    SeqGate = "Length",
    #[cfg(feature = "seq")]
    SeqChord = "Chord",
    #[cfg(feature = "seq")]
    SeqLocks = "Locks",
    #[cfg(feature = "seq")]
    SeqLocksFull = "No room for more locks in this pattern",
    #[cfg(feature = "seq")]
    SeqTune = "Tune",
    #[cfg(feature = "seq")]
    SeqDecay = "Decay",
    #[cfg(feature = "seq")]
    SeqTone = "Tone",
    #[cfg(feature = "seq")]
    SeqSnap = "Snap",
    #[cfg(feature = "seq")]
    SeqPan = "Pan",
    #[cfg(feature = "seq")]
    SeqCutoff = "Cutoff",
    #[cfg(feature = "seq")]
    SeqResonance = "Resonance",
    #[cfg(feature = "seq")]
    SeqGlide = "Glide",
    #[cfg(feature = "seq")]
    SeqPreset = "Preset",
    #[cfg(feature = "seq")]
    SeqWave = "Wave",
    #[cfg(feature = "seq")]
    SeqDetune = "Detune",
    #[cfg(feature = "seq")]
    SeqEnvAmount = "Filter envelope",
    #[cfg(feature = "seq")]
    SeqAttack = "Attack",
    #[cfg(feature = "seq")]
    SeqSustain = "Sustain",
    #[cfg(feature = "seq")]
    SeqRelease = "Release",
    #[cfg(feature = "seq")]
    SeqVibrato = "Vibrato",
    #[cfg(feature = "seq")]
    SeqVoices = "Voices",
    #[cfg(feature = "seq")]
    SeqDelaySend = "Delay send",
    #[cfg(feature = "seq")]
    SeqReverbSend = "Reverb send",
    #[cfg(feature = "seq")]
    SeqKitB = "Kit B",
    #[cfg(feature = "seq")]
    SeqScale = "Scale",
    #[cfg(feature = "seq")]
    SeqRoot = "Root",
    #[cfg(feature = "seq")]
    SeqTranspose = "Transpose",
    #[cfg(feature = "seq")]
    SeqFill = "Fill pattern",
    #[cfg(feature = "seq")]
    SeqAddEntry = "Add pattern",
    #[cfg(feature = "seq")]
    SeqInsertEntry = "Insert a copy",
    #[cfg(feature = "seq")]
    SeqDeleteEntry = "Delete entry",
    #[cfg(feature = "seq")]
    SeqPlaySong = "Play the song",
    #[cfg(feature = "seq")]
    SeqRecord = "Record",
    #[cfg(feature = "seq")]
    SeqRecording = "Recording",
    #[cfg(feature = "seq")]
    SeqHintKeys = "◀▶▲▼ note · OK enter",
    #[cfg(feature = "seq")]
    SeqHintLive = "hold OK: fill",
    #[cfg(feature = "seq")]
    SeqHintSong = "◀▶ pattern · OK +1",
    #[cfg(feature = "seq")]
    SeqHintLocks = "◀▶ lock · OK clear",
    #[cfg(feature = "seq")]
    SeqNewKit = "Default kit",
    #[cfg(feature = "seq")]
    SeqNewLength = "Default length",
    #[cfg(feature = "seq")]
    SeqNewSwing = "Default swing",
    #[cfg(feature = "seq")]
    SeqMetronome = "Metronome",
    #[cfg(feature = "seq")]
    SeqCountIn = "Count-in",
    #[cfg(feature = "seq")]
    SeqStepLength = "Step length",
    #[cfg(feature = "seq")]
    SeqClickPlaying = "Key clicks in play",
    #[cfg(feature = "seq")]
    SeqNoFill = "None",
    #[cfg(feature = "seq")]
    SeqMore = "More",
    // A11Y-1: the screen reader's settings, behind `screen-reader` (not in
    // the firmware: no speech output there yet), after the Sequencer's
    // block for the same reason as it.
    #[cfg(feature = "screen-reader")]
    ScreenReader = "Screen reader",
    #[cfg(feature = "screen-reader")]
    SpeechRate = "Speech rate",
    #[cfg(feature = "screen-reader")]
    Voice = "Voice",
    #[cfg(feature = "screen-reader")]
    VoiceNeutral = "Neutral",
    #[cfg(feature = "screen-reader")]
    VoiceBright = "Bright",
    #[cfg(feature = "screen-reader")]
    Pitch = "Pitch",
    #[cfg(feature = "screen-reader")]
    PitchLower = "Lower",
    #[cfg(feature = "screen-reader")]
    PitchHigher = "Higher",
    #[cfg(feature = "screen-reader")]
    Verbosity = "Verbosity",
    #[cfg(feature = "screen-reader")]
    VerbBrief = "Brief",
    #[cfg(feature = "screen-reader")]
    VerbDetailed = "Detailed",
    #[cfg(feature = "screen-reader")]
    Earcons = "Sound cues",
    #[cfg(feature = "screen-reader")]
    WhereAmIKeys = "Where am I",
    #[cfg(feature = "screen-reader")]
    DarkKeys = "Keys with screen off",
    // TTS-3: the speech's level and the music while speaking.
    #[cfg(feature = "screen-reader")]
    SpeechVolume = "Speech volume",
    #[cfg(feature = "screen-reader")]
    MusicWhileSpeaking = "Music while speaking",
    #[cfg(feature = "screen-reader")]
    MusicLowered = "Lowered",
    #[cfg(feature = "screen-reader")]
    MusicPaused = "Paused",
    #[cfg(feature = "screen-reader")]
    MusicUnchanged = "Unchanged",
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
        // `from_pack` wants `'static` data: keep it in statics (reachable, so
        // LeakSanitizer does not report it as `Box::leak` did).
        static PACK: std::sync::OnceLock<std::vec::Vec<u8>> = std::sync::OnceLock::new();
        static TRUNCATED: std::sync::OnceLock<std::vec::Vec<u8>> = std::sync::OnceLock::new();
        let pack: &'static [u8] = PACK.get_or_init(|| build("ru", "Русский", &[(Str::AppMusic, "Музыка")]));
        let l = Lang::from_pack(pack).unwrap();
        assert_eq!(tr!(l, AppMusic), "Музыка");
        assert_eq!(tr!(l, AppFiles), "Files", "missing entries fall back to English");
        assert_eq!((l.code(), l.name()), ("ru", "Русский"));
        assert!(Lang::from_pack(b"nope").is_none());
        let truncated: &'static [u8] = TRUNCATED.get_or_init(|| pack[..20].to_vec());
        assert!(Lang::from_pack(truncated).is_none());
    }
}
