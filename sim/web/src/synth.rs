//! The synthesiser of the browser card's demo content (WEB-2). The music is
//! made here (plucked strings, filtered noise, FM bells, a drum machine,
//! test tones) and so is the night-sky picture: no third-party material.
//!
//! Plain `std` only: `cargo xtask wasm --demo` includes this file to write
//! the sources that `ffmpeg` encodes into `sim/web/demo/` (committed); the
//! browser build synthesises the test tones at run time.

/// One compressed demo track (committed, embedded in the module).
pub struct Track {
    /// Path on the card.
    pub path: &'static str,
    pub title: &'static str,
    /// File in `sim/web/demo/`.
    pub file: &'static str,
    /// Seconds, sample rate, channels of the synthesised source.
    pub secs: f32,
    pub rate: u32,
    pub channels: u8,
    /// `ffmpeg` encoder options.
    pub encode: &'static [&'static str],
    pub synth: fn(u32, f32) -> Stereo,
}

pub const ARTIST: &str = "Orpheus demo";
pub const ALBUM: &str = "Synthesised";

pub const TRACKS: [Track; 4] = [
    Track {
        path: "/Music/Orpheus demo/01 Lyre Study.mp3",
        title: "Lyre Study",
        file: "lyre.mp3",
        secs: 30.0,
        rate: 44_100,
        channels: 2,
        encode: &["-c:a", "libmp3lame", "-b:a", "96k", "-id3v2_version", "3", "-write_xing", "1"],
        synth: lyre,
    },
    Track {
        path: "/Music/Orpheus demo/02 Tide.ogg",
        title: "Tide",
        file: "tide.ogg",
        secs: 30.0,
        rate: 44_100,
        channels: 2,
        encode: &["-c:a", "libvorbis", "-q:a", "1"],
        synth: tide,
    },
    Track {
        path: "/Music/Orpheus demo/03 Bells at Dusk.opus",
        title: "Bells at Dusk",
        file: "bells.opus",
        secs: 30.0,
        rate: 48_000,
        channels: 2,
        encode: &["-c:a", "libopus", "-b:a", "48k"],
        synth: bells,
    },
    Track {
        path: "/Music/Orpheus demo/04 Pulse.flac",
        title: "Pulse",
        file: "pulse.flac",
        secs: 16.0,
        rate: 22_050,
        channels: 1,
        encode: &["-c:a", "flac", "-compression_level", "8"],
        synth: pulse,
    },
];

// ------------------------------------------------------------ synthesis

/// Stereo samples in −1…1.
pub struct Stereo {
    pub rate: u32,
    pub l: Vec<f32>,
    pub r: Vec<f32>,
}

impl Stereo {
    fn new(rate: u32, secs: f32) -> Stereo {
        let n = (rate as f32 * secs) as usize;
        Stereo { rate, l: vec![0.0; n], r: vec![0.0; n] }
    }

    fn len(&self) -> usize {
        self.l.len()
    }

    /// Add `v` at sample `i` panned by `pan` (−1 left … 1 right).
    fn add(&mut self, i: usize, v: f32, pan: f32) {
        if i < self.l.len() {
            self.l[i] += v * (0.5 - pan * 0.5).sqrt();
            self.r[i] += v * (0.5 + pan * 0.5).sqrt();
        }
    }

    /// Scale to `peak`, with 20 ms fades at both ends.
    fn finish(mut self, peak: f32) -> Stereo {
        let m = self.l.iter().chain(&self.r).fold(0f32, |m, x| m.max(x.abs()));
        let g = if m > 0.0 { peak / m } else { 1.0 };
        let fade = (self.rate / 50) as usize;
        let n = self.len();
        for i in 0..n {
            let f = (i.min(n - 1 - i) as f32 / fade as f32).min(1.0);
            self.l[i] *= g * f;
            self.r[i] *= g * f;
        }
        self
    }

