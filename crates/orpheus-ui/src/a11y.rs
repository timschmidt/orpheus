//! The screen reader's toolkit side (A11Y-1, 05 §7.14), feature
//! `screen-reader` (off in the firmware until its speech output exists).
//!
//! * **Descriptions.** Every screen fills a [`Desc`] from
//!   [`Screen::describe`](crate::nav::Screen::describe): a title, a "where am
//!   I" summary and the focused element (role, label, value, state,
//!   position "3 of 12"). The shell describes its own overlays (menu
//!   sheets, dialogs, toasts, the volume) in
//!   [`Shell::describe`](crate::shell::Shell::describe). Lists take their
//!   focus from the row the [`ListView`] focuses ([`Desc::list`]), so a
//!   screen built on the toolkit's list says what it shows without more
//!   code. [`Desc::problem`] is the lint the simulator runs over every
//!   screen.
//! * **The reader.** [`Reader::observe`] compares each description with the
//!   last one and decides what to say: a new screen or overlay (its title,
//!   then the focus), a moved focus, a changed value or state, a toast, the
//!   volume, a list end or roll-around (earcons); any key press silences
//!   what is being said first ([`SpeechService::stop`]). Verbosity
//!   ([`Verbosity`]) decides how much of a focus is said.
//! * **The speech service.** [`SpeechService`] is what the firmware's speech
//!   voice (TTS-3) and the simulator implement: an utterance queue
//!   ([`UtteranceQueue`], fixed RAM), stop now, earcons ([`Earcon`], tones as
//!   data), the rate, voice and pitch ([`Config`]).
//! * **Chords** ([`Chords`]): the toggle (hold Back and Menu for 2 s) and
//!   "where am I" (hold a configurable pair; 2 s: time and battery), taken
//!   from the raw key edges before the
//!   [`KeyMapper`](crate::input::KeyMapper) so that they work from
//!   any screen, locked or with the screen off. The two keys of a chord
//!   are one of ladder 1 (Back, Next, Vol+, Vol−) and one of ladder 2 (Menu,
//!   Prev, Down, OK, Up): the hardware sees one key per ladder at a time
//!   (05 §1), so Vol+ with Vol− cannot be a chord.
//!
//! Spoken text is English (the speech engine is English only, D-14): role
//! and state words are written here, labels are the UI's strings.
//!
//! `no_std`, no allocation: a [`Desc`] is 536 B, a [`Reader`] 296 B (with
//! its scratch line), a [`Chords`] 24 B (host sizes).

use crate::event::{Instant, Key, KeyPhase};
use crate::text::StrBuf;
use crate::widgets::bars::Status;
use crate::widgets::list::{Accessory, Item, ListSource, ListView, Row};

/// What a focusable element is (spoken after its label and value).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Role {
    /// A row or action that does something on OK.
    #[default]
    Item,
    /// A row that opens a sub-level (a chevron).
    Submenu,
    /// An on/off switch.
    Switch,
    /// One of a group of choices.
    Radio,
    /// A check box.
    Check,
    /// A button (dialogs, on-screen controls).
    Button,
    /// A value changed with Up/Down (slider, picker field, band).
    Slider,
    /// A tab (Stopwatch, Timer, Alarm).
    Tab,
    /// A cell of a grid or board (games, the Sequencer's steps).
    Cell,
    /// A key of an on-screen pad.
    Key,
    /// Read-only text (an information row, a book's page).
    Text,
}

impl Role {
    /// The word said at Normal verbosity (empty: nothing).
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            Role::Item | Role::Text => "",
            Role::Submenu => "submenu",
            Role::Switch => "switch",
            Role::Radio => "option",
            Role::Check => "check box",
            Role::Button => "button",
            Role::Slider => "adjustable",
            Role::Tab => "tab",
            Role::Cell => "cell",
            Role::Key => "key",
        }
    }

    /// What to do with it (Detailed verbosity).
    #[must_use]
    pub const fn hint(self) -> &'static str {
        match self {
            Role::Item | Role::Button | Role::Key => "OK to activate",
            Role::Submenu => "OK to open",
            Role::Switch | Role::Check => "OK to change",
            Role::Radio => "OK to choose",
            Role::Slider => "Up and Down to change",
            Role::Tab => "Previous and Next for other tabs",
            Role::Cell => "arrows to move",
            Role::Text => "",
        }
    }
}

/// A focusable element's state.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum State {
    #[default]
    None,
    On,
    Off,
    Selected,
    NotSelected,
    Checked,
    NotChecked,
}

impl State {
    #[must_use]
    pub const fn word(self) -> &'static str {
        match self {
            State::None => "",
            State::On => "on",
            State::Off => "off",
            State::Selected => "selected",
            State::NotSelected => "not selected",
            State::Checked => "checked",
            State::NotChecked => "not checked",
        }
    }
}

/// What the description is of.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Layer {
    #[default]
    Screen,
    /// A context menu sheet over the screen.
    Menu,
    /// A dialog over the screen.
    Dialog,
}

/// How much is said of a focus (Settings › Accessibility › Verbosity).
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, Default)]
pub enum Verbosity {
    /// Label, value and on/off only.
    Brief,
    /// Also the role, "unavailable" and the position ("3 of 12").
    #[default]
    Normal,
    /// Also the screen's summary on entry and what OK does.
    Detailed,
}

impl Verbosity {
    #[must_use]
    pub const fn from_index(v: i16) -> Verbosity {
        match v {
            0 => Verbosity::Brief,
            2 => Verbosity::Detailed,
            _ => Verbosity::Normal,
        }
    }
}

/// The "where am I" key pair (Settings › Accessibility › Where am I).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum WhereKeys {
    Off,
    /// Hold Back and OK.
    #[default]
    BackOk,
    /// Hold Prev and Next.
    PrevNext,
}

impl WhereKeys {
    #[must_use]
    pub const fn from_index(v: i16) -> WhereKeys {
        match v {
            0 => WhereKeys::Off,
            2 => WhereKeys::PrevNext,
            _ => WhereKeys::BackOk,
        }
    }

    /// The pair (ladder 1 key, ladder 2 key).
    #[must_use]
    pub const fn keys(self) -> Option<(Key, Key)> {
        match self {
            WhereKeys::Off => None,
            WhereKeys::BackOk => Some((Key::Back, Key::Ok)),
            WhereKeys::PrevNext => Some((Key::Next, Key::Prev)),
        }
    }
}

