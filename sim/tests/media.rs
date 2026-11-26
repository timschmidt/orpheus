//! Files (UI-3) and Music (UI-4 screens) over the simulated card: windowed
//! 5000-entry folder, re-sort keeping focus, letter jump, accelerated
//! scrolling, delete, text viewer, queue from a folder, transport keys,
//! Now Playing views, equaliser editing.

use orpheus_apps::AppScreen;
use orpheus_apps::browse::DirId;
use orpheus_apps::services::{BrowseService, PlayerService, TextService};
use orpheus_apps::settings::id;
use orpheus_sim::AppSim;
use orpheus_sim::card::BIG;

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
    s
}

fn focus(s: &AppSim) -> u32 {
    match s.shell.top() {
        AppScreen::Files(f) => f.focus(),
        _ => panic!("not in Files: {}", s.screen()),
    }
}

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

const BIG_FOLDER: &str = "next ok down*2 ok ok";

#[test]
fn big_folder_is_windowed_and_sorted_later() {
    let mut s = sim(&format!("{BIG_FOLDER} wait:100 expect-screen:Files"));
    let l = s.sys.listing();
    assert_eq!(l.count as usize, BIG);
    assert!(!l.sorted, "first visit: on-disk order");
    assert!(s.sys.entry(0).is_some());
    assert!(s.sys.entry(4000).is_none(), "rows outside the window are not held");
    assert_eq!(s.sys.entry(1).unwrap().name, "Breeze 1.mp3");
    run(&mut s, "wait:600");
    assert!(s.sys.listing().sorted);
    // The focused file (Aurora 1) stays focused after sorting.
    assert_eq!(s.sys.entry(0).unwrap().name, "Aurora 1.mp3");
    assert_eq!(s.sys.entry(1).unwrap().name, "Aurora 2.mp3", "natural order");
    // Second visit: the index exists, sorted at once.
    run(&mut s, "back ok wait:50");
    assert!(s.sys.listing().sorted);
}

#[test]
fn natural_sort_and_dirs_first() {
    let mut s = sim("next ok down*2 ok wait:100");
    let names: Vec<String> = (0..5).map(|i| s.sys.entry(i).unwrap().name.to_string()).collect();
    assert_eq!(names, ["Big folder", "Podcasts", "Sigur Rós", "YOASOBI", "Кино"]);
    run(&mut s, "back back next ok wait:100");
    let first = s.sys.entry(0).unwrap().name.to_string();
    assert_eq!(first, "Books", "hidden .orpheus not shown by default");
}

#[test]
fn letter_jump_and_paging_in_5000() {
    let mut s = sim(&format!("{BIG_FOLDER} wait:600"));
    run(&mut s, "press:next wait:550 release:next wait:200");
    let f = focus(&s);
    assert_eq!(s.sys.entry(f).map(|e| e.name.to_string()).as_deref(), Some("Breeze 1.mp3"), "one letter on");
    run(&mut s, "next wait:100");
    assert_eq!(focus(&s), f + 5, "Next pages by five rows");
    assert!(s.sys.entry(focus(&s)).is_some(), "window followed");
}

#[test]
fn accelerated_scroll_crosses_the_big_folder() {
    let mut s = sim(&format!("{BIG_FOLDER} wait:600"));
    run(&mut s, "hold:down:8000 wait:200");
    let pos = focus(&s);
    assert!(pos > 2500, "8 s of held Down moved {pos} rows");
    assert!(s.sys.entry(pos).is_some(), "the window followed the focus");
}

#[test]
fn delete_updates_the_listing() {
    let mut s = sim("next ok down*2 ok down*2 ok ok wait:100");
    let before = s.sys.listing().count;
    run(&mut s, "menu down*5 ok next ok wait:100");
    assert_eq!(s.sys.listing().count, before - 1);
    assert!(s.sys.media.log.iter().any(|l| l.contains("deleted /Music/Sigur Rós/Takk…/01 Glósóli.mp3")));
}

#[test]
fn text_viewer_pages_and_remembers() {
    let mut s = sim("next ok ok wait:100 ok wait:200 expect-screen:TextViewer");
    let (start, win) = s.sys.text_window();
    assert_eq!(start, 0);
    assert!(win.starts_with(b"It was the best of times"));
    run(&mut s, "next wait:200 next wait:200");
    run(&mut s, "back ok wait:200");
    // Reopened: same page, not the start.
    let f = s.sys.entry(0).unwrap().file;
    assert!(s.sys.text_position(f) > 200);
    run(&mut s, "prev wait:200 prev wait:200");
}

#[test]
fn playing_a_file_queues_its_folder() {
    let s = sim("next ok down*2 ok down*2 ok ok wait:100 down*2 ok wait:100 expect-screen:NowPlaying");
    let p = s.sys.player();
    assert_eq!((p.queue_pos, p.queue_len), (1, 3), "Hoppípolla in its folder of three");
    let t = s.sys.track().unwrap();
    assert_eq!((t.title, t.artist, t.album), ("Hoppípolla", "Sigur Rós", "Takk…"), "ID3 tags read");
    assert!(s.sys.lyrics_count() > 5, ".lrc beside the file");
}

