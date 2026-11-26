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

// ------------------------------------------------------------ SEQ-2

const SONG: &str = "set:seq=on set:seqsong=1 goto:Sequencer wait:50 down ok wait:50 expect-screen:SeqGrid";

/// Note entry on the keys: from a new project's grid, Menu › Keys opens
/// the bass's strip; Prev/Next walk the scale (chromatic here), Up an
/// octave, OK enters the note at the step cursor and moves on, hold OK
/// takes the last one back; the notes play (audio in the capture) and
/// survive a save and a restart.
#[test]
fn note_entry_on_the_keys() {
    let mut s = sim(&format!("{LIST} ok wait:50 expect-screen:SeqGrid down*8 menu down*10 ok wait:50"));
    assert_eq!(s.screen(), "SeqKeys");
    // C2 (the bass's default), Next ×7: G2; OK; Up: G3; Prev ×2: F3; OK.
    run(&mut s, "next*7 ok up prev*2 ok next*2 ok hold:ok");
    let p = &s.sys.seq().project;
    let b = |k| p.pattern(0).synth_step(0, k);
    assert_eq!((b(0).on(), b(0).pitch()), (true, 43));
    assert_eq!((b(1).on(), b(1).pitch()), (true, 53));
    assert!(!b(2).on(), "held OK took step 3's note back");
    run(&mut s, "set:seqcapture=on back wait:50 menu ok wait:1000");
    let cap = s.sys.seq.capture.clone().unwrap();
    assert!(peak(&cap) > 3000, "the bass sounds: {}", peak(&cap));
    run(&mut s, "menu ok back wait:50 expect-screen:Sequencer");
    s.sys.seq = SimSeq::default();
    run(&mut s, &format!("goto:Home {LIST} down ok wait:50 expect-screen:SeqGrid"));
    assert_eq!(s.sys.seq().project.pattern(0).synth_step(0, 1).pitch(), 53);
}

/// The scale lock: with A minor, the keys walk only its notes and a
/// note entered between them snaps; a triad on the step screen is the
/// scale's.
#[test]
fn scale_lock_on_the_keys() {
    // Transport: scale (row 9) Minor, root (row 10) A.
    let mut s =
        sim(&format!("{LIST} ok wait:50 menu down ok wait:50 down*9 next*2 down next*9 back wait:50"));
    assert_eq!(s.sys.seq().project.scale(), (orpheus_seq::Scale::Minor, 9));
    run(&mut s, "down*8 menu down*10 ok wait:50 expect-screen:SeqKeys");
    // C2 is in A minor; Next: D2, E2, F2 (no C♯, D♯).
    run(&mut s, "next ok next ok next ok");
    let p = &s.sys.seq().project;
    let notes: Vec<u8> = (0..3).map(|k| p.pattern(0).synth_step(0, k).pitch()).collect();
    assert_eq!(notes, [38, 40, 41]);
}

/// Live recording: Record on, playing, a key pressed writes its note into
/// the step nearest to the press (quantised), wherever the cursor is; the
/// write is heard at once.
#[test]
fn live_record_from_the_keys() {
    let mut s = sim(&format!("{LIST} ok wait:50 down*8 menu down*10 ok wait:50 menu down ok back"));
    run(&mut s, "set:seqdry=on menu ok wait:1250");
    // 120 BPM: a step is 125 ms; 1,250 ms in: step 10 (or 11) has begun.
    let at = s.sys.seq().seq.position().unwrap().step;
    run(&mut s, "ok");
    let p = &s.sys.seq().project;
    let on: Vec<usize> = (0..16).filter(|&k| p.pattern(0).synth_step(0, k).on()).collect();
    assert_eq!(on.len(), 1, "one note recorded");
    assert!(on[0] == usize::from(at) || on[0] == usize::from(at) + 1, "near step {at}: {on:?}");
    assert_eq!(p.pattern(0).synth_step(0, on[0]).pitch(), 36);
    run(&mut s, "wait:100");
    assert!(s.sys.seq.notes.iter().any(|h| h.track == 8 && h.note == 36 && h.velocity > 0), "heard");
}

