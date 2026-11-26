//! Render the demo beat (`orpheus_seq::demo::beat`, a song of four
//! patterns made through the model) as a WAV for listening, and save its
//! project file: `cargo run --release -p orpheus-seq --example render --
//! <dir> [rate]` writes `<dir>/night-bus.wav` and `<dir>/SEQ01.OSQ`.

use std::path::Path;
use std::process::ExitCode;

use orpheus_seq::{Project, Sequencer, demo, file};
use orpheus_synth::Engine;

fn wav(path: &Path, rate: u32, samples: &[i16]) -> std::io::Result<()> {
    let mut b = Vec::with_capacity(44 + samples.len() * 2);
    let data = (samples.len() * 2) as u32;
    b.extend_from_slice(b"RIFF");
    b.extend_from_slice(&(36 + data).to_le_bytes());
    b.extend_from_slice(b"WAVEfmt ");
    b.extend_from_slice(&16u32.to_le_bytes());
    b.extend_from_slice(&1u16.to_le_bytes());
    b.extend_from_slice(&2u16.to_le_bytes());
    b.extend_from_slice(&rate.to_le_bytes());
    b.extend_from_slice(&(rate * 4).to_le_bytes());
    b.extend_from_slice(&4u16.to_le_bytes());
    b.extend_from_slice(&16u16.to_le_bytes());
    b.extend_from_slice(b"data");
    b.extend_from_slice(&data.to_le_bytes());
    for s in samples {
        b.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(path, b)
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().collect();
    let Some(dir) = args.get(1) else {
        eprintln!("usage: render <dir> [rate]");
        return ExitCode::FAILURE;
    };
    let rate: u32 = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(48_000);
    let dir = Path::new(dir);
    std::fs::create_dir_all(dir).expect("dir");
    let mut p = Box::new(Project::NEW);
    demo::beat(&mut p);
    let steps = demo::song_steps(&p) as u64;
    // A step is 15 / BPM s; the song once, then 2 s for the tails.
    let frames = (steps * 15 * 100 * u64::from(rate) / u64::from(p.tempo()) + 2 * u64::from(rate)) as usize;
    let mut e = Box::new(Engine::new(rate, 1));
    let mut s = Sequencer::new(rate);
    s.play();
    let mut out = vec![0i16; 2 * frames];
    let song_end = frames - 2 * rate as usize;
    for (k, block) in out.chunks_mut(2 * 1024).enumerate() {
        if k * 1024 >= song_end && s.playing() {
            s.stop();
        }
        s.render_i16(&p, &mut e, block);
    }
    let peak = out.iter().map(|x| x.unsigned_abs()).max().unwrap_or(0);
    let wav_path = dir.join("night-bus.wav");
    wav(&wav_path, rate, &out).expect("write wav");
    let mut buf = vec![0u8; file::MAX_FILE];
    let n = file::save(&p, &mut buf).expect("save");
    std::fs::write(dir.join("SEQ01.OSQ"), &buf[..n]).expect("write project");
    println!(
        "{}: \"{}\", {} steps at {} BPM, swing {} %, kit {}, {:.1} s, {} hits, peak {:.1} dBFS; project file {} B",
        wav_path.display(),
        p.name(),
        steps,
        f64::from(p.tempo()) / 100.0,
        p.swing(),
        p.kit_name(),
        frames as f64 / f64::from(rate),
        s.hits(),
        20.0 * (f64::from(peak.max(1)) / 32768.0).log10(),
        n
    );
    ExitCode::SUCCESS
}
