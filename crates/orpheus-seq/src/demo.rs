//! A beat of our own for the example WAV, the tests and the simulator's
//! card: four patterns chained into a song, written in a step notation.
//!
//! Notation of [`row`]: one character per step — `.` off, `x` a hit at
//! the row's velocity, `X` accented, `g` a ghost note (velocity 45), `2`
//! `3` `4` ratchets, `?` half probability, `>` delayed a quarter step
//! (6/24), `-` a hit at velocity 70.

use crate::model::{PlayMode, Project, Step};

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

/// Tracks of the demo: kick, snare, clap, closed hat, open hat, low tom,
/// high tom, crash (the default tracks with the rim made a crash).
pub const TRACKS: [u8; 8] = [36, 38, 39, 42, 46, 45, 50, 49];

/// The demo beat: "Night Bus", 98 BPM, swing 56 %, the punch kit; A × 2,
/// B × 2, A, C (32 steps, toms), B, D (the fill). About 38 s.
pub fn beat(p: &mut Project) {
    p.reset();
    p.set_name("Night Bus");
    p.set_tempo(9_800);
    p.set_swing(56);
    p.set_factory_kit(2);
    for (t, n) in TRACKS.iter().enumerate() {
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

/// The song's length in steps (one pass of the chain).
#[must_use]
pub fn song_steps(p: &Project) -> usize {
    p.song.entries().iter().map(|e| p.pattern(usize::from(e.pattern)).len() * usize::from(e.repeats)).sum()
}
