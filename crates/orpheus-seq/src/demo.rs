//! Songs of our own for the example WAVs, the tests and the simulator's
//! card, written in step notations: [`beat`] (SEQ-1's drum beat, kept as
//! it was) and [`song`] (SEQ-2: drums from two kits, bass and lead with
//! chords, locks, a fill pattern).
//!
//! Notation of [`row`]: one character per step — `.` off, `x` a hit at
//! the row's velocity, `X` accented, `g` a ghost note (velocity 45), `2`
//! `3` `4` ratchets, `?` half probability, `>` delayed a quarter step
//! (6/24), `-` a hit at velocity 70.
//!
//! Notation of [`line`]: one token per step, separated by spaces — `.`
//! off, `-` the previous note held one step longer, else a note name and
//! octave (`a2`, `c#3`, `eb4`; C4 = 60) followed by any of `:t` (triad),
//! `:s` (seventh), `:p` (power chord), `!` (accent), `'` (short: half a
//! step), `>` (delayed a quarter step).

use crate::model::{Chord, GATES, LockParam, PlayMode, Project, Scale, Step, SynthStep, TRACKS};

/// Set the steps of `track` in `pattern` from `notation` (see the module).
pub fn row(p: &mut Project, pattern: usize, track: usize, velocity: u8, notation: &str) {
    for (i, c) in notation.bytes().filter(|b| !b.is_ascii_whitespace() && *b != b'|').enumerate() {
        let hit = Step::hit(velocity);
        let s = match c {
            b'x' => hit,
            b'X' => hit.with_accent(true),
            b'g' => Step::hit(45),
            b'-' => Step::hit(70),
            b'2' => hit.with_ratchet(2),
            b'3' => hit.with_ratchet(3),
            b'4' => hit.with_ratchet(4),
            b'?' => hit.with_prob(8),
            b'>' => hit.with_micro(6),
            _ => Step::OFF,
        };
        p.set_step(pattern, track, i, s);
    }
}

/// A note name ("a2", "c#3", "eb4") as a MIDI note (C4 = 60).
#[must_use]
pub fn parse_note(s: &str) -> Option<u8> {
    let b = s.as_bytes();
    let base: i16 = match b.first()?.to_ascii_lowercase() {
        b'c' => 0,
        b'd' => 2,
        b'e' => 4,
        b'f' => 5,
        b'g' => 7,
        b'a' => 9,
        b'b' => 11,
        _ => return None,
    };
    let (acc, rest) = match b.get(1) {
        Some(b'#') => (1, &s[2..]),
        Some(b'b') if b.len() > 2 => (-1, &s[2..]),
        _ => (0, &s[1..]),
    };
    let octave: i16 = rest.parse().ok()?;
    u8::try_from((octave + 1) * 12 + base + acc).ok().filter(|n| *n <= 127)
}

/// The gate code nearest below `quarters` quarter steps.
#[must_use]
pub fn gate_code(quarters: u16) -> u8 {
    GATES.iter().rposition(|g| *g <= quarters).unwrap_or(0) as u8
}

/// Set synth track `st`'s steps in `pattern` from `notation` (see the
/// module) at `velocity`.
pub fn line(p: &mut Project, pattern: usize, st: usize, velocity: u8, notation: &str) {
    let mut last: Option<(usize, u16)> = None;
    for (i, tok) in notation.split_whitespace().filter(|t| *t != "|").enumerate() {
        if tok == "-" {
            if let Some((at, q)) = last.as_mut() {
                *q += 4;
                let s = p.pattern(pattern).synth_step(st, *at).with_gate(gate_code(*q));
                p.set_synth_step(pattern, st, *at, s);
            }
            continue;
        }
        let off = SynthStep::off(p.pattern(pattern).synth_step(st, i).pitch());
        if tok == "." {
            p.set_synth_step(pattern, st, i, off);
            last = None;
            continue;
        }
        let end = tok.find([':', '!', '\'', '>']).unwrap_or(tok.len());
        let Some(n) = parse_note(&tok[..end]) else { continue };
        let mods = &tok[end..];
        let mut s = SynthStep::note(n, velocity);
        let mut q = 4;
        if mods.contains(":t") {
            s = s.with_chord(Chord::Triad);
        } else if mods.contains(":s") {
            s = s.with_chord(Chord::Seventh);
        } else if mods.contains(":p") {
            s = s.with_chord(Chord::Power);
        }
        if mods.contains('!') {
            s = s.with_common(s.common().with_accent(true));
        }
        if mods.contains('\'') {
            q = 2;
        }
        if mods.contains('>') {
            s = s.with_common(s.common().with_micro(6));
        }
        p.set_synth_step(pattern, st, i, s.with_gate(gate_code(q)));
        last = Some((i, q));
    }
}

