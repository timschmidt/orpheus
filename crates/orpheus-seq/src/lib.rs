//! `orpheus-seq`: the Sequencer's project model, its file, the step
//! player and the song's export (docs/plan/05-ui-apps.md §7.16; WPs
//! SEQ-1, SEQ-2).
//!
//! ```ignore
//! use orpheus_seq::{Project, Sequencer, Step};
//! let mut p = Project::NEW;
//! p.set_step(0, 0, 0, Step::hit(110));         // pattern 0, kick, step 1
//! p.set_tempo(9_600);                         // 96 BPM (hundredths)
//! p.set_swing(58);
//! let mut synth = orpheus_synth::Engine::new(48_000, 1);
//! let mut seq = Sequencer::new(48_000);
//! seq.play();
//! let mut block = [0i16; 2 * 512];
//! seq.render_i16(&p, &mut synth, &mut block);  // hits on their samples
//! ```
//!
//! * [`model`]: [`Project`] — 16 patterns × 1–64 steps × 8 drum tracks
//!   and 2 synth tracks (velocity, probability, ratchet 1–4, micro-timing
//!   delay, accent per step; note, gate and chord on the synth tracks),
//!   parameter locks per step, track note/kit/level/mute/solo, two kits,
//!   the synth tracks' patches and sends, the scale lock, the fill
//!   pattern, a song of pattern chains, tempo 40–240 BPM, swing 50–75 %.
//! * [`scale`]: scales and their chords.
//! * [`player`]: [`Sequencer`] — exact step and note timing, pattern
//!   chaining, live edits at the next step, fill, metronome and count-in,
//!   notes from the keys, rendering into `orpheus-synth` (or a dry run of
//!   the notes) without any UI: what the firmware's synth task calls with
//!   the screen off.
//! * [`file`]: the `OSQ1` project file (versioned records, CRC-32,
//!   migration of SEQ-1's version 1).
//! * [`export`]: the song as a WAV file (offline render) and a Standard
//!   MIDI File.
//! * [`Studio`]: what the app's service holds: the open project, the
//!   player, the list of saved projects.
//! * [`demo`]: songs of our own (the example WAVs, the simulator's card).
//!
//! `no_std`, no allocation, no `unsafe`, no floating point outside the
//! synthesis engine.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod demo;
pub mod export;
pub mod file;
pub mod model;
pub mod player;
pub mod scale;
pub mod tap;

pub use orpheus_synth;

pub use file::{FileError, load_into, save};
pub use model::{
    Chord, LockParam, PlayMode, Project, Scale, Song, SongEntry, Step, SynthStep, SynthTrack, Track,
};
pub use player::{Hit, Options, Position, Sequencer, StepLen};
pub use tap::TapTempo;

use orpheus_synth::bank::Name;

/// The folder of the project files on the card (CLAUDE.md rule 5: the
/// player writes only under `/.orpheus/`).
pub const DIR: &str = "/.orpheus/seq";
/// The file name of project file `no`'s WAV export (`SEQ01.WAV`).
#[must_use]
pub fn wav_name(no: u8) -> [u8; 9] {
    let mut n = file_name(no);
    n[6..].copy_from_slice(b"WAV");
    n
}

/// The file name of project file `no`'s MIDI export (`SEQ01.MID`).
#[must_use]
pub fn midi_name(no: u8) -> [u8; 9] {
    let mut n = file_name(no);
    n[6..].copy_from_slice(b"MID");
    n
}

/// Projects the list holds (file numbers 1–99; the list shows 32).
pub const MAX_PROJECTS: usize = 32;
/// Highest file number.
pub const MAX_FILE_NO: u8 = 99;

/// The file name of project file `no` (1–99): `SEQ01.OSQ` … `SEQ99.OSQ`
/// (8.3, so any card's file system takes it).
#[must_use]
pub fn file_name(no: u8) -> [u8; 9] {
    let no = no.clamp(1, MAX_FILE_NO);
    let mut n = *b"SEQ00.OSQ";
    n[3] = b'0' + no / 10;
    n[4] = b'0' + no % 10;
    n
}

/// The file number of a project file name (any case), if it is one.
#[must_use]
pub fn parse_file_name(name: &str) -> Option<u8> {
    let b = name.as_bytes();
    if b.len() != 9 || !b[..3].eq_ignore_ascii_case(b"SEQ") || !b[5..].eq_ignore_ascii_case(b".OSQ") {
        return None;
    }
    let (d0, d1) = (b[3].wrapping_sub(b'0'), b[4].wrapping_sub(b'0'));
    let no = (d0 < 10 && d1 < 10).then_some(d0 * 10 + d1)?;
    (1..=MAX_FILE_NO).contains(&no).then_some(no)
}