    /// A simple stereo echo (two feedback delays) as room.
    fn echo(&mut self, dl: f32, dr: f32, fb: f32, mix: f32) {
        let (a, b) = ((dl * self.rate as f32) as usize, (dr * self.rate as f32) as usize);
        let (mut el, mut er) = (vec![0f32; self.len()], vec![0f32; self.len()]);
        for i in 0..self.len() {
            let pl = if i >= a { el[i - a] } else { 0.0 };
            let pr = if i >= b { er[i - b] } else { 0.0 };
            el[i] = self.r[i] + pr * fb;
            er[i] = self.l[i] + pl * fb;
        }
        for i in 0..self.len() {
            let wl = if i >= a { el[i - a] } else { 0.0 };
            let wr = if i >= b { er[i - b] } else { 0.0 };
            self.l[i] += wl * mix;
            self.r[i] += wr * mix;
        }
    }
}

/// Deterministic white noise (xorshift32).
struct Noise(u32);

impl Noise {
    fn next(&mut self) -> f32 {
        let mut x = self.0;
        x ^= x << 13;
        x ^= x >> 17;
        x ^= x << 5;
        self.0 = x;
        (x as f32 / u32::MAX as f32) * 2.0 - 1.0
    }
}

fn hz(midi: f32) -> f32 {
    440.0 * 2f32.powf((midi - 69.0) / 12.0)
}

const TAU: f32 = core::f32::consts::TAU;

/// A Karplus–Strong pluck of `secs` at `f` Hz into `out` from sample `at`.
fn pluck(out: &mut Stereo, at: usize, f: f32, secs: f32, amp: f32, pan: f32, noise: &mut Noise) {
    let len = ((out.rate as f32 / f) as usize).max(2);
    let mut buf: Vec<f32> = (0..len).map(|_| noise.next()).collect();
    // Soften the attack a little.
    for i in 1..len {
        buf[i] = 0.6 * buf[i] + 0.4 * buf[i - 1];
    }
    let n = (secs * out.rate as f32) as usize;
    let mut p = 0;
    for k in 0..n {
        let next = (p + 1) % len;
        let v = buf[p];
        buf[p] = 0.996 * 0.5 * (buf[p] + buf[next]);
        p = next;
        out.add(at + k, v * amp, pan);
    }
}

/// "Lyre Study": plucked arpeggios in A minor pentatonic over a slow bass.
pub fn lyre(rate: u32, secs: f32) -> Stereo {
    let mut s = Stereo::new(rate, secs);
    let mut n = Noise(0x0A11_CE55);
    // Am, F, C, G as pentatonic-friendly voicings (MIDI notes).
    let chords: [[f32; 4]; 4] = [
        [57.0, 60.0, 64.0, 69.0],
        [53.0, 57.0, 60.0, 65.0],
        [48.0, 55.0, 60.0, 64.0],
        [55.0, 59.0, 62.0, 67.0],
    ];
    let step = 60.0 / 100.0 / 2.0; // eighths at 100 bpm
    let pattern = [0usize, 1, 2, 3, 2, 1, 3, 2];
    let mut t = 0.4f32;
    let mut i = 0usize;
    while t < secs - 2.0 {
        let chord = chords[(i / 16) % 4];
        let note = chord[pattern[i % 8]] + if (i / 8) % 2 == 1 { 12.0 } else { 0.0 };
        let at = (t * rate as f32) as usize;
        let pan = if i % 2 == 0 { -0.45 } else { 0.45 };
        pluck(&mut s, at, hz(note), 1.6, 0.5, pan, &mut n);
        if i % 8 == 0 {
            pluck(&mut s, at, hz(chord[0] - 12.0), 3.0, 0.7, 0.0, &mut n);
        }
        t += step;
        i += 1;
    }
    // Last chord, let ring.
    let at = (t * rate as f32) as usize;
    for (k, note) in chords[0].iter().enumerate() {
        pluck(&mut s, at + k * rate as usize / 40, hz(*note), 1.9, 0.45, k as f32 * 0.3 - 0.45, &mut n);
    }
    s.echo(0.33, 0.41, 0.35, 0.3);
    s.finish(0.8)
}