/// Parameter locks from the step screen: Prev/Next on a lock row lock the
/// parameter (from the track's own value), OK clears it; with the
/// pattern's store full a new lock is refused and nothing changes.
#[test]
fn locks_from_the_step_screen() {
    let mut s = sim(&format!("{DEMO} down next*4 hold:ok wait:50 expect-screen:SeqStep down*6"));
    // The snare's Tune lock row: Next locks a quarter tone above its tune.
    run(&mut s, "next next");
    let p = &s.sys.seq().project;
    let e = p.entry(1).unwrap();
    let base = 96 + i16::from(e.tune);
    assert_eq!(p.pattern(0).locks.get(4, 1, orpheus_seq::LockParam::Tune), Some((base + 2) as u8));
    run(&mut s, "ok");
    assert!(s.sys.seq().project.pattern(0).locks.is_empty(), "OK clears");
    run(&mut s, "set:seqlocksfull=1 down next");
    let locks = s.sys.seq().project.pattern(0).locks;
    assert!(locks.is_full());
    assert_eq!(locks.get(4, 1, orpheus_seq::LockParam::Decay), None, "refused");
}

/// The sound screens: a drum track's tune changes its kit entry (heard on
/// the next hit); a synth track's preset and cutoff change its patch.
#[test]
fn sound_screens_edit_the_kit_and_the_patch() {
    let mut s = sim(&format!("{DEMO} menu down*9 ok wait:50 expect-screen:SeqSound"));
    let before = *s.sys.seq().project.entry(0).unwrap();
    run(&mut s, "next next down next");
    let after = *s.sys.seq().project.entry(0).unwrap();
    assert_eq!(after.tune, before.tune + 8, "two semitones up");
    assert_eq!(after.decay, before.decay + 5);
    run(&mut s, "back wait:50 down*9 menu down*9 ok wait:50 expect-screen:SeqSound");
    let name0 = s.sys.seq().project.synth(1).name;
    run(&mut s, "next");
    assert_ne!(s.sys.seq().project.synth(1).name, name0, "another preset");
    let cut = s.sys.seq().project.synth(1).patch.filter.cutoff;
    run(&mut s, "down*2 prev*3");
    let p = &s.sys.seq().project;
    assert!(p.synth(1).patch.filter.cutoff < cut);
    assert_eq!(p.synth(1).name.as_str(), "custom");
}

/// The song screen: add entries, change their pattern and repeats,
/// delete one; Menu › Play the song plays the chain in song mode.
#[test]
fn song_editing() {
    let mut s = sim(&format!("{LIST} ok wait:50 menu down*11 ok down ok wait:50 expect-screen:SeqSong"));
    assert!(s.sys.seq().project.song.is_empty());
    // Add ×3 (the new entry takes the focus): P1, P2, P3; on the first,
    // Next: P2; OK ×2: three repeats.
    run(&mut s, "ok down ok down ok up*2 next ok ok");
    let e: Vec<(u8, u8)> =
        s.sys.seq().project.song.entries().iter().map(|e| (e.pattern, e.repeats)).collect();
    assert_eq!(e, [(1, 3), (1, 1), (2, 1)]);
    // Delete the second (Menu: Insert, Delete, Play).
    run(&mut s, "down menu down ok");
    let e: Vec<(u8, u8)> =
        s.sys.seq().project.song.entries().iter().map(|e| (e.pattern, e.repeats)).collect();
    assert_eq!(e, [(1, 3), (2, 1)]);
    run(&mut s, "set:seqdry=on menu down*2 ok wait:500");
    let st = s.sys.seq();
    assert_eq!(st.project.mode(), PlayMode::Song);
    assert!(st.seq.playing());
    assert_eq!(st.seq.position().unwrap().pattern, 1, "the song's first entry");
}

