//! Exporting the song: a WAV file rendered offline through the engine, and
//! a Standard MIDI File (type 1) from a dry run (05 §7.16, SEQ-2).
//!
//! What is exported is one pass of what Play plays: in song mode the
//! song's chain once (an empty song: the current pattern), in pattern mode
//! the current pattern once; with the step length of the options, without
//! the metronome or a count-in, the fill key up. Both are pure functions
//! of the project (the probability draws are seeded), so an export is the
//! same every time and on every machine.
//!
//! **WAV** ([`WavExport`]): 16-bit stereo at the engine's rate, the song
//! and then [`TAIL_SECONDS`] for the tails (the synth notes still sounding
//! at the end are released there). The caller renders it in pieces of
//! any size ([`WavExport::render`]) and writes each piece after the
//! 44-byte header ([`wav_header`]): a firmware's card task can do it in
//! the DAC's blocks, the simulator in one go.
//!
//! **MIDI** ([`midi`]): format 1 at [`TICKS_PER_STEP`] ticks a step (so a
//! division of 2,400 ticks a quarter for 1/16 steps) — every hit, delay,
//! ratchet and gate lands on a whole tick, nothing is rounded. Track 0:
//! the name, tempo and 4/4; track 1 "Drums" on channel 10 (each hit a
//! fifth of a step long, shortened when the same note comes again);
//! tracks 2 and 3 "Bass" and "Lead" on channels 1 and 2 with a General
//! MIDI program near the sound and the level as volume (CC 7). The tick of
//! a note is its exact position: the dry run goes at a rate where a frame
//! is a fifth of a unit (`tempo · 96 · d / m` Hz).

use orpheus_midi::smf::Division;
use orpheus_midi::write::Writer;
use orpheus_synth::Engine;

use crate::model::{PlayMode, Project, SYNTH_TRACKS, TRACKS};
use crate::player::{CLICK, Hit, Options, Sequencer, StepLen, scaled_frames, step_units};

/// Seconds rendered after the song for the tails.
pub const TAIL_SECONDS: u32 = 2;
/// MIDI ticks of a step.
pub const TICKS_PER_STEP: u64 = 600;
/// Units of a MIDI tick (every position is a multiple: see the module).
pub const UNITS_PER_TICK: u64 = crate::player::UNITS_PER_STEP / TICKS_PER_STEP;
/// MIDI channel of the drums (10, counted from 1).
pub const DRUM_CHANNEL: u8 = 9;
/// Length of an exported drum hit, ticks (a fifth of a step).
pub const DRUM_TICKS: u64 = TICKS_PER_STEP / 5;

/// Why an export failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportError {
    /// The output buffer is too small (a very long song).
    Full,
}

/// The patterns one pass plays, with how often (the song's entries, or
/// the current pattern once).
fn pass(p: &Project, mut f: impl FnMut(usize, usize)) {
    if p.mode() == PlayMode::Song && !p.song.is_empty() {
        for e in p.song.entries() {
            f(usize::from(e.pattern), usize::from(e.repeats));
        }
    } else {
        f(p.current(), 1);
    }
}

/// Units of one pass (see the module).
#[must_use]
pub fn song_units(p: &Project) -> u64 {
    let mut u = 0;
    pass(p, |pat, times| {
        let one: u64 = (0..p.pattern(pat).len()).map(|k| step_units(p.swing(), k)).sum();
        u += one * times as u64;
    });
    u
}

/// Steps of one pass.
#[must_use]
pub fn song_steps(p: &Project) -> u64 {
    let mut n = 0;
    pass(p, |pat, times| n += (p.pattern(pat).len() * times) as u64);
    n
}

/// Frames of one pass at `rate`.
#[must_use]
pub fn song_frames(p: &Project, rate: u32, step_len: StepLen) -> u64 {
    scaled_frames(song_units(p), rate, p.tempo(), step_len.ratio())
}

