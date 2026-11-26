//! Scales for the synth tracks' note entry (scale lock) and their chords.
//!
//! A [`Scale`] is a set of pitch classes over a root (0 = C … 11 = B).
//! Note entry snaps to it ([`Scale::snap`]) and moves through it
//! ([`Scale::step`]); a chord step stacks the scale's degrees in thirds
//! from its note ([`Scale::chord`]), so a triad or a seventh is always
//! in the key. Chromatic has no degrees to stack: its triad is major and
//! its seventh dominant.

/// A scale (the code is its index: the file stores it).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Scale {
    /// Every semitone (no lock).
    #[default]
    Chromatic,
    /// Major (Ionian).
    Major,
    /// Natural minor (Aeolian).
    Minor,
    /// Dorian.
    Dorian,
    /// Phrygian.
    Phrygian,
    /// Lydian.
    Lydian,
    /// Mixolydian.
    Mixolydian,
    /// Locrian.
    Locrian,
    /// Major pentatonic.
    MajorPentatonic,
    /// Minor pentatonic.
    MinorPentatonic,
}

/// The chord a synth step plays on its note.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Chord {
    /// The note alone.
    #[default]
    Single,
    /// Three notes in thirds of the scale.
    Triad,
    /// Four notes in thirds of the scale.
    Seventh,
    /// Root, fifth and octave (any scale).
    Power,
}

impl Chord {
    /// Every chord in code order.
    pub const ALL: [Chord; 4] = [Chord::Single, Chord::Triad, Chord::Seventh, Chord::Power];

    /// The chord of a two-bit code.
    #[must_use]
    pub const fn from_code(c: u8) -> Chord {
        match c & 3 {
            0 => Chord::Single,
            1 => Chord::Triad,
            2 => Chord::Seventh,
            _ => Chord::Power,
        }
    }

    /// Its code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// A short label for the screens.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Chord::Single => "Single",
            Chord::Triad => "Triad",
            Chord::Seventh => "Seventh",
            Chord::Power => "Power",
        }
    }
}

/// Note names of the pitch classes.
pub const NOTE_NAMES: [&str; 12] = ["C", "C♯", "D", "E♭", "E", "F", "F♯", "G", "A♭", "A", "B♭", "B"];

/// `note` as "C3", "F♯4" (C4 = 60).
pub fn note_name(note: u8) -> (&'static str, i8) {
    (NOTE_NAMES[usize::from(note % 12)], (note / 12) as i8 - 1)
}

impl Scale {
    /// Every scale in code order.
    pub const ALL: [Scale; 10] = [
        Scale::Chromatic,
        Scale::Major,
        Scale::Minor,
        Scale::Dorian,
        Scale::Phrygian,
        Scale::Lydian,
        Scale::Mixolydian,
        Scale::Locrian,
        Scale::MajorPentatonic,
        Scale::MinorPentatonic,
    ];

    /// The scale of a code (chromatic for an unknown one).
    #[must_use]
    pub fn from_code(c: u8) -> Scale {
        Scale::ALL.get(usize::from(c)).copied().unwrap_or(Scale::Chromatic)
    }

    /// Its code.
    #[must_use]
    pub const fn code(self) -> u8 {
        self as u8
    }