/// "Tide": waves of filtered noise under a slow pad.
pub fn tide(rate: u32, secs: f32) -> Stereo {
    let mut s = Stereo::new(rate, secs);
    let mut n = Noise(0x7_1DE5);
    let (mut lp_l, mut lp_r) = (0f32, 0f32);
    let chords: [[f32; 3]; 4] =
        [[57.0, 60.0, 64.0], [53.0, 57.0, 60.0], [48.0, 52.0, 55.0], [55.0, 59.0, 62.0]];
    let dt = 1.0 / rate as f32;
    let bar = secs / 4.0;
    for i in 0..s.len() {
        let t = i as f32 * dt;
        let swell = 0.5 - 0.5 * (TAU * t / 7.5).cos();
        let cut = 0.01 + 0.08 * swell * swell;
        lp_l += cut * (n.next() - lp_l);
        lp_r += cut * (n.next() - lp_r);
        let surf = 2.2 * (0.25 + swell);
        let c = ((t / bar) as usize).min(3);
        let into = t - c as f32 * bar;
        let env = (into / 2.0).min(1.0) * ((bar - into) / 1.5).clamp(0.0, 1.0);
        let mut pad = 0.0;
        for (k, note) in chords[c].iter().enumerate() {
            let f = hz(*note);
            pad += (TAU * f * t).sin() + 0.5 * (TAU * f * 1.003 * t + k as f32).sin();
        }
        let pad = pad * 0.08 * env;
        s.l[i] = lp_l * surf + pad;
        s.r[i] = lp_r * surf + pad * 0.9;
    }
    s.finish(0.7)
}

/// An FM bell (carrier : modulator = 1 : 1.4).
fn bell(out: &mut Stereo, at: usize, f: f32, amp: f32, pan: f32) {
    let rate = out.rate as f32;
    let n = (4.0 * rate) as usize;
    for k in 0..n {
        let t = k as f32 / rate;
        let index = 5.0 * (-t * 1.5).exp();
        let env = (-t * 1.2).exp() * (t * 400.0).min(1.0);
        let v = (TAU * f * t + index * (TAU * f * 1.4 * t).sin()).sin();
        out.add(at + k, v * env * amp, pan);
    }
}

/// "Bells at Dusk": an FM-bell melody in D major pentatonic.
pub fn bells(rate: u32, secs: f32) -> Stereo {
    let mut s = Stereo::new(rate, secs);
    let scale = [62.0, 64.0, 66.0, 69.0, 71.0, 74.0, 76.0, 78.0];
    let tune = [0usize, 2, 4, 3, 5, 4, 2, 1, 0, 3, 2, 4, 6, 5, 7, 4, 3, 2, 0, 1, 2, 4, 3, 0];
    let mut t = 0.3f32;
    for (i, &k) in tune.iter().enumerate() {
        if t > secs - 4.0 {
            break;
        }
        let at = (t * rate as f32) as usize;
        bell(&mut s, at, hz(scale[k]), 0.5, (i as f32 * 0.7).sin() * 0.6);
        if i % 4 == 0 {
            bell(&mut s, at, hz(scale[k] - 24.0), 0.35, 0.0);
        }
        t += if i % 4 == 3 { 1.5 } else { 0.95 };
    }
    s.echo(0.45, 0.6, 0.4, 0.35);
    s.finish(0.8)
}

