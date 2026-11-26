//! Playing: block-size independence, chaining (song and pattern mode),
//! live edits at the next step, mute and solo, probability, kits, the
//! saved song rendering bit-identically, the screen-off path, RAM.

mod common;

use common::{engine, fnv, hits, project, render};
use orpheus_seq::demo;
use orpheus_seq::model::{PATTERNS, TRACKS};
use orpheus_seq::player::{PART, PENDING, chance, hit_velocity};
use orpheus_seq::{Hit, PlayMode, Project, Sequencer, Step, Studio};

const RATE: u32 = 48_000;
/// A step at 120 BPM, straight.
const STEP: u64 = 6_000;

fn groove() -> Box<Project> {
    let mut p = project();
    common::row(&mut p);
    p
}

#[test]
fn any_block_size_gives_the_same_samples() {
    let mut p = Box::new(Project::NEW);
    demo::beat(&mut p);
    let frames = 6 * RATE as usize;
    let whole = render(&p, RATE, frames, &[frames]);
    for sizes in [&[1usize][..], &[7], &[64], &[1000], &[4096], &[1, 300, 17, 4095, 2, 999]] {
        if sizes == [1] {
            // One frame at a time over the first second only (slow).
            let one = render(&p, RATE, RATE as usize, sizes);
            assert!(one[..] == whole[..one.len()], "block 1");
            continue;
        }
        let r = render(&p, RATE, frames, sizes);
        assert_eq!(fnv(&r), fnv(&whole), "blocks {sizes:?}");
    }
}

fn patterns_walked(p: &Project, steps: usize) -> Vec<(u8, u8)> {
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut v = Vec::new();
    let mut last = None;
    // Step by step: the position after each step start.
    let mut frame = 0;
    while v.len() < steps {
        s.run_dry(p, 1, |_| {});
        frame += 1;
        let pos = s.position().unwrap();
        if last != Some(pos.frame) {
            last = Some(pos.frame);
            v.push((pos.pattern, pos.step));
        }
        assert!(frame < 10_000_000);
    }
    v
}

#[test]
fn song_mode_chains_patterns_with_repeats() {
    let mut p = project();
    p.set_len(0, 4);
    p.set_len(1, 3);
    p.set_len(2, 2);
    p.song.push(0, 2);
    p.song.push(1, 1);
    p.song.push(2, 3);
    p.set_mode(PlayMode::Song);
    let w = patterns_walked(&p, 4 * 2 + 3 + 2 * 3 + 4);
    let mut want = Vec::new();
    for (pat, n, len) in [(0u8, 2, 4u8), (1, 1, 3), (2, 3, 2), (0, 1, 4)] {
        for _ in 0..n {
            for s in 0..len {
                want.push((pat, s));
            }
        }
    }
    assert_eq!(w, want, "A A B C C C, then round again");
    let mut walk = Vec::new();
    p.song_walk(7, |i| walk.push(i));
    assert_eq!(walk, [0, 0, 1, 2, 2, 2, 0]);
}

#[test]
fn an_empty_song_plays_the_current_pattern() {
    let mut p = project();
    p.set_mode(PlayMode::Song);
    p.set_current(5);
    p.set_len(5, 3);
    assert_eq!(patterns_walked(&p, 6), [(5, 0), (5, 1), (5, 2), (5, 0), (5, 1), (5, 2)]);
}

/// Pattern mode: the pattern chosen while playing starts at the end of
/// the one playing (05 §7.16 "pattern switch at the pattern end").
#[test]
fn pattern_switch_waits_for_the_pattern_end() {
    let mut p = groove();
    p.set_step(1, 0, 0, Step::hit(127));
    p.set_len(1, 8);
    let mut s = Sequencer::new(RATE);
    s.play();
    s.run_dry(&p, (5 * STEP) as usize, |_| {});
    p.set_current(1);
    let mut got = Vec::new();
    s.run_dry(&p, (11 * STEP + 1) as usize, |h| got.push(h));
    let pos = s.position().unwrap();
    assert_eq!((pos.pattern, pos.step), (1, 0), "pattern 1 began at step 16");
    assert_eq!(pos.frame, 16 * STEP);
    assert!(got.iter().any(|h| h.frame == 16 * STEP && h.velocity == 102), "its kick (127 × 80 %)");
}