/// Tracks of the demo: kick, snare, clap, closed hat, open hat, low tom,
/// high tom, crash (the default tracks with the rim made a crash).
pub const TRACKS_SEQ1: [u8; 8] = [36, 38, 39, 42, 46, 45, 50, 49];

/// The demo beat: "Night Bus", 98 BPM, swing 56 %, the punch kit; A × 2,
/// B × 2, A, C (32 steps, toms), B, D (the fill). About 38 s.
pub fn beat(p: &mut Project) {
    p.reset();
    p.set_name("Night Bus");
    p.set_tempo(9_800);
    p.set_swing(56);
    p.set_factory_kit(2);
    for (t, n) in TRACKS_SEQ1.iter().enumerate() {
        p.set_note(t, *n);
    }
    p.set_level(3, 60);
    p.set_level(4, 55);
    p.set_level(7, 55);
    // A: the groove.
    row(p, 0, 0, 115, "X... ..x. ..x. ....");
    row(p, 0, 1, 105, ".... X... .... X..g");
    row(p, 0, 3, 90, "x-x- x-x- x-x- x-.-");
    row(p, 0, 4, 85, ".... .... .... ..x.");
    // B: kick variation, hats with chance, a ratchet.
    row(p, 1, 0, 115, "X..x ..x. X.x. ....");
    row(p, 1, 1, 105, ".... X..g .... X.g?");
    row(p, 1, 2, 80, ".... .... .... x...");
    row(p, 1, 3, 90, "x?x? x?x? x?x? 3.x.");
    row(p, 1, 4, 85, "..>. .... ..>. ....");
    // C: 32 steps, toms answering.
    p.set_len(2, 32);
    row(p, 2, 0, 115, "X... ..x. ..x. .... X... ..x. x.x. ....");
    row(p, 2, 1, 105, ".... X... .... X... .... X... .... X.gg");
    row(p, 2, 3, 85, "x.x. x.x. x.x. x.x. x-x- x-x- x-x- x-x-");
    row(p, 2, 5, 95, ".... .... ..x. .x.. .... .... ..x. .x.x");
    row(p, 2, 6, 95, ".... .... x... .... .... .... x... x...");
    row(p, 2, 7, 90, "X... .... .... .... .... .... .... ....");
    // D: the fill, ratchets growing.
    row(p, 3, 0, 115, "X... .... X... ....");
    row(p, 3, 1, 100, "x... 2... 3... 4.44");
    row(p, 3, 5, 95, "..x. ..x. ..xx .x..");
    row(p, 3, 6, 95, ".x.. .x.. .x.. x...");
    row(p, 3, 7, 90, "X... .... .... ....");
    p.song.clear();
    for (pat, n) in [(0, 2), (1, 2), (0, 1), (2, 1), (1, 1), (3, 1)] {
        p.song.push(pat, n);
    }
    p.set_mode(PlayMode::Song);
    p.set_current(0);
}

/// Bass and lead.
const BASS: usize = 0;
const LEAD: usize = 1;

