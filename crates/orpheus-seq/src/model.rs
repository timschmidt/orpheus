//! The project: patterns of drum steps, tracks, a song, tempo and swing.
//!
//! Everything is fixed-size plain data (no allocation): a [`Project`] holds
//! [`PATTERNS`] patterns of up to [`MAX_STEPS`] steps for [`TRACKS`] drum
//! tracks, the kit they play, and a song of up to [`SONG_LEN`] entries.
//! A [`Step`] is three bytes; the whole project is about 25 KiB
//! ([`crate::ram`]).
//!
//! Every setter clamps its value to the documented range, so the model
//! never holds a value the player or the file format cannot take; every
//! change bumps [`Project::rev`] (autosave and redraws compare it).

use orpheus_synth::bank::{NAME_LEN, Name};
use orpheus_synth::kit::{self, Kit};

/// Patterns in a project.
pub const PATTERNS: usize = 16;
/// Most steps in a pattern (1/16 notes: four bars of 4/4).
pub const MAX_STEPS: usize = 64;
/// Steps on one page of the grid.
pub const PAGE: usize = 16;
/// Drum tracks (SEQ-2 adds two synth tracks).
pub const TRACKS: usize = 8;
/// Entries of the song (pattern chain).
pub const SONG_LEN: usize = 64;
/// Most repeats of one song entry.
pub const MAX_REPEATS: u8 = 16;
/// Tempo range, hundredths of a BPM (40–240 BPM).
pub const TEMPO_MIN: u16 = 4_000;
/// Highest tempo, hundredths of a BPM.
pub const TEMPO_MAX: u16 = 24_000;
/// A new project's tempo (120 BPM).
pub const TEMPO_DEFAULT: u16 = 12_000;
/// Swing range, per cent of a step pair given to its first step (50 =
/// straight, 66.7 ≈ triplet feel, 75 = dotted).
pub const SWING_MIN: u8 = 50;
/// Most swing, per cent.
pub const SWING_MAX: u8 = 75;
/// Most micro-timing delay, 1/24 of a step (half a step).
pub const MICRO_MAX: u8 = 12;
/// Most ratchet hits in a step.
pub const MAX_RATCHET: u8 = 4;
/// Probability levels: a step plays with probability `level/16`.
pub const PROB_LEVELS: u8 = 16;
/// Velocity added by an accent (clamped at 127).
pub const ACCENT: u8 = 32;
/// The default velocity of a new step.
pub const VELOCITY_DEFAULT: u8 = 100;
/// The default track level, per cent.
pub const LEVEL_DEFAULT: u8 = 80;

/// One step of one drum track, three bytes:
///
/// ```text
/// a: bit 7 on, bits 0–6 velocity (1–127; kept while off)
/// b: bits 0–3 probability code k (plays with (16 − k)/16), bits 4–5
///    ratchet − 1, bit 6 accent, bit 7 reserved (0)
/// c: bits 0–4 micro-timing delay in 1/24 step (0–12), bits 5–7 reserved
/// ```
///
/// That is also its form in the project file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Step {
    a: u8,
    b: u8,
    c: u8,
}

impl Default for Step {
    fn default() -> Self {
        Self::OFF
    }
}

impl Step {
    /// An empty step with the default parameters.
    pub const OFF: Step = Step { a: VELOCITY_DEFAULT, b: 0, c: 0 };

    /// A step that plays at `velocity`.
    #[must_use]
    pub const fn hit(velocity: u8) -> Step {
        Step::OFF.with_velocity(velocity).with_on(true)
    }

    /// The step from its three stored bytes, sanitised: reserved bits
    /// cleared, velocity 0 read as 1, the delay clamped.
    #[must_use]
    pub const fn from_bytes(b: [u8; 3]) -> Step {
        let a = if b[0] & 0x7F == 0 { (b[0] & 0x80) | 1 } else { b[0] };
        let mut c = b[2] & 0x1F;
        if c > MICRO_MAX {
            c = MICRO_MAX;
        }
        Step { a, b: b[1] & 0x7F, c }
    }

    /// The three stored bytes.
    #[must_use]
    pub const fn to_bytes(self) -> [u8; 3] {
        [self.a, self.b, self.c]
    }

