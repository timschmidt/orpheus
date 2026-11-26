//! Drum models, kits and send effects (SYN-2): sample-accurate hits, choke
//! groups, delay timing at tempo, reverb decay, no NaN, no subnormals, no
//! output beyond the limiter.

use orpheus_synth::fx::{DELAY_LEN, Delay, Reverb};
use orpheus_synth::kit::{FACTORY, Kit, KitEntry};
use orpheus_synth::math::Rng;
use orpheus_synth::{
    ChorusParams, DelayParams, DrumKind, DrumParams, Engine, Event, EventKind, FxParam, Param, Patch,
    ReverbParams,
};

/// Render `frames` frames of `e` (f32, interleaved) in random blocks,
/// with `events` at absolute frames.
fn render(e: &mut Engine, frames: usize, events: &[(usize, EventKind)], seed: u32) -> Vec<f32> {
    let mut rng = Rng::new(seed);
    let mut out = vec![0.0f32; 2 * frames];
    let mut pos = 0;
    while pos < frames {
        let n = (1 + (rng.next_u32() % 500) as usize).min(frames - pos);
        let ev: Vec<Event> = events
            .iter()
            .filter(|(at, _)| *at >= pos && *at < pos + n)
            .map(|(at, k)| Event { offset: (at - pos) as u32, kind: *k })
            .collect();
        e.render_f32(&mut out[2 * pos..2 * (pos + n)], &ev);
        pos += n;
    }
    out
}

fn on(part: u8, note: u8, velocity: u8) -> EventKind {
    EventKind::NoteOn { part, note, velocity }
}

#[test]
fn every_drum_hit_starts_on_its_sample() {
    // A hit at any offset is the same samples as a hit at 0, shifted:
    // nothing before it, the identical waveform from it (the first 3000
    // frames; the end is decided on control ticks).
    for kind in DrumKind::ALL {
        let mut e = Engine::new(48_000, 3);
        e.set_patch(0, &Patch::drum(DrumParams::new(kind)));
        let reference = render(&mut e, 3000, &[(0, on(0, 60, 100))], 1);
        assert!(reference.iter().any(|&x| x != 0.0), "{kind:?} silent");
        let mut rng = Rng::new(kind as u32 + 10);
        for trial in 0..6 {
            let at = (rng.next_u32() % 3000) as usize;
            let mut e = Engine::new(48_000, 3);
            e.set_patch(0, &Patch::drum(DrumParams::new(kind)));
            let out = render(&mut e, at + 3000, &[(at, on(0, 60, 100))], trial + 2);
            assert!(out[..2 * at].iter().all(|&x| x == 0.0), "{kind:?}: sound before {at}");
            assert!(out[2 * at..] == reference[..], "{kind:?}: hit at {at} differs");
        }
    }
}

/// Level of part 9's voices playing `note`.
fn level_of(e: &Engine, note: u8) -> f32 {
    e.voices().filter(|v| v.note == note).map(|v| v.level).fold(0.0, f32::max)
}

#[test]
fn choke_groups_cut_their_members_only() {
    let kit = Kit::new(&[
        KitEntry::drum(46, DrumKind::OpenHat).choke(1).decay(150),
        KitEntry::drum(42, DrumKind::ClosedHat).choke(1),
        KitEntry::drum(49, DrumKind::Cymbal).choke(2),
        KitEntry::drum(51, DrumKind::Cymbal),
        KitEntry::drum(36, DrumKind::Kick),
    ]);
    let run = |events: &[(usize, EventKind)], frames: usize| {
        let mut e = Engine::new(48_000, 1);
        e.set_kit(1, &kit);
        e.set_patch(9, &Patch::kit(1));
        e.set_patch(8, &Patch::kit(1));
        render(&mut e, frames, events, 5);
        e
    };
    // Open hat alone: still loud 130 ms later.
    let end = 4800 + 1440;
    let e = run(&[(0, on(9, 46, 100))], end);
    let free = level_of(&e, 46);
    assert!(free > 0.1, "open hat {free}");
    // Closed hat at 100 ms chokes it: below −60 dB 30 ms after.
    let e = run(&[(0, on(9, 46, 100)), (4800, on(9, 42, 100))], end);
    assert!(level_of(&e, 46) < free * 1e-3, "choked open hat {}", level_of(&e, 46));
    // Same group, the other way: the open hat cuts a sounding closed hat
    // (10 ms later: −23 dB instead of −6 dB).
    let e = run(&[(0, on(9, 42, 100))], 580);
    let closed = level_of(&e, 42);
    let e = run(&[(0, on(9, 42, 100)), (100, on(9, 46, 100))], 580);
    assert!(level_of(&e, 42) < closed * 0.1, "closed hat {} vs {closed}", level_of(&e, 42));
    // Other groups, no group, other parts: untouched.
    for (second, part) in [(on(9, 49, 100), 9), (on(9, 51, 100), 9), (on(9, 36, 100), 9), (on(8, 42, 100), 8)]
    {
        let e = run(&[(0, on(9, 46, 100)), (4800, second)], end);
        let l = level_of(&e, 46);
        assert!(l == free, "{second:?} on part {part} changed the open hat: {l} vs {free}");
    }
    // Group 2: a crash chokes a crash, not the ride (no group).
    let e = run(&[(0, on(9, 51, 100)), (10, on(9, 49, 100)), (4800, on(9, 49, 90))], end);
    assert!(level_of(&e, 51) > 0.05, "ride cut");
}

