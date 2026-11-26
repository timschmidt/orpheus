//! Drum kits as data: a note → sound table with choke groups.
//!
//! A [`Kit`] maps MIDI notes to sounds: a synthesised drum
//! ([`crate::drum`]) or a sample hit ([`crate::sample`]), each with its own
//! tune, decay, tone, snap, level and pan. A [`KitEntry`] is 9 bytes of
//! plain integers (percent and quarter-semitone units), which is also its
//! binary form in a sound bank ([`crate::bank`]); the engine keeps
//! [`KIT_SLOTS`] kits and a kit patch (`Kind::Kit(slot)`) plays the one in
//! its slot.
//!
//! **Choke groups.** An entry with `choke` = g ≠ 0 silences (fades within
//! about 10 ms) every sounding voice of the same part whose entry is in
//! group g when it starts: closed, pedal and open hats share a group, so a
//! closed hat cuts the ringing open hat; a muted conga can cut the open one.
//! A note retriggered on itself fades its previous hit (256 samples) as
//! every voice does.
//!
//! The factory kits ([`FACTORY`]) are our own settings of the drum models.
//! [`STANDARD`] follows the General MIDI percussion map (channel 10) and is
//! the engine's default kit in every slot.

use crate::drum::{DrumKind, DrumParams};
use crate::math::clampf;

/// Entries a kit holds.
pub const KIT_ENTRIES: usize = 32;
/// Kits the engine holds.
pub const KIT_SLOTS: usize = 2;
/// `KitEntry::sound` codes from this value up are sample slots
/// (`SAMPLE + slot`).
pub const SAMPLE: u8 = 0x80;

/// One note of a kit. Units: `tune` in quarter semitones, `decay` in 1/50
/// (50 = 1.0, the model's own time; 5–200), `tone`, `snap`, `level` in
/// percent, `pan` in percent left (−) or right (+).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct KitEntry {
    /// MIDI note.
    pub note: u8,
    /// A [`DrumKind`] code (`DrumKind::code`), or [`SAMPLE`] + a sample
    /// slot.
    pub sound: u8,
    /// Choke group, 0 = none.
    pub choke: u8,
    /// Pan, −100 (left) to 100 (right).
    pub pan: i8,
    /// Pitch, quarter semitones (−96 to 96).
    pub tune: i8,
    /// Decay scale ×50.
    pub decay: u8,
    /// Tone, percent.
    pub tone: u8,
    /// Snap, percent.
    pub snap: u8,
    /// Level, percent.
    pub level: u8,
}

/// What a kit entry plays, decoded.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum KitSound {
    /// A drum model.
    Drum(DrumParams),
    /// A sample hit ([`crate::sample::HitParams`]).
    Sample(crate::sample::HitParams),
}

impl KitEntry {
    /// An entry playing the drum model `kind` with its defaults.
    #[must_use]
    pub const fn drum(note: u8, kind: DrumKind) -> Self {
        Self { note, sound: kind.code(), choke: 0, pan: 0, tune: 0, decay: 50, tone: 50, snap: 50, level: 80 }
    }

    /// An entry playing sample slot `slot` as a one-shot.
    #[must_use]
    pub const fn sample(note: u8, slot: u8) -> Self {
        Self { sound: SAMPLE | (slot & 0x7F), tone: 100, decay: 200, ..Self::drum(note, DrumKind::Kick) }
    }

    /// With pitch `quarter` quarter semitones.
    #[must_use]
    pub const fn tune(self, quarter: i8) -> Self {
        Self { tune: quarter, ..self }
    }

    /// With decay ×50.
    #[must_use]
    pub const fn decay(self, d: u8) -> Self {
        Self { decay: d, ..self }
    }

    /// With tone and snap, percent.
    #[must_use]
    pub const fn colour(self, tone: u8, snap: u8) -> Self {
        Self { tone, snap, ..self }
    }

    /// With level, percent.
    #[must_use]
    pub const fn level(self, l: u8) -> Self {
        Self { level: l, ..self }
    }

    /// With pan, percent.
    #[must_use]
    pub const fn pan(self, p: i8) -> Self {
        Self { pan: p, ..self }
    }

    /// In choke group `g`.
    #[must_use]
    pub const fn choke(self, g: u8) -> Self {
        Self { choke: g, ..self }
    }

    /// Pan as −1…1.
    #[must_use]
    pub fn pan_f(&self) -> f32 {
        clampf(f32::from(self.pan) / 100.0, -1.0, 1.0)
    }

    /// The sound with its settings in engine units, or `None` for an
    /// unknown sound code.
    #[must_use]
    pub fn decode(&self) -> Option<KitSound> {
        let tune = f32::from(self.tune) / 4.0;
        let decay = f32::from(self.decay) / 50.0;
        let tone = f32::from(self.tone) / 100.0;
        let snap = f32::from(self.snap) / 100.0;
        let level = f32::from(self.level) / 100.0;
        if self.sound >= SAMPLE {
            return Some(KitSound::Sample(
                crate::sample::HitParams { slot: self.sound - SAMPLE, tune, decay, tone, level }.sanitized(),
            ));
        }
        let kind = DrumKind::from_code(self.sound)?;
        Some(KitSound::Drum(DrumParams { kind, tune, decay, tone, snap, level }.sanitized()))
    }
}