/// The reader's settings (Settings › Accessibility, read by the integrator
/// from the settings: `orpheus_apps::services::reader_config`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    /// Screen reader on.
    pub on: bool,
    /// Speech rate in quarters (1–40: 0.25×–10×, 4 = 1×; the owner's
    /// 0.25× steps).
    pub rate: u8,
    /// Voice index (the speech engine's voices: Neutral, Low, Bright).
    pub voice: u8,
    /// Pitch shift in semitones.
    pub pitch: i8,
    pub verbosity: Verbosity,
    /// Earcons (short tones for list ends, screens, toggles).
    pub earcons: bool,
    pub where_keys: WhereKeys,
    /// Keys work while the screen is off (no wake press; the screen stays
    /// dark).
    pub dark_keys: bool,
}

impl Config {
    /// The defaults (reader off).
    pub const DEFAULT: Config = Config {
        on: false,
        rate: 4,
        voice: 0,
        pitch: 0,
        verbosity: Verbosity::Normal,
        earcons: true,
        where_keys: WhereKeys::BackOk,
        dark_keys: true,
    };
}

impl Default for Config {
    fn default() -> Self {
        Config::DEFAULT
    }
}

impl Config {
    /// The screen timers with the reader's keys with the screen off: while
    /// the reader is on, the backlight timeout darkens the screen but does
    /// not lock the keys (Power still does).
    #[must_use]
    pub const fn timeouts(&self, t: crate::input::Timeouts) -> crate::input::Timeouts {
        if self.on && self.dark_keys { crate::input::Timeouts { lock_ms: None, ..t } } else { t }
    }
}

/// Short non-speech sounds.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Earcon {
    /// A screen opened (deeper).
    Enter = 1,
    /// Back to a screen (shallower).
    Back,
    /// A menu sheet opened.
    Menu,
    /// A dialog opened.
    Dialog,
    /// A list rolled round from one end to the other.
    Wrap,
    /// A list end: the key moved nothing.
    End,
    /// Something turned on.
    On,
    /// Something turned off.
    Off,
    /// A toast.
    Toast,
    /// The screen reader came on.
    ReaderOn,
    /// The screen reader goes off.
    ReaderOff,
}

/// An earcon as a tone: a glide from `hz[0]` to `hz[1]` over `ms`, at
/// `level` / 255 of full scale, with 4 ms fades (the service renders it).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Tone {
    pub hz: [u16; 2],
    pub ms: u16,
    pub level: u8,
}

impl Earcon {
    pub const ALL: [Earcon; 11] = [
        Earcon::Enter,
        Earcon::Back,
        Earcon::Menu,
        Earcon::Dialog,
        Earcon::Wrap,
        Earcon::End,
        Earcon::On,
        Earcon::Off,
        Earcon::Toast,
        Earcon::ReaderOn,
        Earcon::ReaderOff,
    ];

    #[must_use]
    pub const fn tone(self) -> Tone {
        let (a, b, ms) = match self {
            Earcon::Enter => (880, 1320, 40),
            Earcon::Back => (1320, 880, 40),
            Earcon::Menu => (660, 990, 50),
            Earcon::Dialog => (523, 784, 70),
            Earcon::Wrap => (1760, 880, 60),
            Earcon::End => (220, 220, 50),
            Earcon::On => (660, 1320, 50),
            Earcon::Off => (1320, 660, 50),
            Earcon::Toast => (1046, 1046, 30),
            Earcon::ReaderOn => (440, 880, 120),
            Earcon::ReaderOff => (880, 440, 120),
        };
        Tone { hz: [a, b], ms, level: 70 }
    }

    /// Log and script name.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Earcon::Enter => "enter",
            Earcon::Back => "back",
            Earcon::Menu => "menu",
            Earcon::Dialog => "dialog",
            Earcon::Wrap => "wrap",
            Earcon::End => "end",
            Earcon::On => "on",
            Earcon::Off => "off",
            Earcon::Toast => "toast",
            Earcon::ReaderOn => "reader-on",
            Earcon::ReaderOff => "reader-off",
        }
    }

    #[must_use]
    pub fn from_u8(v: u8) -> Option<Earcon> {
        Earcon::ALL.into_iter().find(|e| *e as u8 == v)
    }
}

/// What speaks (the firmware's speech voice, the simulator's renderer).
pub trait SpeechService {
    /// Silence what is being said now (`Stop::Now`) and drop the queue.
    fn stop(&mut self);
    /// Queue an utterance after what is queued.
    fn say(&mut self, text: &str);
    /// Queue an earcon.
    fn earcon(&mut self, e: Earcon);
    /// Take the rate, voice and pitch (called before each utterance the
    /// reader makes).
    fn configure(&mut self, _cfg: &Config) {}
}

/// One queued utterance as [`UtteranceQueue::pop`] returns it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Utterance {
    /// The text is in the caller's buffer.
    Text,
    Earcon(Earcon),
}

/// A fixed byte ring of utterances (text up to 255 bytes, earcons): the
/// queue a [`SpeechService`] keeps between the reader and the synthesiser.
/// A text that does not fit is cut at a character boundary; when the
/// ring is full the newest utterance is dropped (the next key clears it).
#[derive(Clone, Debug)]
pub struct UtteranceQueue<const N: usize> {
    buf: [u8; N],
    head: usize,
    len: usize,
    /// Utterances dropped or cut because the ring was full.
    pub dropped: u32,
}

impl<const N: usize> Default for UtteranceQueue<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> UtteranceQueue<N> {
    #[must_use]
    pub const fn new() -> Self {
        UtteranceQueue { buf: [0; N], head: 0, len: 0, dropped: 0 }
    }