#[test]
fn factory_kits_play_every_entry_bounded() {
    for (name, entries) in FACTORY {
        let mut e = Engine::new(48_000, 1);
        e.set_kit(0, &Kit::new(entries));
        e.set_patch(9, &Patch::KIT);
        let ev: Vec<(usize, EventKind)> =
            entries.iter().enumerate().map(|(i, x)| (i * 2400, on(9, x.note, 127))).collect();
        let out = render(&mut e, entries.len() * 2400 + 48_000, &ev, 9);
        for (i, x) in entries.iter().enumerate() {
            let seg = &out[2 * i * 2400..2 * (i + 1) * 2400];
            let peak = seg.iter().fold(0.0f32, |m, v| m.max(v.abs()));
            assert!(peak > 0.02, "{name}: note {} silent ({peak})", x.note);
        }
        assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0), "{name}");
    }
}

fn subnormal(x: f32) -> bool {
    x != 0.0 && x.abs() < f32::MIN_POSITIVE
}

#[test]
fn no_nan_no_subnormals_no_clipping_beyond_the_limiter() {
    // Every drum at extreme settings on 16 parts at once, every effect at
    // its maximum, then 12 s of tails: finite, never subnormal, within ±1;
    // the tails end in exact silence.
    let mut e = Engine::new(48_000, 11);
    let mut ev = Vec::new();
    for part in 0..16u8 {
        let kind = DrumKind::ALL[usize::from(part) % DrumKind::ALL.len()];
        let (tune, decay, x) = match part / 11 {
            // The cymbal at 4 would ring for 33 s.
            0 if kind == DrumKind::Cymbal => (24.0, 1.5, 1.0),
            0 => (24.0, 4.0, 1.0),
            _ => (-24.0, 0.1, 0.0),
        };
        e.set_patch(part, &Patch::drum(DrumParams { kind, tune, decay, tone: x, snap: 1.0 - x, level: 1.0 }));
        ev.push((0, EventKind::Param { part, param: Param::Volume, value: 1.0 }));
        for p in [Param::DelaySend, Param::ChorusSend, Param::ReverbSend] {
            ev.push((0, EventKind::Param { part, param: p, value: 1.0 }));
        }
        for k in 0..8 {
            ev.push((k * 6000 + usize::from(part) * 37, on(part, 60, 127)));
        }
    }
    e.set_delay(&DelayParams {
        beats: 0.0,
        time: 0.9,
        feedback: 0.95,
        damp: 0.0,
        low_cut: 0.0,
        ..DelayParams::DEFAULT
    });
    e.set_chorus(&ChorusParams { rate: 5.0, depth: 1.0, level: 1.0 });
    e.set_reverb(&ReverbParams { decay: 10.0, damp: 0.0, level: 1.0 });
    let frames = 48_000;
    let out = render(&mut e, frames, &ev, 3);
    assert!(out.iter().all(|x| x.is_finite() && x.abs() <= 1.0));
    assert!(!out.iter().any(|&x| subnormal(x)));
    assert!(out.iter().any(|x| x.abs() > 0.9), "not loud enough to test the limiter");
    assert!(e.limiter_gain() < 1.0, "the limiter did not engage");
    // Tails: drop the feedback and decay so they end within the test.
    e.apply(EventKind::Fx { param: FxParam::DelayFeedback, value: 0.2 });
    e.apply(EventKind::Fx { param: FxParam::ReverbDecay, value: 2.0 });
    let tail = render(&mut e, 48_000 * 16, &[], 4);
    assert!(tail.iter().all(|x| x.is_finite() && x.abs() <= 1.0 && !subnormal(*x)));
    assert!(tail[tail.len() - 4800..].iter().all(|&x| x == 0.0), "tails did not end in silence");
    assert_eq!(e.active_voices(), 0);
}

