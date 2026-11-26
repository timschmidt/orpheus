//! Player CPU estimate for MIDI playback (MIDI-1):
//! `cargo run --release -p orpheus-midi --example estimate`.
//!
//! Renders the demo songs and, at every block, adds up an estimated cost
//! of each sounding voice by its sound (M4 cycles per voice-sample from
//! SYN-1/SYN-2's thumbv7em disassembly estimates, ±30 %: subtractive
//! two-oscillator voices 110–190, FM 120/190/260 for 2/3/4 operators, drums
//! by model), plus the engine, limiter and the active effects per sample.
//! Prints the mean and the worst block in MHz at 44.1 and 48 kHz, the voice
//! counts, and the host time per MIDI event (parser + clock + sequencer).
//! SYN-3 replaces the table by measured cycles.

use orpheus_midi::{MidiPlayer, Options, SliceSource, demo};
use orpheus_synth::DrumKind;
use orpheus_synth::kit::KitSound;

/// Cycles per voice-sample of the GM sounds (index = `gm::SET`).
const SOUND: [u32; 32] = [
    190, 260, 165, 120, 120, 120, 130, 120, 190, 150, 175, 140, 130, 125, 110, 125, //
    140, 140, 130, 140, 160, 140, 145, 180, 180, 140, 190, 140, 190, 140, 90, 80,
];

fn drum(kind: DrumKind) -> u32 {
    match kind {
        DrumKind::Kick => 145,
        DrumKind::Snare => 230,
        DrumKind::ClosedHat | DrumKind::OpenHat => 295,
        DrumKind::Clap => 105,
        DrumKind::Tom => 155,
        DrumKind::Rim => 120,
        DrumKind::Cowbell => 155,
        DrumKind::Cymbal => 315,
        DrumKind::Clave => 100,
        DrumKind::Shaker => 75,
    }
}

/// Engine and limiter, reverb, chorus per sample (SYN-1/2).
const ENGINE: u32 = 135;
const REVERB: u32 = 290;
const CHORUS: u32 = 120;

fn main() {
    type Song = fn(&mut [u8]) -> Option<usize>;
    let songs: [(&str, Song, bool); 3] = [
        ("etude", demo::etude, false),
        ("hill-song", demo::karaoke, true),
        ("sound-tour", demo::tour, false),
    ];
    for (name, f, kar) in songs {
        let mut buf = vec![0u8; 64 * 1024];
        let n = f(&mut buf).unwrap();
        let file = &buf[..n];
        let rate = 48_000u32;
        let mut src = SliceSource::new(file);
        let mut p = Box::new(MidiPlayer::new(Options::new(rate)));
        let info = p.open_from(&mut src, kar).unwrap();
        let mut out = vec![0i16; 2 * 1152];
        let (mut sum, mut worst, mut blocks) = (0f64, 0u32, 0u32);
        let (mut vsum, mut vmax) = (0usize, 0usize);
        loop {
            // Cost of the voices sounding at the block's start.
            let mut c = ENGINE + REVERB;
            let mut voices = 0;
            for v in p.engine().voices() {
                voices += 1;
                let s = p.sequencer().sound(v.part);
                c += if usize::from(s) < SOUND.len() {
                    SOUND[usize::from(s)]
                } else {
                    match p.engine().kit(0).and_then(|k| k.find(v.note)).and_then(|e| e.decode()) {
                        Some(KitSound::Drum(d)) => drum(d.kind),
                        _ => 90,
                    }
                };
            }
            if (0..16).any(|c| p.sequencer().channel(c).chorus > 0) {
                c += CHORUS;
            }
            let got = p.render_from(&mut src, &mut out).unwrap();
            if got == 0 {
                break;
            }
            sum += f64::from(c);
            worst = worst.max(c);
            blocks += 1;
            vsum += voices;
            vmax = vmax.max(voices);
        }
        let mean = sum / f64::from(blocks);
        println!(
            "{name}: {:.0} s, voices mean {:.1} max {vmax}; cycles/sample mean {mean:.0} worst {worst}; \
             MHz at 44.1 kHz {:.0} (worst {:.0}), at 48 kHz {:.0} (worst {:.0})",
            info.duration as f64 / f64::from(rate),
            vsum as f64 / f64::from(blocks),
            mean * 44_100.0 / 1e6,
            f64::from(worst) * 44_100.0 / 1e6,
            mean * 48_000.0 / 1e6,
            f64::from(worst) * 48_000.0 / 1e6,
        );
        // Host cost per event: the scan without the engine, many times.
        let t = std::time::Instant::now();
        let mut events = 0u64;
        for _ in 0..200 {
            let (i, _) = orpheus_midi::summary(&mut SliceSource::new(file), rate, kar, |_| {}).unwrap();
            events += u64::from(i.events);
        }
        println!(
            "  parser + clock: {:.0} ns per event (host)",
            t.elapsed().as_nanos() as f64 / events as f64
        );
    }
}
