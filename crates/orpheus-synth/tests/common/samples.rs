//! Test samples (WAV files made here, from our own synthesis) and the
//! sample-playback scenarios, shared by `tests/samples.rs` (golden hashes)
//! and the `render` example (WAVs for listening).

#![allow(dead_code)]

use orpheus_synth::demo::{Score, drive};
use orpheus_synth::sample::{SampleCache, SliceSource, Want};
use orpheus_synth::{
    Adsr, Engine, EventKind, FilterMode, FilterParams, Kit, KitEntry, Param, Patch, ReverbParams, SampleLoop,
    SampleParams,
};

/// A WAV file: `channels` × `bits` PCM (8 unsigned, 16, 24) of `frames`
/// (each a slice of channel values in −1…1), with an optional `smpl` chunk
/// (unity note, loop start, loop end inclusive) and a `LIST` chunk before
/// the data (parsers must skip it).
pub fn wav(
    rate: u32,
    channels: u16,
    bits: u16,
    frames: &[[f32; 2]],
    smpl: Option<(u32, u32, u32)>,
) -> Vec<u8> {
    let bps = u32::from(bits / 8);
    let align = u32::from(channels) * bps;
    let mut data = Vec::new();
    for f in frames {
        for x in &f[..usize::from(channels)] {
            let x = x.clamp(-1.0, 1.0);
            match bits {
                8 => data.push(((x * 127.0).round() as i32 + 128) as u8),
                16 => data.extend_from_slice(&((x * 32767.0).round() as i16).to_le_bytes()),
                24 => data.extend_from_slice(&((x * 8_388_607.0).round() as i32).to_le_bytes()[..3]),
                _ => panic!("bits"),
            }
        }
    }
    let mut v = Vec::new();
    v.extend_from_slice(b"RIFF\0\0\0\0WAVE");
    v.extend_from_slice(b"fmt ");
    v.extend_from_slice(&16u32.to_le_bytes());
    v.extend_from_slice(&1u16.to_le_bytes());
    v.extend_from_slice(&channels.to_le_bytes());
    v.extend_from_slice(&rate.to_le_bytes());
    v.extend_from_slice(&(rate * align).to_le_bytes());
    v.extend_from_slice(&(align as u16).to_le_bytes());
    v.extend_from_slice(&bits.to_le_bytes());
    v.extend_from_slice(b"LIST");
    v.extend_from_slice(&5u32.to_le_bytes());
    v.extend_from_slice(b"INFOx\0"); // odd size: one pad byte
    if let Some((unity, start, end)) = smpl {
        v.extend_from_slice(b"smpl");
        v.extend_from_slice(&60u32.to_le_bytes());
        for x in [0u32, 0, 1_000_000_000 / rate, unity, 0, 0, 0, 1, 0, 0, 0, start, end, 0, 0] {
            v.extend_from_slice(&x.to_le_bytes());
        }
    }
    v.extend_from_slice(b"data");
    v.extend_from_slice(&(data.len() as u32).to_le_bytes());
    v.extend_from_slice(&data);
    if data.len() % 2 == 1 {
        v.push(0);
    }
    let riff = (v.len() - 8) as u32;
    v[4..8].copy_from_slice(&riff.to_le_bytes());
    v
}

/// A plucked-string tone at 220 Hz (A3), 0.6 s, 16-bit mono at 44.1 kHz:
/// six harmonics, bright at first, steady from 0.35 s, a short noise
/// attack. Loop: the last
/// 0.2 s, on whole periods (unity note 57).
pub fn pluck() -> Vec<u8> {
    let rate = 44_100;
    let n = (0.6 * rate as f32) as usize;
    let mut seed = 0x1234_5678u32;
    let frames: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let mut x = 0.0;
            for h in 1..=6 {
                // A bright attack settling by 0.35 s to a steady tone, so the
                // loop (the last 0.2 s: 44 whole periods) has no seam.
                let fall = (1.0 - t / 0.35).max(0.0);
                let a = 0.5 / h as f32 * (0.35 + 0.65 * fall * fall)
                    / (1.0 + 0.3 * (h as f32 - 1.0) * (1.0 - fall));
                x += a * (std::f32::consts::TAU * 220.0 * h as f32 * t).sin();
            }
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = (seed >> 8) as f32 / 8_388_608.0 - 1.0;
            [x + 0.3 * noise * (-t * 200.0).exp(); 2]
        })
        .collect();
    // 44_100 / 220 = 200.45 frames per period: 44 periods ≈ 8820 frames.
    let end = n as u32 - 1;
    let start = end + 1 - 8820;
    wav(rate, 1, 16, &frames, Some((57, start, end)))
}