    pub fn clear(&mut self) {
        self.head = 0;
        self.len = 0;
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn put(&mut self, b: u8) {
        let i = (self.head + self.len) % N;
        self.buf[i] = b;
        self.len += 1;
    }

    fn take(&mut self) -> u8 {
        let b = self.buf[self.head];
        self.head = (self.head + 1) % N;
        self.len -= 1;
        b
    }

    /// Queue a text (a tag byte 0, its length, the bytes).
    pub fn push_text(&mut self, s: &str) -> bool {
        let room = N.saturating_sub(self.len).saturating_sub(2).min(255);
        let mut n = s.len().min(room);
        while n > 0 && !s.is_char_boundary(n) {
            n -= 1;
        }
        if n == 0 {
            self.dropped += 1;
            return false;
        }
        if n < s.len() {
            self.dropped += 1;
        }
        self.put(0);
        self.put(u8::try_from(n).unwrap_or(255));
        for &b in &s.as_bytes()[..n] {
            self.put(b);
        }
        true
    }

    /// Queue an earcon (one byte).
    pub fn push_earcon(&mut self, e: Earcon) -> bool {
        if self.len >= N {
            self.dropped += 1;
            return false;
        }
        self.put(e as u8);
        true
    }

    /// The oldest utterance; a text is copied into `out`.
    pub fn pop(&mut self, out: &mut StrBuf<255>) -> Option<Utterance> {
        if self.len == 0 {
            return None;
        }
        let tag = self.take();
        if tag != 0 {
            return Some(Earcon::from_u8(tag).map_or(Utterance::Earcon(Earcon::Toast), Utterance::Earcon));
        }
        let n = usize::from(self.take());
        let mut bytes = [0u8; 255];
        for b in bytes.iter_mut().take(n) {
            *b = self.take();
        }
        out.clear();
        out.push_str(core::str::from_utf8(&bytes[..n]).unwrap_or(""));
        Some(Utterance::Text)
    }
}

/// The focused element.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Focus {
    pub role: Role,
    pub label: StrBuf<96>,
    pub value: StrBuf<64>,
    pub state: State,
    /// Unavailable (drawn dim).
    pub dim: bool,
    /// 1-based position and the count (0: no position).
    pub index: u32,
    pub count: u32,
    /// Not loaded yet (a pending list row): nothing is said until it is.
    pub pending: bool,
}

impl Focus {
    pub fn value(&mut self, v: &str) -> &mut Self {
        self.value.clear();
        self.value.push_str_or_ellipsis(v);
        self
    }

    pub fn state(&mut self, s: State) -> &mut Self {
        self.state = s;
        self
    }

    /// Position `index` (0-based) of `count`.
    pub fn at(&mut self, index: u32, count: u32) -> &mut Self {
        self.index = index.saturating_add(1);
        self.count = count;
        self
    }

    pub fn dim(&mut self, dim: bool) -> &mut Self {
        self.dim = dim;
        self
    }
}

/// Longest list scanned for positions (headers are not counted); a longer
/// one counts by row number (a 5,000-entry folder has no headers).
const SCAN: u32 = 512;

/// What a screen (or an overlay) says about itself.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Desc {
    pub layer: Layer,
    /// The screen's id (`ScreenId.0`), filled by the shell.
    pub screen: u8,
    /// Navigation depth, filled by the shell (deeper: the enter earcon).
    pub depth: u8,
    pub title: StrBuf<48>,
    /// Under a menu or dialog: the screen's title.
    pub under: StrBuf<48>,
    pub focus: Option<Focus>,
    /// What the screen shows beside the focus (where am I).
    pub summary: StrBuf<192>,
    /// The toast shown (empty: none) and when it was raised.
    pub toast: StrBuf<48>,
    pub toast_at: u64,
    /// The last volume step: (level, max) and the time it was made.
    pub volume: Option<(u8, u8)>,
}

impl Desc {
    #[must_use]
    pub const fn new() -> Desc {
        Desc {
            layer: Layer::Screen,
            screen: 0,
            depth: 0,
            title: StrBuf::new(),
            under: StrBuf::new(),
            focus: None,
            summary: StrBuf::new(),
            toast: StrBuf::new(),
            toast_at: 0,
            volume: None,
        }
    }

    pub fn clear(&mut self) {
        *self = Desc::new();
    }

    /// The screen's title.
    pub fn title(&mut self, t: &str) -> &mut Self {
        self.title.clear();
        self.title.push_str_or_ellipsis(t);
        self
    }

    /// Append a sentence to the summary.
    pub fn say(&mut self, s: &str) -> &mut Self {
        if s.is_empty() {
            return self;
        }
        if !self.summary.is_empty() {
            self.summary.push_str(". ");
        }
        self.summary.push_str_or_ellipsis(s);
        self
    }

    /// Append a number to the summary (after a word: "Tempo 120").
    pub fn say_u32(&mut self, label: &str, v: u32) -> &mut Self {
        let mut b = StrBuf::<48>::new();
        b.push_str(label);
        if !label.is_empty() {
            b.push(' ');
        }
        b.push_u32(v);
        self.say(b.as_str())
    }

    /// Set the focus.
    pub fn focus(&mut self, role: Role, label: &str) -> &mut Focus {
        let mut f = Focus { role, ..Focus::default() };
        f.label.push_str_or_ellipsis(label);
        self.focus.insert(f)
    }

    /// The focus from a list row.
    pub fn item(&mut self, it: &Item<'_>) -> &mut Focus {
        let (role, state) = match it.accessory {
            Accessory::None => (Role::Item, State::None),
            Accessory::Chevron => (Role::Submenu, State::None),
            Accessory::Toggle(on) => (Role::Switch, if on { State::On } else { State::Off }),
            Accessory::Radio(on) => (Role::Radio, if on { State::Selected } else { State::NotSelected }),
            Accessory::Check(on) => (Role::Check, if on { State::Checked } else { State::NotChecked }),
        };
        let (label, value) =
            if it.text.is_empty() { (it.value.unwrap_or(""), None) } else { (it.text, it.value) };
        let f = self.focus(role, label);
        f.state = state;
        f.dim = it.dim;
        if let Some(v) = value {
            f.value(v);
        }
        f
    }

    /// The focus of a toolkit list: the focused row, its position among the
    /// focusable rows (section headers skipped). An empty list says
    /// `empty` instead (no focus).
    pub fn list(&mut self, src: &dyn ListSource, view: &ListView, empty: &str) {
        let n = src.len();
        if n == 0 {
            self.focus = None;
            self.say(empty);
            return;
        }
        let f = view.focus.min(n - 1);
        let (index, count) = if n <= SCAN {
            let focusable = |i: u32| !matches!(src.row(i), Row::Header(_));
            ((0..f).filter(|&i| focusable(i)).count(), (0..n).filter(|&i| focusable(i)).count())
        } else {
            (usize::try_from(f).unwrap_or(0), usize::try_from(n).unwrap_or(0))
        };
        let (index, count) = (u32::try_from(index).unwrap_or(0), u32::try_from(count).unwrap_or(0));
        match src.row(f) {
            Row::Item(it) => {
                self.item(&it).at(index, count);
            }
            Row::Header(h) => {
                self.focus(Role::Text, h).at(index, count);
            }
            Row::Pending => {
                let fo = self.focus(Role::Item, "loading");
                fo.pending = true;
                fo.at(index, count);
            }
        }
    }