    /// Whether the step plays.
    #[must_use]
    pub const fn on(self) -> bool {
        self.a & 0x80 != 0
    }

    /// Velocity, 1–127.
    #[must_use]
    pub const fn velocity(self) -> u8 {
        self.a & 0x7F
    }

    /// Probability level 1–16: plays with `level/16`.
    #[must_use]
    pub const fn prob(self) -> u8 {
        PROB_LEVELS - (self.b & 0x0F)
    }

    /// Hits in the step, 1–4.
    #[must_use]
    pub const fn ratchet(self) -> u8 {
        ((self.b >> 4) & 3) + 1
    }

    /// Accented.
    #[must_use]
    pub const fn accent(self) -> bool {
        self.b & 0x40 != 0
    }

    /// Delay after the step's start, 1/24 of a nominal step (0–12).
    #[must_use]
    pub const fn micro(self) -> u8 {
        self.c
    }

    /// With `on`.
    #[must_use]
    pub const fn with_on(self, on: bool) -> Step {
        Step { a: (self.a & 0x7F) | if on { 0x80 } else { 0 }, ..self }
    }

    /// With velocity `v` (clamped to 1–127).
    #[must_use]
    pub const fn with_velocity(self, v: u8) -> Step {
        let v = if v == 0 {
            1
        } else if v > 127 {
            127
        } else {
            v
        };
        Step { a: (self.a & 0x80) | v, ..self }
    }

    /// With probability level `p` (clamped to 1–16).
    #[must_use]
    pub const fn with_prob(self, p: u8) -> Step {
        let p = if p == 0 {
            1
        } else if p > PROB_LEVELS {
            PROB_LEVELS
        } else {
            p
        };
        Step { b: (self.b & !0x0F) | (PROB_LEVELS - p), ..self }
    }

    /// With `r` hits (clamped to 1–4).
    #[must_use]
    pub const fn with_ratchet(self, r: u8) -> Step {
        let r = if r == 0 {
            1
        } else if r > MAX_RATCHET {
            MAX_RATCHET
        } else {
            r
        };
        Step { b: (self.b & !0x30) | ((r - 1) << 4), ..self }
    }

    /// With the accent set or cleared.
    #[must_use]
    pub const fn with_accent(self, on: bool) -> Step {
        Step { b: (self.b & !0x40) | if on { 0x40 } else { 0 }, ..self }
    }

    /// With delay `m` (clamped to 0–12).
    #[must_use]
    pub const fn with_micro(self, m: u8) -> Step {
        Step { c: if m > MICRO_MAX { MICRO_MAX } else { m }, ..self }
    }

    /// The probability in per cent (rounded), for the screen.
    #[must_use]
    pub const fn prob_percent(self) -> u8 {
        ((self.prob() as u16 * 100 + 8) / 16) as u8
    }
}

/// A pattern: its length and the steps of every track.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Pattern {
    len: u8,
    steps: [[Step; MAX_STEPS]; TRACKS],
}

impl Pattern {
    /// Sixteen empty steps.
    pub const EMPTY: Pattern = Pattern { len: PAGE as u8, steps: [[Step::OFF; MAX_STEPS]; TRACKS] };