/// One saved project in the list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Saved {
    /// File number.
    pub no: u8,
    /// The project's name (from the file).
    pub name: Name,
}

/// The saved projects, by file number.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProjectList {
    n: u8,
    items: [Saved; MAX_PROJECTS],
}

impl Default for ProjectList {
    fn default() -> Self {
        Self::EMPTY
    }
}

impl ProjectList {
    /// No projects.
    pub const EMPTY: ProjectList =
        ProjectList { n: 0, items: [Saved { no: 0, name: Name([0; 16]) }; MAX_PROJECTS] };

    /// The projects.
    #[must_use]
    pub fn items(&self) -> &[Saved] {
        &self.items[..usize::from(self.n)]
    }

    /// Forget them all.
    pub fn clear(&mut self) {
        self.n = 0;
    }

    /// Add or rename file `no`, keeping the order; false when full.
    pub fn put(&mut self, no: u8, name: Name) -> bool {
        let n = usize::from(self.n);
        if let Some(i) = self.items[..n].iter().position(|s| s.no == no) {
            self.items[i].name = name;
            return true;
        }
        if n == MAX_PROJECTS {
            return false;
        }
        let at = self.items[..n].iter().position(|s| s.no > no).unwrap_or(n);
        self.items.copy_within(at..n, at + 1);
        self.items[at] = Saved { no, name };
        self.n += 1;
        true
    }

    /// Remove file `no`.
    pub fn remove(&mut self, no: u8) {
        let n = usize::from(self.n);
        if let Some(i) = self.items[..n].iter().position(|s| s.no == no) {
            self.items.copy_within(i + 1..n, i);
            self.n -= 1;
        }
    }

    /// The lowest unused file number.
    #[must_use]
    pub fn free_no(&self) -> Option<u8> {
        (1..=MAX_FILE_NO).find(|no| !self.items().iter().any(|s| s.no == *no))
    }
}

/// What the Sequencer's service holds (everything but the 62 KB synth
/// engine, which lives where the decoder arena is on the player): the open
/// project, the player and the project list. The screens read and edit
/// it; the synth task (or the simulator) calls [`Studio::render_i16`]
/// while [`Sequencer::busy`].
#[derive(Clone, Debug)]
pub struct Studio {
    /// The open project.
    pub project: Project,
    /// The player.
    pub seq: Sequencer,
    /// The saved projects (filled by the service from the card).
    pub list: ProjectList,
    /// The open project's file number (`None`: not saved yet).
    pub open: Option<u8>,
    /// [`Project::rev`] when it was last saved or loaded.
    pub saved_rev: u32,
}

impl Studio {
    /// Nothing open, stopped, at `rate` Hz. (A const value of 27 KB: on
    /// the player, place the studio in its memory and call
    /// [`Studio::init`] instead of moving this into it.)
    #[must_use]
    pub const fn new(rate: u32) -> Self {
        Studio {
            project: Project::NEW,
            seq: Sequencer::new(rate),
            list: ProjectList::EMPTY,
            open: None,
            saved_rev: 0,
        }
    }

    /// Make `self` a new studio at `rate` Hz in place (no 27 KB copy).
    pub fn init(&mut self, rate: u32) {
        self.project.reset();
        self.seq = Sequencer::new(rate);
        self.list.clear();
        self.open = None;
        self.saved_rev = self.project.rev();
    }

    /// Edited since saved.
    #[must_use]
    pub fn dirty(&self) -> bool {
        self.project.rev() != self.saved_rev
    }

    /// Start a new project named `name`, unsaved.
    pub fn new_project(&mut self, name: &str) {
        self.seq.stop();
        self.project.reset();
        self.project.set_name(name);
        self.open = None;
        self.saved_rev = self.project.rev().wrapping_sub(1);
    }

    /// Render the open project (no UI involved: the screen-off path).
    pub fn render_i16(&mut self, eng: &mut orpheus_synth::Engine, out: &mut [i16]) {
        self.seq.render_i16(&self.project, eng, out);
    }

    /// Start or stop.
    pub fn toggle_play(&mut self) {
        if self.seq.playing() {
            self.seq.stop();
        } else {
            self.seq.play();
        }
    }

