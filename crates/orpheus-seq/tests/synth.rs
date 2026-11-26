//! SEQ-2's playing: synth notes (exact on and off frames at any tempo,
//! swing, delay and gate; chords in the scale; transpose; ratchets; the
//! sounding-note table), parameter locks applied at their step (dry and
//! as sound, against the engine fed by hand), live recording quantised to
//! the nearest step, fill while held, the metronome, the count-in, the
//! step length, kit B.

mod common;

use common::{engine, project};
use orpheus_seq::model::{GATES, TRACKS};
use orpheus_seq::player::{
    CLICK, CLICK_NOTES, HELD, NO_LOCK, PART_SYNTH, lock_value, step_units, units_to_frames,
};
use orpheus_seq::{
    Chord, Hit, LockParam, Options, Project, Scale, Sequencer, Step, StepLen, Studio, SynthStep,
};
use orpheus_synth::{Engine, Event, EventKind, Param};

const RATE: u32 = 48_000;
/// A step at 120 BPM, straight.
const STEP: u64 = 6_000;
const BASS: usize = TRACKS;
const LEAD: usize = TRACKS + 1;

fn dry(p: &Project, frames: usize, opts: Options) -> Vec<Hit> {
    let mut s = Sequencer::new(RATE);
    s.set_options(opts);
    s.play();
    let mut v = Vec::new();
    s.run_dry(p, frames, |h| v.push(h));
    v
}

fn ons(h: &[Hit], track: usize) -> Vec<(u64, u8)> {
    h.iter().filter(|x| usize::from(x.track) == track && x.velocity > 0).map(|x| (x.frame, x.note)).collect()
}

fn offs(h: &[Hit], track: usize) -> Vec<(u64, u8)> {
    h.iter().filter(|x| usize::from(x.track) == track && x.velocity == 0).map(|x| (x.frame, x.note)).collect()
}

/// Every synth note starts on the floor of its exact time and ends on the
/// floor of its exact end (start + gate), at 8, 44.1 and 48 kHz, 40–240
/// BPM, swing 50–75 %, every delay and every gate up to four steps;
/// computed independently in seconds.
#[test]
fn synth_notes_start_and_end_on_their_exact_frames() {
    for rate in [8_000u32, 44_100, 48_000] {
        for tempo in [4_000u16, 9_337, 13_333, 24_000] {
            for swing in [50u8, 61, 75] {
                let mut p = project();
                p.set_tempo(tempo);
                p.set_swing(swing);
                for k in 0..16 {
                    let s = SynthStep::note(40 + k as u8, 100).with_gate((k % 8) as u8);
                    let s = s.with_common(s.common().with_micro((k % 13) as u8));
                    p.set_synth_step(0, 0, k, s);
                }
                // Seconds of `units` (a pair is 30/BPM s).
                let exact = |units: u64| -> u64 {
                    // frames = units / 57600 · 30 · 100 / tempo · rate
                    u64::try_from(i128::from(units) * 3000 * i128::from(rate) / (57_600 * i128::from(tempo)))
                        .unwrap()
                };
                let mut s = Sequencer::new(rate);
                s.play();
                let mut h = Vec::new();
                let len: u64 = (0..16).map(|k| step_units(swing, k)).sum();
                s.run_dry(&p, exact(len + 4 * 28_800) as usize, |x| h.push(x));
                let on = ons(&h, BASS);
                let off = offs(&h, BASS);
                assert!(on.len() >= 16, "16 notes");
                let mut start = 0u64;
                for k in 0..16 {
                    let u = start + (k as u64 % 13) * 1200;
                    let g = u64::from(GATES[k % 8]) * 7200;
                    assert_eq!(
                        on[k],
                        (exact(u), 40 + k as u8),
                        "rate {rate} tempo {tempo} swing {swing} step {k}"
                    );
                    let o = off.iter().find(|x| x.1 == 40 + k as u8).expect("an off");
                    assert_eq!(o.0, exact(u + g), "the end of step {k}'s note");
                    let hit = h.iter().find(|x| x.velocity > 0 && x.note == 40 + k as u8).unwrap();
                    assert_eq!(u64::from(hit.len), exact(u + g) - exact(u));
                    start += step_units(swing, k);
                }
            }
        }
    }
}

