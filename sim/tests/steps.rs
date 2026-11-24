//! UI-5c / SENS-1 over the fake accelerometer (`sim/src/steps.rs`, the
//! real detector at 25 Hz): counting while walking, nothing at rest, the
//! app's settings and reset, day rollover, no history without a clock.

use orpheus_apps::services::{StepsService, other};
use orpheus_apps::settings::id;
use orpheus_sim::AppSim;
use orpheus_ui::event::{Event, ServiceEvent};

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

const APP: &str = "next*6 ok wait:100 expect-screen:Steps";

#[test]
fn walking_counts_and_the_screen_follows() {
    let mut s = sim(&format!("set:walk=on {APP}"));
    run(&mut s, "wait:60000");
    // 1.9 steps/s for 60 s; the first step only primes the filters.
    let n = s.sys.steps_day(0);
    assert!((108..=116).contains(&n), "{n}");
    // The count on screen is redrawn from the service.
    let before = s.fb.clone();
    run(&mut s, "wait:5000");
    assert!(s.sys.steps_day(0) > n);
    assert_ne!(s.fb, before, "the count on screen changed");
    // Stopped walking: the count stays.
    run(&mut s, "set:walk=off wait:30000");
    let stop = s.sys.steps_day(0);
    run(&mut s, "wait:60000");
    assert_eq!(s.sys.steps_day(0), stop);
}

#[test]
fn nothing_counted_on_a_table_for_ten_minutes() {
    let mut s = sim("");
    run(&mut s, "wait:600000");
    assert_eq!(s.sys.steps_day(0), 0);
}

#[test]
fn counting_sends_steps_events() {
    use orpheus_sim::Host;
    let mut sys = orpheus_sim::apps::HostServices::default();
    sys.steps.walking = true;
    let mut seen = 0;
    for _ in 0..40 {
        sys.advance(500);
        seen += sys
            .take_events()
            .iter()
            .filter(|e| matches!(e, Event::Service(ServiceEvent::Other { kind: other::STEPS, .. })))
            .count();
    }
    assert!(seen > 0, "a STEPS event when the count changed");
    assert!(seen <= 40, "at most one per advance");
}

#[test]
fn reset_today_from_the_menu() {
    let mut s = sim(&format!("set:week=6482/9120 {APP} menu down*6 ok"));
    assert!(s.shell.dialog_open());
    // Cancel keeps the count.
    run(&mut s, "ok");
    assert_eq!(s.sys.steps_day(0), 6482);
    run(&mut s, "menu down*6 ok next ok wait:100");
    assert_eq!(s.sys.steps_day(0), 0);
    assert_eq!(s.sys.steps_day(1), 9120, "yesterday untouched");
}

#[test]
fn settings_from_the_menu() {
    // Daily goal: slider in steps of 500.
    let mut s = sim(&format!("{APP} menu ok wait:100 expect-screen:Slider up*4 ok"));
    assert_eq!(s.sys.settings.get(id::STEP_GOAL), 12_000);
    // Units: miles.
    run(&mut s, "menu down*3 ok wait:100 expect-screen:Choice down ok");
    assert_eq!(s.sys.settings.get(id::STEP_UNITS), 1);
    // Count in background: off.
    run(&mut s, "expect-screen:Steps menu down*4 ok wait:100 up ok");
    assert_eq!(s.sys.settings.get(id::STEP_BACKGROUND), 0);
    assert!(!s.sys.steps_info().counting);
    // Sensitivity: high.
    run(&mut s, "expect-screen:Steps menu down*5 ok wait:100 down ok");
    assert_eq!(s.sys.settings.get(id::STEP_SENSITIVITY), 2);
}

#[test]
fn midnight_moves_today_into_the_history() {
    let mut s = sim("set:steps=5000 set:clock=23:59:58 set:clockrun=1");
    run(&mut s, "wait:3000");
    assert_eq!((s.sys.steps_day(0), s.sys.steps_day(1)), (0, 5000));
    // The week view shows yesterday.
    run(&mut s, &format!("{APP} next down"));
}

#[test]
fn no_history_without_a_clock() {
    let s = sim(&format!("set:clockvalid=0 set:steps=40 {APP}"));
    assert!(!s.sys.steps_info().dated);
    assert_eq!(s.sys.steps_day(0), 40, "counting goes on");
}

#[test]
fn week_selection_stays_in_the_week() {
    let mut s = sim(&format!("set:week=1/2/3/4/5/6/7 {APP} next"));
    run(&mut s, "down*20 up*3 expect-screen:Steps");
    // Leaving and coming back keeps the tab.
    run(&mut s, "menu ok wait:100 back expect-screen:Steps");
}
