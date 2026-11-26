//! Helpers shared by the integration tests.

#![allow(dead_code)]

use orpheus_seq::model::{MAX_STEPS, PATTERNS, SYNTH_TRACKS, TRACKS};
use orpheus_seq::{Chord, Hit, LockParam, Project, Scale, Sequencer, Step, SynthStep};
use orpheus_synth::Engine;

/// A boxed project (25 KB: keep it off the test thread's stack).
pub fn project() -> Box<Project> {
    Box::new(Project::NEW)
}

/// A boxed engine.
pub fn engine(rate: u32) -> Box<Engine> {
    Box::new(Engine::new(rate, 1))
}

/// A file of the data folder (`tests/data/`).
pub fn data(name: &str) -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/data").join(name))
        .unwrap_or_else(|e| panic!("{name}: {e}"))
}

/// The hits of `frames` frames played from the start, in order.
pub fn hits(p: &Project, rate: u32, frames: usize) -> Vec<Hit> {
    let mut s = Sequencer::new(rate);
    s.play();
    let mut v = Vec::new();
    s.run_dry(p, frames, |h| v.push(h));
    v
}

/// `frames` frames of `p` rendered from the start in blocks of `block`
/// frames (a block of 0: the sizes of `sizes` in turn).
pub fn render(p: &Project, rate: u32, frames: usize, sizes: &[usize]) -> Vec<i16> {
    let mut e = engine(rate);
    let mut s = Sequencer::new(rate);
    s.play();
    let mut out = vec![0i16; 2 * frames];
    let mut at = 0;
    let mut k = 0;
    while at < frames {
        let n = sizes[k % sizes.len()].min(frames - at);
        s.render_i16(p, &mut e, &mut out[2 * at..2 * (at + n)]);
        at += n;
        k += 1;
    }
    assert_eq!(s.dropped(), 0);
    out
}

/// FNV-1a over the samples.
pub fn fnv(s: &[i16]) -> u64 {
    let mut h = 0xcbf2_9ce4_8422_2325u64;
    for x in s {
        for b in x.to_le_bytes() {
            h = (h ^ u64::from(b)).wrapping_mul(0x100_0000_01b3);
        }
    }
    h
}

/// xorshift64*.
pub struct Rng(pub u64);

impl Rng {
    pub fn next(&mut self) -> u64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        self.0.wrapping_mul(0x2545_F491_4F6C_DD1D)
    }
    pub fn below(&mut self, n: u64) -> u64 {
        self.next() % n
    }
}

/// A project with random contents everywhere (every field, every pattern
/// at a random length, steps past the lengths too, a song, a kit), and
/// then the SEQ-2 parts: synth steps, transposes, locks, kit B and the
/// tracks that play it, the synth tracks' sounds and mix, the scale and
/// the fill (from a second random stream, so the drum part is
/// [`random_drums`]'s).
pub fn random(seed: u64) -> Box<Project> {
    let mut p = random_drums(seed);
    let mut r = Rng((seed ^ 0x5E02) | 1);
    p.set_factory_kit_of(1, r.below(5) as usize);
    for t in 0..TRACKS {
        p.set_track_kit(t, u8::from(r.below(3) == 0));
    }
    for st in 0..SYNTH_TRACKS {
        p.set_preset(st, r.below(16) as usize);
        if r.below(2) == 0 {
            let cut = 100.0 + r.below(8000) as f32;
            p.edit_patch(st, |x| x.filter.cutoff = cut);
        }
        p.set_level(TRACKS + st, r.below(101) as u8);
        p.set_synth_sends(st, r.below(101) as u8, r.below(101) as u8);
        p.set_mute(TRACKS + st, r.below(5) == 0);
        p.set_solo(TRACKS + st, r.below(8) == 0);
    }
    p.set_drum_sends(r.below(101) as u8, r.below(101) as u8);
    p.set_scale(Scale::ALL[r.below(10) as usize], r.below(12) as u8);
    p.set_fill(if r.below(3) == 0 { None } else { Some(r.below(16) as usize) });
    for pat in 0..PATTERNS {
        if r.below(3) == 0 {
            continue;
        }
        for st in 0..SYNTH_TRACKS {
            for s in 0..MAX_STEPS {
                if r.below(3) == 0 {
                    let x = SynthStep::note(r.below(128) as u8, 1 + r.below(127) as u8)
                        .with_on(r.below(4) != 0)
                        .with_gate(r.below(16) as u8)
                        .with_chord(Chord::ALL[r.below(4) as usize])
                        .with_common(
                            Step::hit(1 + r.below(127) as u8)
                                .with_on(r.below(4) != 0)
                                .with_prob(1 + r.below(16) as u8)
                                .with_ratchet(1 + r.below(4) as u8)
                                .with_accent(r.below(3) == 0)
                                .with_micro(r.below(13) as u8),
                        );
                    p.set_synth_step(pat, st, s, x);
                }
            }
        }
        if r.below(3) == 0 {
            p.set_transpose(pat, (r.below(49) as i8) - 24);
        }
        for _ in 0..r.below(30) {
            let t = r.below(10) as usize;
            let params = LockParam::of_track(t);
            let param = params[r.below(3) as usize];
            p.set_lock(pat, r.below(64) as usize, t, param, r.below(256) as u8);
        }
    }
    p
}