/// The 44-byte header of a 16-bit stereo WAV file of `frames` frames.
#[must_use]
pub fn wav_header(rate: u32, frames: u64) -> [u8; 44] {
    let data = u32::try_from(frames * 4).unwrap_or(u32::MAX - 36);
    let mut h = [0u8; 44];
    let mut put = |at: usize, b: &[u8]| h[at..at + b.len()].copy_from_slice(b);
    put(0, b"RIFF");
    put(4, &(36 + data).to_le_bytes());
    put(8, b"WAVEfmt ");
    put(16, &16u32.to_le_bytes());
    put(20, &1u16.to_le_bytes());
    put(22, &2u16.to_le_bytes());
    put(24, &rate.to_le_bytes());
    put(28, &(rate * 4).to_le_bytes());
    put(32, &4u16.to_le_bytes());
    put(34, &16u16.to_le_bytes());
    put(36, b"data");
    put(40, &data.to_le_bytes());
    h
}

/// A WAV export in progress (its own player; about 4.3 KiB).
#[derive(Clone, Debug)]
pub struct WavExport {
    seq: Sequencer,
    song: u64,
    total: u64,
    done: u64,
}

impl WavExport {
    /// Start an export of `p` at `rate` Hz (the engine's rate) with the
    /// step length `step_len`.
    #[must_use]
    pub fn new(p: &Project, rate: u32, step_len: StepLen) -> WavExport {
        let mut seq = Sequencer::new(rate);
        seq.set_options(Options { metronome: false, count_in: 0, step_len });
        seq.play();
        let song = song_frames(p, rate, step_len);
        WavExport { seq, song, total: song + u64::from(TAIL_SECONDS) * u64::from(rate), done: 0 }
    }

    /// Frames of the whole file (song and tail).
    #[must_use]
    pub const fn total_frames(&self) -> u64 {
        self.total
    }

    /// Frames rendered so far.
    #[must_use]
    pub const fn done_frames(&self) -> u64 {
        self.done
    }

    /// Whether everything is rendered.
    #[must_use]
    pub const fn finished(&self) -> bool {
        self.done >= self.total
    }

    /// The file's header.
    #[must_use]
    pub fn header(&self) -> [u8; 44] {
        wav_header(self.seq.rate(), self.total)
    }

    /// Render the next frames into `out` (interleaved stereo, as much as
    /// fits and is left) with `eng` (a fresh engine at the export's rate,
    /// used for nothing else meanwhile); returns the frames written.
    pub fn render(&mut self, p: &Project, eng: &mut Engine, out: &mut [i16]) -> usize {
        let want = ((out.len() / 2) as u64).min(self.total - self.done) as usize;
        let mut at = 0;
        while at < want {
            let mut n = want - at;
            if self.done < self.song {
                n = n.min((self.song - self.done) as usize);
            }
            self.seq.render_i16(p, eng, &mut out[2 * at..2 * (at + n)]);
            at += n;
            self.done += n as u64;
            if self.done == self.song && self.seq.playing() {
                // The end of the song: no more steps, the notes released.
                self.seq.stop();
            }
        }
        want
    }
}

/// A General MIDI program near a factory sound (by its name), else the
/// track's kind (bass: Synth Bass 1, lead: Lead 1).
#[must_use]
pub fn gm_program(name: &str, st: usize) -> u8 {
    match name {
        "square-bass" | "acid-bass" | "sub-bass" => 38,
        "fm-bass" => 39,
        "saw-lead" => 81,
        "pwm-pad" => 90,
        "tri-flute" => 73,
        "organ" => 16,
        "vocal" => 54,
        "pluck" => 45,
        "fm-epiano" => 4,
        "fm-bell" => 14,
        "sweep-fx" => 95,
        "noise-hit" => 127,
        "strings" => 50,
        "brass" => 61,
        _ if st == 0 => 38,
        _ => 80,
    }
}

/// The notes of one pass for the tracks `keep` accepts, in order, with
/// their ticks (synth note-offs as velocity 0; the last ones at the end).
pub fn notes(p: &Project, step_len: StepLen, keep: impl Fn(u8) -> bool, mut f: impl FnMut(u64, &Hit)) {
    let (m, d) = step_len.ratio();
    let rate = u32::try_from(u64::from(p.tempo()) * 96 * d / m).unwrap_or(u32::MAX);
    let mut s = Sequencer::new(rate);
    s.set_options(Options { metronome: false, count_in: 0, step_len });
    s.play();
    let frames = song_frames(p, rate, step_len);
    // A frame is a fifth of a unit: tick = frame / (5 · UNITS_PER_TICK).
    let per_tick = 5 * UNITS_PER_TICK;
    let mut cb = |h: Hit| {
        if h.track != CLICK && keep(h.track) {
            f(h.frame / per_tick, &h);
        }
    };
    let mut left = frames;
    while left > 0 {
        let n = left.min(1 << 20);
        s.run_dry(p, n as usize, &mut cb);
        left -= n;
    }
    s.stop();
    s.run_dry(p, 1, &mut cb);
}

