//! The Sequencer app's service, settings and helpers (05 §7.16, SEQ-1,
//! SEQ-2; feature `seq`). The model, the file, the step player and the
//! export are `orpheus-seq`; the screens are `screens::seq`.
//!
//! The service holds an [`orpheus_seq::Studio`] (the open project, the
//! player, the saved projects) the screens read and edit directly; card
//! work goes through [`SeqService::seq_request`] and is answered by
//! changing the studio (the list, the open project) and an
//! `other::SEQ` service event. Rendering is the service's: the firmware's
//! synth task (SYN-3) or the simulator calls `Studio::render_i16` into
//! the output while `Sequencer::busy`, whatever the screen shows or
//! whether it is on.
//!
//! # Settings (05 §7.16 "Settings")
//!
//! Seven settings, packed into the word `0x1180` (Sequencer › list ›
//! Menu › Settings): the kit, pattern length and swing of new projects
//! ([`new_project`]), the metronome, the count-in and the step length
//! ([`options`], taken at Play and from the next step), and whether the
//! key click sounds while playing ([`key_click_allowed`]; off by default:
//! "key click off while playing"). MIDI clock out over USB is not here
//! (with M16's USB MIDI, optional).

pub use orpheus_seq as core;
use orpheus_seq::Studio;
use orpheus_seq::model::{MAX_STEPS, PATTERNS, TEMPO_MAX, TEMPO_MIN, note_label};
use orpheus_seq::orpheus_synth::kit::FACTORY;
use orpheus_seq::player::{Options, StepLen};
use orpheus_ui::text::StrBuf;

use crate::services::SettingsService;
use crate::settings::{Choice, Setting, SettingId, Unit, c, r, t};
use crate::strings::Str;

/// Card work the screens ask for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeqRequest {
    /// Read the project list (`Studio::list`) from `/.orpheus/seq/`.
    List,
    /// Open saved project file `no` (stopping and saving the open one).
    Open(u8),
    /// Save the open project (a new one takes the lowest free number);
    /// the list follows.
    Save,
    /// Delete saved project file `no` (closing it if open).
    Delete(u8),
    /// Export the open project's song as `SEQnn.WAV` next to it (saving
    /// it first if new), rendered offline with the step length setting.
    ExportWav,
    /// Export the song as a Standard MIDI File, `SEQnn.MID`.
    ExportMidi,
}

/// What the last request did (`SeqService::seq_result`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum SeqResult {
    /// Nothing asked yet, or still working.
    #[default]
    None,
    /// Done.
    Done,
    /// The file could not be read, written or removed.
    Failed,
}

/// The Sequencer's service (part of `Services` with the feature).
pub trait SeqService {
    fn seq(&self) -> &Studio;
    fn seq_mut(&mut self) -> &mut Studio;
    fn seq_request(&mut self, r: SeqRequest);
    /// The outcome of the last request.
    fn seq_result(&self) -> SeqResult;
}

/// The settings' ids. Stored as one word ([`id::WORD`]); each reads and
/// writes under its own key.
pub mod id {
    use crate::settings::SettingId;
    pub const NEW_KIT: SettingId = SettingId(0x1101);
    pub const NEW_LENGTH: SettingId = SettingId(0x1102);
    pub const NEW_SWING: SettingId = SettingId(0x1103);
    pub const METRONOME: SettingId = SettingId(0x1104);
    pub const COUNT_IN: SettingId = SettingId(0x1105);
    pub const STEP_LENGTH: SettingId = SettingId(0x1106);
    pub const CLICK_PLAYING: SettingId = SettingId(0x1107);
    /// The stored word.
    pub const WORD: SettingId = SettingId(0x1180);
}

const KITS: &[Choice] = &[
    Choice::Lit(FACTORY[0].0),
    Choice::Lit(FACTORY[1].0),
    Choice::Lit(FACTORY[2].0),
    Choice::Lit(FACTORY[3].0),
    Choice::Lit(FACTORY[4].0),
];
/// Steps of new patterns by the setting's value.
pub const LENGTHS: [usize; 4] = [16, 32, 48, 64];
const LENGTH_CHOICES: &[Choice] =
    &[Choice::Lit("16"), Choice::Lit("32"), Choice::Lit("48"), Choice::Lit("64")];
/// Swing of new projects by the setting's value, per cent.
pub const SWINGS: [u8; 7] = [50, 54, 58, 62, 66, 70, 75];
const SWING_CHOICES: &[Choice] = &[
    Choice::S(Str::Off),
    Choice::Lit("54 %"),
    Choice::Lit("58 %"),
    Choice::Lit("62 %"),
    Choice::Lit("66 %"),
    Choice::Lit("70 %"),
    Choice::Lit("75 %"),
];
const COUNT_INS: &[Choice] = &[Choice::S(Str::Off), Choice::Lit("1 bar"), Choice::Lit("2 bars")];
const STEP_LENGTHS: &[Choice] = &[Choice::Lit("1/16"), Choice::Lit("1/8"), Choice::Lit("1/32")];