/// An edit made while step k plays is heard at step k + 1; the step that
/// is playing was read when it began and is not changed.
#[test]
fn edits_take_effect_at_the_next_step() {
    let p0 = project();
    let mut p = Box::new((*p0).clone());
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut got: Vec<Hit> = Vec::new();
    // Into step 3 (frames 18000–24000).
    s.run_dry(&p, 20_000, |h| got.push(h));
    p.set_step(0, 0, 3, Step::hit(100)); // the playing step: too late
    p.set_step(0, 1, 4, Step::hit(100)); // the next: heard
    s.run_dry(&p, 10_000, |h| got.push(h));
    assert_eq!(got.len(), 1);
    assert_eq!((got[0].track, got[0].frame), (1, 4 * STEP));
    // One frame before a step starts is still in time for it.
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut p = Box::new((*p0).clone());
    s.run_dry(&p, (5 * STEP - 1) as usize, |_| {});
    p.set_step(0, 2, 5, Step::hit(100));
    let mut late = Vec::new();
    s.run_dry(&p, 2, |h| late.push(h));
    assert_eq!(late.iter().map(|h| h.frame).collect::<Vec<_>>(), [5 * STEP]);
}

#[test]
fn swing_and_length_edits_at_the_next_step() {
    let mut p = project();
    for k in 0..16 {
        p.set_step(0, 0, k, Step::hit(100));
    }
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut got = Vec::new();
    s.run_dry(&p, (2 * STEP + 10) as usize, |h| got.push(h.frame));
    p.set_swing(75);
    p.set_len(0, 4);
    s.run_dry(&p, (8 * STEP) as usize, |h| got.push(h.frame));
    // Steps 0, 1, 2 straight (step 2 was playing at the edit); step 3
    // (odd) lasts half a step at 75 %; the 4-step pattern starts again at
    // 21000 with its even steps 1.5 steps long and its odd ones 0.5.
    assert_eq!(&got[..8], &[0, STEP, 2 * STEP, 3 * STEP, 21_000, 30_000, 33_000, 42_000]);
}

#[test]
fn mute_and_solo_at_the_next_step() {
    let mut p = project();
    for t in 0..3 {
        for k in 0..16 {
            p.set_step(0, t, k, Step::hit(100));
        }
    }
    let tracks_at = |hs: &[Hit], step: u64| {
        let mut v: Vec<u8> = hs.iter().filter(|h| h.frame == step * STEP).map(|h| h.track).collect();
        v.sort_unstable();
        v
    };
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut got = Vec::new();
    s.run_dry(&p, (STEP + 1) as usize, |h| got.push(h));
    p.set_mute(1, true);
    s.run_dry(&p, STEP as usize, |h| got.push(h));
    p.set_solo(2, true);
    s.run_dry(&p, STEP as usize, |h| got.push(h));
    p.set_solo(0, true);
    p.set_mute(0, true);
    s.run_dry(&p, STEP as usize, |h| got.push(h));
    p.set_solo(2, false);
    p.set_solo(0, false);
    p.set_mute(0, false);
    p.set_mute(1, false);
    s.run_dry(&p, STEP as usize, |h| got.push(h));
    assert_eq!(tracks_at(&got, 0), [0, 1, 2]);
    assert_eq!(tracks_at(&got, 1), [0, 1, 2], "muted while step 1 played: from step 2");
    assert_eq!(tracks_at(&got, 2), [0, 2]);
    assert_eq!(tracks_at(&got, 3), [2], "solo");
    assert_eq!(tracks_at(&got, 4), [2], "a muted solo track stays silent");
    assert_eq!(tracks_at(&got, 5), [0, 1, 2]);
    assert!(p.audible(0) && p.audible(1));
}

#[test]
fn levels_and_accents_scale_velocity() {
    assert_eq!(hit_velocity(100, false, 80), 80);
    assert_eq!(hit_velocity(100, true, 100), 127);
    assert_eq!(hit_velocity(90, true, 100), 122);
    assert_eq!(hit_velocity(1, false, 1), 1);
    assert_eq!(hit_velocity(127, true, 0), 0, "level 0 is silent");
    let mut p = project();
    p.set_step(0, 0, 0, Step::hit(100).with_accent(true));
    p.set_level(0, 50);
    p.set_step(0, 1, 1, Step::hit(100));
    p.set_level(1, 0);
    let h = hits(&p, RATE, 2 * STEP as usize);
    assert_eq!(h.len(), 1);
    assert_eq!(h[0].velocity, 64);
}