/// A kit: up to [`KIT_ENTRIES`] entries; the first entry for a note wins.
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Kit {
    entries: [KitEntry; KIT_ENTRIES],
    len: u8,
}

impl Kit {
    /// No entries.
    pub const EMPTY: Self = Self { entries: [KitEntry::drum(0, DrumKind::Kick); KIT_ENTRIES], len: 0 };

    /// A kit of `entries` (those past [`KIT_ENTRIES`] are dropped).
    #[must_use]
    pub const fn new(entries: &[KitEntry]) -> Self {
        let mut k = Self::EMPTY;
        let mut i = 0;
        while i < entries.len() && i < KIT_ENTRIES {
            k.entries[i] = entries[i];
            i += 1;
        }
        k.len = i as u8;
        k
    }

    /// The entries.
    #[must_use]
    pub fn entries(&self) -> &[KitEntry] {
        self.entries.get(..usize::from(self.len)).unwrap_or_default()
    }

    /// The entry for `note`.
    #[must_use]
    pub fn find(&self, note: u8) -> Option<&KitEntry> {
        self.entries().iter().find(|e| e.note == note)
    }
}

impl Default for Kit {
    fn default() -> Self {
        Self::new(STANDARD)
    }
}

use DrumKind::{Clap, Clave, ClosedHat, Cowbell, Cymbal, Kick, OpenHat, Rim, Shaker, Snare, Tom};

const fn e(note: u8, kind: DrumKind) -> KitEntry {
    KitEntry::drum(note, kind)
}

/// Hats' choke group in the factory kits.
const HATS: u8 = 1;
/// Congas' choke group (muted cuts open).
const CONGAS: u8 = 2;

/// The General MIDI style kit: the defaults of each model on the GM
/// percussion notes (the SYN-1 kit, completed).
pub const STANDARD: &[KitEntry] = &[
    e(35, Kick).tune(-8),
    e(36, Kick),
    e(37, Rim),
    e(38, Snare),
    e(39, Clap),
    e(40, Snare).colour(80, 70),
    e(41, Tom).tune(-28),
    e(42, ClosedHat).choke(HATS),
    e(43, Tom).tune(-16),
    e(44, ClosedHat).choke(HATS).decay(35).level(60),
    e(45, Tom).tune(-8),
    e(46, OpenHat).choke(HATS),
    e(47, Tom),
    e(48, Tom).tune(12),
    e(49, Cymbal).colour(85, 60).pan(-30),
    e(50, Tom).tune(20),
    e(51, Cymbal).colour(15, 20).decay(80).level(55).pan(30),
    e(52, Cymbal).tune(-12).colour(100, 80).level(60),
    e(53, Cymbal).tune(16).colour(0, 0).decay(40).level(55).pan(30),
    e(54, Shaker).colour(100, 100).decay(80).level(60),
    e(55, Cymbal).tune(16).colour(90, 50).decay(20).pan(-40),
    e(56, Cowbell).level(60),
    e(57, Cymbal).tune(4).colour(95, 60).pan(40),
    e(59, Cymbal).tune(-4).colour(20, 10).decay(90).level(50).pan(30),
    e(60, Tom).tune(64).decay(15).pan(20),
    e(61, Tom).tune(52).decay(18).pan(20),
    e(62, Tom).tune(44).decay(8).choke(CONGAS).pan(-20),
    e(63, Tom).tune(44).decay(22).choke(CONGAS).pan(-20),
    e(64, Tom).tune(32).decay(25).pan(-25),
    e(70, Shaker).colour(70, 60),
    e(75, Clave).level(60),
    e(76, Clave).tune(-20).colour(100, 50).level(60).pan(25),
];

/// Deep, long analogue kit: boomy kick, soft snare, long toms, dark hats.
pub const ANALOG: &[KitEntry] = &[
    e(36, Kick).tune(-6).decay(120).colour(20, 25),
    e(37, Rim).level(60),
    e(38, Snare).tune(-4).colour(35, 60).decay(60),
    e(39, Clap).colour(30, 50).decay(70),
    e(42, ClosedHat).colour(20, 35).choke(HATS).level(55),
    e(46, OpenHat).colour(20, 35).decay(70).choke(HATS).level(55),
    e(45, Tom).tune(-12).decay(90),
    e(47, Tom).tune(0).decay(90),
    e(50, Tom).tune(14).decay(90),
    e(49, Cymbal).colour(60, 40).decay(70).level(55),
    e(56, Cowbell).level(55).pan(20),
    e(75, Clave).level(55).pan(-20),
    e(70, Shaker).colour(50, 40).level(55),
];