/// The Sequencer's settings in the settings screen's order, then the
/// word.
pub const SETTINGS: [Setting; 8] = [
    c(0x1101, Str::SeqNewKit, KITS, 0).packed(0x1180, 0, 3),
    c(0x1102, Str::SeqNewLength, LENGTH_CHOICES, 0).packed(0x1180, 3, 2),
    c(0x1103, Str::SeqNewSwing, SWING_CHOICES, 0).packed(0x1180, 5, 3),
    t(0x1104, Str::SeqMetronome, 0).packed(0x1180, 8, 1),
    c(0x1105, Str::SeqCountIn, COUNT_INS, 0).packed(0x1180, 9, 2),
    c(0x1106, Str::SeqStepLength, STEP_LENGTHS, 0).packed(0x1180, 11, 2),
    t(0x1107, Str::SeqClickPlaying, 0).packed(0x1180, 13, 1),
    // The word: every default above is 0.
    r(0x1180, Str::AppSequencer, i16::MIN, i16::MAX, 1, Unit::None, 0),
];

/// Stored items of [`SETTINGS`] (the word).
pub const STORED_SETTINGS: usize = 1;

/// The settings the Sequencer's settings screen shows.
#[must_use]
pub fn shown_settings() -> &'static [Setting] {
    &SETTINGS[..7]
}

fn get<S: SettingsService + ?Sized>(s: &S, id: SettingId) -> usize {
    usize::try_from(s.setting(id)).unwrap_or(0)
}

/// The player's options from the settings (metronome, count-in, step
/// length).
pub fn options<S: SettingsService + ?Sized>(s: &S) -> Options {
    Options {
        metronome: s.setting(id::METRONOME) != 0,
        count_in: get(s, id::COUNT_IN).min(2) as u8,
        step_len: match get(s, id::STEP_LENGTH) {
            1 => StepLen::Eighth,
            2 => StepLen::ThirtySecond,
            _ => StepLen::Sixteenth,
        },
    }
}

/// Start playing with the options the settings give now.
pub fn play<S: SettingsService + SeqService + ?Sized>(s: &mut S) {
    let o = options(s);
    let st = s.seq_mut();
    st.seq.set_options(o);
    st.seq.play();
}

/// Play or stop (see [`play`]).
pub fn toggle_play<S: SettingsService + SeqService + ?Sized>(s: &mut S) {
    if s.seq().seq.playing() {
        s.seq_mut().seq.stop();
    } else {
        play(s);
    }
}

/// Start a new project named `name` with the settings' kit, pattern
/// length and swing.
pub fn new_project<S: SettingsService + SeqService + ?Sized>(s: &mut S, name: &str) {
    let kit = get(s, id::NEW_KIT) % FACTORY.len();
    let len = LENGTHS[get(s, id::NEW_LENGTH).min(LENGTHS.len() - 1)].min(MAX_STEPS);
    let swing = SWINGS[get(s, id::NEW_SWING).min(SWINGS.len() - 1)];
    let st = s.seq_mut();
    st.new_project(name);
    let p = &mut st.project;
    if kit != 0 {
        p.set_factory_kit(kit);
    }
    if len != 16 {
        for i in 0..PATTERNS {
            p.set_len(i, len);
        }
    }
    if swing != 50 {
        p.set_swing(swing);
    }
    st.saved_rev = st.project.rev().wrapping_sub(1);
}

/// Whether a key press may click now: not while the Sequencer plays,
/// unless "Key click while playing" is on.
pub fn key_click_allowed<S: SettingsService + SeqService + ?Sized>(s: &S) -> bool {
    !s.seq().seq.playing() || s.setting(id::CLICK_PLAYING) != 0
}

/// `tempo` (1/100 BPM) as "98" or "93.5" (one decimal when not whole).
pub fn push_tempo<const N: usize>(b: &mut StrBuf<N>, tempo: u16) {
    let t10 = (u32::from(tempo) + 5) / 10;
    b.push_u32(t10 / 10);
    if t10 % 10 != 0 {
        b.push('.');
        b.push_u32(t10 % 10);
    }
}