    /// The lint (A11Y-1): what is wrong with this description for a
    /// reader, if anything. Run by the simulator over every screen.
    #[must_use]
    pub fn problem(&self) -> Option<&'static str> {
        if speakable_len(self.title.as_str()) == 0 {
            return Some("no title");
        }
        if let Some(f) = &self.focus {
            if speakable_len(f.label.as_str()) == 0 {
                return Some("an unlabelled focusable element");
            }
            if f.index > f.count {
                return Some("a position past the count");
            }
        } else if speakable_len(self.summary.as_str()) == 0 {
            return Some("no focus and nothing to say");
        }
        None
    }
}

/// Bytes [`push_speakable`] would say of `s`, spaces and punctuation aside.
#[must_use]
pub fn speakable_len(s: &str) -> usize {
    let mut b = StrBuf::<255>::new();
    push_speakable(&mut b, s);
    b.as_str().chars().filter(|c| c.is_alphanumeric()).count()
}

/// Append `s` as the speech engine should read it: the UI's symbols in
/// words or left out (`·` and `|` a pause, `×` "times", `−` "minus", `—`
/// alone "none", arrows and ellipses dropped), white space collapsed.
pub fn push_speakable<const N: usize>(out: &mut StrBuf<N>, s: &str) {
    if s.trim() == "—" || s.trim() == "–" {
        out.push_str("none");
        return;
    }
    // A space is written only before the next word: none at the end, none
    // doubled.
    let mut space = false;
    let mut put = |out: &mut StrBuf<N>, t: &str| {
        for c in t.chars() {
            if c == ' ' {
                space = !out.is_empty() && !out.as_str().ends_with(' ');
            } else {
                if space && !matches!(c, ',' | '.') {
                    out.push(' ');
                }
                space = false;
                out.push(c);
            }
        }
    };
    let mut chars = s.chars().peekable();
    let mut prev = ' ';
    while let Some(c) = chars.next() {
        let next = chars.peek().copied().unwrap_or(' ');
        match c {
            '·' | '•' | '|' | '—' | '–' => put(out, ", "),
            '×' => put(out, " times "),
            '−' => put(out, " minus "),
            '+' if prev == ' ' && next == ' ' => put(out, " and "),
            '+' if next.is_ascii_digit() => put(out, " plus "),
            '›' | '‹' | '»' | '«' | '▲' | '▼' | '◀' | '▶' | '↵' | '…' | '♪' | '★' | '☆' =>
            {
                put(out, " ");
            }
            '\n' | '\t' => put(out, " "),
            _ => {
                let mut b = [0u8; 4];
                put(out, c.encode_utf8(&mut b));
            }
        }
        prev = c;
    }
}

/// Why the reader is asked to look.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cause {
    /// A key phase the shell has handled.
    Key(Key, KeyPhase),
    /// Anything else (a service event, time passing).
    Other,
}

fn hash(s: &str) -> u32 {
    s.bytes().fold(0x811C_9DC5u32, |h, b| (h ^ u32::from(b)).wrapping_mul(0x0100_0193))
}

/// What the reader remembers of the last description.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Snap {
    layer: Layer,
    screen: u8,
    depth: u8,
    title: u32,
    focus: bool,
    label: u32,
    value: u32,
    state: State,
    dim: bool,
    index: u32,
    count: u32,
    pending: bool,
    summary_empty: bool,
}

impl Snap {
    fn of(d: &Desc) -> Snap {
        let f = d.focus.unwrap_or_default();
        Snap {
            layer: d.layer,
            screen: d.screen,
            depth: d.depth,
            title: hash(d.title.as_str()),
            focus: d.focus.is_some(),
            label: hash(f.label.as_str()),
            value: hash(f.value.as_str()),
            state: f.state,
            dim: f.dim,
            index: f.index,
            count: f.count,
            pending: f.pending,
            summary_empty: d.summary.is_empty(),
        }
    }

    fn context(&self) -> (Layer, u8, u8, u32) {
        (self.layer, self.screen, self.depth, self.title)
    }
}

/// The screen reader's decisions (see the module documentation).
#[derive(Clone, Debug, Default)]
pub struct Reader {
    prev: Option<Snap>,
    toast_at: u64,
    volume: Option<(u8, u8)>,
    line: StrBuf<255>,
}

/// Up to the next sentence: append `s` to `out` with ", " between parts.
fn part<const N: usize>(out: &mut StrBuf<N>, s: &str) {
    if speakable_len(s) == 0 {
        return;
    }
    if !out.is_empty() && !out.as_str().ends_with(' ') {
        out.push_str(", ");
    }
    push_speakable(out, s);
}

/// End the sentence so far (". ", or a space after its own full stop).
fn full_stop<const N: usize>(out: &mut StrBuf<N>) {
    let t = out.as_str();
    if t.is_empty() || t.ends_with(' ') {
        return;
    }
    if t.ends_with(['.', '?', '!']) {
        out.push(' ');
    } else {
        out.push_str(". ");
    }
}

fn sentence<const N: usize>(out: &mut StrBuf<N>, s: &str) {
    if speakable_len(s) == 0 {
        return;
    }
    full_stop(out);
    push_speakable(out, s);
}

/// The focus as said at verbosity `v`.
fn focus_phrase<const N: usize>(out: &mut StrBuf<N>, f: &Focus, v: Verbosity) {
    part(out, f.label.as_str());
    part(out, f.value.as_str());
    if v >= Verbosity::Normal {
        part(out, f.role.word());
    }
    part(out, f.state.word());
    if v >= Verbosity::Normal && f.dim {
        part(out, "unavailable");
    }
    if v >= Verbosity::Normal && f.count > 1 && f.index > 0 {
        let mut b = StrBuf::<24>::new();
        b.push_u32(f.index);
        b.push_str(" of ");
        b.push_u32(f.count);
        part(out, b.as_str());
    }
    if v == Verbosity::Detailed && !f.dim {
        part(out, f.role.hint());
    }
}