/// Punchy kit: short driven kick with a click, bright snare, crisp hats.
pub const PUNCH: &[KitEntry] = &[
    e(36, Kick).tune(4).decay(30).colour(80, 90).level(85),
    e(37, Rim).colour(70, 80),
    e(38, Snare).tune(6).colour(85, 75).decay(45).level(85),
    e(39, Clap).colour(70, 50).decay(40),
    e(42, ClosedHat).colour(90, 60).decay(35).choke(HATS).level(60),
    e(44, ClosedHat).colour(80, 60).decay(25).choke(HATS).level(45),
    e(46, OpenHat).colour(90, 60).decay(45).choke(HATS).level(60),
    e(45, Tom).tune(-4).decay(45).colour(50, 70),
    e(47, Tom).tune(8).decay(45).colour(50, 70),
    e(50, Tom).tune(20).decay(45).colour(50, 70),
    e(49, Cymbal).colour(100, 70).decay(45).level(60),
    e(51, Cymbal).colour(30, 30).decay(60).level(50),
];

/// Short, tight electro kit: high tunings and fast decays.
pub const TIGHT: &[KitEntry] = &[
    e(36, Kick).tune(12).decay(20).colour(60, 100),
    e(37, Rim).tune(8).decay(30),
    e(38, Snare).tune(16).colour(100, 90).decay(25),
    e(39, Clap).colour(100, 50).decay(25),
    e(42, ClosedHat).colour(100, 90).decay(15).choke(HATS).level(55),
    e(46, OpenHat).colour(100, 90).decay(25).choke(HATS).level(55),
    e(56, Cowbell).tune(12).decay(25).level(50),
    e(75, Clave).tune(-12).decay(40).level(55),
    e(70, Shaker).colour(100, 100).decay(30).level(55),
    e(45, Tom).tune(24).decay(25),
    e(47, Tom).tune(36).decay(25),
];

/// Hand percussion: congas, bongos, cowbells, claves, wood blocks,
/// shakers and a ride.
pub const PERCUSSION: &[KitEntry] = &[
    e(60, Tom).tune(64).decay(15).colour(50, 40).pan(25),
    e(61, Tom).tune(52).decay(18).colour(50, 40).pan(25),
    e(62, Tom).tune(44).decay(8).choke(CONGAS).pan(-25),
    e(63, Tom).tune(44).decay(22).choke(CONGAS).pan(-25),
    e(64, Tom).tune(32).decay(25).pan(-30),
    e(56, Cowbell).level(55).pan(10),
    e(67, Cowbell).tune(10).colour(70, 60).decay(30).level(50).pan(-35),
    e(68, Cowbell).tune(-2).colour(40, 60).decay(30).level(50).pan(-35),
    e(75, Clave).level(55).pan(35),
    e(76, Clave).tune(-20).colour(100, 50).level(55).pan(-15),
    e(77, Clave).tune(-32).colour(100, 50).level(55).pan(-15),
    e(70, Shaker).colour(70, 60).level(65).pan(15),
    e(69, Shaker).colour(40, 30).decay(70).level(60).pan(-10),
    e(54, Shaker).colour(100, 100).decay(80).level(55),
    e(51, Cymbal).colour(15, 20).decay(80).level(45),
    e(36, Kick).tune(-4).decay(70).colour(10, 10).level(70),
];

/// The factory kits by name.
pub const FACTORY: [(&str, &[KitEntry]); 5] = [
    ("standard", STANDARD),
    ("analog", ANALOG),
    ("punch", PUNCH),
    ("tight", TIGHT),
    ("percussion", PERCUSSION),
];

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn factory_kits_fit_and_decode() {
        for (name, k) in FACTORY {
            assert!(k.len() <= KIT_ENTRIES, "{name}: {} entries", k.len());
            let kit = Kit::new(k);
            for (i, e) in kit.entries().iter().enumerate() {
                assert!(e.decode().is_some(), "{name}: {e:?}");
                assert!(kit.entries()[..i].iter().all(|x| x.note != e.note), "{name}: note {} twice", e.note);
            }
        }
    }

    #[test]
    fn standard_matches_the_syn1_kit() {
        // SYN-1's GM mapping, which the golden renders of the kit keep.
        let kit = Kit::new(STANDARD);
        let d = |n| match kit.find(n).and_then(KitEntry::decode) {
            Some(KitSound::Drum(d)) => d,
            other => panic!("{n}: {other:?}"),
        };
        assert_eq!(d(36), DrumParams::new(Kick));
        assert_eq!(d(35), DrumParams { tune: -2.0, ..DrumParams::new(Kick) });
        assert_eq!(d(40), DrumParams { tone: 0.8, snap: 0.7, ..DrumParams::new(Snare) });
        assert_eq!(d(41), DrumParams { tune: -7.0, ..DrumParams::new(Tom) });
        assert_eq!(d(50), DrumParams { tune: 5.0, ..DrumParams::new(Tom) });
        assert_eq!(d(46), DrumParams::new(OpenHat));
        assert_eq!(kit.find(58), None);
    }
}
