//! MIDI clock out (`orpheus_seq::clock`, SEQ-2): Start, 24 Clocks a
//! quarter and Stop on their frames — exact against an independent
//! computation in seconds at any rate, tempo, swing and step length; the
//! phase kept across tempo and step-length changes; nothing during the
//! count-in; Stop on a stop, on a restart and when turned off; any block
//! size the same; a full queue drops and counts without losing the phase;
//! the USB MIDI packets.

mod common;

use common::project;
use orpheus_seq::clock::{ClockEvent, PPQN, QUEUE, Realtime, tick_units};
use orpheus_seq::{Options, Project, Sequencer, Step, StepLen};

/// Play `p` for `frames` in blocks of `sizes` (in turn), collecting the
/// clock's messages after every block.
fn run(seq: &mut Sequencer, p: &Project, frames: u64, sizes: &[usize]) -> Vec<ClockEvent> {
    let mut v = Vec::new();
    let mut done = 0u64;
    let mut i = 0;
    while done < frames {
        let n = (sizes[i % sizes.len()] as u64).min(frames - done) as usize;
        i += 1;
        seq.run_dry(p, n, |_| {});
        seq.take_clock(|e| v.push(e));
        done += n as u64;
    }
    v
}

fn player(rate: u32, opts: Options) -> Sequencer {
    let mut s = Sequencer::new(rate);
    s.set_options(opts);
    s.set_midi_clock(true);
    s.play();
    s
}

fn clocks(v: &[ClockEvent]) -> Vec<u64> {
    v.iter().filter(|e| e.msg == Realtime::Clock).map(|e| e.frame).collect()
}

/// Clock `k` after a start on `start` at `tempo` (1/100 BPM): k/24 of a
/// quarter of 6000/tempo seconds, floored to a frame.
fn exact(start: u64, k: u64, rate: u32, tempo: u16) -> u64 {
    start + k * u64::from(rate) * 6_000 / (PPQN * u64::from(tempo))
}

/// At a steady tempo every Clock is on the floor of its exact time at 8,
/// 44.1 and 48 kHz, 40–240 BPM, for every step length and swing and an odd
/// pattern length; Start comes first, on the first Clock's frame.
#[test]
fn clocks_are_exact_at_any_rate_tempo_swing_and_step_length() {
    for rate in [8_000u32, 44_100, 48_000] {
        for tempo in [4_000u16, 9_337, 12_000, 24_000] {
            for swing in [50u8, 57, 75] {
                for len in [StepLen::Sixteenth, StepLen::Eighth, StepLen::ThirtySecond] {
                    let mut p = project();
                    p.set_tempo(tempo);
                    p.set_swing(swing);
                    p.set_len(0, 7);
                    let mut s = player(rate, Options { step_len: len, ..Options::default() });
                    let v = run(&mut s, &p, u64::from(rate) * 3, &[509, 64, 1_000]);
                    assert_eq!(v[0], ClockEvent { frame: 0, msg: Realtime::Start });
                    assert!(v[1..].iter().all(|e| e.msg == Realtime::Clock));
                    let c = clocks(&v);
                    // Three seconds at `tempo`: 3 · tempo/100/60 · 24 clocks.
                    let want = (3 * u64::from(tempo) * PPQN).div_ceil(6_000);
                    assert_eq!(c.len() as u64, want, "{rate} Hz {tempo} {swing} {len:?}");
                    for (k, f) in c.iter().enumerate() {
                        assert_eq!(
                            *f,
                            exact(0, k as u64, rate, tempo),
                            "{rate} Hz {tempo} {swing} {len:?} #{k}"
                        );
                    }
                    assert_eq!(s.clock_dropped(), 0);
                }
            }
        }
    }
}

/// With swing 50 every step starts on a Clock: every 6th for 1/16 steps,
/// every 12th for 1/8, every 3rd for 1/32; with swing every pair of steps.
#[test]
fn steps_start_on_clocks() {
    for (len, per) in [(StepLen::Sixteenth, 6usize), (StepLen::Eighth, 12), (StepLen::ThirtySecond, 3)] {
        for swing in [50u8, 66] {
            let mut p = project();
            p.set_tempo(13_100);
            p.set_swing(swing);
            for k in 0..16 {
                p.set_step(0, 0, k, Step::hit(100));
            }
            let mut s = player(44_100, Options { step_len: len, ..Options::default() });
            let mut steps = Vec::new();
            let mut v = Vec::new();
            for _ in 0..1_500 {
                s.run_dry(&p, 300, |h| steps.push(h.frame));
                s.take_clock(|e| v.push(e));
            }
            let c = clocks(&v);
            let every = if swing == 50 { 1 } else { 2 };
            for (i, f) in steps.iter().enumerate().step_by(every) {
                if i * per < c.len() {
                    assert_eq!(*f, c[i * per], "{len:?} swing {swing} step {i}");
                }
            }
            assert!(steps.len() > 16);
        }
    }
}