impl Reader {
    #[must_use]
    pub const fn new() -> Reader {
        Reader { prev: None, toast_at: 0, volume: None, line: StrBuf::new() }
    }

    /// Forget what was said: the next description is spoken whole (the
    /// reader just came on, the language changed).
    pub fn reset(&mut self) {
        self.prev = None;
    }

    /// Take the description without saying anything (the keys are locked
    /// with the screen off: the lock notice ending is not news).
    pub fn note(&mut self, d: &Desc) {
        self.prev = Some(Snap::of(d));
        self.toast_at = d.toast_at;
    }

    fn flush(&mut self, cfg: &Config, sp: &mut dyn SpeechService) {
        if !self.line.is_empty() {
            sp.configure(cfg);
            sp.say(self.line.as_str());
            self.line.clear();
        }
    }

    fn earcon(cfg: &Config, sp: &mut dyn SpeechService, e: Earcon) {
        if cfg.earcons {
            sp.earcon(e);
        }
    }

    /// The whole description: title (and, for a dialog, its text), the
    /// summary when there is no focus or at Detailed, the focus.
    fn context_line(&mut self, d: &Desc, v: Verbosity) {
        self.line.clear();
        let mut t = StrBuf::<64>::new();
        t.push_str(d.title.as_str());
        match d.layer {
            Layer::Menu if d.title.as_str() != "Menu" => {
                t.push_str(", menu");
            }
            Layer::Dialog => {
                t.push_str(", dialog");
            }
            _ => {}
        }
        sentence(&mut self.line, t.as_str());
        if d.layer == Layer::Dialog || d.focus.is_none() || v == Verbosity::Detailed {
            sentence(&mut self.line, d.summary.as_str());
        }
        if let Some(f) = &d.focus
            && !f.pending
        {
            full_stop(&mut self.line);
            // An editor named as its screen (a slider): the label once.
            let mut f = *f;
            if f.label.as_str() == d.title.as_str() && !f.value.is_empty() {
                f.label.clear();
            }
            focus_phrase(&mut self.line, &f, v);
        }
    }

    /// Look at the description after `cause` and speak what changed.
    pub fn observe(&mut self, d: &Desc, cause: Cause, cfg: &Config, sp: &mut dyn SpeechService) {
        let v = cfg.verbosity;
        let key = match cause {
            Cause::Key(k, p) => Some((k, p)),
            Cause::Other => None,
        };
        // A key press silences what is being said (05 §7.14 "a key stops
        // the current utterance").
        if let Some((_, KeyPhase::Down | KeyPhase::Repeat)) = key {
            sp.stop();
        }
        let s = Snap::of(d);
        let prev = self.prev.replace(s);
        // The volume, wherever it was stepped.
        if let Some((k, KeyPhase::Down | KeyPhase::Repeat)) = key
            && matches!(k, Key::VolUp | Key::VolDown)
            && let Some((level, max)) = d.volume
        {
            self.line.clear();
            if v > Verbosity::Brief {
                self.line.push_str("Volume ");
            }
            self.line.push_u32(u32::from(level));
            if level == max && v > Verbosity::Brief {
                self.line.push_str(", maximum");
            }
            self.volume = d.volume;
            self.flush(cfg, sp);
        }
        match prev {
            None => {
                self.context_line(d, v);
                self.flush(cfg, sp);
            }
            Some(p) if p.context() != s.context() => {
                if key.is_none() {
                    // A screen raised by an event (an alarm, a dialog of a
                    // finished job): it speaks over what was said.
                    sp.stop();
                }
                let e = match (s.layer, p.layer) {
                    (Layer::Dialog, l) if l != Layer::Dialog => Some(Earcon::Dialog),
                    (Layer::Menu, l) if l != Layer::Menu => Some(Earcon::Menu),
                    (Layer::Screen, Layer::Screen) if s.depth > p.depth => Some(Earcon::Enter),
                    (Layer::Screen, Layer::Screen) if s.depth < p.depth => Some(Earcon::Back),
                    (Layer::Screen, Layer::Screen) => Some(Earcon::Enter),
                    (Layer::Screen, _) => Some(Earcon::Back),
                    _ => None,
                };
                if let Some(e) = e {
                    Self::earcon(cfg, sp, e);
                }
                self.context_line(d, v);
                self.flush(cfg, sp);
            }
            Some(p) => self.changed(d, &p, &s, key, cfg, sp),
        }
        // A new toast is said after the rest.
        if !d.toast.is_empty() && d.toast_at != self.toast_at {
            self.toast_at = d.toast_at;
            Self::earcon(cfg, sp, Earcon::Toast);
            self.line.clear();
            push_speakable(&mut self.line, d.toast.as_str());
            self.flush(cfg, sp);
        }
    }

    /// The same screen: a moved focus, a changed value or state, a list end.
    fn changed(
        &mut self,
        d: &Desc,
        p: &Snap,
        s: &Snap,
        key: Option<(Key, KeyPhase)>,
        cfg: &Config,
        sp: &mut dyn SpeechService,
    ) {
        let v = cfg.verbosity;
        let Some(f) = &d.focus else {
            // No focus: a summary that appears with a key (a game's
            // state) is not followed; where am I says it.
            return;
        };
        if f.pending {
            return;
        }
        let moved = !p.focus || p.label != s.label || p.index != s.index || p.pending;
        let stepped = matches!(
            key,
            Some((Key::Up | Key::Down, KeyPhase::Down | KeyPhase::Repeat))
                | Some((Key::Prev | Key::Next, KeyPhase::Up { held: false }))
        );
        if moved {
            if key.is_none() && p.focus && p.label == s.label && !p.pending {
                return;
            }
            // Rolled round: from the last to the first or back.
            if stepped && p.count == s.count && s.count > 1 {
                let wrapped = (p.index == p.count && s.index == 1) || (p.index == 1 && s.index == s.count);
                if wrapped {
                    Self::earcon(cfg, sp, Earcon::Wrap);
                }
            }
            self.line.clear();
            focus_phrase(&mut self.line, f, v);
            self.flush(cfg, sp);
            return;
        }
        if key.is_none() {
            // Values that move by themselves (clocks, levels) are said on
            // request only.
            return;
        }
        if p.state != s.state || p.value != s.value || p.dim != s.dim {
            self.line.clear();
            match f.state {
                State::On | State::Checked => Self::earcon(cfg, sp, Earcon::On),
                State::Off | State::NotChecked => Self::earcon(cfg, sp, Earcon::Off),
                _ => {}
            }
            if p.value != s.value {
                part(&mut self.line, f.value.as_str());
            }
            if p.state != s.state {
                part(&mut self.line, f.state.word());
            }
            if p.dim != s.dim && v > Verbosity::Brief {
                part(&mut self.line, if f.dim { "unavailable" } else { "available" });
            }
            if self.line.is_empty() {
                focus_phrase(&mut self.line, f, v);
            }
            self.flush(cfg, sp);
            return;
        }
        // Up or Down moved nothing: a list end (roll-around off, or a held
        // key that stops there). Not Prev/Next: a tap's release follows a
        // press that may have moved the focus already (Home, editors).
        if matches!(key, Some((Key::Up | Key::Down, KeyPhase::Down | KeyPhase::Repeat))) && s.count > 0 {
            Self::earcon(cfg, sp, Earcon::End);
        }
    }

