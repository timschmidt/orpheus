//! Helpers shared by the integration tests.

#![allow(dead_code)]

use orpheus_seq::model::{MAX_STEPS, PATTERNS, TRACKS};
use orpheus_seq::{Hit, Project, Sequencer, Step};
use orpheus_synth::Engine;

/// A boxed project (25 KB: keep it off the test thread's stack).
pub fn project() -> Box<Project> {
    Box::new(Project::NEW)
}

/// A boxed engine.
pub fn engine(rate: u32) -> Box<Engine> {
    Box::new(Engine::new(rate, 1))
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
/// at a random length, steps past the lengths too, a song, a kit).
pub fn random(seed: u64) -> Box<Project> {
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
