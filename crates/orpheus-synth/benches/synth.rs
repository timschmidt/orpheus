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
    for kind in [
        DrumKind::Kick,
        DrumKind::Snare,
        DrumKind::ClosedHat,
        DrumKind::OpenHat,
        DrumKind::Clap,
        DrumKind::Tom,
    ] {
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
        [("delay", Param::DelaySend), ("chorus", Param::ChorusSend), ("reverb", Param::ReverbSend)]
    {
        let t = time(&fx(p));
        println!("{name:40} {:6.2}", t - dry);
    }
}
