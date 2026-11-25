//! OTP-2 / USB-5 in the simulator: the Authenticator over the real vault
//! service (`orpheus_otp::service` on a RAM sector) and the real typist
//! into a fake computer (`orpheus_hid::host::FakeHost`). Set-up, unlock
//! and the back-off, codes and typing in each layout, imports, provisioning,
//! HOTP, edits, relocking.

use orpheus_apps::services::{AuthOutcome, AuthService, VaultState, unix_of};
use orpheus_otp::otp::{self, Algorithm, Code};
use orpheus_sim::AppSim;

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

/// Keys that enter `pin` on the pad (cursor on "1"), then finish.
fn pin_keys(pin: &str) -> String {
    let mut cur = 0i32;
    let mut out = String::new();
    for c in pin.bytes() {
        let d = i32::from(c - b'0');
        let want = if d == 0 { 10 } else { d - 1 };
        let (r0, c0, r1, c1) = (cur / 3, cur % 3, want / 3, want % 3);
        for _ in 0..(r1 - r0).rem_euclid(4) {
            out.push_str("down ");
        }
        for _ in 0..(c1 - c0).rem_euclid(3) {
            out.push_str("next ");
        }
        out.push_str("ok ");
        cur = want;
    }
    out.push_str("hold:ok wait:200");
    out
}

const APP: &str = "goto:Authenticator wait:100";

/// GitHub's demo secret is RFC 6238's SHA-1 seed; the sim clock is
/// 2026-11-24 12:41:00 UTC.
fn github_code() -> Code {
    let t = unix_of(&orpheus_sim::apps::HostServices::default().clock);
    Code::new(otp::totp(Algorithm::Sha1, b"12345678901234567890", t, 30, 6), 6)
}

#[test]
fn set_up_confirms_the_pin_and_unlocks() {
    // Home: the Authenticator is the ninth app. The honest-limit note first.
    let mut s = sim("next*8 ok wait:100 expect-screen:Authenticator");
    assert_eq!(s.sys.vault(), VaultState::Absent);
    run(&mut s, "ok");
    // A different confirmation starts over.
    run(&mut s, &pin_keys("2468"));
    run(&mut s, &pin_keys("2469"));
    assert_eq!(s.sys.vault(), VaultState::Absent);
    run(&mut s, &pin_keys("2468"));
    run(&mut s, &pin_keys("2468"));
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 0 });
    // Back leaves and locks; the PIN opens it again.
    run(&mut s, "back wait:100 expect-screen:Home");
    assert!(matches!(s.sys.vault(), VaultState::Locked { failures: 0, .. }));
    run(&mut s, &format!("{APP} {}", pin_keys("2468")));
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 0 });
}

#[test]
fn wrong_pins_back_off_and_the_count_survives() {
    let mut s = sim(&format!("set:vault=locked {APP}"));
    run(&mut s, &pin_keys("1111"));
    assert!(
        matches!(s.sys.vault(), VaultState::Locked { failures: 1, wait_ms } if wait_ms > 0 && wait_ms <= 1000)
    );
    run(&mut s, &format!("wait:1000 {}", pin_keys("1234")));
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 7 });
    // Four failures: 8 s, during which the pad does not submit.
    let mut s = sim(&format!("set:vault=locked set:wrongpin=4 {APP}"));
    assert!(matches!(s.sys.vault(), VaultState::Locked { failures: 4, wait_ms } if wait_ms > 7000));
    run(&mut s, &pin_keys("1234"));
    assert!(matches!(s.sys.vault(), VaultState::Locked { failures: 4, .. }));
    // The digits stay: finishing after the wait unlocks.
    run(&mut s, "wait:8000 hold:ok wait:200");
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 7 });
}

#[test]
fn ok_types_the_code_and_enter() {
    let mut s = sim(&format!("set:vault=unlocked set:usb=host {APP} ok wait:50 ok wait:100"));
    assert_eq!(s.sys.auth.typed(), format!("{}\n", github_code().as_str()));
    assert_eq!(s.sys.auth_outcome(), AuthOutcome::Typed);
    assert!(s.sys.auth.host.all_released());
    assert_eq!(s.sys.auth.host.overlaps, 0);
    // Without Enter, in the German and French layouts, keypad digits.
    for (layout, keypad) in [(2, 0), (3, 0), (3, 1), (1, 1)] {
        run(&mut s, &format!("set:0x0906=0 set:0x0904={layout} set:0x0905={keypad} ok wait:100"));
        assert_eq!(s.sys.auth.typed(), github_code().as_str(), "layout {layout} keypad {keypad}");
    }
}

