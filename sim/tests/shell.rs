//! Behaviour of the toolkit shell through the simulator: key grammar
//! (05 §3), navigation stack and resume records (05 §5.1), lock and wake,
//! overlays, and dirty-rectangle economy.

use yoton_sim::Sim;
use yoton_ui::Key;
use yoton_ui::gfx::{CONTENT, STATUS_BAR, STRIP};

fn sim(script: &str) -> Sim {
    let mut s = Sim::new();
    s.run_script(script).unwrap();
    s
}

#[test]
fn script_basics_and_expect_screen() {
    let mut s = sim("ok expect-screen:Library");
    assert!(s.run_script("expect-screen:Home").is_err());
    s.run_script("back expect-screen:Home").unwrap();
    assert!(s.run_script("bogus").is_err());
    assert!(s.run_script("wait:x").is_err());
}

#[test]
fn hold_back_goes_home_from_anywhere() {
    let mut s = sim("ok ok expect-screen:NowPlaying");
    assert_eq!(s.shell.depth(), 3);
    s.run_script("hold:back expect-screen:Home").unwrap();
    assert_eq!(s.shell.depth(), 1);
}

#[test]
fn pop_restores_focus_from_resume_record() {
    // Library, move to the 5th artist, open the player, come back.
    let mut s = sim("ok down*7");
    let before = s.fb.clone();
    s.run_script("ok expect-screen:Library").unwrap(); // artist: toast only
    s.run_script("wait:2100").unwrap();
    s.run_script("up*6 ok expect-screen:NowPlaying back expect-screen:Library").unwrap();
    s.run_script("down*6 wait:10").unwrap();
    // Same focus and scroll as before: identical list pixels.
    let content = |fb: &[u16]| fb[CONTENT.y as usize * 128..CONTENT.bottom() as usize * 128].to_vec();
    assert_eq!(content(&before), content(&s.fb));
}

#[test]
fn back_at_root_stays() {
    let mut s = sim("back back");
    assert!(s.at_home());
    s.run_script("expect-screen:Home").unwrap();
}

#[test]
fn menu_tap_opens_and_closes_context_menu() {
    let mut s = sim("ok menu");
    assert!(s.shell.menu_open());
    s.run_script("menu").unwrap();
    assert!(!s.shell.menu_open());
    s.run_script("menu back").unwrap();
    assert!(!s.shell.menu_open());
    // Home has no context menu: nothing opens.
    let mut h = sim("menu");
    assert!(!h.shell.menu_open());
    h.run_script("expect-screen:Home").unwrap();
}

#[test]
fn menu_toggle_applies_in_place_and_submenu_radio() {
    let mut s = sim("ok menu down ok");
    assert!(s.shell.menu_open(), "toggle keeps the menu open");
    assert!(s.sys.shuffle);
    s.run_script("next").unwrap();
    assert!(!s.sys.shuffle, "Next flips a toggle row");
    s.run_script("down ok down*2 ok").unwrap();
    assert_eq!(s.sys.repeat, 2);
    s.run_script("back back").unwrap();
    assert!(!s.shell.menu_open());
}

#[test]
fn destructive_dialog_focuses_safe_button() {
    let mut s = sim("ok menu down*5 ok");
    assert!(s.shell.dialog_open());
    s.run_script("ok").unwrap();
    assert!(!s.shell.dialog_open());
    assert!(!s.sys.log.iter().any(|l| l == "deleted"), "OK on the focused (safe) button deletes nothing");
    s.run_script("menu down*5 ok next ok").unwrap();
    assert!(s.sys.log.iter().any(|l| l == "deleted"));
    s.run_script("menu down*5 ok back").unwrap();
    assert!(s.sys.log.iter().any(|l| l == "dialog cancelled"));
}