/// A drum-like hit, 24-bit stereo at 48 kHz (0.25 s): a falling sine with
/// a noise burst, the noise wider on the right.
pub fn hit24() -> Vec<u8> {
    let rate = 48_000;
    let n = rate / 4;
    let mut seed = 0x0BAD_5EEDu32;
    let mut phase = 0.0f32;
    let frames: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            phase += (90.0 + 200.0 * (-t * 40.0).exp()) / rate as f32;
            let body = (std::f32::consts::TAU * phase).sin() * (-t * 14.0).exp();
            seed ^= seed << 13;
            seed ^= seed >> 17;
            seed ^= seed << 5;
            let noise = ((seed >> 8) as f32 / 8_388_608.0 - 1.0) * (-t * 60.0).exp();
            [0.8 * body + 0.2 * noise, 0.8 * body + 0.35 * noise]
        })
        .collect();
    wav(rate, 2, 24, &frames, None)
}

/// A short 8-bit mono blip at 22.05 kHz (0.1 s): a square chirp.
pub fn blip8() -> Vec<u8> {
    let rate = 22_050;
    let n = rate / 10;
    let frames: Vec<[f32; 2]> = (0..n)
        .map(|i| {
            let t = i as f32 / rate as f32;
            let f = 1200.0 - 6000.0 * t;
            let s = if (f * t).fract() < 0.5 { 0.6 } else { -0.6 };
            [s * (1.0 - t * 10.0); 2]
        })
        .collect();
    wav(rate, 1, 8, &frames, None)
}

/// The three sample files, as file numbers 0–2 of a [`SliceSource`].
pub fn files() -> [Vec<u8>; 3] {
    [pluck(), hit24(), blip8()]
}

/// A sample-playback scenario.
pub struct SampleScenario {
    /// Name (the key in `tests/goldens-samples.txt`).
    pub name: &'static str,
    /// Seconds.
    pub seconds: f32,
    /// Cache pages pinned per sample at load (`u32::MAX`: all).
    pub head: u32,
    /// Cache pages.
    pub pages: usize,
    set: fn(&mut Engine, &mut Score),
}

/// Melody on the looped pluck (its root is the file's unity note), a
/// sustained note crossing the loop many times.
fn melody(e: &mut Engine, s: &mut Score) {
    let mut p = Patch::sample(SampleParams::new(0));
    p.filter =
        FilterParams { mode: FilterMode::LowPass, cutoff: 5000.0, resonance: 0.1, ..FilterParams::OFF };
    p.amp_env = Adsr::new(0.002, 0.0, 1.0, 0.25);
    e.set_patch(0, &p);
    e.set_reverb(&ReverbParams { decay: 1.5, damp: 0.4, level: 0.5 });
    s.param(0.0, 0, Param::ReverbSend, 0.25);
    for (i, n) in [57u8, 60, 64, 69, 64, 60].iter().enumerate() {
        s.note(0.25 * i as f32, 0.22, 0, *n, 100);
    }
    s.note(1.5, 1.6, 0, 45, 110);
    s.note(1.5, 1.6, 0, 52, 90);
}

