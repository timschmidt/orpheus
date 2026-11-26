//! M17 Sequencer (SEQ-1): journeys through the app over the simulator's
//! back end (`sim/src/seq.rs`): making a beat with the keys and hearing
//! it, saving and opening it again after a restart, editing while it
//! plays, the transport (tempo, tap tempo, swing, length), step editing,
//! mute and solo from the mixer, playing on with the screen off, files
//! only under `/.orpheus/seq`.

use orpheus_apps::seq::{SeqResult, SeqService};
use orpheus_seq::Step;
use orpheus_seq::model::PlayMode;
use orpheus_sim::AppSim;
use orpheus_sim::games::read;
use orpheus_sim::seq::{RATE, SimSeq};

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

const LIST: &str = "set:seq=on goto:Sequencer wait:50 expect-screen:Sequencer";
const DEMO: &str = "set:seq=on set:seqdemo=1 goto:Sequencer wait:50 down ok wait:50 expect-screen:SeqGrid";

fn peak(a: &[i16]) -> u16 {
    a.iter().map(|x| x.unsigned_abs()).max().unwrap_or(0)
}

/// From Home: the Sequencer is the cell before Settings; a new project
/// opens an empty grid; four kicks by the keys; Menu › Play plays them
/// (audio in the capture, a hit every four steps); Back saves it under
/// `/.orpheus/seq/`; after a restart it is in the list and opens with
/// its steps.
#[test]
fn make_a_beat_save_it_and_open_it_again() {
    let mut s = sim("set:seq=on prev*2 ok wait:50 expect-screen:Sequencer ok wait:50 expect-screen:SeqGrid");
    assert_eq!(s.sys.seq().project.name(), "Beat 1");
    assert_eq!(s.sys.seq().open, Some(1), "a new project is saved at once");
    run(&mut s, "ok next*4 ok next*4 ok next*4 ok");
    let p = &s.sys.seq().project;
    let on: Vec<usize> = (0..16).filter(|&k| p.pattern(0).step(0, k).on()).collect();
    assert_eq!(on, [0, 4, 8, 12]);
    run(&mut s, "set:seqcapture=on menu ok wait:2000");
    assert!(s.sys.seq().seq.playing());
    let cap = s.sys.seq.capture.clone().unwrap();
    assert!(peak(&cap) > 4000, "the kicks sound: {}", peak(&cap));
    // 120 BPM: a kick every 0.5 s; ≈ 2 s played: 4 or 5 hits.
    let hits = s.sys.seq().seq.hits();
    assert!((4..=5).contains(&hits), "{hits} hits");
    run(&mut s, "back wait:50 expect-screen:Sequencer");
    assert!(!s.sys.seq().dirty());
    let file = read(&mut s.sys.media, "/.orpheus/seq/SEQ01.OSQ").expect("saved");
    assert!(file.starts_with(b"OSQ1"));
    run(&mut s, "back wait:50");
    assert!(!s.sys.seq().seq.playing(), "leaving the app stops it");
    // The player starts again: the studio is new, the card the same.
    s.sys.seq = SimSeq::default();
    run(&mut s, &format!("goto:Home {LIST} down ok wait:50 expect-screen:SeqGrid"));
    let p = &s.sys.seq().project;
    assert_eq!(p.name(), "Beat 1");
    assert_eq!((0..16).filter(|&k| p.pattern(0).step(0, k).on()).count(), 4);
}

/// The grid's Prev/Next wrap within the pattern's length, held they move
/// by pages; Up/Down change track; hold OK opens the step.
#[test]
fn grid_keys() {
    let mut s = sim(&format!("{DEMO} menu down*4 ok menu down*4 ok"));
    assert_eq!(s.sys.seq().project.current(), 2, "Next pattern twice");
    run(&mut s, "prev wait:50");
    run(&mut s, "hold:next:600 wait:50 expect-screen:SeqGrid");
    run(&mut s, "down*9 hold:ok wait:50 expect-screen:SeqStep");
    run(&mut s, "back wait:50 expect-screen:SeqGrid");
}

/// Edits made while the beat plays are heard from the next step: muting
/// every track from the mixer silences the output after the tails;
/// unmuting brings it back.
#[test]
fn mixer_mute_and_solo_while_playing() {
    let mut s = sim(&format!("{DEMO} set:seqcapture=on menu ok wait:1000"));
    run(&mut s, "menu down*2 ok wait:50 expect-screen:SeqMixer");
    run(&mut s, "ok down ok down ok down ok down ok down ok down ok down ok wait:50");
    assert!((0..8).all(|t| !s.sys.seq().project.audible(t)));
    let at = s.sys.seq.capture.as_ref().unwrap().len();
    run(&mut s, "wait:3000");
    let cap = s.sys.seq.capture.clone().unwrap();
    // The tails of the last hits die within 1.5 s.
    let quiet = &cap[at + 2 * 3 * RATE as usize / 2..];
    assert_eq!(peak(quiet), 0, "all muted: silence");
    // Solo the kick (hold OK on the first row): a muted solo track stays
    // silent; unmute it and it alone plays.
    run(&mut s, "up*7 hold:ok ok wait:50");
    let p = &s.sys.seq().project;
    assert!(p.track(0).solo && !p.track(0).mute);
    assert!(p.audible(0) && !(1..8).any(|t| p.audible(t)));
    let at = s.sys.seq.capture.as_ref().unwrap().len();
    run(&mut s, "wait:2000");
    let cap = s.sys.seq.capture.clone().unwrap();
    assert!(peak(&cap[at..]) > 3000, "the kick alone");
}