/// Fill while held (the live screen): with OK down the fill pattern's
/// steps play from the next step; let go, the pattern's own again.
#[test]
fn fill_while_held_on_the_live_screen() {
    let mut s = sim(&format!(
        "{SONG} set:seqdry=on menu down*11 ok ok wait:50 expect-screen:SeqLive menu ok wait:500"
    ));
    let n0 = s.sys.seq.notes.len();
    run(&mut s, "press:ok wait:600");
    assert!(s.sys.seq().seq.fill());
    let held: Vec<_> = s.sys.seq.notes[n0..].to_vec();
    run(&mut s, "release:ok wait:600");
    assert!(!s.sys.seq().seq.fill());
    // Pattern D's snare (with its tune locks) only plays in the fill.
    let locked_snare = |v: &[orpheus_seq::Hit]| v.iter().any(|h| h.track == 1 && h.locks[0] != 0xFF);
    assert!(locked_snare(&held), "the fill pattern played");
    let n1 = s.sys.seq.notes.len();
    run(&mut s, "wait:600");
    assert!(!locked_snare(&s.sys.seq.notes[n1..]), "and stops when let go");
}

/// Export: Menu › More › Export WAV and Export MIDI write `SEQnn.WAV` and
/// `SEQnn.MID` next to the project under `/.orpheus/seq/` (nowhere else);
/// the WAV is the song and two seconds, the MIDI parses; a card that
/// fails is reported.
#[test]
fn export_wav_and_midi() {
    let mut s = sim(&format!("{SONG} menu down*11 ok down*2 ok wait:100 menu down*11 ok down*3 ok wait:100"));
    assert_eq!(s.sys.seq_result(), SeqResult::Done);
    let wav = read(&mut s.sys.media, "/.orpheus/seq/SEQ01.WAV").expect("the WAV");
    assert_eq!(&wav[..4], b"RIFF");
    let p = s.sys.seq().project.clone();
    let frames =
        orpheus_seq::export::song_frames(&p, RATE, orpheus_seq::StepLen::Sixteenth) + 2 * u64::from(RATE);
    let size = {
        use orpheus_fs::host::block_on;
        let vol = s.sys.media.volume();
        let f = block_on(vol.open("/.orpheus/seq/SEQ01.WAV", orpheus_fs::OpenMode::Read)).unwrap();
        let n = f.size();
        let _ = block_on(vol.close(f));
        n
    };
    assert_eq!(size, 44 + 4 * frames);
    assert_eq!(u64::from(u32::from_le_bytes([wav[40], wav[41], wav[42], wav[43]])), 4 * frames);
    let midi = read(&mut s.sys.media, "/.orpheus/seq/SEQ01.MID").expect("the MIDI file");
    assert!(midi.starts_with(b"MThd"));
    assert!(orpheus_midi::probe(&midi));
    run(&mut s, "set:seqfail=on menu down*11 ok down*3 ok wait:100");
    assert_eq!(s.sys.seq_result(), SeqResult::Failed);
}

/// A project file SEQ-1 wrote (version 1) on the card opens unchanged
/// and is saved as version 2.
#[test]
fn a_seq1_file_opens() {
    let mut s = sim("set:seq=on goto:Home");
    let v1 = std::fs::read(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../crates/orpheus-seq/tests/data/seq1-demo.osq"),
    )
    .unwrap();
    assert!(orpheus_sim::seq::write(&mut s.sys.media, "/.orpheus/seq/SEQ05.OSQ", &v1));
    run(&mut s, &format!("{LIST} down ok wait:50 expect-screen:SeqGrid"));
    let mut want = Box::new(orpheus_seq::Project::NEW);
    orpheus_seq::demo::beat(&mut want);
    assert!(s.sys.seq().project.same_as(&want));
    run(&mut s, "ok back wait:50");
    let d = read(&mut s.sys.media, "/.orpheus/seq/SEQ05.OSQ").unwrap();
    assert_eq!(u16::from_le_bytes([d[4], d[5]]), orpheus_seq::file::VERSION);
}

