//! Engine behaviour: sample-accurate scheduling, block-size independence,
//! voice stealing, mono legato, sustain, filter stability, determinism, and
//! the RAM table.

use orpheus_synth::demo::{SCENARIOS, Score};
use orpheus_synth::filter::Svf;
use orpheus_synth::math::Rng;
use orpheus_synth::{
    Adsr, Engine, Event, EventKind, FilterMode, FilterParams, Kind, OscParams, Param, Patch, SubParams,
    Waveform, presets,
};

/// A patch whose output is a constant while a note sounds: a pulse at a
/// very low note starting at phase 0.25 (+1 for ~460 samples), a gate
/// envelope (full level on the note-on sample, silent on the note-off
/// sample), no filter, no velocity sensitivity.
fn gate_patch() -> Patch {
    Patch {
        kind: Kind::Subtractive(SubParams {
            osc: [OscParams { phase: 0.25, ..OscParams::new(Waveform::Pulse) }, OscParams::OFF],
            noise: 0.0,
        }),
        filter: FilterParams::OFF,
        amp_env: Adsr::GATE,
        velocity: 0.0,
        level: 1.0,
        ..Patch::INIT
    }
}

fn level_on() -> f32 {
    // Full level × part volume 0.8 × centre pan cos(π/4).
    let mut e = Engine::new(48_000, 1);
    e.set_patch(0, &gate_patch());
    let mut out = [0.0f32; 2];
    e.render_f32(&mut out, &[Event::note_on(0, 0, 20, 100)]);
    out[0]
}

#[test]
fn every_event_lands_on_its_sample() {
    let on = level_on();
    assert!(on > 0.5, "{on}");
    let mut rng = Rng::new(42);
    for trial in 0..20 {
        let frames = 20_000;
        let mut s = Score::new(48_000);
        // Non-overlapping notes with random starts and lengths (< 400 samples).
        let mut t = u64::from(rng.next_u32() % 50);
        let mut notes = Vec::new();
        while t + 400 < frames as u64 {
            let len = 1 + u64::from(rng.next_u32() % 399);
            s.push_at(t, EventKind::NoteOn { part: 0, note: 20, velocity: 100 });
            s.push_at(t + len, EventKind::NoteOff { part: 0, note: 20 });
            notes.push((t as usize, (t + len) as usize));
            t += len + 1 + u64::from(rng.next_u32() % 300);
        }
        // Random block sizes (1–700).
        let mut block_rng = Rng::new(trial + 1);
        let mut e = Engine::new(48_000, 1);
        e.set_patch(0, &gate_patch());
        let mut out = vec![0.0f32; 2 * frames];
        let mut start = 0;
        while start < frames {
            let b = (1 + (block_rng.next_u32() % 700) as usize).min(frames - start);
            let ev: Vec<Event> = s
                .events()
                .iter()
                .filter(|x| (x.at as usize) >= start && (x.at as usize) < start + b)
                .map(|x| Event { offset: (x.at as usize - start) as u32, kind: x.kind })
                .collect();
            e.render_f32(&mut out[2 * start..2 * (start + b)], &ev);
            start += b;
        }
        let mut expect = vec![0.0f32; frames];
        for &(a, b) in &notes {
            expect[a..b].fill(on);
        }
        for i in 0..frames {
            assert_eq!(out[2 * i], expect[i], "trial {trial} frame {i} (left)");
            assert_eq!(out[2 * i + 1], expect[i], "trial {trial} frame {i} (right)");
        }
    }
}

#[test]
fn parameter_changes_start_on_their_sample() {
    let on = level_on();
    for k in [1u32, 15, 16, 17, 100, 350] {
        let mut e = Engine::new(48_000, 1);
        e.set_patch(0, &gate_patch());
        let mut out = vec![0.0f32; 2 * 600];
        e.render_f32(&mut out, &[Event::note_on(0, 0, 20, 100), Event::param(k, 0, Param::Volume, 0.4)]);
        let k = k as usize;
        assert_eq!(out[2 * (k - 1)], on, "before {k}");
        assert!(out[2 * k] < on, "at {k}: {}", out[2 * k]);
        // The ramp reaches the new gain within two control periods (a tick
        // inside the first restarts it from where it is).
        let new = out[2 * (k + 2 * orpheus_synth::CONTROL_PERIOD)];
        assert!((new - on * 0.5).abs() < 1e-4, "after {k}: {new}");
    }
}

#[test]
fn output_does_not_depend_on_block_size() {
    for sc in SCENARIOS.iter().filter(|s| ["demo-song", "steal", "glide-mono", "fx-delay"].contains(&s.name))
    {
        let frames = sc.frames().min(48_000 * 2);
        let mut reference = vec![0i16; 2 * frames];
        sc.render_i16(&mut reference, 256);
        for block in [1usize, 7, 16, 1023, 4096] {
            let mut out = vec![0i16; 2 * frames];
            sc.render_i16(&mut out, block);
            assert!(out == reference, "{} differs at block size {block}", sc.name);
        }
    }
}

