//! Keys and events (05 §5.1).

/// The ten keys (05 §1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Key {
    Up = 0,
    Down,
    Prev,
    Next,
    Ok,
    Back,
    Menu,
    VolUp,
    VolDown,
    Power,
}

impl Key {
    pub const ALL: [Key; 10] = [
        Key::Up,
        Key::Down,
        Key::Prev,
        Key::Next,
        Key::Ok,
        Key::Back,
        Key::Menu,
        Key::VolUp,
        Key::VolDown,
        Key::Power,
    ];

    #[must_use]
    pub const fn index(self) -> usize {
        self as usize
    }

    /// Script and log name: `up down prev next ok back menu vol+ vol- power`.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Key::Up => "up",
            Key::Down => "down",
            Key::Prev => "prev",
            Key::Next => "next",
            Key::Ok => "ok",
            Key::Back => "back",
            Key::Menu => "menu",
            Key::VolUp => "vol+",
            Key::VolDown => "vol-",
            Key::Power => "power",
        }
    }

    /// Parse a script name (also accepts `volup`, `voldown`, `play`,
    /// `return`, `mode`, the stock firmware's names).
    #[must_use]
    pub fn from_name(s: &str) -> Option<Key> {
        Some(match s {
            "up" => Key::Up,
            "down" => Key::Down,
            "prev" | "left" => Key::Prev,
            "next" | "right" => Key::Next,
            "ok" | "play" => Key::Ok,
            "back" | "return" => Key::Back,
            "menu" | "mode" => Key::Menu,
            "vol+" | "volup" => Key::VolUp,
            "vol-" | "voldown" => Key::VolDown,
            "power" => Key::Power,
            _ => return None,
        })
    }

    /// Keys that auto-repeat while held (05 §3).
    #[must_use]
    pub const fn repeats(self) -> bool {
        matches!(self, Key::Up | Key::Down | Key::Prev | Key::Next | Key::VolUp | Key::VolDown)
    }
}

/// Phases of one key press (05 §5.1). A *tap* is `Up { held: false }`; act on
/// it for keys whose hold means something else (OK, Back, Menu, Prev/Next,
/// Power). Up/Down act on `Down` and `Repeat`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum KeyPhase {
    /// The key went down.
    Down,
    /// Auto-repeat while held (repeating keys only), accelerating.
    Repeat,
    /// Held for the hold threshold (500 ms; Power 1.5 s).
    Hold,
    /// Held for the long threshold (Power only: 4 s, power off).
    Long,
    /// Released; `held` if `Hold` was sent for this press.
    Up { held: bool },
}

/// Milliseconds since boot.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub struct Instant(pub u64);

impl Instant {
    #[must_use]
    pub const fn ms(self) -> u64 {
        self.0
    }

    #[must_use]
    pub const fn add_ms(self, ms: u64) -> Instant {
        Instant(self.0 + ms)
    }

    /// Milliseconds since `earlier` (0 if `earlier` is later).
    #[must_use]
    pub const fn since(self, earlier: Instant) -> u64 {
        self.0.saturating_sub(earlier.0)
    }
}

/// Playback state as the status bar and mini-player see it.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Source {
    #[default]
    Idle,
    Playing,
    Paused,
    Recording,
    Radio,
}

/// Bluetooth state for the status bar.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum BtState {
    #[default]
    Off,
    On,
    Connected,
    Streaming,
}

/// Notifications from the services (05 §5.1). The set grows with the apps;
/// `Other` carries anything not yet given a variant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ServiceEvent {
    Battery { percent: u8, charging: bool },
    Playback(Source),
    TrackChanged,
    Volume(u8),
    CardInserted,
    CardRemoved,
    Bluetooth(BtState),
    Usb { host: bool },
    Other { kind: u16, value: u32 },
}

/// A row range, page or image a screen asked for has arrived.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct DataReady {
    /// The screen's own request tag.
    pub tag: u16,
    pub first: u32,
    pub count: u32,
}

/// Results of toolkit overlays, delivered to the screen that opened them.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiEvent {
    /// A context-menu item was chosen. `value` is the new toggle state (0/1),
    /// the chosen radio index, or 0 for actions.
    Menu { id: u16, value: u16 },
    /// A dialog closed; `button` is the index chosen, or
    /// [`DIALOG_CANCELLED`] for Back.
    Dialog { id: u16, button: u8 },
}

/// `UiEvent::Dialog::button` when the dialog was closed with Back.
pub const DIALOG_CANCELLED: u8 = 0xFF;

/// Everything a screen receives.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Key(Key, KeyPhase),
    /// Only while the screen asked for ticks ([`Wants::ticks`](crate::Wants)).
    Tick(Instant),
    Service(ServiceEvent),
    Data(DataReady),
    Ui(UiEvent),
}

impl Event {
    /// True for a tap (release without hold) of `k`.
    #[must_use]
    pub fn is_tap(&self, k: Key) -> bool {
        matches!(*self, Event::Key(kk, KeyPhase::Up { held: false }) if kk == k)
    }

    /// True for `Down` or `Repeat` of `k` (Up/Down movement).
    #[must_use]
    pub fn is_step(&self, k: Key) -> bool {
        matches!(*self, Event::Key(kk, KeyPhase::Down | KeyPhase::Repeat) if kk == k)
    }

    /// True for `Hold` of `k`.
    #[must_use]
    pub fn is_hold(&self, k: Key) -> bool {
        matches!(*self, Event::Key(kk, KeyPhase::Hold) if kk == k)
    }
}