/// Drum note-offs waiting.
const DRUM_OFFS: usize = 64;

/// Write the drum note-offs due by `upto` (in tick order), and `note`'s
/// pending one at `upto`.
fn flush(w: &mut Writer<'_>, offs: &mut [(u64, u8); DRUM_OFFS], n: &mut usize, upto: u64, note: Option<u8>) {
    loop {
        let pick = (0..*n)
            .filter(|&i| offs[i].0 <= upto || Some(offs[i].1) == note)
            .min_by_key(|&i| (offs[i].0.min(upto), i));
        let Some(i) = pick else { break };
        let (t, nt) = offs[i];
        w.channel(t.min(upto), 0x80 | DRUM_CHANNEL, nt, 0);
        offs.copy_within(i + 1..*n, i);
        *n -= 1;
    }
}

/// Write one pass of `p` as a Standard MIDI File (format 1) into `buf`;
/// returns its length.
///
/// # Errors
///
/// [`ExportError::Full`] when `buf` is too small.
pub fn midi(p: &Project, step_len: StepLen, buf: &mut [u8]) -> Result<usize, ExportError> {
    let ppq = TICKS_PER_STEP * u64::from(step_len.beat_steps());
    let end = song_units(p) / UNITS_PER_TICK;
    let mut w = Writer::new(buf, 1, Division::Metrical(ppq as u16));
    // The conductor: name, tempo (µs a quarter), 4/4.
    w.track();
    w.meta(0, 0x03, p.name().as_bytes());
    let us = (6_000_000_000u64 + u64::from(p.tempo()) / 2) / u64::from(p.tempo());
    w.tempo(0, us as u32);
    w.meta(0, 0x58, &[4, 2, 24, 8]);
    w.end_track(end);
    // The drums: every hit a fifth of a step (earlier when the note comes
    // again).
    w.track();
    w.meta(0, 0x03, b"Drums");
    w.channel(0, 0xB0 | DRUM_CHANNEL, 7, 100);
    let mut offs: [(u64, u8); DRUM_OFFS] = [(0, 0); DRUM_OFFS];
    let mut n_off = 0usize;
    notes(
        p,
        step_len,
        |t| usize::from(t) < TRACKS,
        |tick, h| {
            flush(&mut w, &mut offs, &mut n_off, tick, Some(h.note));
            w.channel(tick, 0x90 | DRUM_CHANNEL, h.note, h.velocity);
            if n_off == offs.len() {
                let first = offs[0];
                w.channel(tick, 0x80 | DRUM_CHANNEL, first.1, 0);
                offs.copy_within(1.., 0);
                n_off -= 1;
            }
            offs[n_off] = (tick + DRUM_TICKS, h.note);
            n_off += 1;
        },
    );
    flush(&mut w, &mut offs, &mut n_off, u64::MAX, None);
    w.end_track(end);
    // The synth tracks.
    for st in 0..SYNTH_TRACKS {
        let s = p.synth(st);
        let ch = st as u8;
        w.track();
        w.meta(0, 0x03, if st == 0 { b"Bass" } else { b"Lead" });
        w.channel(0, 0xC0 | ch, gm_program(s.name.as_str(), st), 0);
        w.channel(0, 0xB0 | ch, 7, ((u32::from(s.level) * 127 + 50) / 100) as u8);
        let track = (TRACKS + st) as u8;
        notes(
            p,
            step_len,
            |t| t == track,
            |tick, h| {
                if h.velocity == 0 {
                    w.channel(tick, 0x80 | ch, h.note, 0);
                } else {
                    w.channel(tick, 0x90 | ch, h.note, h.velocity);
                }
            },
        );
        w.end_track(end);
    }
    w.finish().ok_or(ExportError::Full)
}