#[test]
fn same_seed_same_output_other_seed_other_noise() {
    let render = |seed: u32| {
        let mut e = Engine::new(48_000, seed);
        e.set_patch(0, &presets::NOISE_HIT);
        e.set_patch(9, &Patch::KIT);
        let mut out = vec![0i16; 2 * 9600];
        e.render_i16(&mut out, &[Event::note_on(0, 0, 60, 100), Event::note_on(5, 9, 38, 100)]);
        out
    };
    assert_eq!(render(7), render(7));
    assert_ne!(render(7), render(8));
    // reset() restarts the same way.
    let mut e = Engine::new(48_000, 3);
    e.set_patch(0, &presets::NOISE_HIT);
    let mut a = vec![0i16; 4800];
    e.render_i16(&mut a, &[Event::note_on(0, 0, 60, 100)]);
    e.reset(3);
    let mut b = vec![0i16; 4800];
    e.render_i16(&mut b, &[Event::note_on(0, 0, 60, 100)]);
    assert_eq!(a, b);
}

fn held_patch() -> Patch {
    Patch { amp_env: Adsr::new(0.0, 0.0, 1.0, 0.5), ..presets::SAW_LEAD }
}

fn notes(e: &Engine, main_only: bool) -> Vec<u8> {
    let mut v: Vec<u8> = e
        .voices()
        .filter(|v| !main_only || (v.slot < orpheus_synth::MAX_VOICES && !v.fading))
        .map(|v| v.note)
        .collect();
    v.sort_unstable();
    v
}

#[test]
fn stealing_takes_the_oldest_then_prefers_released_and_quiet() {
    let mut e = Engine::new(48_000, 1);
    e.set_patch(0, &held_patch());
    e.set_voice_limit(4);
    let mut out = vec![0.0f32; 2 * 64];
    let ev: Vec<Event> = (0..5u8).map(|i| Event::note_on(u32::from(i) * 10, 0, 60 + i, 100)).collect();
    e.render_f32(&mut out, &ev);
    // The fifth note took the oldest (60), which fades in a tail slot.
    assert_eq!(notes(&e, true), [61, 62, 63, 64]);
    assert!(e.voices().any(|v| v.note == 60 && v.fading));
    e.render_f32(&mut vec![0.0; 2 * 256], &[]);
    assert!(!e.voices().any(|v| v.note == 60), "tail did not end within 256 samples");
    // A released voice goes before an older held one.
    e.render_f32(&mut out, &[Event::note_off(0, 0, 63), Event::note_on(10, 0, 70, 100)]);
    assert_eq!(notes(&e, true), [61, 62, 64, 70]);
    // A held voice below −40 dB goes before older loud ones.
    let mut quiet = held_patch();
    quiet.velocity = 1.0;
    e.set_patch(1, &quiet);
    e.render_f32(&mut out, &[Event::note_on(0, 1, 80, 1)]);
    e.render_f32(&mut out, &[]);
    // Part 1's quiet voice stole a slot (oldest: 61); now a new note steals the quiet one.
    assert_eq!(notes(&e, true), [62, 64, 70, 80]);
    e.render_f32(&mut out, &[Event::note_on(0, 0, 72, 100)]);
    assert_eq!(notes(&e, true), [62, 64, 70, 72]);
    // Same note on the same part retriggers its own slot.
    e.render_f32(&mut out, &[Event::note_on(0, 0, 64, 100)]);
    assert_eq!(notes(&e, true), [62, 64, 70, 72]);
}

#[test]
fn sixteen_voices_and_no_more() {
    let mut e = Engine::new(48_000, 1);
    e.set_patch(0, &held_patch());
    let ev: Vec<Event> = (0..30u8).map(|i| Event::note_on(u32::from(i), 0, 30 + i, 100)).collect();
    let mut out = vec![0.0f32; 2 * 32];
    e.render_f32(&mut out, &ev);
    assert_eq!(notes(&e, true).len(), 16);
    assert_eq!(notes(&e, true), (44..60).collect::<Vec<u8>>());
    assert!(out.iter().all(|x| x.abs() <= 1.0));
}