/// Synth notes in a long play: the 10,000th step's note is still on the
/// floor of its exact frame (no drift), and so is its end.
#[test]
fn synth_notes_do_not_drift() {
    let mut p = project();
    p.set_tempo(13_717);
    p.set_swing(58);
    p.set_synth_step(0, 1, 3, SynthStep::note(70, 90).with_gate(6));
    let mut s = Sequencer::new(44_100);
    s.play();
    // Loop `k`'s step 3 lies at k · 8 pairs + the three swung steps before.
    let loop_units = 8 * 57_600u64;
    let at = |k: u64| k * loop_units + 576 * (58 + 42 + 58);
    let f = |u: u64| units_to_frames(u, 44_100, 13_717);
    let frames = f(650 * loop_units) as usize;
    let mut k = 0u64;
    s.run_dry(&p, frames, |h| {
        if h.velocity > 0 {
            assert_eq!(h.frame, f(at(k)), "loop {k}");
            assert_eq!(u64::from(h.len), f(at(k) + 12 * 7200) - h.frame);
            k += 1;
        }
    });
    assert_eq!(k, 650, "every loop's note");
}

/// Chords are built in the project's scale on the step's note plus the
/// pattern's transpose; a chord plays once (its ratchet ignored); a
/// single note ratchets with its gate cut to the ratchet's length.
#[test]
fn chords_transpose_and_ratchets() {
    let mut p = project();
    p.set_scale(Scale::Minor, 9);
    p.set_synth_step(0, 1, 0, SynthStep::note(57, 100).with_chord(Chord::Triad));
    let s = SynthStep::note(57, 100).with_chord(Chord::Seventh);
    p.set_synth_step(0, 1, 1, s.with_common(s.common().with_ratchet(4)));
    p.set_synth_step(0, 1, 2, SynthStep::note(45, 100).with_chord(Chord::Power));
    let s = SynthStep::note(48, 100).with_gate(7);
    p.set_synth_step(0, 0, 3, s.with_common(s.common().with_ratchet(3)));
    let h = dry(&p, (4 * STEP) as usize, Options::default());
    let lead = ons(&h, LEAD);
    assert_eq!(lead[..3], [(0, 57), (0, 60), (0, 64)], "A minor");
    assert_eq!(lead[3..7], [(STEP, 57), (STEP, 60), (STEP, 64), (STEP, 67)], "A minor 7, once");
    assert_eq!(lead[7..10], [(2 * STEP, 45), (2 * STEP, 52), (2 * STEP, 57)]);
    let bass = h.iter().filter(|x| x.track as usize == BASS && x.velocity > 0).collect::<Vec<_>>();
    assert_eq!(bass.iter().map(|x| x.frame).collect::<Vec<_>>(), [18_000, 20_000, 22_000], "three hits");
    assert!(bass.iter().all(|x| u64::from(x.len) == 2_000), "the gate (4 steps) cut to a third of a step");
    // Transpose +2: B in A minor gives B D F.
    p.set_transpose(0, 2);
    let h = dry(&p, 10, Options::default());
    assert_eq!(ons(&h, LEAD), [(0, 59), (0, 62), (0, 65)]);
}

/// The sounding-note table: a held note played again keeps one entry and
/// gets the new end; more notes than it holds end the oldest first; every
/// note gets exactly one off; a stop ends them all at the next render.
#[test]
fn sounding_notes_end_once() {
    let mut p = project();
    for k in 0..16 {
        let s = SynthStep::note(50 + (k % 3) as u8 * 4, 100).with_chord(Chord::Seventh).with_gate(9);
        p.set_synth_step(0, 1, k, s);
    }
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut h = Vec::new();
    s.run_dry(&p, (20 * STEP) as usize, |x| h.push(x));
    s.stop();
    s.run_dry(&p, 5, |x| h.push(x));
    let mut sounding: Vec<u8> = Vec::new();
    for x in &h {
        if x.velocity > 0 {
            if !sounding.contains(&x.note) {
                sounding.push(x.note);
            }
        } else {
            let i = sounding.iter().position(|n| *n == x.note).expect("an off for a sounding note");
            sounding.remove(i);
        }
        assert!(sounding.len() <= HELD);
    }
    assert!(sounding.is_empty(), "the stop ended every note");
    assert_eq!(offs(&h, LEAD).last().unwrap().0, 20 * STEP, "at the stop's render");
    assert!(!s.busy());
}