    /// Steps, 1–64.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// Never: a pattern has at least one step (for clippy's `len` rule).
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        false
    }

    /// Whether no step is on and the length is the default (not written to
    /// the file).
    #[must_use]
    pub fn is_blank(&self) -> bool {
        // A loop, not a comparison with `Pattern::EMPTY`: that would put
        // a 1.5 KB image of it in read-only data.
        self.len() == PAGE && self.steps.iter().all(|t| t.iter().all(|s| *s == Step::OFF))
    }

    /// Empty: sixteen steps, all off (in place).
    pub fn clear(&mut self) {
        self.len = PAGE as u8;
        for t in &mut self.steps {
            t.fill(Step::OFF);
        }
    }

    /// The step of `track` at `step` (an empty step outside).
    #[must_use]
    pub fn step(&self, track: usize, step: usize) -> Step {
        self.steps.get(track).and_then(|t| t.get(step)).copied().unwrap_or(Step::OFF)
    }

    /// The steps of `track` (all 64, also past the length).
    #[must_use]
    pub fn track(&self, track: usize) -> &[Step; MAX_STEPS] {
        &self.steps[track.min(TRACKS - 1)]
    }

    /// Steps that are on in the pattern's length.
    #[must_use]
    pub fn hits(&self) -> usize {
        self.steps.iter().map(|t| t[..self.len()].iter().filter(|s| s.on()).count()).sum()
    }

    /// Steps of each track worth storing: the length, or more when a step
    /// past it is not empty (a shortened pattern keeps its steps).
    #[must_use]
    pub fn stored(&self) -> usize {
        let last = self
            .steps
            .iter()
            .filter_map(|t| t.iter().rposition(|s| *s != Step::OFF))
            .max()
            .map_or(0, |i| i + 1);
        last.max(self.len())
    }

    pub(crate) fn set_len_raw(&mut self, n: usize) {
        self.len = n.clamp(1, MAX_STEPS) as u8;
    }

    pub(crate) fn set_raw(&mut self, track: usize, step: usize, s: Step) {
        if let Some(x) = self.steps.get_mut(track).and_then(|t| t.get_mut(step)) {
            *x = s;
        }
    }
}

/// A drum track: which kit note it plays, its level, mute and solo.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Track {
    /// The kit note (MIDI note number, 0–127).
    pub note: u8,
    /// Level, per cent (0–100): scales the velocities.
    pub level: u8,
    /// Muted.
    pub mute: bool,
    /// Soloed (while any track is soloed only soloed tracks sound).
    pub solo: bool,
}

impl Track {
    /// A track on `note` at the default level.
    #[must_use]
    pub const fn new(note: u8) -> Track {
        Track { note, level: LEVEL_DEFAULT, mute: false, solo: false }
    }
}

/// A new project's probability seed.
pub const SEED_DEFAULT: u32 = 0x05E0_0001;

/// The default tracks: kick, snare, clap, closed hat, open hat, low tom,
/// high tom, rim (General MIDI notes, so every factory kit has most).
pub const DEFAULT_NOTES: [u8; TRACKS] = [36, 38, 39, 42, 46, 45, 50, 37];

/// One entry of the song: a pattern played `repeats` times.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SongEntry {
    /// Pattern index, 0–15.
    pub pattern: u8,
    /// Times it plays, 1–16.
    pub repeats: u8,
}

/// The song: a chain of patterns.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Song {
    len: u8,
    entries: [SongEntry; SONG_LEN],
}

impl Song {
    /// No entries.
    pub const EMPTY: Song = Song { len: 0, entries: [SongEntry { pattern: 0, repeats: 1 }; SONG_LEN] };

    /// The entries.
    #[must_use]
    pub fn entries(&self) -> &[SongEntry] {
        &self.entries[..usize::from(self.len)]
    }

    /// Entries.
    #[must_use]
    pub const fn len(&self) -> usize {
        self.len as usize
    }

    /// No entries.
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// Append an entry (pattern and repeats clamped); false when full.
    pub fn push(&mut self, pattern: u8, repeats: u8) -> bool {
        let Some(e) = self.entries.get_mut(usize::from(self.len)) else { return false };
        *e = SongEntry { pattern: pattern.min(PATTERNS as u8 - 1), repeats: repeats.clamp(1, MAX_REPEATS) };
        self.len += 1;
        true
    }

    /// Remove every entry.
    pub fn clear(&mut self) {
        self.len = 0;
        self.entries.fill(SongEntry { pattern: 0, repeats: 1 });
    }
}

/// How the transport walks through the patterns.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum PlayMode {
    /// The current pattern loops; a pattern chosen while playing starts at
    /// the end of the current one.
    #[default]
    Pattern,
    /// The song's chain, from its first entry, round again at its end (an
    /// empty song plays the current pattern).
    Song,
}

/// A whole project.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Project {
    name: Name,
    tempo: u16,
    swing: u8,
    /// The pattern being edited (and played in pattern mode).
    current: u8,
    mode: PlayMode,
    /// Seed of the probability draws (a saved song renders the same).
    pub seed: u32,
    /// The kit the drum tracks play, and its name.
    kit: Kit,
    kit_name: Name,
    tracks: [Track; TRACKS],
    patterns: [Pattern; PATTERNS],
    /// The song.
    pub song: Song,
    rev: u32,
    kit_rev: u32,
}

