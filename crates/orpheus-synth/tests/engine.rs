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

// ------------------------------------------------- the load governor's controls

/// A held saw on part 0 sending fully to the chorus and the reverb.
fn wet_engine() -> Engine {
    let mut e = Engine::new(48_000, 1);
    e.set_patch(0, &held_patch());
    e.apply(EventKind::Param { part: 0, param: Param::ChorusSend, value: 1.0 });
    e.apply(EventKind::Param { part: 0, param: Param::ReverbSend, value: 1.0 });
    e.render_f32(&mut [0.0; 2 * 64], &[Event::note_on(0, 0, 60, 100)]);
    // The reverb's tail built up (its shortest path is ≈ 14 ms).
    e.render_f32(&mut vec![0.0; 2 * 4096], &[]);
    e
}

/// With every effect enabled (the default) `set_fx_enabled` changes
/// nothing: the render is the same bit for bit.
#[test]
fn every_effect_enabled_is_the_plain_render() {
    let (mut a, mut b) = (wet_engine(), wet_engine());
    b.set_fx_enabled(orpheus_synth::load::FX_ALL);
    let (mut x, mut y) = ([0.0f32; 2 * 4096], [0.0f32; 2 * 4096]);
    a.render_f32(&mut x, &[]);
    b.render_f32(&mut y, &[]);
    assert!(x.iter().zip(&y).all(|(p, q)| p.to_bits() == q.to_bits()));
}

/// An effect switched off fades out over `FX_FADE` samples (no step),
/// then adds nothing: the output equals an engine whose reverb never
/// sounded. Switched on again it is fed again (from silence).
#[test]
fn an_effect_switched_off_fades_out_then_is_silent() {
    use orpheus_synth::engine::FX_FADE;
    use orpheus_synth::load::{FX_ALL, FX_CHORUS, FX_DELAY, FX_REVERB, PLAYER};
    let fade = usize::from(FX_FADE);
    let mut on = wet_engine();
    let mut off = wet_engine();
    off.set_fx_enabled(FX_DELAY | FX_CHORUS);
    assert_eq!(off.fx_enabled() & FX_REVERB, 0);
    let n = 48_000;
    let (mut a, mut b) = (vec![0.0f32; 2 * n], vec![0.0f32; 2 * n]);
    on.render_f32(&mut a, &[]);
    off.render_f32(&mut b, &[]);
    // The same voice and chorus with the reverb off from the start.
    let mut dry = Engine::new(48_000, 1);
    dry.set_patch(0, &held_patch());
    dry.apply(EventKind::Param { part: 0, param: Param::ChorusSend, value: 1.0 });
    dry.set_fx_enabled(FX_DELAY | FX_CHORUS);
    dry.render_f32(&mut [0.0; 2 * 64], &[Event::note_on(0, 0, 60, 100)]);
    dry.render_f32(&mut vec![0.0; 2 * 4096], &[]);
    let mut c = vec![0.0f32; 2 * n];
    dry.render_f32(&mut c, &[]);
    // The reverb's share: in full where it stays on, fading where it was
    // switched off (starting at its full level: no step), then nothing.
    let full = |k: usize| (a[2 * k] - c[2 * k]).abs();
    let fading = |k: usize| (b[2 * k] - c[2 * k]).abs();
    assert!(fading(0) > 0.0 && (fading(0) - full(0)).abs() <= full(0) * 0.01, "{} {}", fading(0), full(0));
    let first: f32 = (0..fade / 4).map(fading).sum();
    let last: f32 = (3 * fade / 4..fade).map(fading).sum();
    assert!(last < first * 0.5, "fading: {first} then {last}");
    for k in fade..n {
        assert_eq!(b[2 * k].to_bits(), c[2 * k].to_bits(), "frame {k}");
    }
    off.set_fx_enabled(FX_ALL);
    assert_eq!(off.fx_enabled(), FX_ALL);
    assert_ne!(off.estimate(&PLAYER).running & FX_REVERB, 0, "fed by the part's send again");
}

/// `shed` fades out released voices first (oldest first), then the
/// oldest held ones, down to `keep`.
#[test]
fn shed_takes_released_voices_first_then_the_oldest() {
    let mut e = Engine::new(48_000, 1);
    e.set_patch(0, &held_patch());
    let mut buf = [0.0f32; 2 * 64];
    for n in [60, 62, 64, 65, 67] {
        e.render_f32(&mut buf, &[Event::note_on(0, 0, n, 100)]);
    }
    // 64 released (its 0.5 s release keeps it sounding).
    e.render_f32(&mut buf, &[Event::note_off(0, 0, 64)]);
    assert_eq!(e.shed(5), 0);
    assert_eq!(e.shed(3), 2);
    let mut fading: Vec<u8> = e.voices().filter(|v| v.fading).map(|v| v.note).collect();
    fading.sort_unstable();
    assert_eq!(fading, [60, 64]);
    assert_eq!(notes(&e, true), [62, 65, 67]);
    e.render_f32(&mut [0.0; 2 * 512], &[]);
    assert_eq!(notes(&e, false), [62, 65, 67]);
}

/// The estimate counts each sounding voice by its class, the fading ones
/// apart, and an effect only while it is fed or rings.
#[test]
fn the_estimate_counts_voices_by_class_and_effects_that_run() {
    use orpheus_synth::load::{FX_CHORUS, FX_REVERB, PLAYER};
    let mut e = Engine::new(48_000, 1);
    assert_eq!(e.estimate(&PLAYER).total(), PLAYER.empty);
    e.set_patch(0, &held_patch());
    e.set_patch(1, &presets::FM_EPIANO);
    e.set_patch(2, &Patch::drum(presets::DRUMS[2].1));
    let mut buf = [0.0f32; 2 * 64];
    e.render_f32(
        &mut buf,
        &[Event::note_on(0, 0, 60, 100), Event::note_on(0, 1, 60, 100), Event::note_on(0, 2, 60, 100)],
    );
    let est = e.estimate(&PLAYER);
    assert_eq!((est.n, est.n_fading), (3, 0));
    assert_eq!(est.voices, PLAYER.sub + PLAYER.fm2 + 2 * PLAYER.fm_op + PLAYER.drum[2]);
    assert_eq!(est.running, 0, "no part sends: no effect runs");
    e.apply(EventKind::Param { part: 1, param: Param::ReverbSend, value: 0.5 });
    let est = e.estimate(&PLAYER);
    assert_eq!((est.running, est.wanted), (FX_REVERB, FX_REVERB));
    assert_eq!(est.total(), PLAYER.empty + est.voices + PLAYER.fx[2]);
    e.render_f32(&mut buf, &[]);
    e.set_fx_enabled(FX_CHORUS);
    let est = e.estimate(&PLAYER);
    assert_eq!(est.running, FX_REVERB, "fading out still costs");
    e.render_f32(&mut [0.0; 2 * 512], &[]);
    let est = e.estimate(&PLAYER);
    assert_eq!((est.running, est.wanted & FX_REVERB), (0, FX_REVERB));
    e.shed(1);
    let est = e.estimate(&PLAYER);
    assert_eq!(est.n_fading, 2);
    assert!(est.lasting() < est.total());
}