/// Locks in the dry run: the locked step's hit carries the values, the
/// track's other steps and other tracks none.
#[test]
fn locks_are_applied_at_their_step() {
    let mut p = project();
    for k in [0, 4, 8] {
        p.set_step(0, 1, k, Step::hit(100));
        p.set_synth_step(0, 0, k, SynthStep::note(40, 100));
    }
    assert!(p.set_lock(0, 4, 1, LockParam::Decay, 77));
    assert!(p.set_lock(0, 8, BASS, LockParam::Resonance, 40));
    assert!(p.set_lock(0, 8, BASS, LockParam::Cutoff, 10));
    let h = dry(&p, (12 * STEP) as usize, Options::default());
    let locks = |track: usize| {
        h.iter()
            .filter(|x| usize::from(x.track) == track && x.velocity > 0)
            .map(|x| x.locks)
            .collect::<Vec<_>>()
    };
    let n = NO_LOCK;
    assert_eq!(locks(1), [[n, n, n], [n, 77, n], [n, n, n]]);
    assert_eq!(locks(BASS), [[n, n, n], [n, n, n], [10, 40, n]]);
}

fn render(p: &Project, frames: usize) -> Vec<i16> {
    common::render(p, RATE, frames, &[999, 4096])
}

/// Drum locks as sound: a step with its tune and decay locked sounds as
/// the same step of a kit whose entry has those values — bit for bit —
/// and the next hit of the kit (another note, after the lock) is plain.
#[test]
fn drum_locks_sound_as_the_entry_changed_by_hand() {
    let mut a = project();
    a.set_step(0, 1, 0, Step::hit(110));
    a.set_note(2, 40);
    a.set_step(0, 2, 4, Step::hit(110));
    let e = *a.entry(1).unwrap();
    let tune = 96 + i16::from(e.tune) + 10;
    assert!(a.set_lock(0, 0, 1, LockParam::Tune, tune as u8));
    assert!(a.set_lock(0, 0, 1, LockParam::Decay, 35));
    let mut b = project();
    b.set_step(0, 1, 0, Step::hit(110));
    b.set_note(2, 40);
    b.set_step(0, 2, 4, Step::hit(110));
    b.edit_entry(1, |x| {
        x.tune += 10;
        x.decay = 35;
    });
    let frames = (12 * STEP) as usize;
    let (ra, rb) = (render(&a, frames), render(&b, frames));
    assert!(ra == rb, "locked = changed by hand");
    assert!(ra != render(&project_with_plain_snare(), frames));
}

fn project_with_plain_snare() -> Box<Project> {
    let mut b = project();
    b.set_step(0, 1, 0, Step::hit(110));
    b.set_note(2, 40);
    b.set_step(0, 2, 4, Step::hit(110));
    b
}

/// Synth locks as sound: the sequencer's output equals an engine fed by
/// hand with the cutoff and resonance set before the locked note and set
/// back to the patch's own before the next.
#[test]
fn synth_locks_sound_as_the_engine_fed_by_hand() {
    let mut p = project();
    p.set_synth_step(0, 0, 0, SynthStep::note(36, 100));
    p.set_synth_step(0, 0, 4, SynthStep::note(43, 100));
    assert!(p.set_lock(0, 0, BASS, LockParam::Cutoff, 100));
    assert!(p.set_lock(0, 0, BASS, LockParam::Resonance, 80));
    let frames = (8 * STEP) as usize;
    let got = render(&p, frames);
    let patch = p.synth(0).patch;
    let part = PART_SYNTH[0];
    let mut e = Box::new(Engine::new(RATE, 1));
    e.set_patch(part, &patch);
    let pe =
        |at: u64, param, value| Event { offset: at as u32, kind: EventKind::Param { part, param, value } };
    let ev = [
        pe(0, Param::Cutoff, lock_value(LockParam::Cutoff, 100)),
        pe(0, Param::Resonance, 0.8),
        Event::note_on(0, part, 36, 100),
        Event::note_off(STEP as u32, part, 36),
        pe(4 * STEP, Param::Cutoff, patch.filter.cutoff),
        pe(4 * STEP, Param::Resonance, patch.filter.resonance),
        Event::note_on(4 * STEP as u32, part, 43, 100),
        Event::note_off(5 * STEP as u32, part, 43),
    ];
    let mut want = vec![0i16; 2 * frames];
    e.render_i16(&mut want, &ev);
    assert!(got == want, "the same samples");
    let mut plain = p.clone();
    plain.clear_lock(0, 0, BASS, LockParam::Cutoff);
    assert!(render(&plain, frames) != got, "the lock is heard");
}