/// "Pulse": a small drum machine with a square-wave bass (mono).
pub fn pulse(rate: u32, secs: f32) -> Stereo {
    let mut s = Stereo::new(rate, secs);
    let mut n = Noise(0x9_0153);
    let beat = 0.5f32; // 120 bpm
    let r = rate as f32;
    let roots = [45.0, 41.0, 48.0, 43.0];
    let mut prev = 0f32;
    let mut lp = 0f32;
    for i in 0..s.len() {
        let t = i as f32 / r;
        let in_beat = t % beat;
        // Kick on every beat: a falling sine.
        let kf = 50.0 + 100.0 * (-in_beat * 30.0).exp();
        let kick = (TAU * kf * in_beat).sin() * (-in_beat * 9.0).exp();
        // Hat on the off-beats: differentiated noise.
        let off = (t + beat / 2.0) % beat;
        let x = n.next();
        let hat = (x - prev) * 0.5 * (-off * 60.0).exp();
        prev = x;
        // Bass: eighth notes of the bar's root, low-passed square.
        let bar = ((t / (beat * 4.0)) as usize) % 4;
        let eighth = t % (beat / 2.0);
        let f = hz(roots[bar] - 12.0 + if (t / (beat / 2.0)) as usize % 4 == 3 { 12.0 } else { 0.0 });
        let sq = if (t * f).fract() < 0.5 { 1.0 } else { -1.0 };
        lp += 0.08 * (sq - lp);
        let bass = lp * (-eighth * 5.0).exp();
        let v = kick * 0.9 + hat * 0.5 + bass * 0.5;
        s.l[i] = v;
        s.r[i] = v;
    }
    s.finish(0.85)
}

/// A logarithmic sine sweep 20 Hz – 20 kHz (mono).
pub fn sweep(rate: u32, secs: f32) -> Stereo {
    let mut s = Stereo::new(rate, secs);
    let (f0, f1) = (20f32, 20_000f32.min(rate as f32 * 0.45));
    let k = (f1 / f0).ln();
    let mut ph = 0f32;
    for i in 0..s.len() {
        let t = i as f32 / rate as f32;
        let f = f0 * (k * t / secs).exp();
        ph = (ph + TAU * f / rate as f32) % TAU;
        let v = ph.sin() * 0.5;
        s.l[i] = v;
        s.r[i] = v;
    }
    s.finish(0.5)
}

/// 1 kHz beeps: left, right, left … (one per second).
pub fn left_right(rate: u32, secs: f32) -> Stereo {
    let mut s = Stereo::new(rate, secs);
    for i in 0..s.len() {
        let t = i as f32 / rate as f32;
        let in_sec = t.fract();
        if in_sec < 0.5 {
            let env = (in_sec * 200.0).min(1.0) * ((0.5 - in_sec) * 200.0).min(1.0);
            let v = (TAU * 1000.0 * t).sin() * 0.5 * env;
            if (t as u32) % 2 == 0 {
                s.l[i] = v;
            } else {
                s.r[i] = v;
            }
        }
    }
    s
}

/// 16-bit PCM WAV of `s` with `channels` (1 = left only).
#[must_use]
pub fn wav(s: &Stereo, channels: u16) -> Vec<u8> {
    let frames = s.len() as u32;
    let data_len = frames * u32::from(channels) * 2;
    let mut v = Vec::with_capacity(44 + data_len as usize);
    v.extend(b"RIFF");
    v.extend((36 + data_len).to_le_bytes());
    v.extend(b"WAVEfmt ");
    v.extend(16u32.to_le_bytes());
    v.extend(1u16.to_le_bytes());
    v.extend(channels.to_le_bytes());
    v.extend(s.rate.to_le_bytes());
    v.extend((s.rate * u32::from(channels) * 2).to_le_bytes());
    v.extend((channels * 2).to_le_bytes());
    v.extend(16u16.to_le_bytes());
    v.extend(b"data");
    v.extend(data_len.to_le_bytes());
    let q = |x: f32| ((x.clamp(-1.0, 1.0) * 32767.0).round() as i16).to_le_bytes();
    for i in 0..s.len() {
        v.extend(q(s.l[i]));
        if channels == 2 {
            v.extend(q(s.r[i]));
        }
    }
    v
}

/// Size of the night-sky picture.
pub const LYRA_SIZE: (u32, u32) = (320, 200);