impl Default for Project {
    fn default() -> Self {
        Self::NEW
    }
}

const fn name16(s: &str) -> Name {
    let mut n = [0u8; NAME_LEN];
    let b = s.as_bytes();
    let mut i = 0;
    while i < b.len() && i < NAME_LEN {
        n[i] = b[i];
        i += 1;
    }
    Name(n)
}

const fn default_tracks() -> [Track; TRACKS] {
    let mut t = [Track::new(0); TRACKS];
    let mut i = 0;
    while i < TRACKS {
        t[i] = Track::new(DEFAULT_NOTES[i]);
        i += 1;
    }
    t
}

impl Project {
    /// A new, empty project: 120 BPM, straight, the standard kit, the
    /// default tracks, 16 empty patterns of 16 steps.
    pub const NEW: Project = Project {
        name: Project::DEFAULT_NAME,
        tempo: TEMPO_DEFAULT,
        swing: SWING_MIN,
        current: 0,
        mode: PlayMode::Pattern,
        seed: SEED_DEFAULT,
        kit: Kit::new(kit::STANDARD),
        kit_name: name16("standard"),
        tracks: default_tracks(),
        patterns: [Pattern::EMPTY; PATTERNS],
        song: Song::EMPTY,
        rev: 0,
        kit_rev: 0,
    };

    /// Back to [`Project::NEW`] in place, keeping the revision counters
    /// moving (so a redraw and the player's kit follow). Field by field:
    /// `*self = Project::NEW` would copy a 25 KB image of the constant
    /// from read-only data (on the player, flash code pages).
    pub fn reset(&mut self) {
        self.name = Project::DEFAULT_NAME;
        self.tempo = TEMPO_DEFAULT;
        self.swing = SWING_MIN;
        self.current = 0;
        self.mode = PlayMode::Pattern;
        self.seed = SEED_DEFAULT;
        self.kit = Kit::new(kit::STANDARD);
        self.kit_name = name16("standard");
        for (t, n) in self.tracks.iter_mut().zip(DEFAULT_NOTES) {
            *t = Track::new(n);
        }
        for p in &mut self.patterns {
            p.clear();
        }
        self.song.clear();
        self.rev = self.rev.wrapping_add(1);
        self.kit_rev = self.kit_rev.wrapping_add(1);
    }

    /// The project's name.
    #[must_use]
    pub fn name(&self) -> &str {
        self.name.as_str()
    }

    /// Rename (cut to 16 bytes at a character boundary).
    pub fn set_name(&mut self, s: &str) {
        self.name = Name::new(s);
        self.touch();
    }

    /// A new project's name.
    pub const DEFAULT_NAME: Name = name16("Beat");

    pub(crate) fn name_raw(&self) -> Name {
        self.name
    }

    pub(crate) fn set_name_raw(&mut self, n: Name) {
        self.name = n;
    }

    /// Changes so far (wraps): compare with a saved value to see edits.
    #[must_use]
    pub const fn rev(&self) -> u32 {
        self.rev
    }

    /// Kit changes so far: the player installs the kit when it moves.
    #[must_use]
    pub const fn kit_rev(&self) -> u32 {
        self.kit_rev
    }

    /// Zero the revision counters (they are not part of what a project
    /// is: equality after a round trip compares the rest).
    pub fn normalize_revs(&mut self) {
        self.rev = 0;
        self.kit_rev = 0;
    }

    /// Whether `self` and `other` hold the same project (revision counters
    /// aside).
    #[must_use]
    pub fn same_as(&self, other: &Project) -> bool {
        self.name == other.name
            && self.tempo == other.tempo
            && self.swing == other.swing
            && self.current == other.current
            && self.mode == other.mode
            && self.seed == other.seed
            && self.kit == other.kit
            && self.kit_name == other.kit_name
            && self.tracks == other.tracks
            && self.patterns == other.patterns
            && self.song == other.song
    }

