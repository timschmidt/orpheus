//! Host benchmark: `cargo bench -p orpheus-tts`.
//!
//! Prints, best of 3: the renderer's cost per 16 kHz sample and per output
//! frame (48 and 44.1 kHz stereo, 16 kHz mono) on a paragraph of text at 1×
//! (the tokens are converted beforehand, so this is the synthesiser alone),
//! and the front end's cost per second of speech at 1× and 10×, and the
//! speech mixer's (TTS-3: 16 kHz speech resampled and added to stereo music, the
//! music lowered; alone at 16 kHz). The player's figures are estimated
//! from these in 04 §8.

use std::hint::black_box;
use std::time::Instant;

use orpheus_tts::mix::{Gains, Mixer, MusicMode, Source};
use orpheus_tts::{Config, Converter, Options, Speed, Synth, Token};

/// Speech samples for the mixer (a 16 kHz sine-ish ramp, endless).
struct Endless(u32);

impl Source for Endless {
    fn next(&mut self) -> Option<i16> {
        self.0 = self.0.wrapping_add(1);
        Some(((self.0 * 977) % 20_000) as i16 - 10_000)
    }
    fn sounding(&self) -> bool {
        true
    }
    fn stopped(&mut self) -> bool {
        false
    }
    fn drop_stopped(&mut self) {}
}

/// ns per output frame of the mixer at `rate` (stereo music), or alone.
fn mixer(rate: u32, alone: bool) -> f64 {
    let mut best = f64::MAX;
    let frames = 200_000usize;
    for _ in 0..3 {
        let mut m = Mixer::new(rate);
        let mut src = Endless(0);
        let g = Gains::of(1, MusicMode::Lower);
        let mut buf = vec![1234i16; 2 * 256];
        let t = Instant::now();
        let mut n = 0;
        while n < frames {
            if alone {
                for _ in 0..256 {
                    black_box(m.alone(g, &mut src));
                }
            } else {
                m.mix_i16(black_box(&mut buf), 2, g, &mut src);
            }
            n += 256;
        }
        best = best.min(t.elapsed().as_nanos() as f64 / n as f64);
    }
    best
}

const TEXT: &str = "The quick brown fox jumps over the lazy dog. Track three of twelve, four minutes and five \
    seconds. Would you like to resume where you stopped? It was the best of times, it was the worst of times; \
    it was the age of wisdom, it was the age of foolishness. Battery at forty percent (about two hours left).";

fn tokens(speed: Speed) -> Vec<Token> {
    let mut v = Vec::new();
    Converter::new(Options::default(), speed).convert(TEXT.as_bytes(), |_, t| v.push(t));
    v
}

/// (ns per 16 kHz sample, ns per output frame, seconds of audio).
fn synth(rate: u32, channels: u8, toks: &[Token]) -> (f64, f64, f64) {
    let mut best = f64::MAX;
    let mut frames = 0usize;
    for _ in 0..3 {
        let mut s = Synth::new(Config { rate, channels, ..Config::default() });
        let mut buf = vec![0i16; 1024 * usize::from(channels)];
        let mut n = 0;
        let mut it = toks.iter();
        let t = Instant::now();
        loop {
            while s.space() > 0 {
                match it.next() {
                    Some(tk) => {
                        s.push(*tk);
                    }
                    None => {
                        s.finish();
                        break;
                    }
                }
            }
            let got = s.render(black_box(&mut buf), |_, _| {});
            n += got;
            if s.is_idle() {
                break;
            }
        }
        best = best.min(t.elapsed().as_nanos() as f64);
        frames = n;
    }
    let secs = frames as f64 / f64::from(rate);
    (best / (secs * 16_000.0), best / frames as f64, secs)
}

fn front(speed: Speed) -> (f64, f64) {
    let mut best = f64::MAX;
    let mut ms = 0u64;
    for _ in 0..3 {
        let t = Instant::now();
        let mut total = 0u64;
        for _ in 0..20 {
            Converter::new(Options::default(), speed).convert(black_box(TEXT.as_bytes()), |_, tk| match tk {
                Token::Phone(s) => total += u64::from(s.ms),
                Token::Break(p) => total += u64::from(p.ms),
                Token::Word(_) => {}
            });
        }
        best = best.min(t.elapsed().as_nanos() as f64 / 20.0);
        ms = total / 20;
    }
    let secs = ms as f64 / 1000.0;
    (best / secs, secs)
}

fn main() {
    let toks = tokens(Speed::NORMAL);
    println!("synthesiser (text at 1x)                ns/16k-sample  ns/out-frame  audio s");
    for (name, rate, ch) in
        [("16 kHz mono", 16_000, 1), ("44.1 kHz stereo", 44_100, 2), ("48 kHz stereo", 48_000, 2)]
    {
        let (a, b, s) = synth(rate, ch, &toks);
        println!("  {name:<38} {a:10.1} {b:13.1} {s:8.2}");
    }
    println!("\nmixer (TTS-3)                           ns/out-frame");
    for (name, rate, alone) in [
        ("alone 16 kHz", 16_000, true),
        ("over 44.1 kHz stereo", 44_100, false),
        ("over 48 kHz stereo", 48_000, false),
    ] {
        println!("  {name:<38} {:10.1}", mixer(rate, alone));
    }
    println!("\nfront end (converter)                   ns per second of speech  speech s");
    for (name, q) in [("1x", 4u8), ("4x", 16), ("10x", 40)] {
        let (ns, s) = front(Speed::from_quarters(q));
        println!("  {name:<38} {ns:14.0} {s:15.2}");
    }
}