/// Live recording: a key pressed while playing writes its note into the
/// step it falls in, or the next one from the step's middle on (with
/// swing: the middle of the swung step), across the pattern's end; it is
/// heard at once; nothing is written while stopped or counting in.
#[test]
fn live_record_quantises_to_the_nearest_step() {
    let mut st = Box::new(Studio::new(RATE));
    st.project.set_swing(66);
    st.seq.play();
    let mut e = engine(RATE);
    // A step pair lasts 12,000 frames: the even step 7,920, the odd 4,080.
    let even = units_to_frames(step_units(66, 0), RATE, 12_000);
    assert_eq!(even, 7_920);
    let mut at = 0u64;
    let mut go = |st: &mut Studio, to: u64| {
        let mut b = vec![0i16; 2 * (to - at) as usize];
        st.render_i16(&mut e, &mut b);
        at = to;
    };
    // Step 2 starts at 12,000: 3,959 frames in is before its middle.
    go(&mut st, 12_000 + 3_959);
    assert_eq!(st.key_note(BASS, 50, 90, true, 12_000 + 3_959), Some((0, 2)));
    go(&mut st, 12_000 + 3_960);
    assert_eq!(st.key_note(BASS, 52, 90, true, 12_000 + 3_960), Some((0, 3)), "the middle: the next step");
    // Step 3 (odd) is 4,080 long: its middle at 2,040.
    go(&mut st, 19_920 + 2_039);
    assert_eq!(st.key_note(BASS, 53, 90, true, 19_920 + 2_039), Some((0, 3)));
    // The last step's second half: step 1 of the next time round.
    go(&mut st, 7 * 12_000 + 7_920 + 3_000);
    assert_eq!(st.key_note(LEAD, 60, 90, true, 7 * 12_000 + 7_920 + 3_000), Some((0, 0)));
    let pat = st.project.pattern(0);
    assert_eq!(pat.synth_step(0, 2), SynthStep::note(50, 90));
    assert_eq!(pat.synth_step(0, 3).pitch(), 53, "the later key wins");
    assert!(pat.synth_step(1, 0).on());
    // A drum track: the hit at the step.
    go(&mut st, 8 * 12_000 + 100);
    assert_eq!(st.key_note(1, 38, 100, true, 8 * 12_000 + 100), Some((0, 0)));
    assert!(st.project.pattern(0).step(1, 0).on());
    // Heard at once: the next render starts with the note.
    let mut heard = Vec::new();
    st.key_note(BASS, 47, 80, false, 0);
    st.seq.run_dry(&st.project, 1, |h| heard.push(h));
    assert!(heard.iter().any(|h| h.note == 47 && h.velocity == 80 && h.frame == at));
    // Stopped: heard, not written.
    st.seq.stop();
    let before = *st.project.pattern(0);
    assert_eq!(st.key_note(BASS, 41, 80, true, at), None);
    assert_eq!(*st.project.pattern(0), before);
}

/// Fill while held: from the next step the steps come from the fill
/// pattern at the same position (within its length); let go, the
/// pattern's own steps again. No fill pattern: nothing changes.
#[test]
fn fill_plays_while_held() {
    let mut p = project();
    for k in 0..16 {
        p.set_step(0, 0, k, Step::hit(100));
    }
    p.set_len(3, 4);
    for k in 0..4 {
        p.set_step(3, 1, k, Step::hit(100));
    }
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut h = Vec::new();
    s.run_dry(&p, (3 * STEP) as usize, |x| h.push(x));
    s.set_fill(true);
    s.run_dry(&p, (3 * STEP) as usize, |x| h.push(x));
    assert!(s.fill());
    let tracks: Vec<u8> = h.iter().map(|x| x.track).collect();
    assert_eq!(tracks, [0, 0, 0, 0, 0, 0], "no fill pattern: the pattern plays");
    p.set_fill(Some(3));
    let mut h = Vec::new();
    s.run_dry(&p, (5 * STEP) as usize, |x| h.push(x));
    s.set_fill(false);
    s.run_dry(&p, (2 * STEP) as usize, |x| h.push(x));
    let got: Vec<(u64, u8)> = h.iter().map(|x| (x.frame / STEP, x.track)).collect();
    assert_eq!(got, [(6, 1), (7, 1), (8, 1), (9, 1), (10, 1), (11, 0), (12, 0)]);
    assert_eq!(s.position().unwrap().pattern, 0, "the position stays the pattern's");
}