/// A kit of sample hits and drum models: the 24-bit stereo hit as a kick
/// and (pitched up, low-passed) a tom, the 8-bit blip as a percussion
/// sound in a choke group with itself retuned.
fn sample_kit(e: &mut Engine, s: &mut Score) {
    let kit = Kit::new(&[
        KitEntry::sample(36, 1),
        KitEntry::sample(45, 1).tune(28).colour(50, 0).decay(20),
        KitEntry::sample(76, 2).choke(3).level(60).pan(-40),
        KitEntry::sample(77, 2).tune(-20).choke(3).level(60).pan(40),
        KitEntry::drum(42, orpheus_synth::DrumKind::ClosedHat).choke(1).level(55),
        KitEntry::drum(38, orpheus_synth::DrumKind::Snare),
    ]);
    e.set_kit(1, &kit);
    e.set_patch(9, &Patch::kit(1));
    let step = 60.0 / 120.0 / 4.0;
    for i in 0..16u8 {
        let t = f32::from(i) * step;
        if i % 4 == 0 {
            s.note(t, 0.05, 9, 36, 120);
        }
        if i % 8 == 4 {
            s.note(t, 0.05, 9, 38, 110);
        }
        if i % 2 == 0 {
            s.note(t, 0.05, 9, 42, 80);
        }
        if i % 3 == 1 {
            s.note(t, 0.05, 9, if i % 2 == 0 { 76 } else { 77 }, 100);
        }
    }
    s.note(1.75, 0.05, 9, 45, 110);
    s.note(1.875, 0.05, 9, 45, 100);
}

/// The blip one-shot played as a melodic patch at several pitches, with a
/// note-off ignored (one-shot), at the engine's 44.1 kHz.
fn blips(e: &mut Engine, s: &mut Score) {
    let p = Patch::sample(SampleParams {
        looping: SampleLoop::Off,
        one_shot: true,
        root: 72,
        ..SampleParams::new(2)
    });
    e.set_patch(3, &p);
    for (i, n) in [72u8, 79, 84, 67, 60].iter().enumerate() {
        s.note(0.12 * i as f32, 0.01, 3, *n, 100);
    }
    s.push(0.7, EventKind::Param { part: 3, param: Param::Bend, value: 1.0 });
    s.note(0.7, 0.3, 3, 72, 100);
}

/// The scenarios.
pub const SCENARIOS: [SampleScenario; 4] = [
    SampleScenario { name: "sample/melody", seconds: 3.5, head: u32::MAX, pages: 256, set: melody },
    SampleScenario { name: "sample/kit", seconds: 2.4, head: u32::MAX, pages: 256, set: sample_kit },
    SampleScenario { name: "sample/blips", seconds: 1.2, head: u32::MAX, pages: 256, set: blips },
    // The melody streamed through a small cache (4 head pages per sample,
    // 48 pages = 24 KiB): the same samples as the pinned render.
    SampleScenario { name: "sample/melody-streamed", seconds: 3.5, head: 4, pages: 48, set: melody },
];

impl SampleScenario {
    /// Rate of the engine.
    pub fn rate(&self) -> u32 {
        if self.name == "sample/blips" { 44_100 } else { 48_000 }
    }

    /// Frames.
    pub fn frames(&self) -> usize {
        (self.seconds * self.rate() as f32) as usize
    }

    /// Render in blocks of `block` frames, servicing the cache before each
    /// block; returns the samples and the engine's miss count.
    pub fn render(&self, block: usize) -> (Vec<i16>, u32) {
        let files = files();
        let refs: Vec<&[u8]> = files.iter().map(Vec::as_slice).collect();
        let mut src = SliceSource::new(&refs);
        let mut mem = vec![0i16; self.pages * 256];
        let mut cache = SampleCache::new(&mut mem);
        for i in 0..3u8 {
            cache.load(&mut src, i, u16::from(i), self.head).expect("load");
        }
        let mut e = Engine::new(self.rate(), 1);
        let mut s = Score::new(self.rate());
        (self.set)(&mut e, &mut s);
        let frames = self.frames();
        let mut out = vec![0i16; 2 * frames];
        let mut wants = [Want::default(); 20];
        drive(&mut e, &s, frames, block, |e, r, ev| {
            let n = e.sample_wants(&mut wants);
            cache.service(&mut src, &wants[..n], 64).expect("service");
            e.render_i16_with(&mut out[2 * r.start..2 * r.end], ev, &cache);
        });
        let misses = e.sample_misses();
        (out, misses)
    }
}
