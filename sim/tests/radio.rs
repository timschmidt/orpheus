//! UI-5a: the Radio app over the firmware's radio service
//! (`orpheus_fm::Radio`) and the fake tuner (`orpheus_fm::fake`): start
//! and stop, music and radio exclusive, tune limits and wrap per region,
//! seek, presets (save, step, list, reorder, delete), auto-scan with its
//! question, mute, region change, the antenna note, a missing tuner, and
//! that leaving the app keeps the radio playing.

use orpheus_apps::services::{
    self, PlaybackService, PlayerService, RadioPhase, RadioRequest, RadioService, SettingsService,
};
use orpheus_apps::settings::id;
use orpheus_fm::chip::Analog;
use orpheus_fm::volume;
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

/// Home grid: Radio is the third app.
const APP: &str = "next*2 ok wait:1100 expect-screen:Radio";

fn khz(s: &AppSim) -> u32 {
    s.sys.radio().khz
}

fn phase(s: &AppSim) -> RadioPhase {
    s.sys.radio().phase
}

#[test]
fn opening_the_app_plays_the_radio_and_stops_the_music() {
    let mut s = sim("set:queue=takk wait:200");
    assert_eq!(s.sys.player().source, Source::Playing);
    run(&mut s, APP);
    assert_eq!(phase(&s), RadioPhase::Playing);
    assert_ne!(s.sys.player().source, Source::Playing, "starting the radio stops the music");
    assert_eq!(khz(&s), 87_500, "the default frequency");
    let chip = &s.sys.radio.chip;
    assert!(chip.enabled() && chip.routed() && chip.audible());
    assert_eq!(services::status(&s.sys).source, Source::Radio, "status bar: FM");
}

#[test]
fn leaving_keeps_playing_and_music_stops_the_radio() {
    let mut s = sim(&format!("set:radio=101900 {APP} back wait:2000 expect-screen:Home"));
    assert_eq!(phase(&s), RadioPhase::Playing, "Back leaves the app, not the radio");
    let p = s.sys.playing().unwrap();
    assert_eq!(p.source, Source::Radio);
    assert_eq!(p.title, "FM 101.90 · PARADISE", "mini-player: frequency and RDS name");
    // Music starts: the radio stops and the tuner is powered down.
    run(&mut s, "set:queue=takk wait:500");
    assert_eq!(phase(&s), RadioPhase::Off);
    assert!(!s.sys.radio.chip.enabled());
    assert!(!s.sys.radio.chip.routed());
    assert_eq!(s.sys.radio.chip.gain(), volume::PA_MUSIC, "the music's amplifier gain is back");
}

#[test]
fn starts_where_it_was_last() {
    let mut s = sim(&format!("set:radio=94700 {APP} hold:ok wait:500"));
    assert_eq!(phase(&s), RadioPhase::Off);
    assert_eq!(s.sys.setting(id::RADIO_FREQ), 9470);
    run(&mut s, "ok wait:1500");
    assert_eq!(phase(&s), RadioPhase::Playing);
    assert_eq!(khz(&s), 94_700);
}

#[test]
fn ok_mutes_and_hold_stops() {
    let mut s = sim(&format!("set:radio=94700 {APP}"));
    run(&mut s, "ok wait:100");
    assert!(s.sys.radio().muted);
    assert!(!s.sys.radio.chip.audible());
    run(&mut s, "ok wait:100");
    assert!(!s.sys.radio().muted);
    assert!(s.sys.radio.chip.audible());
    run(&mut s, "hold:ok wait:300 expect-screen:Radio");
    assert_eq!(phase(&s), RadioPhase::Off);
    assert!(!s.sys.radio.chip.enabled(), "tuner off when the radio is not playing");
    // The route went off only after the gain was ramped to 0.
    let a = &s.sys.radio.chip.analog;
    let off = a.iter().rposition(|x| *x == Analog::Disconnect).unwrap();
    assert_eq!(a[off - 1], Analog::Gain(0));
    assert_eq!(a.last(), Some(&Analog::Gain(volume::PA_MUSIC)));
}