    /// "Where am I": the screen's title (and what it is over), its
    /// summary, the focus with its position, at Detailed verbosity.
    pub fn where_am_i(&mut self, d: &Desc, cfg: &Config, sp: &mut dyn SpeechService) {
        sp.stop();
        self.context_line(d, Verbosity::Detailed);
        if d.layer != Layer::Screen && !d.under.is_empty() {
            let mut b = StrBuf::<64>::new();
            b.push_str("over ");
            b.push_str(d.under.as_str());
            sentence(&mut self.line, b.as_str());
        }
        if d.focus.is_some() && d.layer == Layer::Screen {
            sentence(&mut self.line, d.summary.as_str());
        }
        self.flush(cfg, sp);
        self.prev = Some(Snap::of(d));
    }

    /// "Time and battery" (where-am-I held 2 s).
    pub fn time_battery(&mut self, st: &Status, cfg: &Config, sp: &mut dyn SpeechService) {
        sp.stop();
        self.line.clear();
        if let Some((h, m)) = st.clock {
            self.line.push_u32(u32::from(h));
            self.line.push(':');
            self.line.push_u32_padded(u32::from(m), 2);
            self.line.push_str(". ");
        }
        self.line.push_str("Battery ");
        self.line.push_u32(u32::from(st.battery));
        self.line.push_str(" percent");
        if st.charging {
            self.line.push_str(", charging");
        }
        self.flush(cfg, sp);
    }

    /// Say `text` at once (the reader turned on or off).
    pub fn announce(&mut self, text: &str, e: Earcon, cfg: &Config, sp: &mut dyn SpeechService) {
        sp.stop();
        Self::earcon(cfg, sp, e);
        self.line.clear();
        self.line.push_str(text);
        self.flush(cfg, sp);
    }
}

/// A chord the [`Chords`] detector recognised.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Chord {
    /// Back and Menu held 2 s: the screen reader on or off.
    Toggle,
    /// The where-am-I pair held for the hold delay.
    WhereAmI,
    /// The where-am-I pair held 2 s.
    TimeBattery,
}

/// The toggle pair (05 §7.14 asks for Vol+ and Vol−, which share ladder 1
/// and cannot be seen together: Back and Menu, which no screen acts on
/// while they go down).
pub const TOGGLE_KEYS: (Key, Key) = (Key::Back, Key::Menu);
/// How long the toggle pair is held.
pub const TOGGLE_MS: u64 = 2000;
/// How long the where-am-I pair is held for the time and battery.
pub const TIME_MS: u64 = 2000;

/// What a key edge does with the chords.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Edge {
    /// Not part of a chord: hand it to the key mapper.
    Pass,
    /// Part of a chord: swallow it.
    Swallow,
    /// A chord started with this edge: swallow it, and forget the press of
    /// the other key (`KeyMapper::cancel`, `KeyGate::forget`): its release
    /// is swallowed too.
    Start(Key),
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Active {
    toggle: bool,
    since: u64,
    /// Keys of the chord still down.
    keys: u16,
    fired: u8,
}

/// Chord detection on raw key edges (see the module documentation).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Chords {
    down: u16,
    active: Option<Active>,
}

const fn bit(k: Key) -> u16 {
    1 << k.index()
}

const fn pair(a: Key, b: Key, p: (Key, Key)) -> bool {
    (a as u8 == p.0 as u8 && b as u8 == p.1 as u8) || (a as u8 == p.1 as u8 && b as u8 == p.0 as u8)
}

impl Chords {
    #[must_use]
    pub const fn new() -> Chords {
        Chords { down: 0, active: None }
    }

    /// A raw key went down at `now`. `where_keys`: the where-am-I pair
    /// while the reader is on ([`WhereKeys::Off`] otherwise).
    pub fn press(&mut self, k: Key, now: Instant, where_keys: WhereKeys) -> Edge {
        if let Some(a) = &mut self.active {
            // A third key during a chord: swallowed with it.
            a.keys |= bit(k);
            self.down |= bit(k);
            return Edge::Swallow;
        }
        let mut edge = Edge::Pass;
        for other in Key::ALL {
            if self.down & bit(other) == 0 || other == k {
                continue;
            }
            let toggle = pair(other, k, TOGGLE_KEYS);
            let wher = where_keys.keys().is_some_and(|p| pair(other, k, p));
            if toggle || wher {
                self.active = Some(Active { toggle, since: now.0, keys: bit(other) | bit(k), fired: 0 });
                edge = Edge::Start(other);
                break;
            }
        }
        self.down |= bit(k);
        edge
    }

    /// A raw key went up.
    pub fn release(&mut self, k: Key) -> Edge {
        self.down &= !bit(k);
        if let Some(a) = &mut self.active
            && a.keys & bit(k) != 0
        {
            a.keys &= !bit(k);
            if a.keys == 0 {
                self.active = None;
            }
            return Edge::Swallow;
        }
        Edge::Pass
    }

    /// A chord due at `now` (`hold_ms`: the hold delay).
    pub fn poll(&mut self, now: Instant, hold_ms: u32) -> Option<Chord> {
        let a = self.active.as_mut()?;
        let held = now.0.saturating_sub(a.since);
        if a.toggle {
            if a.fired == 0 && held >= TOGGLE_MS {
                a.fired = 1;
                return Some(Chord::Toggle);
            }
            return None;
        }
        if a.fired == 0 && held >= u64::from(hold_ms) {
            a.fired = 1;
            return Some(Chord::WhereAmI);
        }
        if a.fired == 1 && held >= TIME_MS.max(u64::from(hold_ms) + 500) {
            a.fired = 2;
            return Some(Chord::TimeBattery);
        }
        None
    }