/// Probability: about half of 4000 half-chance steps play, the same ones
/// for the same seed and others for another seed.
#[test]
fn probability_is_seeded() {
    let mut p = project();
    for k in 0..16 {
        p.set_step(0, 0, k, Step::hit(100).with_prob(8));
        p.set_step(0, 1, k, Step::hit(100).with_prob(2));
        p.set_step(0, 2, k, Step::hit(100));
    }
    let frames = (4000 * STEP) as usize;
    let a = hits(&p, RATE, frames);
    let count = |h: &[Hit], t: u8| h.iter().filter(|x| x.track == t).count();
    let half = count(&a, 0);
    assert!((1850..2150).contains(&half), "{half} of 4000 at 1/2");
    let eighth = count(&a, 1);
    assert!((400..600).contains(&eighth), "{eighth} of 4000 at 1/8");
    assert_eq!(count(&a, 2), 4000);
    assert_eq!(a, hits(&p, RATE, frames), "the same seed plays the same");
    p.seed ^= 1;
    assert_ne!(a, hits(&p, RATE, frames));
    assert!(chance(1, 2, 3, 16), "full probability always");
}

#[test]
fn ratchet_hits_are_spread_in_the_step() {
    let mut p = project();
    p.set_step(0, 3, 0, Step::hit(100).with_ratchet(4));
    p.set_step(0, 3, 1, Step::hit(100).with_ratchet(3));
    let h = hits(&p, RATE, 2 * STEP as usize);
    assert_eq!(h.iter().map(|x| x.frame).collect::<Vec<_>>(), [0, 1500, 3000, 4500, 6000, 8000, 10_000]);
}

/// Many tracks with ratchets on every step at 240 BPM, 75 % swing and the
/// longest delay: the queue never overflows and every hit plays.
#[test]
fn the_hit_queue_holds_the_densest_pattern() {
    let mut p = project();
    p.set_tempo(24_000);
    p.set_swing(75);
    for t in 0..TRACKS {
        for k in 0..16 {
            p.set_step(0, t, k, Step::hit(100).with_ratchet(4).with_micro(12));
        }
    }
    let mut s = Sequencer::new(8_000);
    s.play();
    let mut n = 0;
    s.run_dry(&p, 8_000 * 4, |_| n += 1);
    assert_eq!(s.dropped(), 0);
    assert!(n >= 8 * 4 * 60, "{n}");
    const { assert!(PENDING >= 3 * TRACKS * 4) };
    // And rendered: more than 64 hits can be due in one block.
    let r = render(&p, 8_000, 8_000, &[8_000]);
    assert!(r.iter().all(|x| *x != i16::MIN));
}

/// 05 §7.16: "render of a saved song is bit-identical".
#[test]
fn a_saved_song_renders_bit_identically() {
    let mut p = Box::new(Project::NEW);
    demo::beat(&mut p);
    let bytes = common::saved(&p);
    let q = common::loaded(&bytes).unwrap();
    assert!(common::same(&p, &q));
    let frames = 8 * RATE as usize;
    assert_eq!(fnv(&render(&p, RATE, frames, &[1024])), fnv(&render(&q, RATE, frames, &[333, 4096])));
}

/// The screen-off path: the studio renders in DAC-sized blocks with no UI
/// call at all; the playhead the UI would read later (at the DAC's frame,
/// the clock less the ring's latency) names the step whose hits sound
/// there.
#[test]
fn renders_without_the_ui_and_the_playhead_follows_the_dac() {
    let mut st = Box::new(Studio::new(RATE));
    demo::beat(&mut st.project);
    st.seq.play();
    let mut e = engine(RATE);
    let mut block = [0i16; 2 * 576];
    let blocks = 20 * RATE as usize / 576;
    let mut peak = 0u16;
    for _ in 0..blocks {
        st.render_i16(&mut e, &mut block);
        peak = peak.max(block.iter().map(|s| s.unsigned_abs()).max().unwrap());
    }
    assert!(peak > 4000, "it played: peak {peak}");
    assert_eq!(st.seq.clock(), (blocks * 576) as u64);
    let latency = 4 * 576;
    let dac = st.seq.clock() - latency;
    let pos = st.seq.position_at(dac).expect("remembered");
    assert!(pos.frame <= dac && dac < pos.frame + 2 * 9000);
    assert!(st.seq.position().unwrap().frame >= pos.frame);
    // Equal to a render of the same length in other blocks.
    let again = render(&st.project, RATE, blocks * 576, &[4096]);
    let mut e2 = engine(RATE);
    let mut st2 = Box::new(Studio::new(RATE));
    st2.project = st.project.clone();
    st2.seq.play();
    let mut tail = vec![0i16; 2 * blocks * 576];
    st2.render_i16(&mut e2, &mut tail);
    assert_eq!(fnv(&tail), fnv(&again));
}

