//! Three short songs of our own, written as Standard MIDI Files: the
//! material of the golden renders, the listening WAVs and the simulator's
//! MIDI folder.
//!
//! * [`etude`] (format 1, ≈ 23 s): piano arpeggios, a violin melody with a
//!   bend, bass, a string pad with modulation, drums; a tempo change at
//!   bar 5 and a ritardando at the end.
//! * [`karaoke`] (format 1, `.kar`, ≈ 22 s): a flute melody with four lines
//!   of karaoke lyrics, guitar, bass and soft drums.
//! * [`tour`] (format 0, ≈ 52 s): every sound of the GM set plays a bar,
//!   then the standard and the analog kits.
//!
//! Each function writes into `buf` and returns the file's length (`None`
//! if `buf` is too small; 16 KiB is enough).

use crate::gm;
use crate::smf::Division;
use crate::write::{TrackBuf, Writer};

/// Ticks per quarter note of the demo songs.
pub const TPQ: u64 = 480;
const E: u64 = TPQ / 2;
const BAR: u64 = 4 * TPQ;

fn bpm(b: u32) -> u32 {
    60_000_000 / b
}

/// C-major-ish chords as MIDI notes (root position, around C3–C4).
const C: [u8; 3] = [48, 52, 55];
const AM: [u8; 3] = [45, 48, 52];
const F: [u8; 3] = [41, 45, 48];
const G: [u8; 3] = [43, 47, 50];
const EM: [u8; 3] = [40, 43, 47];
const DM: [u8; 3] = [38, 41, 45];

fn conductor(w: &mut Writer<'_>, title: &str, tempos: &[(u64, u32)], end: u64) {
    w.track();
    w.meta(0, 0x03, title.as_bytes());
    w.meta(0, 0x58, &[4, 2, 24, 8]);
    w.meta(0, 0x59, &[0, 0]);
    for &(t, b) in tempos {
        w.tempo(t, bpm(b));
    }
    w.end_track(end);
}

fn drums(t: &mut TrackBuf<512>, bars: core::ops::Range<u64>, soft: bool) {
    let v = if soft { 70 } else { 100 };
    for bar in bars {
        let b = bar * BAR;
        for i in 0..8u64 {
            let at = b + i * E;
            let open = i == 7 && bar % 2 == 1;
            t.note(at, E / 2, 9, if open { 46 } else { 42 }, if i % 2 == 0 { v - 10 } else { v - 35 });
        }
        t.note(b, E, 9, 36, v + 10);
        t.note(b + 2 * TPQ, E, 9, 36, v);
        if !soft || bar % 2 == 1 {
            t.note(b + 2 * TPQ + 3 * E / 2, E, 9, 36, v - 30);
        }
        t.note(b + TPQ, E, 9, if soft { 37 } else { 38 }, v);
        t.note(b + 3 * TPQ, E, 9, if soft { 37 } else { 38 }, v + 5);
    }
}