    fn touch(&mut self) {
        self.rev = self.rev.wrapping_add(1);
    }

    /// How the transport plays.
    #[must_use]
    pub const fn mode(&self) -> PlayMode {
        self.mode
    }

    /// Set the play mode (taken at the end of the playing pattern).
    pub fn set_mode(&mut self, m: PlayMode) {
        self.mode = m;
        self.touch();
    }

    /// Tempo, hundredths of a BPM.
    #[must_use]
    pub const fn tempo(&self) -> u16 {
        self.tempo
    }

    /// Set the tempo (hundredths of a BPM, clamped to 40–240 BPM). The
    /// player takes it at the next step.
    pub fn set_tempo(&mut self, t: u16) {
        self.tempo = t.clamp(TEMPO_MIN, TEMPO_MAX);
        self.touch();
    }

    /// Swing, per cent (50–75).
    #[must_use]
    pub const fn swing(&self) -> u8 {
        self.swing
    }

    /// Set the swing (clamped to 50–75 %).
    pub fn set_swing(&mut self, s: u8) {
        self.swing = s.clamp(SWING_MIN, SWING_MAX);
        self.touch();
    }

    /// The pattern being edited.
    #[must_use]
    pub const fn current(&self) -> usize {
        self.current as usize
    }

    /// Choose the pattern to edit (clamped to 0–15).
    pub fn set_current(&mut self, p: usize) {
        self.current = p.min(PATTERNS - 1) as u8;
        self.touch();
    }

    /// Pattern `i` (the last for an index past the end).
    #[must_use]
    pub fn pattern(&self, i: usize) -> &Pattern {
        &self.patterns[i.min(PATTERNS - 1)]
    }

    /// Track `t`.
    #[must_use]
    pub fn track(&self, t: usize) -> &Track {
        &self.tracks[t.min(TRACKS - 1)]
    }

    /// The tracks.
    #[must_use]
    pub fn tracks(&self) -> &[Track; TRACKS] {
        &self.tracks
    }

    /// Whether track `t` sounds: not muted, and soloed if any track is.
    #[must_use]
    pub fn audible(&self, t: usize) -> bool {
        let tr = self.track(t);
        !tr.mute && (tr.solo || !self.tracks.iter().any(|x| x.solo))
    }

    /// Set a step.
    pub fn set_step(&mut self, pattern: usize, track: usize, step: usize, s: Step) {
        if pattern < PATTERNS {
            self.patterns[pattern].set_raw(track, step, s);
            self.touch();
        }
    }

    /// Turn a step on or off (its parameters kept); returns the new state.
    pub fn toggle(&mut self, pattern: usize, track: usize, step: usize) -> bool {
        let s = self.pattern(pattern).step(track, step);
        let on = !s.on();
        self.set_step(pattern, track, step, s.with_on(on));
        on
    }

    /// Set a pattern's length (clamped to 1–64). Steps past it are kept
    /// (a longer length brings them back).
    pub fn set_len(&mut self, pattern: usize, n: usize) {
        if pattern < PATTERNS {
            self.patterns[pattern].set_len_raw(n);
            self.touch();
        }
    }

    /// Empty a pattern (its length kept).
    pub fn clear_pattern(&mut self, pattern: usize) {
        if pattern < PATTERNS {
            let len = self.patterns[pattern].len();
            self.patterns[pattern].clear();
            self.patterns[pattern].set_len_raw(len);
            self.touch();
        }
    }

    /// Copy pattern `from` over pattern `to`.
    pub fn copy_pattern(&mut self, from: usize, to: usize) {
        if from < PATTERNS && to < PATTERNS {
            self.patterns[to] = self.patterns[from];
            self.touch();
        }
    }

    /// Mute or unmute track `t` (at the next step).
    pub fn set_mute(&mut self, t: usize, on: bool) {
        if let Some(x) = self.tracks.get_mut(t) {
            x.mute = on;
            self.touch();
        }
    }

    /// Solo track `t` or take its solo away (at the next step).
    pub fn set_solo(&mut self, t: usize, on: bool) {
        if let Some(x) = self.tracks.get_mut(t) {
            x.solo = on;
            self.touch();
        }
    }

