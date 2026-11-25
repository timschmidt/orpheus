//! Text to a WAV file on the host:
//!
//! ```text
//! cargo run -p orpheus-tts --example say -- [options] "text"
//!   -o FILE      output (default say.wav)
//!   -s SPEED     0.25 … 10 (default 1)
//!   -v VOICE     neutral | low | bright (default neutral)
//!   -r RATE      output rate in Hz (default 48000)
//!   -m           mono (default stereo)
//!   -p SEMI      pitch shift in semitones
//!   --spell      spell everything
//!   --words      print the word events with their times
//! ```
//!
//! Text "-" reads standard input.

use std::io::Read;

use orpheus_tts::{Config, Event, Options, Speaker, Speed, Voice};

fn main() {
    let mut args = std::env::args().skip(1);
    let mut out = String::from("say.wav");
    let mut speed = Speed::NORMAL;
    let mut voice = Voice::NEUTRAL;
    let mut rate = 48_000u32;
    let mut channels = 2u8;
    let mut pitch = 0.0f32;
    let mut opts = Options::default();
    let mut words = false;
    let mut text = String::new();
    while let Some(a) = args.next() {
        let mut val = || args.next().unwrap_or_else(|| panic!("{a} needs a value"));
        match a.as_str() {
            "-o" => out = val(),
            "-s" => speed = Speed::from_f32(val().parse().expect("speed")),
            "-v" => {
                voice = match val().to_ascii_lowercase().as_str() {
                    "low" => Voice::LOW,
                    "bright" => Voice::BRIGHT,
                    _ => Voice::NEUTRAL,
                }
            }
            "-r" => rate = val().parse().expect("rate"),
            "-p" => pitch = val().parse().expect("semitones"),
            "-m" => channels = 1,
            "--spell" => opts.spell = true,
            "--words" => words = true,
            "-" => {
                std::io::stdin().read_to_string(&mut text).expect("stdin");
            }
            _ => {
                if !text.is_empty() {
                    text.push(' ');
                }
                text.push_str(&a);
            }
        }
    }
    if text.is_empty() {
        text = "Hello. This is Orpheus speaking.".into();
    }
    let cfg = Config { rate, channels, voice: voice.with_pitch(pitch), speed, seed: 0x5EED };
    let mut sp = Speaker::new(cfg, opts);
    let mut pcm: Vec<i16> = Vec::new();
    let mut block = vec![0i16; 1024 * usize::from(channels)];
    let mut rest = text.as_bytes();
    let mut marks = Vec::new();
    loop {
        let n = sp.feed(rest);
        rest = &rest[n..];
        if rest.is_empty() {
            sp.finish();
        }
        let base = pcm.len() / usize::from(channels);
        let got = sp.render(&mut block, |k, e| marks.push((base + k, e)));
        pcm.extend_from_slice(&block[..got * usize::from(channels)]);
        if sp.is_idle() {
            break;
        }
    }
    if words {
        for (k, e) in &marks {
            if let Event::Word { pos, .. } = e {
                let end =
                    text[*pos as usize..].find(char::is_whitespace).map_or(text.len(), |i| *pos as usize + i);
                println!("{:8.3} s  {}", *k as f64 / f64::from(rate), &text[*pos as usize..end]);
            }
        }
    }
    write_wav(&out, rate, channels, &pcm).expect("write");
    let secs = pcm.len() as f64 / f64::from(channels) / f64::from(rate);
    eprintln!(
        "{out}: {secs:.2} s, {} Hz, {} ch, voice {}, speed {}×",
        rate,
        channels,
        voice.name,
        speed.as_f32()
    );
}

fn write_wav(path: &str, rate: u32, channels: u8, pcm: &[i16]) -> std::io::Result<()> {
    let data = (pcm.len() * 2) as u32;
    let mut b = Vec::with_capacity(44 + pcm.len() * 2);
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&u16::from(channels).to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 2 * u32::from(channels)).to_le_bytes());
    b.extend_from_slice(&(2 * u16::from(channels)).to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in pcm {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b)
}