/// The etude (see the module documentation).
pub fn etude(buf: &mut [u8]) -> Option<usize> {
    let chords = [C, AM, F, G, EM, AM, DM, C];
    let bars = chords.len() as u64 + 1;
    let end = bars * BAR;
    let mut w = Writer::new(buf, 1, Division::Metrical(TPQ as u16));
    w.running_status(true);
    conductor(
        &mut w,
        "Etude for Orpheus",
        &[(0, 92), (4 * BAR, 100), (7 * BAR + 2 * TPQ, 88), (8 * BAR, 76), (8 * BAR + 2 * TPQ, 64)],
        end,
    );

    // Piano: broken chords in eighths, the last bar a held chord.
    let mut t = TrackBuf::<512>::new();
    t.program(0, 0, 0);
    t.cc(0, 0, 7, 96);
    t.cc(0, 0, 10, 52);
    t.cc(0, 0, 91, 50);
    for (bar, ch) in chords.iter().enumerate() {
        let b = bar as u64 * BAR;
        let pat = [ch[0], ch[1] + 12, ch[2] + 12, ch[0] + 24, ch[2] + 12, ch[1] + 12, ch[2], ch[1] + 12];
        for (i, &n) in pat.iter().enumerate() {
            let vel = if i % 4 == 0 { 84 } else { 66 };
            t.note(b + i as u64 * E, E * 2, 0, n, vel);
        }
    }
    for n in [C[0] + 12, C[1] + 12, C[2] + 12, C[0] + 24] {
        t.note(8 * BAR, BAR, 0, n, 80);
    }
    t.cc(0, 0, 64, 0);
    for bar in 0..8u64 {
        t.cc(bar * BAR + 10, 0, 64, 100);
        t.cc(bar * BAR + BAR - 20, 0, 64, 0);
    }
    w.track();
    t.write(&mut w);
    w.end_track(end);

    // Violin melody (quarters and halves), a bend up into bar 4.
    let mel: [(u8, u64, u64); 20] = [
        (76, 0, 2),
        (74, 2, 1),
        (72, 3, 1),
        (72, 4, 2),
        (76, 6, 2),
        (77, 8, 3),
        (76, 11, 1),
        (74, 12, 4),
        (71, 16, 2),
        (74, 18, 2),
        (72, 20, 2),
        (69, 22, 2),
        (69, 24, 2),
        (72, 26, 1),
        (74, 27, 1),
        (77, 28, 2),
        (74, 30, 2),
        (76, 32, 4),
        (0, 36, 0),
        (0, 36, 0),
    ];
    let mut t = TrackBuf::<512>::new();
    t.program(0, 3, 40);
    t.cc(0, 3, 7, 90);
    t.cc(0, 3, 10, 80);
    t.cc(0, 3, 91, 60);
    for &(n, at, len) in &mel {
        if n > 0 {
            t.note(at * TPQ, len * TPQ - 30, 3, n, 92);
        }
    }
    // Bend from −1 semitone up to the note at bar 4.
    for i in 0..=8u64 {
        let v = -4096 + (i as i16) * 512;
        t.bend(12 * TPQ + i * 30, 3, v.min(0));
    }
    t.cc(26 * TPQ, 3, 1, 70);
    t.cc(32 * TPQ, 3, 1, 0);
    w.track();
    t.write(&mut w);
    w.end_track(end);

    // Bass: roots, octave bounce.
    let mut t = TrackBuf::<512>::new();
    t.program(0, 1, 33);
    t.cc(0, 1, 7, 110);
    for (bar, ch) in chords.iter().enumerate() {
        let b = bar as u64 * BAR;
        let r = ch[0] - 12;
        t.note(b, TPQ + E, 1, r, 100);
        t.note(b + TPQ + E, E, 1, r + 12, 80);
        t.note(b + 2 * TPQ, TPQ, 1, r, 90);
        t.note(b + 3 * TPQ, TPQ - 40, 1, ch[2] - 12, 85);
    }
    t.note(8 * BAR, BAR, 1, C[0] - 12, 100);
    w.track();
    t.write(&mut w);
    w.end_track(end);

    // String pad: whole notes, the modulation wheel up in bar 7.
    let mut t = TrackBuf::<512>::new();
    t.program(0, 2, 48);
    t.cc(0, 2, 7, 70);
    t.cc(0, 2, 10, 40);
    t.cc(0, 2, 91, 90);
    t.cc(0, 2, 93, 60);
    for (bar, ch) in chords.iter().enumerate() {
        for &n in ch {
            t.note(bar as u64 * BAR, BAR, 2, n + 12, 70);
        }
    }
    for i in 0..16u64 {
        t.cc(6 * BAR + i * E, 2, 1, (i * 8) as u8);
    }
    t.cc(8 * BAR, 2, 1, 0);
    for &n in &C {
        t.note(8 * BAR, BAR, 2, n + 12, 70);
    }
    w.track();
    t.write(&mut w);
    w.end_track(end);

    // Drums, with a crash at bars 1 and 5.
    let mut t = TrackBuf::<512>::new();
    drums(&mut t, 0..8, false);
    t.note(0, TPQ, 9, 49, 90);
    t.note(4 * BAR, TPQ, 9, 49, 100);
    t.note(8 * BAR, TPQ, 9, 49, 90);
    t.note(8 * BAR, TPQ, 9, 36, 100);
    for i in 0..4u64 {
        t.note(7 * BAR + 2 * TPQ + i * E, E, 9, [50, 48, 45, 41][i as usize], 90);
    }
    w.track();
    t.write(&mut w);
    w.end_track(end);
    w.finish()
}