/// The metronome clicks every beat (accented on the bar) from the play's
/// start; a count-in of one or two bars clicks first and the pattern
/// starts after it; the step length makes a step an eighth or a
/// thirty-second note.
#[test]
fn metronome_count_in_and_step_length() {
    let mut p = project();
    p.set_step(0, 0, 0, Step::hit(100));
    let clicks = |h: &[Hit]| -> Vec<(u64, u8)> {
        h.iter().filter(|x| x.track == CLICK).map(|x| (x.frame / STEP, x.note)).collect()
    };
    let h = dry(&p, (32 * STEP) as usize, Options { metronome: true, ..Options::default() });
    let c = clicks(&h);
    assert_eq!(c.len(), 8);
    assert_eq!(c[0], (0, CLICK_NOTES[0]));
    assert_eq!(c[1], (4, CLICK_NOTES[1]));
    assert_eq!(c[4], (16, CLICK_NOTES[0]));
    assert!(clicks(&dry(&p, (32 * STEP) as usize, Options::default())).is_empty(), "off: none");
    for bars in [1u8, 2] {
        let mut s = Sequencer::new(RATE);
        s.set_options(Options { count_in: bars, ..Options::default() });
        s.play();
        assert_eq!(s.counting_in(), u32::from(bars) * 4);
        let mut h = Vec::new();
        s.run_dry(&p, (STEP + 1) as usize, |x| h.push(x));
        assert_eq!(s.counting_in(), u32::from(bars) * 4, "in the first beat");
        s.run_dry(&p, (3 * STEP) as usize, |x| h.push(x));
        assert_eq!(s.counting_in(), u32::from(bars) * 4 - 1, "in the second");
        assert!(s.position().is_none() && s.quantize(4 * STEP).is_none(), "no step while counting in");
        s.run_dry(&p, (u64::from(bars) * 16 * STEP - 4 * STEP) as usize, |x| h.push(x));
        let c = clicks(&h);
        assert_eq!(c.len(), 4 * usize::from(bars), "a click a beat");
        let kick = h.iter().find(|x| x.track == 0).expect("the pattern starts");
        assert_eq!(kick.frame, u64::from(bars) * 16 * STEP);
        assert_eq!(s.counting_in(), 0);
    }
    let first = |len| {
        let h = dry(&p, (40 * STEP) as usize, Options { step_len: len, ..Options::default() });
        h.iter().filter(|x| x.track == 0).map(|x| x.frame).collect::<Vec<_>>()
    };
    assert_eq!(first(StepLen::Sixteenth)[..2], [0, 16 * STEP]);
    assert_eq!(first(StepLen::Eighth)[..2], [0, 32 * STEP]);
    assert_eq!(first(StepLen::ThirtySecond)[..3], [0, 8 * STEP, 16 * STEP]);
    // The metronome follows: a beat is two eighths or eight thirty-seconds.
    let h = dry(
        &p,
        (16 * STEP) as usize,
        Options { metronome: true, step_len: StepLen::Eighth, ..Options::default() },
    );
    assert_eq!(clicks(&h).iter().map(|c| c.0).collect::<Vec<_>>(), [0, 4, 8, 12]);
}

/// Kit B: a drum track on kit B plays on the engine's second kit part,
/// with the project's kit B in the engine's second slot.
#[test]
fn kit_b_reaches_the_engine() {
    let mut p = project();
    p.set_factory_kit_of(1, 4);
    p.set_note(5, 63);
    p.set_track_kit(5, 1);
    p.set_step(0, 5, 0, Step::hit(120));
    let mut e = engine(RATE);
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut b = vec![0i16; 2 * 4800];
    s.render_i16(&p, &mut e, &mut b);
    assert_eq!(e.kit(1).unwrap(), p.kit_of(1));
    assert_eq!(e.patch(orpheus_seq::player::PART_KIT_B).unwrap().kind, orpheus_synth::Kind::Kit(1));
    assert!(b.iter().any(|x| x.unsigned_abs() > 1000), "the conga sounds");
    // Kit A (punch) has no note 63: on kit A it is silent.
    let mut q = p.clone();
    q.set_factory_kit(2);
    q.set_track_kit(5, 0);
    assert!(render(&q, 4800).iter().all(|x| *x == 0));
}