/// Tempo one step up or down: whole BPM (a fractional tempo goes to the
/// next whole one), `big` 10 BPM.
#[must_use]
pub fn tempo_step(tempo: u16, up: bool, big: bool) -> u16 {
    let d: i32 = if big { 1000 } else { 100 };
    let t = i32::from(tempo);
    let next = if up { (t / 100) * 100 + d } else { ((t + 99) / 100) * 100 - d };
    next.clamp(i32::from(TEMPO_MIN), i32::from(TEMPO_MAX)) as u16
}

/// A track's label: "BD Kick".
pub fn push_track<const N: usize>(b: &mut StrBuf<N>, note: u8) {
    let (short, long) = note_label(note);
    b.push_str(short);
    b.push(' ');
    b.push_str(long);
}

/// A note as "C4", "F♯2" (C4 = 60).
pub fn push_note<const N: usize>(b: &mut StrBuf<N>, note: u8) {
    let (name, octave) = orpheus_seq::scale::note_name(note);
    b.push_str(name);
    if octave < 0 {
        b.push('-');
    }
    b.push_u32(u32::from(octave.unsigned_abs()));
}

/// A new project's name: "Beat 3" for file number 3.
pub fn new_name(no: u8) -> StrBuf<16> {
    let mut b = StrBuf::new();
    b.push_str("Beat ");
    b.push_u32(u32::from(no));
    b
}

#[cfg(test)]
mod tests {
    extern crate std;
    use super::*;
    use crate::settings::{SettingsCache, all, def};

    #[test]
    fn tempo_text_and_steps() {
        let s = |t| {
            let mut b = StrBuf::<8>::new();
            push_tempo(&mut b, t);
            std::string::String::from(b.as_str())
        };
        assert_eq!(s(12_000), "120");
        assert_eq!(s(9_350), "93.5");
        assert_eq!(s(9_996), "100", "rounded to tenths");
        assert_eq!(s(9_904), "99");
        assert_eq!(tempo_step(12_000, true, false), 12_100);
        assert_eq!(tempo_step(9_350, true, false), 9_400);
        assert_eq!(tempo_step(9_350, false, false), 9_300);
        assert_eq!(tempo_step(4_000, false, true), 4_000);
        assert_eq!(tempo_step(23_900, true, true), 24_000);
        assert_eq!(new_name(3).as_str(), "Beat 3");
        let n = |x| {
            let mut b = StrBuf::<8>::new();
            push_note(&mut b, x);
            std::string::String::from(b.as_str())
        };
        assert_eq!(n(60), "C4");
        assert_eq!(n(42), "F♯2");
        assert_eq!(n(3), "E♭-1");
    }

    struct S(SettingsCache);
    impl SettingsService for S {
        fn setting(&self, id: SettingId) -> i16 {
            self.0.get(id)
        }
        fn set_setting(&mut self, id: SettingId, v: i16) {
            self.0.set(id, v);
        }
        fn factory_reset(&mut self) {}
    }

    /// Every Sequencer setting is in the settings table, packed in the one
    /// word without overlaps, valid by default, and has an entry in the
    /// effects manifest (VER-13); the options follow them.
    #[test]
    fn settings_table() {
        let manifest = include_str!("../../../tests/settings/effects.toml");
        let mut used = 0u16;
        for s in &SETTINGS {
            assert!(def(s.id).is_some(), "{:#06x} in the table", s.id.0);
            assert_eq!(s.validate(s.default), s.default);
            let key = std::format!("id = \"{:#06X}\"", s.id.0).replace("0X", "0x");
            assert!(manifest.contains(&key), "{key} missing from tests/settings/effects.toml");
            if let Some((w, shift, bits)) = s.pack {
                assert_eq!(w, id::WORD);
                let mask = ((1u16 << bits) - 1) << shift;
                assert_eq!(used & mask, 0, "{:#06x} overlaps", s.id.0);
                used |= mask;
                if let crate::settings::Kind::Choice(c) = s.kind {
                    assert!(c.len() <= 1 << bits, "{:#06x}: its values fit its bits", s.id.0);
                }
            }
        }
        assert_eq!(all().filter(|s| (s.id.0 >> 8) == 0x11 && s.stored()).count(), STORED_SETTINGS);
        let mut c = S(SettingsCache::new());
        for s in &SETTINGS[..7] {
            assert_eq!(c.setting(s.id), s.default, "{:#06x} default through the word", s.id.0);
        }
        assert_eq!(options(&c), Options::default());
        c.set_setting(id::METRONOME, 1);
        c.set_setting(id::COUNT_IN, 2);
        c.set_setting(id::STEP_LENGTH, 2);
        assert_eq!(options(&c), Options { metronome: true, count_in: 2, step_len: StepLen::ThirtySecond });
        assert_eq!(c.setting(id::NEW_KIT), 0, "the others unchanged");
    }
}