    /// Name for the screens.
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Scale::Chromatic => "Chromatic",
            Scale::Major => "Major",
            Scale::Minor => "Minor",
            Scale::Dorian => "Dorian",
            Scale::Phrygian => "Phrygian",
            Scale::Lydian => "Lydian",
            Scale::Mixolydian => "Mixolydian",
            Scale::Locrian => "Locrian",
            Scale::MajorPentatonic => "Major pent.",
            Scale::MinorPentatonic => "Minor pent.",
        }
    }

    /// The pitch classes above the root, as a 12-bit mask (bit 0 = root).
    #[must_use]
    pub const fn mask(self) -> u16 {
        // Bits of the semitones 0–11.
        const fn m(s: &[u8]) -> u16 {
            let mut v = 0u16;
            let mut i = 0;
            while i < s.len() {
                v |= 1 << s[i];
                i += 1;
            }
            v
        }
        match self {
            Scale::Chromatic => 0xFFF,
            Scale::Major => m(&[0, 2, 4, 5, 7, 9, 11]),
            Scale::Minor => m(&[0, 2, 3, 5, 7, 8, 10]),
            Scale::Dorian => m(&[0, 2, 3, 5, 7, 9, 10]),
            Scale::Phrygian => m(&[0, 1, 3, 5, 7, 8, 10]),
            Scale::Lydian => m(&[0, 2, 4, 6, 7, 9, 11]),
            Scale::Mixolydian => m(&[0, 2, 4, 5, 7, 9, 10]),
            Scale::Locrian => m(&[0, 1, 3, 5, 6, 8, 10]),
            Scale::MajorPentatonic => m(&[0, 2, 4, 7, 9]),
            Scale::MinorPentatonic => m(&[0, 3, 5, 7, 10]),
        }
    }

    /// Whether `note` is in the scale on `root` (0–11).
    #[must_use]
    pub const fn contains(self, root: u8, note: u8) -> bool {
        let pc = (note % 12 + 12 - root % 12) % 12;
        self.mask() & (1 << pc) != 0
    }

    /// The scale's note nearest to `note` (the lower one on a tie), within
    /// 0–127.
    #[must_use]
    pub fn snap(self, root: u8, note: u8) -> u8 {
        let note = note.min(127);
        for d in 0..12u8 {
            if let Some(n) = note.checked_sub(d)
                && self.contains(root, n)
            {
                return n;
            }
            let n = note + d;
            if n <= 127 && self.contains(root, n) {
                return n;
            }
        }
        note
    }

    /// The next note of the scale above (`up`) or below `note`, `note`
    /// itself at the ends of 0–127.
    #[must_use]
    pub fn step(self, root: u8, note: u8, up: bool) -> u8 {
        let mut n = note.min(127);
        loop {
            n = match (up, n) {
                (true, 127) | (false, 0) => return note.min(127),
                (true, _) => n + 1,
                (false, _) => n - 1,
            };
            if self.contains(root, n) {
                return n;
            }
        }
    }

    /// The notes of `chord` on `note` (snapped first), at most four, each
    /// within 0–127; returns how many.
    #[must_use]
    pub fn chord(self, root: u8, note: u8, chord: Chord, out: &mut [u8; 4]) -> usize {
        let base = if self == Scale::Chromatic { note.min(127) } else { self.snap(root, note) };
        let stack = |thirds: usize, out: &mut [u8; 4]| {
            if self == Scale::Chromatic {
                const MAJOR7: [u8; 4] = [0, 4, 7, 10];
                for (o, d) in out.iter_mut().zip(&MAJOR7[..thirds]) {
                    *o = base.saturating_add(*d).min(127);
                }
            } else {
                let mut n = base;
                for (k, o) in out.iter_mut().take(thirds).enumerate() {
                    if k > 0 {
                        n = self.step(root, self.step(root, n, true), true);
                    }
                    *o = n;
                }
            }
            thirds
        };
        match chord {
            Chord::Single => {
                out[0] = base;
                1
            }
            Chord::Triad => stack(3, out),
            Chord::Seventh => stack(4, out),
            Chord::Power => {
                out[0] = base;
                out[1] = base.saturating_add(7).min(127);
                out[2] = base.saturating_add(12).min(127);
                3
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn masks_and_snapping() {
        for s in Scale::ALL {
            assert_eq!(Scale::from_code(s.code()), s);
            assert!(s.contains(0, 0), "{s:?} has its root");
        }
        assert_eq!(Scale::Major.mask().count_ones(), 7);
        assert_eq!(Scale::MinorPentatonic.mask().count_ones(), 5);
        // C major: C♯ snaps down to C, A♭ down to G; D major: C to C♯? (B
        // and C♯ are both a semitone away: the lower, B).
        assert_eq!(Scale::Major.snap(0, 61), 60);
        assert_eq!(Scale::Major.snap(0, 68), 67);
        assert_eq!(Scale::Major.snap(2, 60), 59);
        assert_eq!(Scale::Chromatic.snap(5, 61), 61);
        for n in 0..=127u8 {
            for s in Scale::ALL {
                for root in 0..12 {
                    let x = s.snap(root, n);
                    assert!(s.contains(root, x));
                    assert!(x.abs_diff(n) <= 2);
                }
            }
        }
        // Stepping walks the scale and stops at the ends.
        assert_eq!(Scale::Major.step(0, 60, true), 62);
        assert_eq!(Scale::Major.step(0, 64, true), 65);
        assert_eq!(Scale::MinorPentatonic.step(9, 57, true), 60);
        assert_eq!(Scale::Chromatic.step(0, 127, true), 127);
        assert_eq!(Scale::Chromatic.step(0, 0, false), 0);
        assert_eq!(Scale::Major.step(0, 0, false), 0);
    }

    #[test]
    fn chords_stay_in_the_key() {
        let mut o = [0u8; 4];
        assert_eq!(Scale::Major.chord(0, 60, Chord::Triad, &mut o), 3);
        assert_eq!(o[..3], [60, 64, 67], "C major");
        assert_eq!(Scale::Major.chord(0, 62, Chord::Seventh, &mut o), 4);
        assert_eq!(o, [62, 65, 69, 72], "D minor 7 in C major");
        assert_eq!(Scale::Minor.chord(9, 57, Chord::Triad, &mut o), 3);
        assert_eq!(o[..3], [57, 60, 64], "A minor");
        assert_eq!(Scale::Chromatic.chord(0, 61, Chord::Seventh, &mut o), 4);
        assert_eq!(o, [61, 65, 68, 71], "chromatic: dominant seventh");
        assert_eq!(Scale::Dorian.chord(2, 50, Chord::Power, &mut o), 3);
        assert_eq!(o[..3], [50, 57, 62]);
        assert_eq!(Scale::Major.chord(0, 126, Chord::Seventh, &mut o), 4);
        assert!(o.iter().all(|n| *n <= 127));
        assert_eq!(note_name(60), ("C", 4));
        assert_eq!(note_name(37), ("C♯", 2));
    }
}