#[test]
fn prev_next_step_and_wrap_per_region() {
    let mut s = sim(&format!("set:radio=108000 {APP}"));
    run(&mut s, "next wait:200");
    assert_eq!(khz(&s), 87_500, "Europe wraps 108.0 → 87.5");
    run(&mut s, "prev wait:200");
    assert_eq!(khz(&s), 108_000);
    run(&mut s, "prev wait:200");
    assert_eq!(khz(&s), 107_900, "100 kHz steps");
    // United States: odd tenths, 200 kHz.
    let mut s = sim(&format!("set:0x0701=1 set:radio=107900 {APP}"));
    run(&mut s, "next wait:200");
    assert_eq!(khz(&s), 87_900);
    run(&mut s, "next wait:200");
    assert_eq!(khz(&s), 88_100);
    // Japan: 76–95 MHz.
    let mut s = sim(&format!("set:0x0701=3 set:radio=95000 {APP}"));
    run(&mut s, "next wait:200");
    assert_eq!(khz(&s), 76_000);
    // World: 50 kHz.
    let mut s = sim(&format!("set:0x0701=2 set:radio=87500 {APP}"));
    run(&mut s, "next wait:200");
    assert_eq!(khz(&s), 87_550);
    assert_eq!(s.sys.radio.chip.khz(), 87_550, "the register read-back matches the display");
}

#[test]
fn hold_seeks_to_the_next_station() {
    let mut s = sim(&format!("set:radio=94700 {APP}"));
    run(&mut s, "hold:next wait:1000");
    assert_eq!(khz(&s), 99_900, "the 96.0 MHz reference harmonic is skipped");
    run(&mut s, "hold:next wait:1000");
    assert_eq!(khz(&s), 101_900);
    run(&mut s, "hold:prev wait:1000");
    assert_eq!(khz(&s), 99_900);
    // A tap stops a running seek.
    let mut s = sim(&format!("set:seekreads=200 set:radio=94700 {APP} hold:next wait:200"));
    assert_eq!(phase(&s), RadioPhase::Seeking);
    run(&mut s, "next wait:300");
    assert_eq!(phase(&s), RadioPhase::Playing);
    assert_eq!(s.sys.radio.log.last(), Some(&RadioRequest::Cancel));
}

#[test]
fn save_presets_and_step_through_them() {
    let mut s = sim(&format!("set:radio=94700 {APP}"));
    assert!(s.sys.presets().is_empty());
    run(&mut s, "down wait:100");
    assert_eq!(khz(&s), 94_700, "no presets: nothing to step to");
    // Menu › Save preset (the station's RDS name comes with it).
    run(&mut s, "wait:2000 menu ok wait:100");
    assert_eq!(s.sys.presets().len(), 1);
    let p = s.sys.presets().get(0).unwrap();
    assert_eq!((p.khz, p.name()), (94_700, &b"ROCK FM"[..]));
    run(&mut s, "hold:next wait:1000 menu ok wait:100");
    assert_eq!(s.sys.presets().get(1).unwrap().khz, 99_900);
    // Down = next preset, wrapping; Up = previous.
    run(&mut s, "down wait:200");
    assert_eq!(khz(&s), 94_700);
    run(&mut s, "down wait:200");
    assert_eq!(khz(&s), 99_900);
    run(&mut s, "up wait:200");
    assert_eq!(khz(&s), 94_700);
    // On a preset the menu offers Remove.
    run(&mut s, "menu ok wait:100");
    assert_eq!(s.sys.presets().len(), 1);
    // Presets stay with the service when the app is left and opened again.
    run(&mut s, "back ok wait:100 expect-screen:Radio");
    assert_eq!(s.sys.presets().len(), 1);
}

#[test]
fn presets_list_tunes_reorders_and_deletes() {
    let mut s = sim(&format!("set:presets=demo set:radio=94700 {APP} menu down ok wait:100"));
    assert_eq!(s.screen(), "RadioPresets");
    // Focus starts on the preset playing (P02); Move up.
    run(&mut s, "menu ok wait:100");
    assert_eq!(s.sys.presets().get(0).unwrap().khz, 94_700);
    assert_eq!(s.sys.presets().get(1).unwrap().khz, 101_900);
    // Delete the last one.
    run(&mut s, "down*5 menu down ok wait:100");
    assert_eq!(s.sys.presets().len(), 5);
    // OK tunes and goes back.
    run(&mut s, "up*3 ok wait:300 expect-screen:Radio");
    assert_eq!(khz(&s), 101_900);
}