    /// When [`Chords::poll`] has something next.
    #[must_use]
    pub fn next_deadline(&self, hold_ms: u32) -> Option<Instant> {
        let a = self.active?;
        let at = match (a.toggle, a.fired) {
            (true, 0) => a.since + TOGGLE_MS,
            (false, 0) => a.since + u64::from(hold_ms),
            (false, 1) => a.since + TIME_MS.max(u64::from(hold_ms) + 500),
            _ => return None,
        };
        Some(Instant(at))
    }

    /// A chord is being held.
    #[must_use]
    pub fn active(&self) -> bool {
        self.active.is_some()
    }
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use std::string::String;
    use std::vec::Vec;

    #[derive(Default)]
    struct Log(Vec<String>);

    impl SpeechService for Log {
        fn stop(&mut self) {
            self.0.push("|".into());
        }
        fn say(&mut self, text: &str) {
            self.0.push(text.into());
        }
        fn earcon(&mut self, e: Earcon) {
            self.0.push(std::format!("<{}>", e.name()));
        }
    }

    fn said(l: &Log) -> Vec<&str> {
        l.0.iter().map(String::as_str).filter(|s| *s != "|").collect()
    }

    fn list_desc(title: &str, label: &str, i: u32, n: u32) -> Desc {
        let mut d = Desc::new();
        d.title(title);
        d.focus(Role::Submenu, label).at(i, n);
        d
    }

    #[test]
    fn verbosity_levels() {
        let mut f = Focus { role: Role::Switch, state: State::On, dim: true, ..Focus::default() };
        f.label.push_str("Shuffle");
        f.at(1, 9);
        let say = |f: &Focus, v| {
            let mut b = StrBuf::<128>::new();
            focus_phrase(&mut b, f, v);
            std::string::String::from(b.as_str())
        };
        assert_eq!(say(&f, Verbosity::Brief), "Shuffle, on");
        assert_eq!(say(&f, Verbosity::Normal), "Shuffle, switch, on, unavailable, 2 of 9");
        assert_eq!(say(&f, Verbosity::Detailed), "Shuffle, switch, on, unavailable, 2 of 9");
        f.dim = false;
        assert_eq!(say(&f, Verbosity::Detailed), "Shuffle, switch, on, 2 of 9, OK to change");
    }

    #[test]
    fn a_new_screen_then_moves_and_values() {
        let cfg = Config { on: true, ..Config::DEFAULT };
        let mut r = Reader::new();
        let mut sp = Log::default();
        r.observe(&list_desc("Settings", "Sound", 0, 13), Cause::Other, &cfg, &mut sp);
        assert_eq!(said(&sp), ["Settings. Sound, submenu, 1 of 13"]);
        sp.0.clear();
        r.observe(
            &list_desc("Settings", "Playback", 1, 13),
            Cause::Key(Key::Down, KeyPhase::Down),
            &cfg,
            &mut sp,
        );
        assert_eq!(sp.0, ["|", "Playback, submenu, 2 of 13"], "a key interrupts first");
        sp.0.clear();
        // Back up and roll round from the first row to the last.
        r.observe(&list_desc("Settings", "Sound", 0, 13), Cause::Key(Key::Up, KeyPhase::Down), &cfg, &mut sp);
        sp.0.clear();
        r.observe(
            &list_desc("Settings", "Accessibility", 12, 13),
            Cause::Key(Key::Up, KeyPhase::Down),
            &cfg,
            &mut sp,
        );
        assert_eq!(said(&sp), ["<wrap>", "Accessibility, submenu, 13 of 13"]);
        sp.0.clear();
        // Nothing moved: the list end.
        r.observe(
            &list_desc("Settings", "Accessibility", 12, 13),
            Cause::Key(Key::Down, KeyPhase::Repeat),
            &cfg,
            &mut sp,
        );
        assert_eq!(said(&sp), ["<end>"]);
        sp.0.clear();
        // A deeper screen.
        let mut d = list_desc("Display", "Theme", 0, 12);
        d.depth = 2;
        r.observe(&d, Cause::Key(Key::Ok, KeyPhase::Up { held: false }), &cfg, &mut sp);
        assert_eq!(said(&sp), ["<enter>", "Display. Theme, submenu, 1 of 12"]);
        sp.0.clear();
        // A value changed by a key.
        let mut d2 = d;
        d2.focus = None;
        d2.focus(Role::Switch, "Theme").state(State::On).at(0, 12);
        r.observe(&d2, Cause::Key(Key::Ok, KeyPhase::Up { held: false }), &cfg, &mut sp);
        assert_eq!(said(&sp), ["<on>", "on"]);
        sp.0.clear();
        // Without earcons.
        let quiet = Config { earcons: false, ..cfg };
        let mut d3 = d2;
        d3.focus.as_mut().unwrap().state(State::Off);
        r.observe(&d3, Cause::Key(Key::Ok, KeyPhase::Up { held: false }), &quiet, &mut sp);
        assert_eq!(said(&sp), ["off"]);
    }

    #[test]
    fn events_queue_and_pending_rows_wait() {
        let cfg = Config { on: true, ..Config::DEFAULT };
        let mut r = Reader::new();
        let mut sp = Log::default();
        let mut d = Desc::new();
        d.title("Files");
        let f = d.focus(Role::Item, "loading");
        f.pending = true;
        f.at(0, 40);
        r.observe(&d, Cause::Other, &cfg, &mut sp);
        assert_eq!(said(&sp), ["Files"], "the title, not the placeholder");
        sp.0.clear();
        let d2 = list_desc("Files", "Music", 0, 40);
        r.observe(&d2, Cause::Other, &cfg, &mut sp);
        assert_eq!(sp.0, ["Music, submenu, 1 of 40"], "the row once loaded, queued (no stop)");
        sp.0.clear();
        // A toast.
        let mut d3 = d2;
        d3.toast.push_str("Card removed");
        d3.toast_at = 5;
        r.observe(&d3, Cause::Other, &cfg, &mut sp);
        assert_eq!(said(&sp), ["<toast>", "Card removed"]);
        sp.0.clear();
        r.observe(&d3, Cause::Other, &cfg, &mut sp);
        assert!(sp.0.is_empty(), "said once");
    }