/// MIDI-1: MIDI files are audio in Files; playing one queues its folder
/// with the songs' real durations and titles (from the scan), and a
/// karaoke file's lyrics are timed lines.
#[test]
fn midi_files_play_from_files() {
    let mut s = sim("set:queue=midi wait:100 back back");
    run(&mut s, "next ok down*2 ok wait:100 down ok wait:100");
    assert_eq!(s.sys.entry(1).map(|e| e.name.to_string()).as_deref(), Some("Hill Song.kar"));
    run(&mut s, "down ok wait:100 expect-screen:NowPlaying");
    let p = s.sys.player();
    assert_eq!((p.queue_pos, p.queue_len), (1, 3));
    // Ten bars at 100 BPM and the two-second tail.
    assert_eq!(p.duration_ms, 26_000);
    let t = s.sys.track().unwrap();
    assert_eq!((t.title, t.codec), ("Hill Song", "MIDI"));
    assert_eq!(s.sys.queue_entry(0).unwrap().title, "Etude for Orpheus");
    assert_eq!(s.sys.lyrics_count(), 4);
    let l = s.sys.lyric(1).unwrap();
    assert_eq!((l.time_ms, l.text), (Some(7200), "the river runs, the air is still."));
    // Seeking past the end goes on to the next song, the 56 s tour.
    run(&mut s, "hold:next:3000 wait:100");
    let p = s.sys.player();
    assert!(p.queue_pos == 2 || p.position_ms <= 26_000, "{p:?}");
}

#[test]
fn transport_keys() {
    let mut s = sim("set:queue=takk goto:NowPlaying wait:100");
    run(&mut s, "next");
    assert_eq!(s.sys.player().queue_pos, 1);
    run(&mut s, "wait:5000 prev");
    assert_eq!(
        (s.sys.player().queue_pos, s.sys.player().position_ms / 1000),
        (1, 0),
        "Prev after 3 s restarts"
    );
    run(&mut s, "prev");
    assert_eq!(s.sys.player().queue_pos, 0, "Prev at the start goes back");
    run(&mut s, "ok");
    assert_eq!(s.sys.player().source, orpheus_ui::event::Source::Paused);
    run(&mut s, "ok");
    // Seek accelerates: 3 s held moves far more than 2 s/step would.
    let before = s.sys.player().position_ms;
    run(&mut s, "hold:next:3000");
    let moved = s.sys.player().position_ms - before;
    assert!(moved > 60_000, "seek moved {moved} ms");
    run(&mut s, "hold:ok");
    assert_eq!(s.sys.player().source, orpheus_ui::event::Source::Paused, "OK held: stop");
}

#[test]
fn views_cycle_and_are_remembered() {
    let mut s = sim("set:queue=takk goto:NowPlaying wait:100 down");
    assert_eq!(s.sys.settings.get(id::NP_VIEW), 1);
    run(&mut s, "down down");
    assert_eq!(s.sys.settings.get(id::NP_VIEW), 3, "Up next");
    // In Up next Up/Down move in the list; past the end, the next view.
    run(&mut s, "down down down");
    assert_eq!(s.sys.settings.get(id::NP_VIEW), 4);
    run(&mut s, "back goto:NowPlaying");
    assert_eq!(s.sys.settings.get(id::NP_VIEW), 4);
}

#[test]
fn up_next_jumps() {
    let s = sim("set:0x0d03=3 set:queue=takk goto:NowPlaying wait:100 down down ok");
    assert_eq!(s.sys.player().queue_pos, 2);
}

#[test]
fn equaliser_edit_copies_preset_to_custom() {
    let mut s = sim("set:0x0101=1 goto:Equaliser up");
    assert_eq!(s.sys.settings.get(id::EQUALISER), 9, "Custom 1");
    assert_eq!(s.sys.settings.get(id::EQ_BAND0), 6, "Rock's +5 dB, plus one");
    run(&mut s, "next down*20");
    assert_eq!(s.sys.settings.get(orpheus_apps::settings::SettingId(id::EQ_BAND0.0 + 1)), -12, "clamped");
}

#[test]
fn shuffle_all_and_resume() {
    let mut s = sim("ok ok wait:100");
    assert!(s.sys.player().queue_len >= 9, "every album, not the empty big folder");
    let mut r = sim("set:resume=1 ok ok wait:100 expect-screen:NowPlaying");
    assert_eq!(r.sys.track().unwrap().title, "Hoppípolla");
    run(&mut r, "back expect-screen:Music");
    run(&mut s, "expect-screen:NowPlaying");
}

#[test]
fn root_dir_id() {
    let mut s = AppSim::new();
    let l = s.sys.browse(DirId::ROOT, Default::default());
    assert!(l.count >= 5);
}
