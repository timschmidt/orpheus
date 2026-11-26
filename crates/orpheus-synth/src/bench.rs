//! Fixed loads for measuring the engine's cost per voice and per effect on
//! the player (`synth bench`, SYN-3) — and on the host, where
//! `tests/bench.rs` keeps the hash of each load's render, so the player's
//! output can be compared bit for bit.
//!
//! A load is `n` voices of one sound held for the whole run (melodic
//! sounds with their amplitude envelope's sustain at 1, so nothing
//! decays; drums struck again every [`DRUM_EVERY`] blocks on parts of
//! their own), or one sine voice feeding one effect at full send, or the
//! empty engine (the mix, the master and the limiter). Cost per voice =
//! (cost of the load − cost of [`EMPTY`]) / voices; cost of an effect =
//! (its load − the `sine` load at `n` = 1). Everything is rendered at
//! [`RATE`] in blocks of [`BLOCK`] frames from seed 1.

use crate::engine::{Engine, Event, EventKind, MAX_VOICES, Param};
use crate::fx::{ChorusParams, DelayParams, ReverbParams};
use crate::osc::Waveform;
use crate::patch::{Kind, Patch, SubParams};
use crate::presets;

/// Engine rate of the loads, Hz.
pub const RATE: u32 = 48_000;
/// Frames per render block.
pub const BLOCK: usize = 256;
/// Blocks rendered before the measurement (the voices' attacks, the
/// player's code paged in).
pub const WARM: u32 = 8;
/// Blocks measured by default (16 384 frames, 0.34 s).
pub const BLOCKS: u32 = 64;
/// Drums are struck again every this many blocks (43 ms).
pub const DRUM_EVERY: u32 = 8;
/// Voices of a load by default.
pub const VOICES: u8 = 8;

/// What a load plays.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Load {
    /// No voice.
    Empty,
    /// A pure sine voice per part (one oscillator, no filter).
    Sine,
    /// [`presets::MELODIC`]`[i]`, held, with a full send into effect
    /// `send` (delay 0, chorus 1, reverb 2; none: `NO_SEND`).
    Melodic(usize, u8),
    /// [`presets::DRUMS`]`[i]`, struck again and again.
    Drum(usize),
    /// One sine voice at full send into the delay (k = 1, 250 ms).
    Delay,
    /// … into the delay at 1 s (k = 4, the slow path).
    DelayLong,
    /// … into the chorus.
    Chorus,
    /// … into the reverb.
    Reverb,
}

/// A named load.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Bench {
    /// Name (the key in `tests/goldens-bench.txt`).
    pub name: &'static str,
    /// What it plays.
    pub load: Load,
}

const fn b(name: &'static str, load: Load) -> Bench {
    Bench { name, load }
}

/// The empty engine's load.
pub const EMPTY: Bench = b("empty", Load::Empty);
/// No send ([`Load::Melodic`]).
pub const NO_SEND: u8 = 0xFF;
const N: u8 = NO_SEND;

/// Every load, in the bench's numbering.
pub const BENCHES: [Bench; 36] = [
    EMPTY,
    b("sine", Load::Sine),
    b("saw-lead", Load::Melodic(0, N)),
    b("square-bass", Load::Melodic(1, N)),
    b("pwm-pad", Load::Melodic(2, N)),
    b("tri-flute", Load::Melodic(3, N)),
    b("organ", Load::Melodic(4, N)),
    b("vocal", Load::Melodic(5, N)),
    b("pluck", Load::Melodic(6, N)),
    b("fm-epiano", Load::Melodic(7, N)),
    b("fm-bell", Load::Melodic(8, N)),
    b("fm-bass", Load::Melodic(9, N)),
    b("sweep-fx", Load::Melodic(10, N)),
    b("noise-hit", Load::Melodic(11, N)),
    b("acid-bass", Load::Melodic(12, N)),
    b("strings", Load::Melodic(13, N)),
    b("brass", Load::Melodic(14, N)),
    b("sub-bass", Load::Melodic(15, N)),
    b("kick", Load::Drum(0)),
    b("snare", Load::Drum(1)),
    b("closed-hat", Load::Drum(2)),
    b("open-hat", Load::Drum(3)),
    b("clap", Load::Drum(4)),
    b("tom", Load::Drum(5)),
    b("rim", Load::Drum(6)),
    b("cowbell", Load::Drum(7)),
    b("cymbal", Load::Drum(8)),
    b("clave", Load::Drum(9)),
    b("shaker", Load::Drum(10)),
    b("delay", Load::Delay),
    b("delay-1s", Load::DelayLong),
    b("chorus", Load::Chorus),
    b("reverb", Load::Reverb),
    // The same effects with eight saw-lead voices (an effect's cost does
    // not depend on the voices; these check it).
    b("saw-lead+reverb", Load::Melodic(0, 2)),
    b("pwm-pad+chorus", Load::Melodic(2, 1)),
    b("fm-epiano+delay", Load::Melodic(7, 0)),
];

/// The loads of [`BENCHES`] without their names (the player links no
/// name: they would be resident strings there).
pub static LOADS: [Load; BENCHES.len()] = {
    let mut a = [Load::Empty; BENCHES.len()];
    let mut i = 0;
    while i < BENCHES.len() {
        a[i] = BENCHES[i].load;
        i += 1;
    }
    a
};

