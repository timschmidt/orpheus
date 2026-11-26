//! Host benchmark: `cargo bench -p orpheus-synth`.
//!
//! Prints nanoseconds per voice-sample for each voice type (16 voices held,
//! minus the empty engine) and per sample for each effect (one quiet sine
//! voice with the send on, minus the same without), on the host. These rank
//! the costs; the player's numbers come from SYN-3 (cycles per voice into
//! the budgets).

use std::hint::black_box;
use std::time::Instant;

use orpheus_synth::{
    Adsr, ChorusParams, DelayParams, DrumKind, DrumParams, Engine, Event, FilterParams, Kind, OscParams,
    Param, Patch, ReverbParams, SubParams, Waveform, presets,
};

const RATE: u32 = 48_000;
const FRAMES: usize = RATE as usize * 4;

/// ns per output frame of `e` after `events`, best of 3 runs.
fn time(setup: &dyn Fn() -> (Engine, Vec<Event>)) -> f64 {
    let mut best = f64::MAX;
    for _ in 0..3 {
        let (mut e, ev) = setup();
        let mut buf = vec![0i16; 2 * 1024];
        e.render_i16(&mut buf, &ev);
        let t = Instant::now();
        for _ in 0..FRAMES / 1024 {
            e.render_i16(black_box(&mut buf), &[]);
        }
        best = best.min(t.elapsed().as_nanos() as f64 / FRAMES as f64);
    }
    best
}

fn held(p: Patch, voices: u8) -> impl Fn() -> (Engine, Vec<Event>) {
    move || {
        let mut e = Engine::new(RATE, 1);
        let mut p = p;
        p.amp_env = Adsr::new(0.0, 0.0, 1.0, 0.1);
        p.mono = false;
        if let Kind::Fm(f) = &mut p.kind {
            for o in &mut f.op {
                o.env.sustain = 1.0;
            }
        }
        e.set_patch(0, &p);
        let ev = (0..voices).map(|i| Event::note_on(0, 0, 40 + 2 * i, 100)).collect();
        (e, ev)
    }
}

fn one(wave: Waveform, filter: FilterParams) -> Patch {
    Patch {
        kind: Kind::Subtractive(SubParams { osc: [OscParams::new(wave), OscParams::OFF], noise: 0.0 }),
        filter,
        ..Patch::INIT
    }
}

fn main() {
    let base = time(&held(presets::SAW_LEAD, 0));
    println!("empty engine (limiter, idle effects): {base:6.1} ns/frame");
    println!("\nvoice type (16 held)                     ns/voice-sample");
    let lp = FilterParams::low_pass(2000.0, 0.3);
    let cases: [(&str, Patch); 12] = [
        ("sine, no filter", one(Waveform::Sine, FilterParams::OFF)),
        ("saw, no filter", one(Waveform::Saw, FilterParams::OFF)),
        ("saw + low pass", one(Waveform::Saw, lp)),
        ("pulse + low pass", one(Waveform::Pulse, lp)),
        ("triangle + low pass", one(Waveform::Triangle, lp)),
        ("wavetable + low pass", one(Waveform::Table(1), lp)),
        ("pink noise + low pass", one(Waveform::Pink, lp)),
        ("saw-lead (2 saws, LP, envs, LFO)", presets::SAW_LEAD),
        ("pwm-pad (2 pulses, LP, PWM LFO)", presets::PWM_PAD),
        ("fm 2-op (fm-bass)", presets::FM_BASS),
        ("fm 4-op (fm-epiano)", presets::FM_EPIANO),
        ("fm 4-op + low pass", Patch { filter: lp, ..presets::FM_EPIANO }),
    ];
    for (name, p) in cases {
        let t = time(&held(p, 16));
        println!("{name:40} {:6.2}", (t - base) / 16.0);
    }
    println!("\ndrum voice (16 sounding, decay ×4)       ns/voice-sample");
    for kind in DrumKind::ALL {
        let setup = move || {
            let mut e = Engine::new(RATE, 1);
            for part in 0..16u8 {
                e.set_patch(part, &Patch::drum(DrumParams { decay: 4.0, ..DrumParams::new(kind) }));
            }
            let ev: Vec<Event> = (0..16u8).map(|p| Event::note_on(0, p, 60, 100)).collect();
            (e, ev)
        };
        // Drums decay: time only the first 0.25 s.
        let (mut e, ev) = setup();
        let mut buf = vec![0i16; 2 * 1024];
        e.render_i16(&mut buf, &ev);
        let t = Instant::now();
        let n = RATE as usize / 4 / 1024;
        for _ in 0..n {
            e.render_i16(black_box(&mut buf), &[]);
        }
        let ns = t.elapsed().as_nanos() as f64 / (n * 1024) as f64;
        println!("{:40} {:6.2}", format!("{kind:?}"), (ns - base) / 16.0);
    }
    println!("\neffect (one voice sending)               ns/sample");
    let sine = one(Waveform::Sine, FilterParams::OFF);
    let dry = time(&held(sine, 1));
    let fx = |param: Param| {
        move || {
            let (mut e, mut ev) = held(sine, 1)();
            e.set_delay(&DelayParams { feedback: 0.6, ..DelayParams::DEFAULT });
            e.set_chorus(&ChorusParams::DEFAULT);
            e.set_reverb(&ReverbParams::DEFAULT);
            ev.push(Event::param(0, 0, param, 1.0));
            (e, ev)
        }
    };
    for (name, p) in
        [("delay", Param::DelaySend), ("chorus", Param::ChorusSend), ("reverb (FDN)", Param::ReverbSend)]
    {
        let t = time(&fx(p));
        println!("{name:40} {:6.2}", t - dry);
    }
    // The delay at 1 s: the line at a quarter of the rate, filtered.
    let long = move || {
        let (mut e, mut ev) = held(sine, 1)();
        e.set_delay(&DelayParams { beats: 0.0, time: 1.0, feedback: 0.6, ..DelayParams::DEFAULT });
        ev.push(Event::param(0, 0, Param::DelaySend, 1.0));
        (e, ev)
    };
    println!("{:40} {:6.2}", "delay, 1 s (k = 4)", time(&long) - dry);
    samples();
}