#[test]
fn no_computer_no_clock_no_hotp_code() {
    let s = sim(&format!("set:vault=unlocked {APP} ok wait:50 ok wait:100"));
    assert_eq!(s.sys.auth_outcome(), AuthOutcome::NoComputer);
    assert_eq!(s.sys.auth.typed(), "");
    // TOTP needs the clock.
    let mut s =
        sim(&format!("set:vault=unlocked set:usb=host set:clockvalid=0 {APP} ok wait:50 ok wait:100"));
    assert_eq!(s.sys.auth_outcome(), AuthOutcome::NoCode);
    assert_eq!(s.sys.auth_code(0), None);
    // HOTP: a code only after Next code (the third entry, Bank).
    let mut s = sim(&format!("set:vault=unlocked set:usb=host {APP} down*2 ok wait:50 ok wait:100"));
    assert_eq!(s.sys.auth_outcome(), AuthOutcome::NoCode);
    // Big code menu: Type the code, Next code, …
    run(&mut s, "menu down ok wait:100 ok wait:100");
    let code = s.sys.auth_code(2).expect("a code after Next code");
    assert_eq!(code.as_str().len(), 8);
    assert_eq!(s.sys.auth.typed(), format!("{}\n", code.as_str()));
}

#[test]
fn import_adds_once_and_deletes_the_file() {
    let mut s = sim(&format!("set:vault=empty set:otpfile=demo {APP} menu ok wait:100"));
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 7 });
    match s.sys.auth_outcome() {
        AuthOutcome::Imported { report, deleted } => {
            assert_eq!((report.added, report.duplicates, report.bad), (7, 0, 0));
            assert!(deleted);
        }
        o => panic!("{o:?}"),
    }
    assert!(s.sys.auth.file.is_none(), "the file is wiped and deleted");
    // A second import: no file.
    run(&mut s, "ok menu ok wait:100");
    assert_eq!(s.sys.auth_outcome(), AuthOutcome::NoImportFile);
    // A line that is not understood keeps the file.
    run(&mut s, "set:otpfile=bad menu ok wait:100");
    match s.sys.auth_outcome() {
        AuthOutcome::Imported { report, deleted } => {
            assert_eq!((report.added, report.bad, deleted), (1, 1, false));
        }
        o => panic!("{o:?}"),
    }
    assert!(s.sys.auth.file.is_some());
}

#[test]
fn provisioning_needs_the_users_yes() {
    let mut s = sim(&format!("set:vault=unlocked {APP} set:provision=demo wait:50"));
    assert!(s.sys.provision_pending().is_some());
    // Add (the affirmative button) is focused.
    run(&mut s, "ok wait:100");
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 8 });
    assert_eq!(s.sys.auth_entry(7).unwrap().issuer.as_str(), "Fastmail");
    // Refused while locked.
    let s = sim(&format!("set:vault=locked {APP} set:provision=demo wait:50"));
    assert!(s.sys.provision_pending().is_none());
}

#[test]
fn delete_and_move() {
    // Move Proton (second) up, then delete the first (Proton).
    let mut s = sim(&format!("set:vault=unlocked {APP} down menu down ok wait:100"));
    assert_eq!(s.sys.auth_entry(0).unwrap().issuer.as_str(), "Proton");
    // Focus on the first row: Import, Move down, Delete…
    run(&mut s, "menu down*2 ok wait:50 next ok wait:100");
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 6 });
    assert_eq!(s.sys.auth_entry(0).unwrap().issuer.as_str(), "GitHub");
}

#[test]
fn relocks_after_a_minute_and_on_screen_off() {
    let mut s = sim(&format!("set:vault=unlocked {APP}"));
    run(&mut s, "wait:30000 down wait:59000");
    assert!(matches!(s.sys.vault(), VaultState::Unlocked { .. }));
    run(&mut s, "wait:1500");
    assert!(matches!(s.sys.vault(), VaultState::Locked { .. }));
    let mut s = sim(&format!("set:vault=unlocked {APP} power wait:100"));
    assert!(matches!(s.sys.vault(), VaultState::Locked { .. }));
    run(&mut s, "power wait:100");
}

#[test]
fn change_pin_and_wipe() {
    // Menu on the first row: Import, Move down, Delete…, Time check,
    // Change PIN, Lock, Wipe vault….
    let mut s = sim(&format!("set:vault=unlocked {APP} menu down*4 ok"));
    run(&mut s, &pin_keys("9876"));
    run(&mut s, &pin_keys("9876"));
    assert_eq!(s.sys.auth_outcome(), AuthOutcome::PinChanged);
    run(&mut s, "back wait:100");
    run(&mut s, &format!("{APP} {}", pin_keys("1234")));
    assert!(matches!(s.sys.vault(), VaultState::Locked { failures: 1, .. }));
    run(&mut s, &format!("wait:1100 {}", pin_keys("9876")));
    assert_eq!(s.sys.vault(), VaultState::Unlocked { count: 7 });
    // Wipe (destructive: Cancel focused, Next to Wipe).
    run(&mut s, "menu down*6 ok next ok wait:100");
    assert_eq!(s.sys.vault(), VaultState::Absent);
}