#[test]
fn delay_is_exact_at_tempo() {
    let rate = 48_000.0;
    for bpm in [60.0f32, 90.0, 120.0, 137.0, 174.0] {
        for beats in [0.25f32, 0.5, 0.75, 1.0, 1.5] {
            let exact = f64::from(beats) * 60.0 / f64::from(bpm) * 48_000.0;
            if exact > 4.0 * (DELAY_LEN - 4) as f64 {
                continue;
            }
            let mut d = Delay::new();
            let p = DelayParams {
                beats,
                feedback: 0.5,
                damp: 0.0,
                low_cut: 0.0,
                spread: 0.0,
                level: 1.0,
                ..DelayParams::DEFAULT
            };
            d.set(&p, rate, bpm);
            let (time, k) = d.timing();
            assert_eq!(time, exact.round() as usize, "{bpm} BPM × {beats}");
            // An impulse after a random lead-in (any decimation phase).
            let lead = (bpm as usize * 7 + (beats * 8.0) as usize) % 5;
            let n = lead + 2 * time + 64;
            let mut x = vec![0.0f32; n];
            x[lead] = 1.0;
            let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
            for ((xc, lc), rc) in x.chunks(97).zip(l.chunks_mut(97)).zip(r.chunks_mut(97)) {
                d.process(xc, lc, rc);
            }
            let echo = |from: usize, to: usize| {
                let w: f64 = (from..to).map(|i| f64::from(l[i])).sum();
                let c: f64 = (from..to).map(|i| i as f64 * f64::from(l[i])).sum();
                (w, c / w)
            };
            let half = time / 2;
            let (w1, c1) = echo(lead + half, lead + time + half);
            let (w2, c2) = echo(lead + time + half, lead + 2 * time + 32);
            if k == 1 {
                // Full rate: the echoes are single samples, exactly there.
                assert_eq!(l[lead + time], 1.0, "{bpm} BPM × {beats}");
                assert_eq!(l[lead + 2 * time], 0.5, "{bpm} BPM × {beats}: second repeat");
                assert_eq!(w1, 1.0);
            } else {
                // Decimated: centred within a sample.
                assert!(
                    (c1 - (lead + time) as f64).abs() <= 1.0,
                    "{bpm} × {beats} (k {k}): echo at {c1}, want {}",
                    lead + time
                );
                assert!((c2 - (lead + 2 * time) as f64).abs() <= 1.5, "{bpm} × {beats}: second at {c2}");
                assert!((w1 - 1.0).abs() < 0.02, "{bpm} × {beats}: echo gain {w1}");
                assert!((w2 - 0.5).abs() < 0.02, "{bpm} × {beats}: repeat gain {w2}");
            }
            let quiet = if k == 1 { lead + time } else { lead + time - 4 * k };
            assert!(l[..quiet].iter().all(|&v| v == 0.0), "{bpm} × {beats}: early output");
        }
    }
}

#[test]
fn delay_follows_tempo_events_in_the_engine() {
    // DelayBeats 0.5 at 100 BPM: 300 ms = 14 400 samples → beyond the
    // full-rate line; at 150 BPM: 200 ms = 9600 samples, exact.
    let mut e = Engine::new(48_000, 1);
    e.set_delay(&DelayParams { beats: 0.5, low_cut: 0.0, ..DelayParams::DEFAULT });
    e.apply(EventKind::Tempo(150.0));
    assert_eq!(e.fx().0.beats, 0.5);
    let mut d = Delay::new();
    d.set(&e.fx().0, 48_000.0, 150.0);
    assert_eq!(d.timing(), (9600, 1));
    d.set(&e.fx().0, 48_000.0, 100.0);
    assert_eq!(d.timing(), (14_400, 2));
}