    #[test]
    fn dialogs_and_menus() {
        let cfg = Config { on: true, ..Config::DEFAULT };
        let mut r = Reader::new();
        let mut sp = Log::default();
        r.observe(&list_desc("Files", "a.txt", 0, 3), Cause::Other, &cfg, &mut sp);
        sp.0.clear();
        let mut d = Desc::new();
        d.layer = Layer::Dialog;
        d.title("Delete");
        d.say("Delete a.txt?");
        d.focus(Role::Button, "Cancel").at(0, 2);
        r.observe(&d, Cause::Key(Key::Ok, KeyPhase::Up { held: false }), &cfg, &mut sp);
        assert_eq!(said(&sp), ["<dialog>", "Delete, dialog. Delete a.txt? Cancel, button, 1 of 2"]);
        sp.0.clear();
        let mut m = Desc::new();
        m.layer = Layer::Menu;
        m.title("Menu");
        m.focus(Role::Item, "Play").at(0, 4);
        r.observe(&m, Cause::Key(Key::Menu, KeyPhase::Up { held: false }), &cfg, &mut sp);
        assert_eq!(said(&sp), ["<menu>", "Menu. Play, 1 of 4"]);
    }

    #[test]
    fn speakable_symbols() {
        let s = |t: &str| {
            let mut b = StrBuf::<96>::new();
            push_speakable(&mut b, t);
            std::string::String::from(b.as_str())
        };
        assert_eq!(s("1.25×"), "1.25 times");
        assert_eq!(s("—"), "none");
        assert_eq!(s("Artist · Album"), "Artist, Album");
        assert_eq!(s("Back + OK"), "Back and OK");
        assert_eq!(s("+3 dB"), "plus 3 dB");
        assert_eq!(s("−05:00"), "minus 05:00");
        assert_eq!(s("Next ›"), "Next");
        assert_eq!(speakable_len("◀▶ ·"), 0);
    }

    #[test]
    fn queue_keeps_order_and_fixed_room() {
        let mut q = UtteranceQueue::<64>::new();
        assert!(q.push_earcon(Earcon::Enter));
        assert!(q.push_text("Settings"));
        assert!(q.push_text("Sound, 1 of 13"));
        let mut b = StrBuf::<255>::new();
        assert_eq!(q.pop(&mut b), Some(Utterance::Earcon(Earcon::Enter)));
        assert_eq!(q.pop(&mut b), Some(Utterance::Text));
        assert_eq!(b.as_str(), "Settings");
        assert_eq!(q.pop(&mut b), Some(Utterance::Text));
        assert_eq!(b.as_str(), "Sound, 1 of 13");
        assert_eq!(q.pop(&mut b), None);
        // Wraps round the ring; a long text is cut at a char boundary.
        for _ in 0..10 {
            assert!(q.push_text("ééééé"));
            assert_eq!(q.pop(&mut b), Some(Utterance::Text));
        }
        assert!(q.push_text(&"é".repeat(40)));
        assert_eq!(q.dropped, 1);
        assert_eq!(q.pop(&mut b), Some(Utterance::Text));
        assert_eq!(b.as_str().chars().count(), 31);
        q.clear();
        assert!(q.is_empty());
    }

    #[test]
    fn chords_toggle_and_where_am_i() {
        let mut c = Chords::new();
        let t = Instant;
        assert_eq!(c.press(Key::Back, t(0), WhereKeys::BackOk), Edge::Pass);
        assert_eq!(c.press(Key::Menu, t(50), WhereKeys::BackOk), Edge::Start(Key::Back));
        assert_eq!(c.poll(t(1000), 500), None);
        assert_eq!(c.next_deadline(500), Some(t(2050)));
        assert_eq!(c.poll(t(2050), 500), Some(Chord::Toggle));
        assert_eq!(c.poll(t(3000), 500), None, "once");
        assert_eq!(c.release(Key::Back), Edge::Swallow);
        assert_eq!(c.release(Key::Menu), Edge::Swallow);
        assert!(!c.active());
        // Where am I, then time and battery; off: plain keys.
        assert_eq!(c.press(Key::Ok, t(5000), WhereKeys::BackOk), Edge::Pass);
        assert_eq!(c.press(Key::Back, t(5010), WhereKeys::BackOk), Edge::Start(Key::Ok));
        assert_eq!(c.poll(t(5510), 500), Some(Chord::WhereAmI));
        assert_eq!(c.poll(t(7010), 500), Some(Chord::TimeBattery));
        c.release(Key::Ok);
        c.release(Key::Back);
        assert_eq!(c.press(Key::Ok, t(9000), WhereKeys::Off), Edge::Pass);
        assert_eq!(c.press(Key::Back, t(9010), WhereKeys::Off), Edge::Pass);
        assert_eq!(c.release(Key::Back), Edge::Pass);
        assert_eq!(c.release(Key::Ok), Edge::Pass);
        // Prev + Next.
        assert_eq!(c.press(Key::Prev, t(10_000), WhereKeys::PrevNext), Edge::Pass);
        assert_eq!(c.press(Key::Next, t(10_000), WhereKeys::PrevNext), Edge::Start(Key::Prev));
    }

    #[test]
    fn lint() {
        let mut d = Desc::new();
        assert_eq!(d.problem(), Some("no title"));
        d.title("Home");
        assert_eq!(d.problem(), Some("no focus and nothing to say"));
        d.focus(Role::Button, "◀▶");
        assert_eq!(d.problem(), Some("an unlabelled focusable element"));
        d.focus(Role::Button, "Music").at(0, 12);
        assert_eq!(d.problem(), None);
    }

    #[test]
    fn sizes() {
        assert!(core::mem::size_of::<Desc>() <= 700, "{}", core::mem::size_of::<Desc>());
        assert!(core::mem::size_of::<Reader>() <= 400, "{}", core::mem::size_of::<Reader>());
        assert!(core::mem::size_of::<Chords>() <= 24);
        std::println!(
            "RAM: Desc {} B, Reader {} B, Chords {} B, UtteranceQueue<1024> {} B",
            core::mem::size_of::<Desc>(),
            core::mem::size_of::<Reader>(),
            core::mem::size_of::<Chords>(),
            core::mem::size_of::<UtteranceQueue<1024>>()
        );
    }
}