#[test]
fn auto_scan_asks_before_replacing_and_fills_strongest_first() {
    let mut s = sim(&format!("set:presets=demo set:radio=94700 {APP} menu down*2 ok"));
    // The question; Cancel keeps the list.
    run(&mut s, "back wait:100");
    assert_eq!(s.sys.presets().len(), 6);
    assert_eq!(phase(&s), RadioPhase::Playing);
    run(&mut s, "menu down*2 ok right ok wait:150");
    assert_eq!(phase(&s), RadioPhase::Scanning);
    assert!(!s.sys.radio.chip.audible(), "muted while scanning");
    run(&mut s, "wait:5000");
    assert_eq!(phase(&s), RadioPhase::Playing);
    let got: Vec<u32> = (0..s.sys.presets().len()).map(|i| s.sys.presets().get(i).unwrap().khz).collect();
    assert_eq!(got, [101_900, 94_700, 91_500, 99_900, 88_300, 104_300]);
    assert_eq!(khz(&s), 101_900, "tuned to the strongest");
    assert!(s.sys.radio.chip.audible());
    // Without presets it scans at once.
    let mut s = sim(&format!("set:radio=94700 {APP} menu down*2 ok wait:150"));
    assert_eq!(phase(&s), RadioPhase::Scanning);
    // Back stops a scan: back where it was, presets untouched.
    run(&mut s, "back wait:300 expect-screen:Radio");
    assert_eq!(phase(&s), RadioPhase::Playing);
    assert_eq!(khz(&s), 94_700);
    assert!(s.sys.presets().is_empty());
}

#[test]
fn region_and_mono_from_the_menu() {
    let mut s = sim(&format!("set:radio=99900 {APP}"));
    // Menu › Region › Japan (fourth choice).
    run(&mut s, "menu down*4 ok down*3 ok wait:300 back back");
    assert_eq!(s.sys.setting(id::RADIO_REGION), 3);
    assert_eq!(khz(&s), 95_000, "99.9 is outside 76–95: snapped");
    assert_eq!(s.sys.radio.chip.khz(), 95_000);
    // Force mono: no stereo indicator on a stereo station.
    run(&mut s, "set:0x0701=0 wait:300");
    run(&mut s, "set:radio=101900 wait:200");
    assert!(s.sys.radio().stereo);
    run(&mut s, "menu down*3 ok wait:300 back");
    assert_eq!(s.sys.setting(id::FORCE_MONO), 1);
    assert!(s.sys.radio.chip.mono());
    assert!(!s.sys.radio().stereo);
}

#[test]
fn rds_name_and_text_reach_the_screen() {
    let s = sim(&format!("set:radio=101900 {APP} wait:2000"));
    assert_eq!(s.sys.radio_ps(), "PARADISE");
    assert_eq!(s.sys.radio_rt(), "Radio Paradise - eclectic music");
    assert!(s.sys.radio().stereo);
    assert_eq!(s.sys.radio().bars(), 3);
}

#[test]
fn volume_keys_set_the_vendor_pairs() {
    let mut s = sim(&format!("set:radio=94700 {APP}"));
    run(&mut s, "set:volume=14 vol+ wait:100");
    assert_eq!(s.sys.volume, 15);
    let chip = &s.sys.radio.chip;
    assert_eq!((chip.volume(), chip.gain()), volume::for_ui(15));
}

#[test]
fn antenna_note_and_missing_tuner() {
    let s = sim(&format!("set:antenna=0 {APP}"));
    assert!(!s.sys.radio().antenna);
    assert_eq!(phase(&s), RadioPhase::Playing, "tuning still works without earphones");
    let mut s = sim("set:tuner=absent next*2 ok wait:1500 expect-screen:Radio");
    assert_eq!(phase(&s), RadioPhase::NotResponding);
    assert!(!s.sys.radio.chip.routed(), "the route is never switched");
    // OK tries again.
    run(&mut s, "set:tuner=present ok wait:1500");
    assert_eq!(phase(&s), RadioPhase::Playing);
}