/// The lyrics of [`karaoke`]: (syllable as stored, eighths, note).
pub const LYRICS: [(&str, u64, u8); 30] = [
    ("\\Mor", 2, 64),
    ("ning ", 1, 62),
    ("light ", 3, 60),
    ("up", 1, 64),
    ("on ", 1, 67),
    ("the ", 1, 69),
    ("hill,", 7, 67),
    ("/the ", 1, 64),
    ("ri", 1, 67),
    ("ver ", 2, 69),
    ("runs, ", 4, 72),
    ("the ", 1, 71),
    ("air ", 1, 69),
    ("is ", 2, 67),
    ("still.", 4, 64),
    ("\\Sing ", 2, 67),
    ("a", 1, 69),
    ("long, ", 3, 67),
    ("the ", 1, 64),
    ("day ", 1, 62),
    ("is ", 2, 60),
    ("new,", 6, 62),
    ("/e", 1, 64),
    ("ve", 1, 64),
    ("ry ", 2, 65),
    ("road ", 2, 67),
    ("leads ", 2, 69),
    ("home ", 2, 62),
    ("to ", 2, 64),
    ("you.", 4, 60),
];

/// Bars before the singing starts in [`karaoke`].
pub const KAR_INTRO_BARS: u64 = 1;
/// Tempo of [`karaoke`], BPM.
pub const KAR_BPM: u32 = 100;

/// The karaoke song (see the module documentation).
pub fn karaoke(buf: &mut [u8]) -> Option<usize> {
    let chords = [C, G, AM, EM, F, C, G, C];
    let bars = KAR_INTRO_BARS + chords.len() as u64 + 1;
    let end = bars * BAR;
    let mut w = Writer::new(buf, 1, Division::Metrical(TPQ as u16));
    w.running_status(true);
    w.track();
    w.meta(0, 0x03, b"Hill Song");
    w.meta(0, 0x01, b"@KMIDI KARAOKE FILE");
    w.meta(0, 0x58, &[4, 2, 24, 8]);
    w.tempo(0, bpm(KAR_BPM));
    w.end_track(end);

    // Words.
    w.track();
    w.meta(0, 0x03, b"Words");
    w.meta(0, 0x01, b"@LENGL");
    w.meta(0, 0x01, b"@THill Song");
    w.meta(0, 0x01, b"@TOrpheus demo");
    let mut at = KAR_INTRO_BARS * BAR;
    for &(s, len, _) in &LYRICS {
        w.meta(at, 0x01, s.as_bytes());
        at += len * E;
    }
    w.end_track(end);

    // Melody: flute.
    let mut t = TrackBuf::<512>::new();
    t.program(0, 0, 73);
    t.cc(0, 0, 7, 100);
    t.cc(0, 0, 91, 50);
    let mut at = KAR_INTRO_BARS * BAR;
    for &(_, len, n) in &LYRICS {
        t.note(at, len * E - 20, 0, n + 12, 96);
        at += len * E;
    }
    w.track();
    t.write(&mut w);
    w.end_track(end);

    // Guitar arpeggios, bass, drums.
    let mut g = TrackBuf::<512>::new();
    let mut b = TrackBuf::<512>::new();
    g.program(0, 1, 25);
    g.cc(0, 1, 10, 30);
    g.cc(0, 1, 7, 85);
    b.program(0, 2, 32);
    b.cc(0, 2, 7, 105);
    let all = [&[C][..], &chords[..], &[C][..]];
    for (bar, ch) in all.iter().flat_map(|c| c.iter()).enumerate() {
        let t0 = bar as u64 * BAR;
        let last = bar as u64 == bars - 1;
        if last {
            for &n in ch {
                g.note(t0, BAR, 1, n + 12, 80);
            }
            b.note(t0, BAR, 2, ch[0] - 12, 100);
            continue;
        }
        let pat = [ch[0], ch[1], ch[2], ch[1] + 12, ch[2] + 12, ch[1] + 12, ch[2], ch[1]];
        for (i, &n) in pat.iter().enumerate() {
            g.note(t0 + i as u64 * E, 3 * E, 1, n + 12, if i % 4 == 0 { 82 } else { 64 });
        }
        b.note(t0, 2 * TPQ - 30, 2, ch[0] - 12, 100);
        b.note(t0 + 2 * TPQ, 2 * TPQ - 30, 2, ch[2] - 12, 90);
    }
    w.track();
    g.write(&mut w);
    w.end_track(end);
    w.track();
    b.write(&mut w);
    w.end_track(end);
    let mut d = TrackBuf::<512>::new();
    drums(&mut d, KAR_INTRO_BARS..bars - 1, true);
    d.cc(0, 9, 7, 80);
    d.note((bars - 1) * BAR, TPQ, 9, 49, 70);
    w.track();
    d.write(&mut w);
    w.end_track(end);
    w.finish()
}