/// A tempo change takes effect at the next step, with the clock's phase:
/// before it the old tempo's clocks, from it the new tempo's, the step of
/// the change on a clock (swing 50), and nothing lost or doubled.
/// (Rendered in blocks of 500: a second of clocks would not fit the
/// queue.)
#[test]
fn tempo_change_keeps_the_phase() {
    let rate = 48_000;
    let mut p = project();
    p.set_tempo(12_000);
    for k in 0..16 {
        p.set_step(0, 0, k, Step::hit(100));
    }
    let mut s = player(rate, Options::default());
    let mut steps = Vec::new();
    let mut v = Vec::new();
    // 10 steps and a bit at 120 BPM (6,000 frames a step), then 90 BPM.
    for _ in 0..122 {
        s.run_dry(&p, 500, |h| steps.push(h.frame));
        s.take_clock(|e| v.push(e));
    }
    p.set_tempo(9_000);
    for _ in 0..192 {
        s.run_dry(&p, 500, |h| steps.push(h.frame));
        s.take_clock(|e| v.push(e));
    }
    let c = clocks(&v);
    // The change at step 11 (frame 66,000) = clock 66.
    let at = steps[11];
    assert_eq!(at, 66_000);
    for (k, f) in c.iter().enumerate() {
        let want =
            if k <= 66 { exact(0, k as u64, rate, 12_000) } else { exact(at, k as u64 - 66, rate, 9_000) };
        assert_eq!(*f, want, "clock {k}");
    }
    assert_eq!(c[66], at);
    assert!(c.len() > 120);
}

/// Changing the step length while playing leaves the clock (the tempo)
/// alone: one Clock spacing before and after, the change's step on a clock.
#[test]
fn step_length_change_keeps_the_clock() {
    let rate = 44_100;
    let mut p = project();
    p.set_tempo(10_000);
    for k in 0..16 {
        p.set_step(0, 0, k, Step::hit(100));
    }
    let mut s = player(rate, Options::default());
    let mut steps = Vec::new();
    let mut v = Vec::new();
    for _ in 0..60 {
        s.run_dry(&p, 500, |h| steps.push(h.frame));
        s.take_clock(|e| v.push(e));
    }
    s.set_options(Options { step_len: StepLen::Eighth, ..Options::default() });
    for _ in 0..400 {
        s.run_dry(&p, 500, |h| steps.push(h.frame));
        s.take_clock(|e| v.push(e));
    }
    let c = clocks(&v);
    // A 1/16 step at 100 BPM is 6,615 frames: the change at step 5 = clock 30.
    let at = steps[5];
    assert_eq!(at, 33_075);
    for (k, f) in c.iter().enumerate() {
        let want =
            if k <= 30 { exact(0, k as u64, rate, 10_000) } else { exact(at, k as u64 - 30, rate, 10_000) };
        assert_eq!(*f, want, "clock {k}");
    }
    // From then a step is 12 clocks.
    assert_eq!(steps[6], c[30 + 12]);
    assert_eq!(tick_units(StepLen::Eighth.ratio()) * 2, tick_units(StepLen::Sixteenth.ratio()));
}

/// The count-in sends nothing; Start and the first Clock are on the first
/// step's frame.
#[test]
fn count_in_sends_nothing() {
    let rate = 48_000;
    let mut p = project();
    p.set_tempo(12_000);
    p.set_step(0, 0, 0, Step::hit(100));
    let mut s = player(rate, Options { count_in: 1, ..Options::default() });
    let mut kick = None;
    let mut v = Vec::new();
    for _ in 0..200 {
        s.run_dry(&p, 600, |h| {
            if h.track == 0 && kick.is_none() {
                kick = Some(h.frame);
            }
        });
        s.take_clock(|e| v.push(e));
    }
    let first = kick.expect("the pattern starts");
    assert_eq!(first, 16 * 6_000, "one bar of count-in");
    assert_eq!(v[0], ClockEvent { frame: first, msg: Realtime::Start });
    assert_eq!(v[1], ClockEvent { frame: first, msg: Realtime::Clock });
    assert!(v.iter().all(|e| e.frame >= first));
}