/// VER-13: each Sequencer setting against its reference value, in the
/// journeys it acts on (and the settings screen sets them).
mod settings_effects {
    use super::*;

    /// A new project made with `set` (a setting) first.
    fn new_project(set: &str) -> orpheus_seq::Project {
        let s = sim(&format!("{set} {LIST} ok wait:50 expect-screen:SeqGrid"));
        s.sys.seq().project.clone()
    }

    /// The demo beat played `ms` with `set` first (the notes only).
    fn played(set: &str, ms: u64) -> AppSim {
        sim(&format!("{set} set:seqdry=on {DEMO} menu ok wait:{ms}"))
    }

    fn clicks_of(s: &AppSim) -> usize {
        s.sys.seq.notes.iter().filter(|h| h.track == orpheus_seq::player::CLICK).count()
    }

    #[test]
    fn default_kit() {
        assert_eq!(new_project("set:0x1101=0").kit_name(), "standard");
        assert_eq!(new_project("set:0x1101=3").kit_name(), "tight");
        // From the settings screen: Prev/Next on its first row.
        let s =
            sim(&format!("{LIST} menu ok wait:50 expect-screen:SeqSettings next*2 back wait:50 ok wait:50"));
        assert_eq!(s.sys.seq().project.kit_name(), "punch");
    }

    #[test]
    fn default_length() {
        assert_eq!(new_project("set:0x1102=0").pattern(5).len(), 16);
        assert_eq!(new_project("set:0x1102=2").pattern(5).len(), 48);
    }

    #[test]
    fn default_swing() {
        assert_eq!(new_project("set:0x1103=0").swing(), 50);
        assert_eq!(new_project("set:0x1103=4").swing(), 66);
    }

    #[test]
    fn metronome() {
        assert_eq!(clicks_of(&played("set:0x1104=0", 2000)), 0);
        // 98 BPM: a beat every 612 ms; 2 s: four clicks.
        assert_eq!(clicks_of(&played("set:0x1104=1", 2000)), 4);
    }

    #[test]
    fn count_in() {
        let first = |s: &AppSim| {
            s.sys.seq.notes.iter().find(|h| h.track != orpheus_seq::player::CLICK).map(|h| h.frame)
        };
        let off = played("set:0x1105=0", 3000);
        let one = played("set:0x1105=1", 3000);
        let (a, b) = (first(&off).unwrap(), first(&one).unwrap());
        // One bar at 98 BPM: 16 steps of 7,346.9 frames at 48 kHz.
        assert_eq!(b - a, orpheus_seq::player::units_to_frames(16 * 28_800, RATE, 9_800));
        assert_eq!(clicks_of(&one), 4);
        assert_eq!(
            played("set:0x1105=2", 3000).sys.seq().seq.counting_in(),
            4,
            "the second bar still counts"
        );
    }

    #[test]
    fn step_length() {
        let steps = |v| played(&format!("set:0x1106={v}"), 2000).sys.seq().seq.steps();
        let (a, b, c) = (steps(0), steps(1), steps(2));
        assert!(b * 2 <= a + 1 && a <= b * 2 + 1, "eighths: half as many ({a}, {b})");
        assert!(c + 1 >= a * 2 && c <= a * 2 + 1, "thirty-seconds: twice as many ({a}, {c})");
    }

    #[test]
    fn key_clicks_in_play() {
        // Key click on; two keys pressed while playing.
        let clicks = |v| {
            let mut s = played(&format!("set:0x0108=1 set:0x1107={v}"), 500);
            let c0 = s.sys.clicks;
            run(&mut s, "down up wait:50");
            let during = s.sys.clicks - c0;
            run(&mut s, "menu ok wait:50");
            let c1 = s.sys.clicks;
            run(&mut s, "down up wait:50");
            (during, s.sys.clicks - c1)
        };
        assert_eq!(clicks(0), (0, 2), "off: silent while playing, clicks when stopped");
        assert_eq!(clicks(1), (2, 2), "on: clicks while playing too");
    }
}