/// A program that plays sound `s` of the GM set (the first that maps to
/// it).
#[must_use]
pub fn program_for(s: u8) -> u8 {
    gm::SOUND_OF.iter().position(|&x| x == s).unwrap_or(0) as u8
}

/// The tour of the sound set (see the module documentation).
pub fn tour(buf: &mut [u8]) -> Option<usize> {
    let sounds = gm::SOUNDS as u64;
    let bars = sounds + 4;
    let end = bars * BAR;
    let mut w = Writer::new(buf, 0, Division::Metrical(TPQ as u16));
    let mut t = TrackBuf::<1024>::new();
    let mut tr = TrackBuf::<512>::new();
    for s in 0..gm::SOUNDS as u8 {
        let b = u64::from(s) * BAR;
        let ch = s % 2;
        let low = matches!(s, 12..=15 | 19);
        let base: u8 = if low { 36 } else { 60 };
        t.program(b, ch, program_for(s));
        t.cc(b, ch, 91, 40);
        if matches!(s, 30 | 31) {
            t.note(b, 3 * TPQ, ch, base + 7, 100);
            continue;
        }
        for (i, n) in [0u8, 4, 7, 12].iter().enumerate() {
            t.note(b + i as u64 * E, E, ch, base + n, 100);
        }
        for n in [0u8, 4, 7] {
            t.note(b + 2 * TPQ, 2 * TPQ - 60, ch, base + n, 90);
        }
        if s == 27 {
            // Saw lead: bend the chord up a tone and back.
            for i in 0..=16i16 {
                let v = if i <= 8 { i * 1023 } else { (16 - i) * 1023 };
                t.bend(b + 2 * TPQ + i as u64 * 50, ch, v);
            }
        }
        if s == 17 {
            for i in 0..8u64 {
                t.cc(b + 2 * TPQ + i * 60, ch, 1, (i * 16) as u8);
            }
            t.cc(b + BAR - 10, ch, 1, 0);
        }
    }
    let k = sounds * BAR;
    drums(&mut tr, sounds..sounds + 2, false);
    tr.program(k + 2 * BAR, 9, 25);
    drums(&mut tr, sounds + 2..sounds + 4, false);
    for (i, n) in [38u8, 38, 45, 45, 47, 48, 50, 49].iter().enumerate() {
        tr.note(k + BAR + 2 * TPQ + i as u64 * E / 2, E, 9, *n, 100);
    }
    tr.note(k + 2 * BAR, TPQ, 9, 49, 100);
    w.track();
    w.meta(0, 0x03, b"Sound set tour");
    w.tempo(0, bpm(160));
    // Merge the two buffers into one track (format 0).
    for e in tr.events() {
        t.push(e.0, e.2[0], e.2[1], e.2[2]);
    }
    t.write(&mut w);
    w.end_track(end);
    w.finish()
}
