//! Write the demo songs as MIDI files and WAV renders for listening:
//! `cargo run --release -p orpheus-midi --example render -- <dir> [rate]`.
//! Also renders any `.mid`/`.kar` paths given after the directory.

use std::path::Path;
use std::process::ExitCode;

use orpheus_midi::{MidiPlayer, Options, SliceSource, demo};

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

fn render(file: &[u8], kar: bool, rate: u32) -> Result<(Vec<i16>, String), orpheus_midi::MidiError> {
    let mut src = SliceSource::new(file);
    let mut p = Box::new(MidiPlayer::new(Options::new(rate)));
    let info = p.open_from(&mut src, kar)?;
    let mut out = vec![0i16; 2 * info.duration as usize];
    let mut done = 0;
    let mut lyrics = String::new();
    while done < out.len() / 2 {
        let hi = (2 * done + 2048).min(out.len());
        let n = p.render_from(&mut src, &mut out[2 * done..hi])?;
        if n == 0 {
            break;
        }
        done += n;
        while let Some(l) = p.pop_lyric() {
            lyrics.push_str(&format!(
                "{:>8.3}s {:?}/{:?} {}\n",
                l.frame as f64 / f64::from(rate),
                l.before,
                l.after,
                String::from_utf8_lossy(l.text())
            ));
        }
    }
    out.truncate(2 * done);
    let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
    let title = String::from_utf8_lossy(info.title()).into_owned();
    Ok((
        out,
        format!(
            "\"{title}\" format {} tracks {} notes {} {:.1} s peak {:.1} dBFS\n{lyrics}",
            info.format,
            info.tracks,
            info.notes,
            info.duration as f64 / f64::from(rate),
            20.0 * (f64::from(peak) / 32768.0).log10()
        ),
    ))
}

/// `--check`: the golden table of `tests/play.rs` (same cases) in this
/// build, against `tests/goldens.txt`.
fn check() -> ExitCode {
    type Song = fn(&mut [u8]) -> Option<usize>;
    let cases: [(&str, Song, bool, u32, u32); 4] = [
        ("etude-48k", demo::etude, false, 48_000, 8),
        ("etude-44k", demo::etude, false, 44_100, 4),
        ("karaoke-44k", demo::karaoke, true, 44_100, 8),
        ("tour-48k", demo::tour, false, 48_000, 12),
    ];
    let want = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens.txt")).unwrap();
    let mut bad = 0;
    for ((name, f, kar, rate, secs), line) in cases.into_iter().zip(want.lines()) {
        let mut buf = vec![0u8; 64 * 1024];
        let n = f(&mut buf).unwrap();
        let mut src = SliceSource::new(&buf[..n]);
        let mut p = Box::new(MidiPlayer::new(Options::new(rate)));
        p.open_from(&mut src, kar).unwrap();
        let mut out = vec![0i16; 2 * (rate * secs) as usize];
        let mut done = 0;
        while done < out.len() {
            let hi = (done + 2 * 1152).min(out.len());
            done += 2 * p.render_from(&mut src, &mut out[done..hi]).unwrap();
        }
        let got = format!("{name} {:016x} {}", orpheus_synth::demo::hash_i16(&out), out.len() / 2);
        let ok = got == line;
        bad += usize::from(!ok);
        println!("{} {got}", if ok { "ok  " } else { "DIFF" });
    }
    println!("{} scenarios, {bad} mismatches", cases.len());
    if bad == 0 { ExitCode::SUCCESS } else { ExitCode::FAILURE }
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--check") {
        return check();
    }
    let Some(dir) = args.first() else {
        eprintln!("usage: render <dir> [rate] [file.mid ...] | render --check");
        return ExitCode::FAILURE;
    };
    let dir = Path::new(dir);
    std::fs::create_dir_all(dir).unwrap();
    let rate: u32 = args.get(1).and_then(|r| r.parse().ok()).unwrap_or(48_000);
    let mut songs: Vec<(String, Vec<u8>, bool)> = Vec::new();
    for (name, f, kar) in [
        ("etude.mid", demo::etude as fn(&mut [u8]) -> Option<usize>, false),
        ("hill-song.kar", demo::karaoke, true),
        ("sound-tour.mid", demo::tour, false),
    ] {
        let mut buf = vec![0u8; 64 * 1024];
        let n = f(&mut buf).expect("demo fits");
        buf.truncate(n);
        std::fs::write(dir.join(name), &buf).unwrap();
        songs.push((name.to_string(), buf, kar));
    }
    for path in args.iter().skip(2) {
        let data = std::fs::read(path).unwrap();
        let name = Path::new(path).file_name().unwrap().to_string_lossy().into_owned();
        let kar = orpheus_midi::extension(&name).unwrap_or(false);
        songs.push((name, data, kar));
    }
    for (name, data, kar) in &songs {
        match render(data, *kar, rate) {
            Ok((pcm, report)) => {
                let stem = name.rsplit_once('.').map_or(name.as_str(), |(s, _)| s);
                let out = dir.join(format!("{stem}.wav"));
                wav(&out, rate, &pcm).unwrap();
                println!("{}: {report}", out.display());
            }
            Err(e) => println!("{name}: {}", e.message()),
        }
    }
    ExitCode::SUCCESS
}