/// Transport: Next on Tempo adds a BPM (held: ten), tap tempo by OK taps
/// 500 ms apart gives 120, swing and length by Prev/Next, the kit by OK.
#[test]
fn transport_tempo_tap_swing_length_kit() {
    let mut s = sim(&format!("{DEMO} menu down ok wait:50 expect-screen:SeqTransport"));
    run(&mut s, "down next next");
    assert_eq!(s.sys.seq().project.tempo(), 10_000);
    run(&mut s, "prev*5");
    assert_eq!(s.sys.seq().project.tempo(), 9_500);
    run(&mut s, "down*2");
    for _ in 0..4 {
        run(&mut s, "ok wait:400");
    }
    assert_eq!(s.sys.seq().project.tempo(), 12_000, "tap tempo: 500 ms a beat");
    run(&mut s, "up next*3");
    assert_eq!(s.sys.seq().project.swing(), 59);
    run(&mut s, "down*2 prev");
    assert_eq!(s.sys.seq().project.mode(), PlayMode::Pattern);
    run(&mut s, "down*2 next*16");
    assert_eq!(s.sys.seq().project.pattern(0).len(), 32);
    run(&mut s, "down ok");
    assert_eq!(s.sys.seq().project.kit_name(), "tight");
    run(&mut s, "up*7 ok");
    assert!(s.sys.seq().seq.playing(), "Play on the transport");
}

/// The step screen edits the grid cursor's step: on, velocity, chance,
/// ratchet, delay, accent.
#[test]
fn step_editing() {
    let mut s = sim(&format!("{DEMO} down*2 next*3 hold:ok wait:50 expect-screen:SeqStep"));
    run(&mut s, "ok down next*5 down prev*4 down next*2 down next*3 down ok");
    let st = s.sys.seq().project.pattern(0).step(2, 3);
    assert_eq!(st, Step::hit(105).with_prob(12).with_ratchet(3).with_micro(3).with_accent(true));
    run(&mut s, "back wait:50 expect-screen:SeqGrid");
}

/// The track screen picks the track's sound among the kit's notes and
/// sets its level.
#[test]
fn track_sound_and_level() {
    let mut s = sim(&format!("{DEMO} down*7 menu down*3 ok wait:50 expect-screen:SeqTrack"));
    assert_eq!(s.sys.seq().project.track(7).note, 49);
    run(&mut s, "next down prev*3");
    let t = s.sys.seq().project.track(7);
    let kit = s.sys.seq().project.kit();
    let i = kit.entries().iter().position(|e| e.note == 49).unwrap();
    assert_eq!(t.note, kit.entries()[(i + 1) % kit.entries().len()].note);
    assert_eq!(t.level, 40);
}

/// Playing goes on with the screen off (the backlight timeout of 5 s;
/// the lock delay Never, so a key only wakes the screen): the audio
/// is rendered and the steps move on without the UI; the playhead is
/// where the time says when the screen comes back.
#[test]
fn plays_on_with_the_screen_off() {
    let mut s = sim(&format!("{DEMO} set:timers=run set:0x0302=0 set:0x0303=4 menu ok wait:100"));
    let (f0, k0) = (s.sys.seq.frames, s.sys.seq().seq.steps());
    run(&mut s, "wait:12000");
    assert!(!s.shell.gate.screen_on, "the screen went off");
    let frames = s.sys.seq.frames - f0;
    assert!(frames >= 12 * u64::from(RATE) - 2048, "{frames} frames in 12 s");
    let steps = s.sys.seq().seq.steps() - k0;
    // 98 BPM: 6.53 steps a second.
    assert!((77..=80).contains(&steps), "{steps} steps in 12 s");
    let pos = s.sys.seq().seq.position().unwrap();
    assert!(pos.frame <= s.sys.seq().seq.clock());
    run(&mut s, "ok wait:50");
    assert!(s.shell.gate.screen_on);
    assert_eq!(s.screen(), "SeqGrid");
}

/// Deleting the open project from the list closes it; a write failure
/// is reported on Save.
#[test]
fn delete_and_save_failure() {
    let mut s = sim(&format!("{DEMO} set:seqfail=on menu down*8 ok"));
    assert_eq!(s.sys.seq_result(), SeqResult::Failed);
    run(&mut s, "set:seqfail=off back wait:50 expect-screen:Sequencer");
    assert_eq!(s.sys.seq().list.items().len(), 1);
    // The list comes back with the project focused.
    run(&mut s, "menu ok wait:50");
    assert!(s.sys.seq().list.items().is_empty());
    assert!(read(&mut s.sys.media, "/.orpheus/seq/SEQ01.OSQ").is_none());
    assert_eq!(s.sys.seq().open, None);
}