#[test]
fn volume_works_everywhere_and_overlay_expires() {
    let mut s = sim("ok menu vol+ vol+ vol-");
    assert_eq!(s.sys.volume, 13);
    assert!(s.shell.menu_open(), "volume does not close the menu");
    let with_overlay = s.fb.clone();
    s.run_script("wait:1300").unwrap();
    assert_ne!(with_overlay, s.fb, "overlay removed after 1.2 s");
    // Ramp while held.
    let mut r = sim("hold:vol+:1500");
    assert!(r.sys.volume >= 12 + 8, "{}", r.sys.volume);
    r.run_script("set:volume=30 vol+").unwrap();
    assert_eq!(r.sys.volume, 30);
}

#[test]
fn power_tap_locks_and_first_key_only_wakes() {
    let mut s = sim("power");
    assert!(s.requests.iter().any(|r| r.screen_off));
    assert!(!s.shell.gate.screen_on);
    s.run_script("down").unwrap();
    assert!(s.requests.iter().any(|r| r.wake));
    assert!(s.shell.gate.screen_on);
    // Volume passes while locked and does not wake.
    let mut v = sim("power vol+");
    assert_eq!(v.sys.volume, 13);
    assert!(!v.requests.iter().any(|r| r.wake));
    // Locked with the screen on: a key shows the lock notice, nothing moves.
    v.shell.gate.screen_on = true;
    let fb = v.fb.clone();
    v.run_script("down").unwrap();
    assert!(v.requests.iter().any(|r| r.lock_notice));
    assert_eq!(fb, v.fb);
}

#[test]
fn power_hold_and_long() {
    let s = sim("hold:power:1600");
    assert!(s.requests.iter().any(|r| r.power_menu));
    let l = sim("hold:power:4100");
    assert!(l.requests.iter().any(|r| r.power_off));
}

#[test]
fn moving_focus_redraws_only_two_rows() {
    let mut s = sim("ok wait:50");
    s.take_dirty();
    s.press(Key::Down);
    let d = s.take_dirty();
    let b = d.bounds();
    assert!(CONTENT.contains_rect(&b), "{b:?}");
    assert!(b.h <= 2 * 19, "{b:?}");
    s.release(Key::Down);
    assert!(s.take_dirty().is_empty(), "release changes nothing");
}

#[test]
fn status_change_redraws_only_status_bar() {
    let mut s = sim("ok wait:50");
    s.take_dirty();
    s.run_script("set:battery=50").unwrap();
    // `set:` invalidates everything in the simulator; a plain status poll
    // must redraw only the bar.
    s.take_dirty();
    s.sys.status.battery = 30;
    s.draw();
    let b = s.take_dirty().bounds();
    assert!(STATUS_BAR.contains_rect(&b), "{b:?}");
}

#[test]
fn mini_player_progress_redraws_only_strip() {
    let mut s = sim("set:playing=1 ok wait:50");
    s.take_dirty();
    if let Some(n) = s.sys.now_playing.as_mut() {
        n.progress = 900;
    }
    s.draw();
    let b = s.take_dirty().bounds();
    assert!(STRIP.contains_rect(&b), "{b:?}");
}

#[test]
fn idle_frames_are_free() {
    let mut s = sim("ok wait:100");
    let frames = s.frames;
    s.run_script("wait:500").unwrap();
    assert_eq!(s.frames, frames, "nothing changes, nothing is drawn");
}

#[test]
fn toast_expires_after_two_seconds() {
    let mut s = sim("ok down menu down*4 ok wait:100");
    let with = s.fb.clone();
    s.run_script("wait:2000").unwrap();
    let strip = |fb: &[u16]| fb[STRIP.y as usize * 128..].to_vec();
    assert_ne!(strip(&with), strip(&s.fb));
}

#[test]
fn rotation_swaps_keys() {
    let mut s = Sim::new();
    s.mapper.rotated = true;
    // Up on a rotated device moves down: Settings instead of Library.
    s.run_script("up ok expect-screen:Settings").unwrap();
    let mut n = Sim::new();
    n.run_script("up ok expect-screen:Library").unwrap();
}