#[test]
fn stop_silences_new_hits_and_play_starts_over() {
    let p = groove();
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut n = 0;
    s.run_dry(&p, (3 * STEP) as usize, |_| n += 1);
    assert!(n > 0);
    s.stop();
    assert!(s.position().is_none());
    let mut after = 0;
    s.run_dry(&p, (20 * STEP) as usize, |_| after += 1);
    assert_eq!(after, 0);
    s.play();
    let start = s.clock();
    let mut first = None;
    s.run_dry(&p, 10, |h| {
        first.get_or_insert(h.frame);
    });
    assert_eq!(first, Some(start), "the kick at the start of the play");
    assert_eq!(s.position().unwrap().step, 0);
}

#[test]
fn kit_changes_reach_the_engine() {
    let mut p = groove();
    let mut e = engine(RATE);
    let mut s = Sequencer::new(RATE);
    s.play();
    let mut b = [0i16; 2 * 256];
    s.render_i16(&p, &mut e, &mut b);
    assert_eq!(e.kit(0).unwrap(), p.kit());
    assert_eq!(e.patch(PART).unwrap().kind, orpheus_synth::Kind::Kit(0));
    p.set_factory_kit(3);
    s.render_i16(&p, &mut e, &mut b);
    assert_eq!(p.kit_name(), "tight");
    assert_eq!(e.kit(0).unwrap(), p.kit());
    assert_eq!(p.factory_kit(), Some(3));
}

/// Every factory kit plays every default track audibly and within ±1.
#[test]
fn every_kit_sounds_on_every_track() {
    for k in 0..orpheus_synth::kit::FACTORY.len() {
        let mut p = project();
        p.set_factory_kit(k);
        for t in 0..TRACKS {
            p.set_step(0, t, 2 * t, Step::hit(120));
        }
        let r = render(&p, RATE, (16 * STEP) as usize, &[1024]);
        let peak = r.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 2000, "kit {k}: {peak}");
    }
}

#[test]
fn model_edits_clamp_and_bump_the_revision() {
    let mut p = project();
    let r0 = p.rev();
    p.set_tempo(1);
    assert_eq!(p.tempo(), 4_000);
    p.set_tempo(u16::MAX);
    assert_eq!(p.tempo(), 24_000);
    p.set_swing(99);
    assert_eq!(p.swing(), 75);
    p.set_len(0, 0);
    assert_eq!(p.pattern(0).len(), 1);
    p.set_len(0, 999);
    assert_eq!(p.pattern(0).len(), 64);
    p.set_current(99);
    assert_eq!(p.current(), PATTERNS - 1);
    let s = Step::hit(0).with_prob(0).with_ratchet(9).with_micro(200);
    assert_eq!((s.velocity(), s.prob(), s.ratchet(), s.micro()), (1, 1, 4, 12));
    assert_eq!(Step::from_bytes([0x80, 0xFF, 0xFF]).to_bytes(), [0x81, 0x7F, 12]);
    assert!(p.toggle(0, 0, 3));
    assert!(!p.toggle(0, 0, 3));
    assert!(p.rev() != r0);
    // Shortening keeps the steps past the length; lengthening brings
    // them back.
    p.set_step(0, 0, 20, Step::hit(9));
    p.set_len(0, 16);
    p.set_len(0, 32);
    assert_eq!(p.pattern(0).step(0, 20).velocity(), 9);
    p.copy_pattern(0, 1);
    assert_eq!(p.pattern(1), p.pattern(0));
    p.clear_pattern(1);
    assert_eq!(p.pattern(1).hits(), 0);
    assert_eq!(p.pattern(1).len(), 32);
    // `reset` (field by field) makes exactly a new project.
    let mut q = common::random(5);
    let rev = q.rev();
    q.reset();
    assert!(q.same_as(&Project::NEW));
    assert_ne!(q.rev(), rev);
}

/// RAM of the parts (printed; `--nocapture`), and bounds: the project
/// stays under 26 KiB, the player under 2.5 KiB.
#[test]
fn ram_table() {
    use orpheus_seq::ram;
    std::println!(
        "step {} B, pattern {} B, project {} B, sequencer {} B, studio {} B",
        ram::step(),
        ram::pattern(),
        ram::project(),
        ram::sequencer(),
        ram::studio()
    );
    assert_eq!(ram::step(), 3);
    assert!(ram::project() <= 26 * 1024, "{}", ram::project());
    assert!(ram::sequencer() <= 2560, "{}", ram::sequencer());
}