/// A night sky over hills with the constellation Lyra (Vega the brightest
/// star), RGB rows.
#[must_use]
pub fn lyra(w: u32, h: u32) -> Vec<[u8; 3]> {
    // Lyra: Vega, ε, ζ, δ, γ, β (positions as fractions of the picture).
    let stars: [(f32, f32, f32); 6] = [
        (0.42, 0.16, 3.2),
        (0.48, 0.13, 1.4),
        (0.47, 0.25, 1.6),
        (0.55, 0.30, 1.5),
        (0.57, 0.46, 1.9),
        (0.48, 0.43, 1.9),
    ];
    let lines = [(0, 2), (2, 3), (3, 4), (4, 5), (5, 2), (0, 1), (1, 2)];
    let mut n = Noise(0x1F2A_77E5);
    let mut px = Vec::with_capacity((w * h) as usize);
    let (wf, hf) = (w as f32, h as f32);
    for y in 0..h {
        for x in 0..w {
            let (fx, fy) = (x as f32 / wf, y as f32 / hf);
            let sky = [8.0 + 30.0 * fy, 12.0 + 40.0 * fy, 40.0 + 70.0 * fy];
            let mut c = sky;
            // Faint background stars.
            if n.next() > 0.996 && fy < 0.75 {
                let b = 120.0 + 100.0 * n.next().abs();
                c = [b, b, b + 20.0];
            }
            // Constellation lines.
            for &(a, b) in &lines {
                let (ax, ay, _) = stars[a];
                let (bx, by, _) = stars[b];
                let (dx, dy) = ((bx - ax) * wf, (by - ay) * hf);
                let (px_, py_) = ((fx - ax) * wf, (fy - ay) * hf);
                let t = ((px_ * dx + py_ * dy) / (dx * dx + dy * dy)).clamp(0.0, 1.0);
                let d = ((px_ - t * dx).powi(2) + (py_ - t * dy).powi(2)).sqrt();
                if d < 0.6 {
                    c = [c[0] + 40.0, c[1] + 60.0, c[2] + 90.0];
                }
            }
            // The stars, with a soft glow.
            for &(sx, sy, r) in &stars {
                let d = (((fx - sx) * wf).powi(2) + ((fy - sy) * hf).powi(2)).sqrt();
                let g = (1.0 - d / (r * 3.0)).max(0.0);
                c = [c[0] + 255.0 * g * g, c[1] + 250.0 * g * g, c[2] + 235.0 * g * g];
            }
            // Hills.
            let ridge = 0.78 + 0.06 * (fx * 6.0).sin() + 0.03 * (fx * 17.0 + 1.0).cos();
            if fy > ridge {
                let k = (fy - ridge) * 2.0;
                c = [6.0 - k, 10.0 + 4.0 * k, 12.0 - k];
            }
            px.push(c.map(|v| v.clamp(0.0, 255.0) as u8));
        }
    }
    px
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lyra_has_bright_stars_and_dark_hills() {
        let (w, h) = (160, 100);
        let p = lyra(w, h);
        assert_eq!(p.len(), (w * h) as usize);
        assert!(p.iter().any(|c| c[0] > 250 && c[1] > 250));
        assert!(p[(w * (h - 1)) as usize][2] < 20);
    }

    #[test]
    fn synths_are_deterministic_and_in_range() {
        for t in &TRACKS {
            let a = (t.synth)(8000, 8.0);
            let b = (t.synth)(8000, 8.0);
            assert_eq!(a.l, b.l, "{}", t.title);
            assert!(a.l.iter().chain(&a.r).all(|x| x.is_finite() && x.abs() <= 1.0), "{}", t.title);
            assert!(a.l.iter().any(|x| x.abs() > 0.1), "{} is silent", t.title);
        }
    }

    #[test]
    fn wav_header() {
        let w = wav(&left_right(8000, 2.0), 2);
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(w.len(), 44 + 2 * 8000 * 4);
    }
}