#[test]
fn reverb_tail_energy_decays_at_the_set_rate() {
    // A loud hit (an impulse of 2.0 into the send) at three decay times.
    for decay in [0.8f32, 2.5, 10.0] {
        let mut v = Reverb::new();
        v.set(&ReverbParams { decay, damp: 0.0, level: 1.0 });
        // Windows of 1/20 of the decay time (3 dB each), at least 50 ms.
        let win = ((48_000.0 * decay / 20.0) as usize).max(2400);
        let dt = win as f64 / 48_000.0;
        let n = (48_000.0 * (decay * 2.0 + 1.0)) as usize / win * win;
        let mut x = vec![0.0f32; n];
        x[0] = 2.0;
        let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
        for ((xc, lc), rc) in x.chunks(256).zip(l.chunks_mut(256)).zip(r.chunks_mut(256)) {
            v.process(xc, lc, rc);
        }
        let energy: Vec<f64> = (0..n / win)
            .map(|w| {
                (w * win..(w + 1) * win).map(|i| f64::from(l[i]).powi(2) + f64::from(r[i]).powi(2)).sum()
            })
            .collect();
        // After the build-up (the first window), each window has less
        // energy than the one before, until silence.
        for w in 2..energy.len() {
            if energy[w - 1] == 0.0 {
                break;
            }
            assert!(energy[w] < energy[w - 1], "decay {decay}: window {w} {} ≥ {}", energy[w], energy[w - 1]);
        }
        // The decay rate: a least-squares line through the windows' levels
        // (dB) from the second window down to −30 dB (−15 dB at 10 s: a
        // single hit's tail at that length drains early, see fx).
        let db: Vec<f64> = energy.iter().map(|&e| 10.0 * (e / energy[1]).log10()).collect();
        let pts: Vec<(f64, f64)> = (1..db.len())
            .take_while(|&w| db[w] > if decay > 5.0 { -15.0 } else { -30.0 })
            .map(|w| (w as f64 * dt, db[w]))
            .collect();
        assert!(pts.len() >= 4, "decay {decay}: {} windows", pts.len());
        let n = pts.len() as f64;
        let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
        let (sxx, sxy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x * x, b + x * y));
        let slope = (n * sxy - sx * sy) / (n * sxx - sx * sx);
        let rt60 = (-60.0 / slope) as f32;
        println!(
            "decay {decay}: RT60 {rt60:.3} s; dB per window {:?}",
            db.iter().map(|x| *x as i32).collect::<Vec<_>>()
        );
        assert!((rt60 / decay - 1.0).abs() < 0.1, "decay {decay}: measured RT60 {rt60}");
        assert!(l.iter().chain(&r).all(|x| x.is_finite() && !subnormal(*x)));
        // Then it drains to exact zeros (no limit cycle) within 2 s.
        let mut more = vec![0.0f32; 48_000 * 2];
        let mut m2 = more.clone();
        let z = vec![0.0f32; more.len()];
        v.process(&z, &mut more, &mut m2);
        assert!(v.is_idle(), "decay {decay}: tail not silent");
    }
}

#[test]
fn ram_of_the_new_parts() {
    use orpheus_synth::ram;
    println!(
        "kit slot {} B, sample player {} B, sample cache state {} B",
        ram::kit(),
        ram::sample_player(),
        ram::sample_cache()
    );
    assert!(ram::kit() <= 300, "kit {} B", ram::kit());
    assert!(ram::sample_player() <= 64, "sample player {} B", ram::sample_player());
    assert!(ram::sample_cache() <= 6 * 1024, "sample cache state {} B", ram::sample_cache());
    println!("delay {} B, chorus {} B, reverb {} B", ram::delay(), ram::chorus(), ram::reverb());
    assert!(ram::delay() <= 24 * 1024 + 384, "delay {} B", ram::delay());
    assert!(ram::reverb() <= 8 * 1024 + 512, "reverb {} B", ram::reverb());
    assert!(ram::chorus() <= 4 * 1024 + 128, "chorus {} B", ram::chorus());
}