/// The SEQ-2 demo song: "Tram Lights", 104 BPM, swing 54 %, A minor
/// (scale lock), kit A punch and kit B percussion (shaker and conga),
/// the acid bass and an electric piano playing chords, sends to the delay
/// and the reverb, locks (the bass's cutoff opening, the snare tuned up
/// in the fill), the fill pattern D. Song: A × 2, B × 2, C × 2, A, B, D
/// (176 steps, 25 s).
pub fn song(p: &mut Project) {
    p.reset();
    p.set_name("Tram Lights");
    p.set_tempo(10_400);
    p.set_swing(54);
    p.set_factory_kit_of(0, 2);
    p.set_factory_kit_of(1, 4);
    for (t, n) in [36u8, 38, 39, 42, 46, 70, 63, 49].iter().enumerate() {
        p.set_note(t, *n);
    }
    p.set_track_kit(5, 1);
    p.set_track_kit(6, 1);
    p.set_level(3, 55);
    p.set_level(4, 50);
    p.set_level(5, 60);
    p.set_level(6, 70);
    p.set_level(7, 50);
    p.set_scale(Scale::Minor, 9);
    p.set_preset(BASS, 12); // acid-bass
    p.set_preset(LEAD, 7); // fm-epiano
    p.set_level(TRACKS + BASS, 75);
    p.set_level(TRACKS + LEAD, 70);
    p.set_synth_sends(LEAD, 30, 40);
    p.set_synth_sends(BASS, 0, 8);
    p.set_drum_sends(0, 12);
    // A: the groove; the bass walks, the piano answers off the beat.
    row(p, 0, 0, 115, "X... ..x. X... ..x.");
    row(p, 0, 1, 105, ".... X... .... X..g");
    row(p, 0, 3, 90, "x-x- x-x- x-x- x-x-");
    row(p, 0, 5, 80, "..x. ..x. ..x. ..xx");
    line(p, 0, BASS, 105, "a1 . a2' . c2 . a1 - e2 . g1 . a1' a1' c2 d2");
    line(p, 0, LEAD, 90, ". . a3:t' . . . . . . . c4:t' . . . g3:t' .");
    // B: open hats, congas, the bass's filter opening by locks.
    p.copy_pattern(0, 1);
    row(p, 1, 4, 85, "..x. ..x. ..x. ..x.");
    row(p, 1, 6, 90, ".... ...x .x.. x..x");
    line(p, 1, BASS, 110, "f1 . f2' . a1 . f1 - g1 . g2' . e2 . d2 c2");
    line(p, 1, LEAD, 92, ". . f3:s' . . . . . . . g3:s' . . . e3:t .");
    for (k, v) in [(0u8, 30u8), (4, 45), (8, 60), (12, 80)] {
        p.set_lock(1, usize::from(k), TRACKS + BASS, LockParam::Cutoff, v);
    }
    // C: the breakdown — no kick, long chords, the bass in half time.
    p.set_len(2, 32);
    row(p, 2, 2, 85, ".... X... .... X... .... X... .... X.x.");
    row(p, 2, 3, 70, "x.x. x.x. x.x. x.x. x.x. x.x. x.x. x?x?");
    row(p, 2, 5, 75, "x.x. x.x. x.x. x.x. x.x. x.x. x.x. x.x.");
    row(p, 2, 7, 80, "X... .... .... .... .... .... .... ....");
    line(p, 2, BASS, 95, "a1 - - - - - - - f1 - - - - - - - c2 - - - - - - - g1 - - - e1 - - -");
    line(p, 2, LEAD, 80, "a3:s - - - - - - - f3:s - - - - - - - c4:t - - - - - - - g3:t - - - e3:t - - -");
    // D: the fill — a snare roll tuned up by locks, toms on the congas.
    row(p, 3, 0, 115, "X... .... X... ....");
    row(p, 3, 1, 100, "x.x. x.x. 2.2. 3344");
    row(p, 3, 6, 90, ".x.x .x.x .x.x ....");
    row(p, 3, 7, 90, "X... .... .... ....");
    for k in 0..16usize {
        if p.pattern(3).step(1, k).on() {
            // From the entry's own tune (+1.5) up a quarter tone a step.
            p.set_lock(3, k, 1, LockParam::Tune, 96 + 6 + 2 * k as u8);
        }
    }
    line(p, 3, BASS, 110, "a1 . a1 . a1 . a1 . e2 . e2 . g2 g2 g#2 g#2");
    line(p, 3, LEAD, 90, "e3:p' . . . e3:p' . . . . . . . e4:p - - -");
    p.set_fill(Some(3));
    p.song.clear();
    for (pat, n) in [(0, 2), (1, 2), (2, 2), (0, 1), (1, 1), (3, 1)] {
        p.song.push(pat, n);
    }
    p.set_mode(PlayMode::Song);
    p.set_current(0);
}

/// The song's length in steps (one pass of the chain).
#[must_use]
pub fn song_steps(p: &Project) -> usize {
    p.song.entries().iter().map(|e| p.pattern(usize::from(e.pattern)).len() * usize::from(e.repeats)).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notation() {
        assert_eq!(parse_note("c4"), Some(60));
        assert_eq!(parse_note("a2"), Some(45));
        assert_eq!(parse_note("c#3"), Some(49));
        assert_eq!(parse_note("eb4"), Some(63));
        assert_eq!(parse_note("b-1"), Some(11));
        assert_eq!(parse_note("h2"), None);
        assert_eq!(gate_code(4), 3);
        assert_eq!(gate_code(8), 5);
        assert_eq!(gate_code(10), 5);
        assert_eq!(gate_code(32), 9);
        let mut p = std::boxed::Box::new(Project::NEW);
        line(&mut p, 0, 0, 100, "a2 - . c3:t! e3' g3>");
        let s = p.pattern(0).synth_step(0, 0);
        assert_eq!((s.on(), s.pitch(), s.gate_quarters()), (true, 45, 8));
        assert!(!p.pattern(0).synth_step(0, 2).on());
        let c = p.pattern(0).synth_step(0, 3);
        assert_eq!((c.chord(), c.common().accent()), (Chord::Triad, true));
        assert_eq!(p.pattern(0).synth_step(0, 4).gate_quarters(), 2);
        assert_eq!(p.pattern(0).synth_step(0, 5).common().micro(), 6);
    }
}