/// [`presets::MELODIC`] and [`presets::DRUMS`] without their names (as
/// [`LOADS`]).
static MELODIC: [Patch; presets::MELODIC.len()] = {
    let mut a = [Patch::INIT; presets::MELODIC.len()];
    let mut i = 0;
    while i < a.len() {
        a[i] = presets::MELODIC[i].1;
        i += 1;
    }
    a
};
static DRUMS: [crate::drum::DrumParams; presets::DRUMS.len()] = {
    let mut a = [presets::DRUMS[0].1; presets::DRUMS.len()];
    let mut i = 0;
    while i < a.len() {
        a[i] = presets::DRUMS[i].1;
        i += 1;
    }
    a
};

/// The sine patch.
const SINE: Patch = {
    let mut p = Patch::INIT;
    p.kind = Kind::Subtractive(SubParams {
        osc: [crate::patch::OscParams::new(Waveform::Sine), crate::patch::OscParams::OFF],
        noise: 0.0,
    });
    p.amp_env.sustain = 1.0;
    p
};

/// Voices a load plays when `n` are asked (effect loads: 1; at most
/// [`MAX_VOICES`]).
#[must_use]
pub fn voices(load: Load, n: u8) -> u8 {
    match load {
        Load::Empty => 0,
        Load::Delay | Load::DelayLong | Load::Chorus | Load::Reverb => 1,
        _ => n.clamp(1, MAX_VOICES as u8),
    }
}

/// Which effect a load feeds (send index: delay 0, chorus 1, reverb 2).
fn send_of(load: Load) -> Option<usize> {
    match load {
        Load::Delay | Load::DelayLong => Some(0),
        Load::Chorus => Some(1),
        Load::Reverb => Some(2),
        Load::Melodic(_, s) if s < 3 => Some(usize::from(s)),
        _ => None,
    }
}

/// Note of voice `v`.
fn note(v: u8) -> u8 {
    48 + 3 * v
}

/// Set `e` (any engine, a zeroed one included) up for load `i` with `n`
/// voices; false for no such load.
pub fn setup(i: usize, n: u8, e: &mut Engine) -> bool {
    let Some(&load) = LOADS.get(i) else { return false };
    e.init(RATE, 1);
    let n = voices(load, n);
    let patch = match load {
        Load::Empty => return true,
        Load::Sine | Load::Delay | Load::DelayLong | Load::Chorus | Load::Reverb => SINE,
        Load::Melodic(k, _) => {
            let Some(&base) = MELODIC.get(k) else { return false };
            let mut p = base;
            p.amp_env.sustain = 1.0;
            p
        }
        Load::Drum(k) => match DRUMS.get(k) {
            Some(&d) => Patch::drum(d),
            None => return false,
        },
    };
    match load {
        Load::Delay => {
            e.set_delay(&DelayParams { time: 0.25, beats: 0.0, feedback: 0.5, ..DelayParams::DEFAULT });
        }
        Load::DelayLong => {
            e.set_delay(&DelayParams { time: 1.0, beats: 0.0, feedback: 0.5, ..DelayParams::DEFAULT });
        }
        Load::Chorus => e.set_chorus(&ChorusParams::DEFAULT),
        Load::Reverb => e.set_reverb(&ReverbParams { decay: 2.0, ..ReverbParams::DEFAULT }),
        _ => {}
    }
    let send = send_of(load);
    for v in 0..n {
        e.set_patch(v, &patch);
        e.apply(EventKind::Param { part: v, param: Param::Volume, value: 0.5 });
        if let Some(s) = send {
            let param = match s {
                0 => Param::DelaySend,
                1 => Param::ChorusSend,
                _ => Param::ReverbSend,
            };
            e.apply(EventKind::Param { part: v, param, value: 1.0 });
        }
    }
    true
}

/// The events of block `k` of load `i` with `n` voices into `out`;
/// returns how many. Block 0 starts every voice; drums are struck again
/// every [`DRUM_EVERY`] blocks.
pub fn events(i: usize, n: u8, k: u32, out: &mut [Event; MAX_VOICES]) -> usize {
    let Some(&load) = LOADS.get(i) else { return 0 };
    let drum = matches!(load, Load::Drum(_));
    if k != 0 && !(drum && k % DRUM_EVERY == 0) {
        return 0;
    }
    let n = usize::from(voices(load, n));
    for (v, o) in (0u8..).zip(out.iter_mut().take(n)) {
        *o = Event::note_on(0, v, note(v), 100);
    }
    n.min(MAX_VOICES)
}

/// Render load `i` with `n` voices for `warm` + `blocks` blocks into
/// `buf` (≥ 2 × [`BLOCK`] samples), calling `measure(true)` just before
/// and `measure(false)` just after each measured block's render (the hash
/// and the voice count are outside); returns the hash of the measured
/// blocks and the sum of the voices sounding after each of them.
pub fn run(
    i: usize,
    n: u8,
    warm: u32,
    blocks: u32,
    e: &mut Engine,
    buf: &mut [i16],
    mut measure: impl FnMut(bool),
) -> Option<(u64, u32)> {
    if !setup(i, n, e) || buf.len() < 2 * BLOCK {
        return None;
    }
    let buf = buf.get_mut(..2 * BLOCK)?;
    let mut ev = [Event::note_off(0, 0, 0); MAX_VOICES];
    let mut h = crate::demo::Fnv::new();
    let mut sounding = 0u32;
    for k in 0..warm + blocks {
        let m = events(i, n, k, &mut ev);
        let timed = k >= warm;
        if timed {
            measure(true);
        }
        e.render_i16(buf, ev.get(..m).unwrap_or_default());
        if timed {
            measure(false);
            h.add(buf);
            sounding += e.active_voices() as u32;
        }
    }
    Some((h.0, sounding))
}
