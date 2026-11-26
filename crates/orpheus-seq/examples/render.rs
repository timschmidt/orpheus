//! Render the demo songs for listening through the WAV export, and save
//! their project and MIDI files: `cargo run --release -p orpheus-seq
//! --example render -- <dir> [rate]` writes, for "Night Bus"
//! (`orpheus_seq::demo::beat`, SEQ-1's drums) and "Tram Lights"
//! (`demo::song`, SEQ-2: two kits, bass and lead with chords, locks, the
//! fill), `<dir>/<name>.wav`, `.mid` and `.osq`.

use std::path::Path;
use std::process::ExitCode;

use orpheus_seq::export::{self, WavExport};
use orpheus_seq::{Project, StepLen, demo, file};
use orpheus_synth::Engine;

fn one(dir: &Path, stem: &str, p: &Project, rate: u32) {
    let mut x = WavExport::new(p, rate, StepLen::Sixteenth);
    let mut e = Box::new(Engine::new(rate, 1));
    let mut b = x.header().to_vec();
    let mut block = vec![0i16; 2 * 1024];
    let (mut peak, mut sum, mut n) = (0u16, 0f64, 0u64);
    while !x.finished() {
        let k = x.render(p, &mut e, &mut block);
        for s in &block[..2 * k] {
            peak = peak.max(s.unsigned_abs());
            sum += f64::from(*s) * f64::from(*s);
            n += 1;
            b.extend_from_slice(&s.to_le_bytes());
        }
    }
    let wav = dir.join(format!("{stem}.wav"));
    std::fs::write(&wav, &b).expect("write wav");
    let mut m = vec![0u8; 1 << 20];
    let mlen = export::midi(p, StepLen::Sixteenth, &mut m).expect("midi");
    std::fs::write(dir.join(format!("{stem}.mid")), &m[..mlen]).expect("write midi");
    let mut buf = vec![0u8; file::MAX_FILE];
    let flen = file::save(p, &mut buf).expect("save");
    std::fs::write(dir.join(format!("{stem}.osq")), &buf[..flen]).expect("write project");
    let db = |v: f64| 20.0 * (v.max(1.0) / 32768.0).log10();
    println!(
        "{}: \"{}\", {} steps at {} BPM, swing {} %, {:.1} s, peak {:.1} dBFS, RMS {:.1} dBFS; MIDI {} B, project {} B",
        wav.display(),
        p.name(),
        export::song_steps(p),
        f64::from(p.tempo()) / 100.0,
        p.swing(),
        x.total_frames() as f64 / f64::from(rate),
        db(f64::from(peak)),
        db((sum / n.max(1) as f64).sqrt()),
        mlen,
        flen
    );
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
    one(dir, "night-bus", &p, rate);
    demo::song(&mut p);
    one(dir, "tram-lights", &p, rate);
    ExitCode::SUCCESS
}