/// Stop goes out on the first frame after a stop and nothing after it; a
/// restart sends Stop then Start; turning the clock off while it runs sends
/// Stop; off, nothing at all; turning it on while playing waits for the
/// next play.
#[test]
fn stop_restart_and_off() {
    let rate = 8_000;
    let p = project();
    let mut s = player(rate, Options::default());
    let mut v = run(&mut s, &p, 4_000, &[256]);
    s.stop();
    let stop_at = s.clock();
    v.extend(run(&mut s, &p, 4_000, &[256]));
    let stops: Vec<_> = v.iter().filter(|e| e.msg == Realtime::Stop).collect();
    assert_eq!(stops.len(), 1);
    assert_eq!(stops[0].frame, stop_at);
    assert!(v.iter().filter(|e| e.frame > stop_at).count() == 0, "nothing after the stop");

    // A restart while playing.
    let mut s = player(rate, Options::default());
    let _ = run(&mut s, &p, 3_000, &[100]);
    s.play();
    let at = s.clock();
    let v = run(&mut s, &p, 100, &[100]);
    assert_eq!(v[0], ClockEvent { frame: at, msg: Realtime::Stop });
    assert_eq!(v[1], ClockEvent { frame: at, msg: Realtime::Start });
    assert_eq!(v[2], ClockEvent { frame: at, msg: Realtime::Clock });

    // Off while running.
    let mut s = player(rate, Options::default());
    let _ = run(&mut s, &p, 3_000, &[100]);
    s.set_midi_clock(false);
    let at = s.clock();
    let v = run(&mut s, &p, 3_000, &[100]);
    assert_eq!(v, [ClockEvent { frame: at, msg: Realtime::Stop }]);
    assert!(s.playing(), "the music goes on");

    // Never on: nothing; on while playing: from the next play.
    let mut s = Sequencer::new(rate);
    s.play();
    assert!(run(&mut s, &p, 3_000, &[100]).is_empty());
    s.set_midi_clock(true);
    assert!(s.midi_clock());
    assert!(run(&mut s, &p, 3_000, &[100]).is_empty());
    s.play();
    let v = run(&mut s, &p, 100, &[100]);
    assert_eq!(v[0].msg, Realtime::Start);
}

/// Any block size gives the same messages on the same frames, song mode
/// and pattern changes included (the clock runs on across them).
#[test]
fn any_block_size_the_same() {
    let rate = 44_100;
    let mut p = project();
    orpheus_seq::demo::song(&mut p);
    p.set_tempo(11_700);
    let mut a = player(rate, Options::default());
    let va = run(&mut a, &p, 20 * u64::from(rate), &[4_096]);
    let mut b = player(rate, Options::default());
    let vb = run(&mut b, &p, 20 * u64::from(rate), &[1, 17, 333, 2_000, 64]);
    assert_eq!(va, vb);
    let c = clocks(&va);
    for (k, f) in c.iter().enumerate() {
        assert_eq!(*f, exact(0, k as u64, rate, 11_700));
    }
}

/// A full queue drops (and counts) messages; the next ones are on their
/// frames all the same.
#[test]
fn full_queue_drops_and_keeps_the_phase() {
    let rate = 48_000;
    let mut p = project();
    p.set_tempo(24_000);
    let mut s = player(rate, Options::default());
    // One second at 240 BPM: 96 clocks and Start into a queue of 32.
    s.run_dry(&p, rate as usize, |_| {});
    let mut v = Vec::new();
    s.take_clock(|e| v.push(e));
    assert_eq!(v.len(), QUEUE);
    assert_eq!(s.clock_dropped(), 96 + 1 - QUEUE as u32);
    let v = run(&mut s, &p, u64::from(rate), &[1_000]);
    let c = clocks(&v);
    assert_eq!(c.len(), 96);
    for (k, f) in c.iter().enumerate() {
        assert_eq!(*f, exact(0, 96 + k as u64, rate, 24_000));
    }
}

/// USB MIDI 1.0 event packets: code index 0xF (single byte) on the cable.
#[test]
fn usb_midi_packets() {
    assert_eq!(Realtime::Clock.usb_packet(0), [0x0F, 0xF8, 0, 0]);
    assert_eq!(Realtime::Start.usb_packet(1), [0x1F, 0xFA, 0, 0]);
    assert_eq!(Realtime::Stop.usb_packet(0x2F), [0xFF, 0xFC, 0, 0]);
}
