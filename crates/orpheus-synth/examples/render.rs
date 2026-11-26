//! Render the golden scenarios to WAV files, or check their hashes.
//!
//! ```text
//! cargo run --release -p orpheus-synth --example render -- <dir>    # WAVs + level table
//! cargo run --release -p orpheus-synth --example render -- --check  # hashes vs tests/goldens*.txt
//! ```
//!
//! `--check` in a release build (opt-level "s", fat LTO) next to the golden
//! tests (opt-level 1) shows that the optimiser does not change a sample.
//! The sample-playback scenarios (`tests/common/samples.rs`, our own test
//! samples through the cache) are included.

#[path = "../tests/common/samples.rs"]
mod samples;

use std::io::Write;
use std::path::Path;
use std::process::ExitCode;

use orpheus_synth::demo::{SCENARIOS, hash_i16};

fn wav(path: &Path, rate: u32, samples: &[i16]) -> std::io::Result<()> {
    let mut f = std::io::BufWriter::new(std::fs::File::create(path)?);
    let data = (samples.len() * 2) as u32;
    f.write_all(b"RIFF")?;
    f.write_all(&(36 + data).to_le_bytes())?;
    f.write_all(b"WAVEfmt ")?;
    f.write_all(&16u32.to_le_bytes())?;
    f.write_all(&1u16.to_le_bytes())?; // PCM
    f.write_all(&2u16.to_le_bytes())?; // stereo
    f.write_all(&rate.to_le_bytes())?;
    f.write_all(&(rate * 4).to_le_bytes())?;
    f.write_all(&4u16.to_le_bytes())?;
    f.write_all(&16u16.to_le_bytes())?;
    f.write_all(b"data")?;
    f.write_all(&data.to_le_bytes())?;
    for s in samples {
        f.write_all(&s.to_le_bytes())?;
    }
    Ok(())
}

fn main() -> ExitCode {
    let arg = std::env::args().nth(1).unwrap_or_default();
    if arg.is_empty() {
        eprintln!("usage: render <dir> | --check");
        return ExitCode::from(2);
    }
    let check = arg == "--check";
    let goldens = include_str!("../tests/goldens.txt");
    let mut bad = 0;
    println!("{:24} {:>7} {:>9} {:>9}  hash", "scenario", "seconds", "peak dBFS", "rms dBFS");
    for sc in &SCENARIOS {
        let mut out = vec![0i16; 2 * sc.frames()];
        sc.render_i16(&mut out, 512);
        let line = format!("{} {:016x} {}", sc.name, hash_i16(&out), sc.frames());
        let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        let rms = (out.iter().map(|&s| f64::from(s) * f64::from(s)).sum::<f64>() / out.len() as f64).sqrt();
        let db = |x: f64| 20.0 * (x.max(1e-9) / 32767.0).log10();
        println!(
            "{:24} {:7.2} {:9.1} {:9.1}  {:016x}",
            sc.name,
            sc.seconds,
            db(f64::from(peak)),
            db(rms),
            hash_i16(&out)
        );
        if check {
            if !goldens.lines().any(|l| l == line) {
                println!("  MISMATCH: {line}");
                bad += 1;
            }
        } else {
            let dir = Path::new(&arg);
            std::fs::create_dir_all(dir).unwrap();
            let name = sc.name.replace(['/', '@'], "_");
            wav(&dir.join(format!("{name}.wav")), sc.rate, &out).unwrap();
        }
    }
    let sample_goldens = include_str!("../tests/goldens-samples.txt");
    for sc in &samples::SCENARIOS {
        let (out, misses) = sc.render(256);
        let line = format!("{} {:016x} {}", sc.name, hash_i16(&out), sc.frames());
        println!(
            "{:24} {:7.2} {:>9} {:>9}  {:016x} ({misses} misses)",
            sc.name,
            sc.seconds,
            "",
            "",
            hash_i16(&out)
        );
        if check {
            if !sample_goldens.lines().any(|l| l == line) {
                println!("  MISMATCH: {line}");
                bad += 1;
            }
        } else {
            let dir = Path::new(&arg);
            let name = sc.name.replace(['/', '@'], "_");
            wav(&dir.join(format!("{name}.wav")), sc.rate(), &out).unwrap();
        }
    }
    if check {
        let profile = if cfg!(debug_assertions) { "debug" } else { "release" };
        println!(
            "{} scenarios, {bad} mismatches ({profile} build)",
            SCENARIOS.len() + samples::SCENARIOS.len()
        );
        if bad > 0 {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