/// SEQ-1's random project (drum tracks only): the same draws as SEQ-1's
/// tests made, so its saved files (`tests/data/seq1-random*.osq`) are
/// this project.
pub fn random_drums(seed: u64) -> Box<Project> {
    let mut r = Rng(seed | 1);
    let mut p = project();
    p.set_name(["Beat", "Груув", "ビート", "x"][r.below(4) as usize]);
    p.set_tempo(4000 + r.below(20001) as u16);
    p.set_swing(50 + r.below(26) as u8);
    p.set_factory_kit(r.below(5) as usize);
    p.seed = r.next() as u32;
    for t in 0..TRACKS {
        p.set_note(t, r.below(128) as u8);
        p.set_level(t, r.below(101) as u8);
        p.set_mute(t, r.below(4) == 0);
        p.set_solo(t, r.below(6) == 0);
    }
    for pat in 0..PATTERNS {
        if r.below(3) == 0 {
            continue;
        }
        for t in 0..TRACKS {
            for s in 0..MAX_STEPS {
                if r.below(3) == 0 {
                    let st = Step::hit(1 + r.below(127) as u8)
                        .with_on(r.below(4) != 0)
                        .with_prob(1 + r.below(16) as u8)
                        .with_ratchet(1 + r.below(4) as u8)
                        .with_accent(r.below(3) == 0)
                        .with_micro(r.below(13) as u8);
                    p.set_step(pat, t, s, st);
                }
            }
        }
        p.set_len(pat, 1 + r.below(64) as usize);
    }
    for _ in 0..r.below(65) {
        p.song.push(r.below(16) as u8, 1 + r.below(16) as u8);
    }
    p.set_mode(if r.below(2) == 0 { orpheus_seq::PlayMode::Pattern } else { orpheus_seq::PlayMode::Song });
    p.set_current(r.below(16) as usize);
    p
}

/// Saved bytes of `p`.
pub fn saved(p: &Project) -> Vec<u8> {
    let mut buf = vec![0u8; orpheus_seq::file::MAX_FILE];
    let n = orpheus_seq::save(p, &mut buf).expect("fits");
    buf.truncate(n);
    buf
}

/// `d` loaded.
pub fn loaded(d: &[u8]) -> Result<Box<Project>, orpheus_seq::FileError> {
    let mut p = project();
    orpheus_seq::load_into(&mut p, d)?;
    Ok(p)
}

/// Fields that survive a file (revision counters do not).
pub fn same(a: &Project, b: &Project) -> bool {
    a.same_as(b)
}

/// A groove on pattern 0: kick, snare with a ghost, hats with accents and
/// a ratchet, an open hat (choked by the next closed one), a delayed clap.
pub fn row(p: &mut Project) {
    use orpheus_seq::demo::row;
    row(p, 0, 0, 115, "X... ..x. ..x. ....");
    row(p, 0, 1, 105, ".... X... .... X..g");
    row(p, 0, 2, 80, ".... ...> .... ....");
    row(p, 0, 3, 90, "x-x- x-x- x-x- 3.x-");
    row(p, 0, 4, 85, ".... .... .... ..x.");
}