/// Sample voices: 16 notes of a looped 16-bit sample (wholly cached),
/// at the sample's rate and pitched (interpolated), through the patch's
/// filter; kit sample hits.
fn samples() {
    use orpheus_synth::sample::{SampleCache, SliceSource};
    use orpheus_synth::{Kit, KitEntry, SampleLoop, SampleParams};
    let n = 48_000usize;
    let mut file = Vec::new();
    file.extend_from_slice(b"RIFF");
    file.extend_from_slice(&(36 + 2 * n as u32).to_le_bytes());
    file.extend_from_slice(b"WAVEfmt ");
    for x in [16u32, 0x0001_0001, 48_000, 96_000, 0x0010_0002] {
        file.extend_from_slice(&x.to_le_bytes());
    }
    file.extend_from_slice(b"data");
    file.extend_from_slice(&(2 * n as u32).to_le_bytes());
    for i in 0..n {
        let x = ((i as f32 * 0.0577).sin() * 12_000.0) as i16;
        file.extend_from_slice(&x.to_le_bytes());
    }
    let files = [file.as_slice()];
    let mut src = SliceSource::new(&files);
    let mut mem = vec![0i16; 256 * 190];
    let mut cache = SampleCache::new(&mut mem);
    cache.load(&mut src, 0, 0, u32::MAX).unwrap();
    // `notes`: (part, note); the patch is on every part.
    let run = |p: Patch, notes: &[(u8, u8)], kit: Option<Kit>| {
        let mut best = f64::MAX;
        for _ in 0..3 {
            let mut e = Engine::new(RATE, 1);
            if let Some(k) = kit {
                e.set_kit(1, &k);
            }
            for part in 0..16 {
                e.set_patch(part, &p);
            }
            let ev: Vec<Event> = notes.iter().map(|&(part, k)| Event::note_on(0, part, k, 100)).collect();
            let mut buf = vec![0i16; 2 * 1024];
            e.render_i16_with(&mut buf, &ev, &cache);
            let t = Instant::now();
            let blocks = RATE as usize / 4 / 1024;
            for _ in 0..blocks {
                e.render_i16_with(black_box(&mut buf), &[], &cache);
            }
            best = best.min(t.elapsed().as_nanos() as f64 / (blocks * 1024) as f64);
        }
        best
    };
    let base = run(Patch::INIT, &[], None);
    println!("\nsample voice (16 sounding)               ns/voice-sample");
    let at_root: Vec<(u8, u8)> = (0..16).map(|part| (part, 60)).collect();
    let pitched: Vec<(u8, u8)> = (0..16).map(|i| (0, 50 + i)).collect();
    let mut p = Patch::sample(SampleParams { root: 60, looping: SampleLoop::Whole, ..SampleParams::new(0) });
    println!("{:40} {:6.2}", "sample at its rate, no filter", (run(p, &at_root, None) - base) / 16.0);
    println!("{:40} {:6.2}", "sample pitched, no filter", (run(p, &pitched, None) - base) / 16.0);
    p.filter = FilterParams::low_pass(3000.0, 0.2);
    println!("{:40} {:6.2}", "sample pitched + low pass", (run(p, &pitched, None) - base) / 16.0);
    let hits: Vec<KitEntry> =
        (0..16u8).map(|i| KitEntry::sample(40 + i, 0).tune(i as i8 * 2).decay(200).colour(60, 0)).collect();
    let kit = Kit::new(&hits);
    let notes: Vec<(u8, u8)> = (40..56).map(|n| (0, n)).collect();
    println!(
        "{:40} {:6.2}",
        "kit sample hit (low pass)",
        (run(Patch::kit(1), &notes, Some(kit)) - base) / 16.0
    );
}