/// Regression (2026-11-25, SYN-2): the reverb wrote its `i16` lines
/// truncated toward zero (SYN-1's combs did the same); a tail lives at tens
/// to hundreds of steps, so the bias shortened the decay — at the first
/// version's 1/8192 steps a 0.8 s setting measured 0.48 s; at today's
/// 1/32768 and half rate, truncation alone still measures 2.14 s for 2.5 s.
/// The lines now round above a threshold that rules out limit cycles.
/// Fails with truncation alone.
#[test]
fn regress_reverb_truncation_shortened_the_decay() {
    let decay = 2.5;
    let mut v = Reverb::new();
    v.set(&ReverbParams { decay, damp: 0.0, level: 1.0 });
    let win = 6000; // 125 ms: 3 dB
    let n = 40 * win;
    let mut x = vec![0.0f32; n];
    x[0] = 2.0;
    let (mut l, mut r) = (vec![0.0f32; n], vec![0.0f32; n]);
    v.process(&x, &mut l, &mut r);
    let e: Vec<f64> = (0..n / win)
        .map(|w| (w * win..(w + 1) * win).map(|i| f64::from(l[i]).powi(2) + f64::from(r[i]).powi(2)).sum())
        .collect();
    // Least squares through the windows' levels down to −30 dB.
    let pts: Vec<(f64, f64)> = (1..e.len())
        .map(|w| (w as f64 * 0.125, 10.0 * (e[w] / e[1]).log10()))
        .take_while(|&(_, db)| db > -30.0)
        .collect();
    let k = pts.len() as f64;
    let (sx, sy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x, b + y));
    let (sxx, sxy) = pts.iter().fold((0.0, 0.0), |(a, b), (x, y)| (a + x * x, b + x * y));
    let rt60 = -60.0 / ((k * sxy - sx * sy) / (k * sxx - sx * sx));
    assert!((rt60 / f64::from(decay) - 1.0).abs() < 0.1, "measured RT60 {rt60} for {decay}");
}

/// SEQ-2's per-step locks: the drum offsets of a part change the next
/// hits only (a tune and decay offset is the kit entry retuned by hand,
/// bit for bit), setting them back does not touch a sounding hit, and
/// they stay until set back.
#[test]
fn drum_offsets_change_the_next_hits_only() {
    let p = |param, value| EventKind::Param { part: 0, param, value };
    let entries =
        [KitEntry::drum(38, DrumKind::Snare).tune(8).decay(60), KitEntry::drum(40, DrumKind::Snare)];
    let kit = Kit::new(&entries);
    let retuned = Kit::new(&[KitEntry::drum(38, DrumKind::Snare).tune(20).decay(30)]);
    let fresh = |k: &Kit| {
        let mut e = Box::new(Engine::new(48_000, 3));
        e.set_kit(0, k);
        e.set_patch(0, &Patch::KIT);
        e
    };
    // Tune +3 semitones (12 quarters), decay × 0.5.
    let locked = render(
        &mut fresh(&kit),
        9000,
        &[(100, p(Param::DrumTune, 3.0)), (100, p(Param::DrumDecay, 0.5)), (100, on(0, 38, 100))],
        5,
    );
    let by_hand = render(&mut fresh(&retuned), 9000, &[(100, on(0, 38, 100))], 6);
    assert_eq!(locked, by_hand, "an offset is the entry changed");
    let plain = render(&mut fresh(&kit), 9000, &[(100, on(0, 38, 100))], 7);
    assert_ne!(locked, plain);
    // Reset at the note-on's sample: the sounding hit keeps its tone.
    let set = [(10, p(Param::DrumTone, 0.4)), (10, on(0, 38, 90))];
    let a = render(&mut fresh(&kit), 6000, &[set[0], set[1], (10, p(Param::DrumTone, 0.0))], 8);
    let b = render(&mut fresh(&kit), 6000, &set, 9);
    assert_eq!(a, b, "setting back leaves the sounding hit alone");
    // The next hit: plain after the reset, offset without it.
    let next = (3000, on(0, 40, 90));
    let reset = render(&mut fresh(&kit), 9000, &[set[0], set[1], (10, p(Param::DrumTone, 0.0)), next], 10);
    let kept = render(&mut fresh(&kit), 9000, &[set[0], set[1], next], 11);
    assert_eq!(reset[..6000], kept[..6000]);
    assert_ne!(reset[6000..], kept[6000..], "the offset stays until set back");
}
