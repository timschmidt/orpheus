//! UI-5d: Stopwatch, Timer and Alarm over the simulated services: key-edge
//! timing and laps, running in the background with the status bar
//! indicators, the timer's alert and presets, alarms on their days, once,
//! snooze, the alert lighting a dark screen.

use orpheus_apps::services::{ClockService, ToolsService};
use orpheus_apps::settings::{SettingId, id};
use orpheus_apps::tools::{self, Alert, RING_MS, SNOOZE_MS};
use orpheus_sim::AppSim;
use orpheus_ui::shell::System;

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

const APP: &str = "next*7 ok wait:100 expect-screen:Stopwatch";

#[test]
fn stopwatch_times_from_the_key_edges() {
    // OK is acted on when it goes down: 60 ms press, 40 ms idle per tap.
    let mut s = sim(APP);
    let t0 = s.now.0;
    run(&mut s, "ok wait:59900 ok");
    let sw = s.sys.tools().stopwatch;
    // Down edges 60,000 ms apart exactly.
    assert_eq!(sw.elapsed(s.now.0), 60_000);
    assert!(!sw.running());
    assert!(s.now.0 - t0 >= 60_000);
}

#[test]
fn laps_and_reset() {
    let mut s = sim(&format!("{APP} ok wait:9900 next wait:4900 next wait:900 ok"));
    let sw = s.sys.tools().stopwatch;
    assert_eq!(sw.laps(), 2);
    let l2 = sw.lap_at(0).unwrap();
    let l1 = sw.lap_at(1).unwrap();
    assert_eq!((l1.number, l1.time_ms, l1.split_ms), (1, 10_000, 10_000));
    assert_eq!((l2.number, l2.time_ms, l2.split_ms), (2, 5_000, 15_000));
    assert_eq!(sw.elapsed(s.now.0), 16_000);
    // Next while stopped with laps: a confirmation; Cancel keeps it.
    run(&mut s, "next");
    assert!(s.shell.dialog_open());
    run(&mut s, "ok");
    assert_eq!(s.sys.tools().stopwatch.laps(), 2);
    run(&mut s, "next next ok wait:50");
    assert_eq!(s.sys.tools().stopwatch.laps(), 0);
    assert!(!s.sys.tools().stopwatch.started());
}

#[test]
fn runs_in_the_background_with_indicators() {
    let mut s = sim(&format!("{APP} ok back expect-screen:Home"));
    assert!(s.sys.status().activity, "stopwatch dot");
    run(&mut s, "wait:30000");
    assert!(s.sys.tools().stopwatch.elapsed(s.now.0) >= 30_000);
    // An alarm switched on: the bell.
    assert!(!s.sys.status().alarm);
    run(&mut s, "set:0x0d32=1");
    assert!(s.sys.status().alarm);
}

#[test]
fn timer_rings_once_and_the_alert_closes() {
    // Timer tab (Next from an idle stopwatch), the 1:00 preset.
    let mut s = sim(&format!("{APP} next ok"));
    assert!(s.sys.tools().timer.running());
    assert!(s.sys.status().alarm, "bell while the timer runs");
    run(&mut s, "wait:59000 expect-screen:Stopwatch");
    run(&mut s, "wait:1100 expect-screen:Alert");
    assert_eq!(s.sys.tools().ringing(), Some(Alert::Timer));
    run(&mut s, "ok expect-screen:Stopwatch");
    assert_eq!(s.sys.tools().ringing(), None);
    // Ready to run the same duration again.
    assert_eq!(s.sys.tools().timer.left(s.now.0), 60_000);
}

#[test]
fn timer_alert_lights_a_dark_screen_and_ends_by_itself() {
    let mut s = sim(&format!("{APP} next ok power"));
    assert!(!s.shell.gate.screen_on);
    run(&mut s, "wait:60500");
    assert!(s.shell.gate.screen_on, "the alert lit the screen");
    run(&mut s, "expect-screen:Alert");
    run(&mut s, &format!("wait:{}", RING_MS + 500));
    assert_eq!(s.sys.tools().ringing(), None);
    assert_ne!(s.screen(), "Alert", "closed when the ringing ended");
}