    /// Set track `t`'s level (clamped to 0–100 %).
    pub fn set_level(&mut self, t: usize, level: u8) {
        if let Some(x) = self.tracks.get_mut(t) {
            x.level = level.min(100);
            self.touch();
        }
    }

    /// Set the kit note track `t` plays (clamped to 0–127).
    pub fn set_note(&mut self, t: usize, note: u8) {
        if let Some(x) = self.tracks.get_mut(t) {
            x.note = note.min(127);
            self.touch();
        }
    }

    /// The kit.
    #[must_use]
    pub fn kit(&self) -> &Kit {
        &self.kit
    }

    /// The kit's name.
    #[must_use]
    pub fn kit_name(&self) -> &str {
        self.kit_name.as_str()
    }

    /// Use `kit` named `name`; the player installs it at its next block.
    pub fn set_kit(&mut self, name: &str, kit: &Kit) {
        self.kit = *kit;
        self.kit_name = Name::new(name);
        self.kit_rev = self.kit_rev.wrapping_add(1);
        self.touch();
    }

    /// Use factory kit `i` ([`orpheus_synth::kit::FACTORY`], wrapping).
    pub fn set_factory_kit(&mut self, i: usize) {
        let (name, entries) = kit::FACTORY[i % kit::FACTORY.len()];
        self.set_kit(name, &Kit::new(entries));
    }

    /// The factory kit this project's kit is, by name.
    #[must_use]
    pub fn factory_kit(&self) -> Option<usize> {
        kit::FACTORY.iter().position(|(n, _)| *n == self.kit_name())
    }

    /// The pattern each step of the song plays, in order, as the player
    /// walks it (`n` patterns from the start); for tests and the screen.
    pub fn song_walk(&self, n: usize, mut out: impl FnMut(usize)) {
        let mut done = 0;
        if self.song.is_empty() {
            return;
        }
        'outer: loop {
            for e in self.song.entries() {
                for _ in 0..e.repeats {
                    if done == n {
                        break 'outer;
                    }
                    out(usize::from(e.pattern));
                    done += 1;
                }
            }
        }
    }

    // ------------------------------------------------- file decoding hooks

    pub(crate) fn set_kit_raw(&mut self, name: Name, kit: Kit) {
        self.kit = kit;
        self.kit_name = name;
        self.kit_rev = self.kit_rev.wrapping_add(1);
    }

    pub(crate) fn pattern_mut(&mut self, i: usize) -> &mut Pattern {
        &mut self.patterns[i.min(PATTERNS - 1)]
    }

    pub(crate) fn track_mut(&mut self, t: usize) -> &mut Track {
        &mut self.tracks[t.min(TRACKS - 1)]
    }

    pub(crate) fn set_raw(&mut self, tempo: u16, swing: u8, current: u8) {
        self.tempo = tempo.clamp(TEMPO_MIN, TEMPO_MAX);
        self.swing = swing.clamp(SWING_MIN, SWING_MAX);
        self.current = current.min(PATTERNS as u8 - 1);
    }
}

/// A short label for a kit note (two letters, as drum machines print
/// them) and a longer one for the header.
#[must_use]
pub fn note_label(note: u8) -> (&'static str, &'static str) {
    match note {
        35 | 36 => ("BD", "Kick"),
        37 => ("RS", "Rim"),
        38 | 40 => ("SD", "Snare"),
        39 => ("CP", "Clap"),
        41 | 43 => ("LT", "Low tom"),
        45 | 47 => ("MT", "Mid tom"),
        48 | 50 => ("HT", "High tom"),
        42 => ("CH", "Closed hat"),
        44 => ("PH", "Pedal hat"),
        46 => ("OH", "Open hat"),
        49 | 52 | 55 | 57 => ("CY", "Crash"),
        51 | 53 | 59 => ("RD", "Ride"),
        54 => ("TB", "Tambourine"),
        56 | 67 | 68 => ("CB", "Cowbell"),
        60 | 61 => ("BG", "Bongo"),
        62..=64 => ("CG", "Conga"),
        69 | 70 => ("SH", "Shaker"),
        75..=77 => ("CL", "Clave"),
        _ => ("--", "Sound"),
    }
}