    /// Track `t`'s note `note` played from the keys now: heard (a synth
    /// note for a step), and while playing with `record` written into the
    /// step nearest to `frame` (the press, on the player's clock: the
    /// DAC's frame) — quantised. Returns the (pattern, step) written.
    pub fn key_note(
        &mut self,
        t: usize,
        note: u8,
        velocity: u8,
        record: bool,
        frame: u64,
    ) -> Option<(u8, u8)> {
        if t >= model::ALL_TRACKS {
            return None;
        }
        self.seq.live(t as u8, note, velocity);
        if !record {
            return None;
        }
        let (pat, step) = self.seq.quantize(frame)?;
        let (pat_i, step_i) = (usize::from(pat), usize::from(step));
        if t < model::TRACKS {
            let s = self.project.pattern(pat_i).step(t, step_i).with_on(true).with_velocity(velocity);
            self.project.set_step(pat_i, t, step_i, s);
        } else {
            let st = t - model::TRACKS;
            let s = self.project.pattern(pat_i).synth_step(st, step_i);
            self.project.set_synth_step(
                pat_i,
                st,
                step_i,
                s.with_note(note).with_on(true).with_velocity(velocity),
            );
        }
        Some((pat, step))
    }
}

/// RAM of the parts, bytes (`size_of`, this build).
pub mod ram {
    use core::mem::size_of;

    /// One step.
    #[must_use]
    pub fn step() -> usize {
        size_of::<crate::Step>()
    }

    /// One pattern.
    #[must_use]
    pub fn pattern() -> usize {
        size_of::<crate::model::Pattern>()
    }

    /// The project.
    #[must_use]
    pub fn project() -> usize {
        size_of::<crate::Project>()
    }

    /// The player.
    #[must_use]
    pub fn sequencer() -> usize {
        size_of::<crate::Sequencer>()
    }

    /// The studio (project, player, list).
    #[must_use]
    pub fn studio() -> usize {
        size_of::<crate::Studio>()
    }

    /// One synth step.
    #[must_use]
    pub fn synth_step() -> usize {
        size_of::<crate::SynthStep>()
    }

    /// A pattern's lock store.
    #[must_use]
    pub fn locks() -> usize {
        size_of::<crate::model::LockStore>()
    }

    /// One synth track (its patch, level, sends).
    #[must_use]
    pub fn synth_track() -> usize {
        size_of::<crate::SynthTrack>()
    }

    /// One kit (each project holds two).
    #[must_use]
    pub fn kit() -> usize {
        size_of::<orpheus_synth::Kit>()
    }

    /// A WAV export's own state.
    #[must_use]
    pub fn wav_export() -> usize {
        size_of::<crate::export::WavExport>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn file_names() {
        assert_eq!(&wav_name(7), b"SEQ07.WAV");
        assert_eq!(&midi_name(12), b"SEQ12.MID");
        assert_eq!(&file_name(7), b"SEQ07.OSQ");
        assert_eq!(&file_name(99), b"SEQ99.OSQ");
        assert_eq!(parse_file_name("seq07.osq"), Some(7));
        assert_eq!(parse_file_name("SEQ00.OSQ"), None);
        assert_eq!(parse_file_name("SEQ7.OSQ"), None);
        assert_eq!(parse_file_name("SEQ0A.OSQ"), None);
        for no in 1..=MAX_FILE_NO {
            let n = file_name(no);
            assert_eq!(parse_file_name(core::str::from_utf8(&n).unwrap()), Some(no));
        }
    }

    #[test]
    fn list_keeps_order_and_bounds() {
        let mut l = ProjectList::EMPTY;
        assert_eq!(l.free_no(), Some(1));
        assert!(l.put(3, Name::new("c")));
        assert!(l.put(1, Name::new("a")));
        assert!(l.put(2, Name::new("b")));
        assert!(l.put(2, Name::new("b2")));
        assert_eq!(l.items().iter().map(|s| s.no).collect::<std::vec::Vec<_>>(), [1, 2, 3]);
        assert_eq!(l.items()[1].name.as_str(), "b2");
        assert_eq!(l.free_no(), Some(4));
        l.remove(2);
        assert_eq!(l.free_no(), Some(2));
        for no in 10..10 + MAX_PROJECTS as u8 {
            l.put(no, Name::new("x"));
        }
        assert_eq!(l.items().len(), MAX_PROJECTS);
        assert!(!l.put(90, Name::new("y")), "full");
    }
}
