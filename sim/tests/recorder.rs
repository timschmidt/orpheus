//! UI-5b: the Recorder app over the firmware's recording session
//! (`orpheus_rec::session`) with a fake ADC, writing real files to the
//! simulated card: start, pause, marks, stop with its question, the files
//! decoded again, formats and sources, voice activation, a card stall that
//! drops blocks (counted, shown, marked in the file), card full, the
//! recordings list (play, delete), and leaving the app.

use orpheus_apps::services::{
    BrowseService, PlaybackService, PlayerService, RadioPhase, RadioService, RecEnd, RecPhase,
    RecorderService, SettingsService,
};
use orpheus_apps::settings::id;
use orpheus_audio::{ContainerInfo, Decoder, PcmBlock, SliceSource, Step};
use orpheus_fs::OpenMode;
use orpheus_fs::host::block_on;
use orpheus_sim::AppSim;
use orpheus_ui::event::Source;

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

/// Home grid: the Recorder is the fifth app.
const APP: &str = "next*4 ok wait:200 expect-screen:Recorder";

fn phase(s: &AppSim) -> RecPhase {
    s.sys.recorder().phase
}

/// The last file the recorder closed, read back from the card.
fn last_file(s: &mut AppSim) -> (String, Vec<u8>) {
    let (path, size) = s.sys.rec.last_file.clone().expect("a file was saved");
    let vol = s.sys.media.volume();
    let mut f = block_on(vol.open(&path, OpenMode::Read)).expect("open");
    let mut b = vec![0u8; usize::try_from(size).unwrap()];
    let n = block_on(vol.read_exact_or_eof(&mut f, &mut b)).unwrap();
    b.truncate(n);
    (path, b)
}

fn decode<'a, D: Decoder<'a>>(file: &[u8], arena: &'a mut Vec<u8>) -> (u32, u8, usize) {
    let mut src = SliceSource::new(file);
    let hint = ContainerInfo::default();
    let plan = D::inspect(&src, &hint).expect("inspect");
    arena.resize(plan.arena, 0);
    let (mut d, info) = D::open(&mut src, arena, &hint).expect("open");
    let mut n = 0;
    loop {
        match d.decode(&mut src).expect("decode") {
            Step::Pcm(PcmBlock::I16(x)) => n += x.len(),
            Step::Pcm(PcmBlock::I32(x)) => n += x.len(),
            Step::NeedData(_) => panic!("short file"),
            Step::EndOfStream => break,
        }
    }
    (info.rate, info.channels, n / usize::from(info.channels))
}

/// (rate, channels, frames) of a recording.
fn decode_file(path: &str, b: &[u8]) -> (u32, u8, usize) {
    let mut arena = Vec::new();
    if path.ends_with(".flac") {
        decode::<orpheus_codec_flac::FlacDecoder<'_>>(b, &mut arena)
    } else {
        decode::<orpheus_codec_wav::WavDecoder<'_>>(b, &mut arena)
    }
}

#[test]
fn opening_arms_the_meters_and_back_disarms() {
    let mut s = sim(&format!("{APP} wait:1000"));
    assert_eq!(phase(&s), RecPhase::Armed);
    let i = s.sys.recorder();
    assert!(i.bar[0] > -40, "the meters move: {i:?}");
    assert!(i.left_s.unwrap_or(0) > 60, "time left on the card");
    run(&mut s, "back wait:100 expect-screen:Home");
    assert_eq!(phase(&s), RecPhase::Off);
    assert!(s.sys.rec.log.contains(&"disarm".to_string()));
}

#[test]
fn record_pause_mark_stop_and_the_file_plays() {
    let mut s = sim(&format!("set:queue=takk wait:200 {APP} ok wait:3000"));
    assert_eq!(phase(&s), RecPhase::Recording);
    assert_ne!(s.sys.player().source, Source::Playing, "no playback while recording");
    assert_eq!(s.sys.playing().map(|p| p.source), Some(Source::Recording), "status bar: recording");
    run(&mut s, "next wait:100 ok wait:2000");
    assert_eq!(phase(&s), RecPhase::Paused);
    let at_pause = s.sys.recorder().elapsed_ms;
    assert!((3_000..=3_500).contains(&at_pause), "{at_pause}");
    run(&mut s, "ok wait:1000");
    assert_eq!(phase(&s), RecPhase::Recording);
    assert_eq!(s.sys.recorder().marks, 1);
    // Back asks; Cancel keeps recording; Stop saves.
    run(&mut s, "back wait:100 prev ok wait:100");
    assert_eq!(phase(&s), RecPhase::Recording, "Cancel");
    run(&mut s, "back wait:100 next ok wait:200");
    assert_eq!(phase(&s), RecPhase::Armed, "stopped, back to the meters");
    assert_eq!(s.sys.recorder().ended, Some(RecEnd::Saved));
    let (path, b) = last_file(&mut s);
    assert!(path.starts_with("/Recordings/REC-20261124-1241"), "{path}");
    let (rate, ch, frames) = decode_file(&path, &b);
    assert_eq!((rate, ch), (44_100, 1));
    let secs = frames as f64 / 44_100.0;
    // 3.3 s, then 1 s and the dialogs' moments, but not the 2-s pause.
    assert!((4.3..5.5).contains(&secs), "{secs} s");
    // The mark is in the file.
    assert!(b.windows(4).any(|w| w == b"cue "));
}