#[test]
fn mono_legato_glides_and_returns_to_held_note() {
    let mut p = presets::SQUARE_BASS;
    p.glide = 0.05;
    let mut e = Engine::new(48_000, 1);
    e.set_patch(0, &p);
    let mut out = vec![0.0f32; 2 * 480];
    e.render_f32(&mut out, &[Event::note_on(0, 0, 36, 100)]);
    let level0 = e.voices().next().unwrap().level;
    e.render_f32(&mut out, &[Event::note_on(0, 0, 43, 100)]);
    assert_eq!(e.active_voices(), 1);
    assert_eq!(notes(&e, false), [43]);
    // Legato: the envelope was not restarted from zero (its level continues).
    let level1 = e.voices().next().unwrap().level;
    assert!(level1 > 0.3 * level0, "{level0} -> {level1}");
    e.render_f32(&mut out, &[Event::note_off(0, 0, 43)]);
    assert_eq!(notes(&e, false), [36]);
    assert!(!e.voices().next().unwrap().released);
    e.render_f32(&mut out, &[Event::note_off(0, 0, 36)]);
    assert!(e.voices().all(|v| v.released));
}

#[test]
fn sustain_pedal_holds_notes() {
    let mut e = Engine::new(48_000, 1);
    e.set_patch(0, &held_patch());
    let mut out = vec![0.0f32; 2 * 64];
    e.render_f32(
        &mut out,
        &[Event::param(0, 0, Param::Sustain, 1.0), Event::note_on(0, 0, 60, 100), Event::note_off(10, 0, 60)],
    );
    assert!(e.voices().all(|v| !v.released));
    e.render_f32(&mut out, &[Event::param(0, 0, Param::Sustain, 0.0)]);
    assert!(e.voices().all(|v| v.released));
}

#[test]
fn filter_stays_stable_under_extreme_modulation() {
    let mut rng = Rng::new(9);
    for rate in [8000.0f32, 44_100.0, 48_000.0] {
        let mut f = Svf::default();
        let mut peak = 0.0f32;
        for i in 0..20_000 {
            // A new cutoff every 16 samples (faster than the engine's control
            // rate): random, with runs at the top.
            let fc = if i % 40 < 10 { 1.0e6 } else { 16.0 + (rng.next_u32() % 24_000) as f32 };
            f.set(fc, 1.0, rate);
            for _ in 0..16 {
                let (lp, bp) = f.tick(rng.bipolar());
                peak = peak.max(lp.abs()).max(bp.abs());
            }
            assert!(f.is_finite());
        }
        // Q = 50 can ring to ~50× the input, but never grows without bound.
        assert!(peak < 200.0, "rate {rate}: peak {peak}");
    }
    // And inside the engine, all three modes, swept by events at full resonance.
    for mode in [FilterMode::LowPass, FilterMode::BandPass, FilterMode::HighPass] {
        let mut p = presets::NOISE_HIT;
        p.filter = FilterParams { mode, resonance: 1.0, ..p.filter };
        p.amp_env = Adsr::GATE;
        let mut e = Engine::new(48_000, 1);
        e.set_patch(0, &p);
        let mut ev = vec![Event::note_on(0, 0, 60, 127)];
        for k in 0..600u32 {
            let fc = if k % 50 < 25 { 30.0 * 1.012f32.powi(k as i32) } else { 24_000.0 };
            ev.push(Event::param(k * 16, 0, Param::Cutoff, fc));
        }
        let mut out = vec![0.0f32; 2 * 9600];
        e.render_f32(&mut out, &ev);
        assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0), "{mode:?}");
        // The filter rings but recovers: the end of a quiet stretch is quiet again.
        e.render_f32(&mut out, &[Event::note_off(0, 0, 60), Event::param(0, 0, Param::Cutoff, 1000.0)]);
        e.render_f32(&mut out, &[]);
        assert!(out.iter().all(|x| x.abs() < 1e-6), "{mode:?} does not settle");
    }
}

#[test]
fn ram_table() {
    use orpheus_synth::ram;
    let rows = [
        ("voice slot", ram::voice()),
        ("  of which a drum voice", ram::drum_voice()),
        ("  of which an oscillator", ram::osc()),
        ("  of which a sample player", ram::sample_player()),
        ("patch", ram::patch()),
        ("kit slot (2 in the engine)", ram::kit()),
        ("delay", ram::delay()),
        ("chorus", ram::chorus()),
        ("reverb", ram::reverb()),
        ("engine core (20 voice slots, 16 parts, 2 kits, buses)", ram::core()),
        ("engine total", ram::engine()),
        ("sample cache state (outside; pages borrowed)", ram::sample_cache()),
    ];
    for (n, b) in rows {
        println!("{n:48} {b:>7} B");
    }
    println!("{:48} {:>7} B (read-only)", "wavetables", ram::WAVETABLE_ROM);
    // The documented budget (lib docs and 04 §9 "as built").
    assert!(ram::voice() <= 336, "voice slot {} B", ram::voice());
    assert!(ram::engine() <= 72 * 1024, "engine {} B exceeds the 72 KiB decoder arena", ram::engine());
    // SYN-2: 62,320 B with kits and samples (the reverb at half rate in 8 KiB):
    // also inside a 64 KiB arena.
    assert!(ram::engine() <= 64 * 1024, "engine {} B exceeds 64 KiB", ram::engine());
}