#[test]
fn custom_duration_becomes_the_first_preset() {
    // Menu › Set duration from 0:01:00: minutes +1, seconds +30 → 2:30.
    let mut s = sim(&format!("{APP} next menu ok wait:100 expect-screen:Picker"));
    run(&mut s, "next up next up*30 ok expect-screen:Stopwatch");
    assert_eq!(s.sys.tools().timer.duration(), 150_000);
    run(&mut s, "ok");
    assert_eq!(tools::preset(&s.sys, 0), 150);
    assert_eq!(tools::preset(&s.sys, 1), 60, "older presets move down");
    assert_eq!(s.sys.settings.get(SettingId(id::TIMER_PRESET0.0 + 4)), 600, "25:00 dropped out");
}

#[test]
fn alarm_rings_on_its_days_only() {
    // Alarm 1: 07:30 on weekdays. The fake clock: Tuesday.
    let mut s = sim("set:0x0d35=1 set:clock=07:29:58 set:clockrun=1");
    run(&mut s, "wait:3000 expect-screen:Alert");
    assert_eq!(s.sys.tools().ringing(), Some(Alert::Alarm(1)));
    run(&mut s, "ok");
    assert_eq!(s.sys.tools().ringing(), None);
    assert_eq!(s.sys.settings.get(SettingId(0x0D35)), 1, "a repeating alarm stays on");
    // Saturday at 07:30: nothing.
    let mut s = sim("set:0x0d35=1 set:weekday=5 set:clock=07:29:58 set:clockrun=1");
    run(&mut s, "wait:3000 expect-screen:Home");
    assert_eq!(s.sys.tools().ringing(), None);
}

#[test]
fn a_one_time_alarm_switches_itself_off() {
    // Alarm 3: 12:00, once.
    let mut s = sim("set:0x0d3b=1 set:clock=11:59:59 set:clockrun=1 wait:2000 expect-screen:Alert");
    assert_eq!(s.sys.settings.get(SettingId(0x0D3B)), 0);
    run(&mut s, "ok expect-screen:Home");
}

#[test]
fn snooze_rings_again() {
    let mut s = sim("set:0x0d35=1 set:clock=07:29:58 set:clockrun=1 wait:3000 expect-screen:Alert");
    run(&mut s, "next expect-screen:Home");
    assert!(s.sys.tools().snoozed());
    assert!(s.sys.status().alarm);
    run(&mut s, &format!("wait:{}", SNOOZE_MS - 2000));
    assert_eq!(s.sys.tools().ringing(), None);
    run(&mut s, "wait:2500 expect-screen:Alert");
    assert_eq!(s.sys.tools().ringing(), Some(Alert::Alarm(1)));
}

#[test]
fn no_alarm_without_a_clock() {
    let mut s = sim("set:0x0d35=1 set:clockvalid=0 set:clock=07:29:58 set:clockrun=1");
    run(&mut s, "wait:3000 expect-screen:Home");
    assert!(!s.sys.clock_valid());
    assert_eq!(s.sys.tools().ringing(), None);
}

#[test]
fn alarm_list_edits() {
    // Alarm tab (Prev from the stopwatch); the first alarm on.
    let mut s = sim(&format!("{APP} prev ok"));
    assert_eq!(s.sys.settings.get(SettingId(0x0D32)), 1);
    // Time: 08:00 (hour +1), switched on.
    run(&mut s, "down menu ok wait:100 expect-screen:Picker up ok expect-screen:Stopwatch");
    assert_eq!(s.sys.settings.get(SettingId(0x0D33)), 8 * 60 + 30);
    assert_eq!(s.sys.settings.get(SettingId(0x0D35)), 1);
    // Repeat: Saturday added.
    run(&mut s, "menu down ok wait:100 expect-screen:AlarmDays down*5 ok back");
    assert_eq!(s.sys.settings.get(SettingId(0x0D34)), 0x3F);
    // Off again: OK on the row.
    run(&mut s, "ok");
    assert_eq!(s.sys.settings.get(SettingId(0x0D35)), 0);
}

#[test]
fn the_note_says_alarms_need_the_player_on() {
    let s = sim(&format!("{APP} prev"));
    assert!(!s.sys.alarm_wakes_from_off());
    // Five rows: four alarms and the note (scrolls below them).
    let s2 = sim(&format!("set:alarmwake=1 {APP} prev down*4"));
    assert!(s2.sys.alarm_wakes_from_off());
    drop(s);
}