#[test]
fn formats_quality_and_flac() {
    for (fmt, q, ext, rate) in [(2, 0, ".flac", 16_000), (1, 2, ".wav", 48_000), (0, 0, ".wav", 16_000)] {
        let mut s = sim(&format!(
            "set:0x0802={fmt} set:0x0803={q} set:recsignal=tone {APP} ok wait:2000 ok hold:ok next ok wait:200"
        ));
        let (path, b) = last_file(&mut s);
        assert!(path.ends_with(ext), "{path}");
        let (r, ch, frames) = decode_file(&path, &b);
        assert_eq!((r, ch), (rate, 1), "{path}");
        assert!(frames as u32 >= rate * 19 / 10, "{path}: {frames}");
    }
}

#[test]
fn fm_source_starts_the_radio_and_names_the_station() {
    let mut s = sim(&format!("set:0x0801=1 {APP} wait:1500"));
    assert_eq!(s.sys.radio().phase, RadioPhase::Playing, "FM recording needs the radio");
    run(&mut s, "ok wait:1500 hold:ok next ok wait:200");
    let (path, b) = last_file(&mut s);
    assert!(path.starts_with("/Recordings/FM-87.50-"), "{path}");
    let (_, ch, _) = decode_file(&path, &b);
    assert_eq!(ch, 2, "FM is recorded in stereo");
}

#[test]
fn a_card_stall_drops_blocks_and_says_so() {
    let mut s =
        sim(&format!("set:0x0803=2 set:recsignal=noise {APP} ok wait:500 set:recstall=3000 wait:4000"));
    let i = s.sys.recorder();
    assert!(i.drops >= 1, "{i:?}");
    run(&mut s, "hold:ok next ok wait:200");
    let (path, b) = last_file(&mut s);
    assert!(b.windows(7).any(|w| w == b"dropout"), "the file says where");
    let (_, _, frames) = decode_file(&path, &b);
    assert!(frames < 48_000 * 4, "the stalled part is missing, not silence: {frames}");
}

#[test]
fn voice_activation_waits_for_sound() {
    let mut s = sim(&format!("set:0x0806=3 set:recsignal=quiet {APP} ok wait:3000"));
    assert_eq!(phase(&s), RecPhase::Waiting);
    assert!(s.sys.recorder().elapsed_ms < 100, "the timer does not run");
    run(&mut s, "set:recsignal=tone");
    // The input changes at the next arming; stop and start again.
    run(&mut s, "hold:ok next ok wait:200 ok wait:1000");
    assert_eq!(phase(&s), RecPhase::Recording);
}

#[test]
fn automatic_gain_brings_quiet_input_up() {
    let mut s = sim(&format!("set:0x0804=1 set:recsignal=quiet {APP} ok wait:15000"));
    let i = s.sys.recorder();
    assert!(i.auto_gain && i.gain_db >= 20, "{i:?}");
    // Up/Down change the manual gain only.
    run(&mut s, "up wait:100");
    assert_eq!(s.sys.setting(id::REC_GAIN), 0);
    run(&mut s, "set:0x0804=0 up up wait:100");
    assert_eq!(s.sys.setting(id::REC_GAIN), 2);
}

#[test]
fn recordings_list_plays_and_deletes() {
    let mut s = sim(&format!(
        "{APP} ok wait:1500 hold:ok next ok wait:200 set:clock=12:45:10 ok wait:1500 hold:ok next ok wait:200"
    ));
    // Menu › Recordings… (last row).
    run(&mut s, "menu down*8 ok wait:200 expect-screen:Recordings");
    assert_eq!(phase(&s), RecPhase::Off, "the list disarms");
    assert_eq!(s.sys.listing().count, 2);
    // Newest first.
    assert!(s.sys.entry(0).unwrap().name.contains("1245"), "{:?}", s.sys.entry(0).map(|e| e.name));
    // Delete the focused one, with its question.
    run(&mut s, "menu down*2 ok wait:100 next ok wait:200");
    assert_eq!(s.sys.listing().count, 1);
    // OK plays it; Back returns to the recorder, armed again.
    run(&mut s, "ok wait:300 expect-screen:NowPlaying");
    assert_eq!(s.sys.player().source, Source::Playing);
    run(&mut s, "back back wait:100 expect-screen:Recorder");
    assert_eq!(phase(&s), RecPhase::Armed);
}

#[test]
fn no_recordings_yet_and_no_card() {
    // The simulated card has an empty /Recordings.
    let s = sim(&format!("{APP} menu down*8 ok wait:100 expect-screen:Recordings"));
    assert_eq!(s.sys.listing().count, 0);
    let s = sim("set:card=absent next*4 ok wait:100");
    assert_eq!(s.screen(), "Notice");
}

#[test]
fn the_card_fills_up_and_the_recording_is_saved() {
    // WAV 48 kHz stereo: the simulated card's free space lasts a minute
    // or two.
    let mut s = sim(&format!("set:0x0801=2 set:recline=1 set:0x0803=2 {APP} ok"));
    let mut ended = None;
    for _ in 0..40 {
        run(&mut s, "wait:10000");
        ended = s.sys.recorder().ended;
        if ended.is_some() {
            break;
        }
    }
    assert_eq!(ended, Some(RecEnd::CardFull));
    let free = block_on(s.sys.media.volume().free_bytes()).unwrap();
    assert!(free >= 1 << 20, "1 MiB is left: {free}");
    let (path, b) = last_file(&mut s);
    let (_, ch, frames) = decode_file(&path, &b);
    assert_eq!(ch, 2);
    assert!(frames > 48_000 * 10, "{frames}");
}
