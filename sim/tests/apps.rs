//! Behaviour of the `orpheus-apps` screens over the simulated services
//! (WP UI-2): every Settings group is walked, settings apply immediately,
//! dialogs, USB, battery, lock, language.

use orpheus_apps::services::{PowerRequest, StorageRequest, UsbMode};
use orpheus_apps::settings::{GROUPS, SettingId, id};
use orpheus_apps::{App, settings};
use orpheus_sim::AppSim;
use orpheus_sim::apps::Logged;

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
    s
}

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

/// Open every row of every group and come back: no panic, no lost screen.
#[test]
fn walk_every_settings_group() {
    for (g, grp) in GROUPS.iter().enumerate() {
        let mut s = sim(&format!("prev ok expect-screen:Settings down*{g} ok expect-screen:SettingsGroup"));
        let rows = grp.items.len();
        for r in 0..rows {
            // Fresh walk to row r each time (toggles flip on OK).
            run(&mut s, &format!("hold:back prev ok down*{g} ok down*{r} ok wait:50"));
            let top = s.screen();
            match top {
                "SettingsGroup" => {
                    // Toggle flipped, a dialog opened, or a toast: close it.
                    if s.shell.dialog_open() {
                        run(&mut s, "back");
                    }
                    // Rotate turned on swaps the keys the walk relies on:
                    // OK (not swapped) on the same row turns it off again.
                    if s.sys.settings.get(orpheus_apps::settings::id::ROTATE) != 0 {
                        run(&mut s, "ok");
                    }
                }
                "Choice" | "Slider" | "Picker" | "Info" | "HomeApps" | "Notice" | "UpdateFirmware" => {
                    run(&mut s, "back");
                }
                other => panic!("group {g} row {r}: unexpected screen {other}"),
            }
            run(&mut s, "expect-screen:SettingsGroup");
        }
    }
}

#[test]
fn choice_applies_immediately_and_restyles() {
    let mut s = sim("prev ok down*2 ok down*3 ok expect-screen:Choice");
    let dark_bg = s.fb[64 * 128 + 64];
    run(&mut s, "down ok expect-screen:SettingsGroup");
    assert_eq!(s.sys.settings.get(id::THEME), 1);
    assert_ne!(s.fb[64 * 128 + 64], dark_bg, "light theme drawn at once");
    // The group row shows the new value after returning.
    run(&mut s, "back back expect-screen:Home");
}

#[test]
fn slider_is_live() {
    let s = sim("prev ok down*2 ok ok expect-screen:Slider up*3");
    assert_eq!(s.sys.settings.get(id::BRIGHTNESS), 9);
    let s = sim("prev ok down*2 ok ok up*20");
    assert_eq!(s.sys.settings.get(id::BRIGHTNESS), 11, "clamped at the stock maximum");
    let s = sim("prev ok down*2 ok ok down*20");
    assert_eq!(s.sys.settings.get(id::BRIGHTNESS), 1, "clamped at the minimum");
}

#[test]
fn toggles_flip_in_place() {
    let s = sim("prev ok down ok ok");
    assert_eq!(s.sys.settings.get(id::SHUFFLE), 1);
    assert_eq!(s.screen(), "SettingsGroup");
}

#[test]
fn factory_reset_asks_with_safe_focus() {
    let mut s = sim("prev ok down*2 ok ok up*3 back back");
    assert_eq!(s.sys.settings.get(id::BRIGHTNESS), 9);
    run(&mut s, "down*9 ok down*4 ok");
    assert!(s.shell.dialog_open());
    run(&mut s, "ok");
    assert_eq!(s.sys.settings.get(id::BRIGHTNESS), 9, "OK on the safe button resets nothing");
    run(&mut s, "ok next ok");
    assert_eq!(s.sys.settings.get(id::BRIGHTNESS), 6);
    assert!(s.sys.log.contains(&Logged::FactoryReset));
}

#[test]
fn format_and_storage_actions() {
    let s = sim("prev ok down*10 ok down*4 ok next ok");
    assert!(s.sys.log.contains(&Logged::Storage(StorageRequest::Format)));
    let s = sim("prev ok down*10 ok down*3 ok");
    assert!(s.sys.log.contains(&Logged::Storage(StorageRequest::CheckCard)));
}

#[test]
fn usb_ask_then_always() {
    let mut s = sim("set:usb=host expect-screen:UsbConnect");
    run(&mut s, "down*4 ok up*3 ok expect-screen:Home");
    assert!(s.sys.log.contains(&Logged::Usb(UsbMode::FileTransfer)));
    assert_eq!(s.sys.settings.get(id::USB_WHEN_CONNECTED), 2);
    s.sys.log.clear();
    run(&mut s, "set:usb=off set:usb=host expect-screen:Home");
    assert!(s.sys.log.contains(&Logged::Usb(UsbMode::FileTransfer)), "no dialog the second time");
}

#[test]
fn low_battery_toasts_then_notice() {
    let mut s = sim("set:battery=15 expect-screen:Home");
    run(&mut s, "set:battery=5 expect-screen:Notice back expect-screen:Home");
}

#[test]
fn lock_screen_shows_for_two_seconds() {
    let mut s = sim("power");
    assert!(!s.shell.gate.screen_on);
    run(&mut s, "down expect-screen:Lock");
    assert!(s.shell.gate.screen_on);
    run(&mut s, "wait:2100 expect-screen:Home");
    assert!(!s.shell.gate.screen_on, "back off after the notice");
    run(&mut s, "power");
    assert!(s.shell.gate.screen_on && !s.shell.gate.locked);
}

#[test]
fn power_menu_actions() {
    let s = sim("hold:power expect-screen:PowerMenu ok expect-screen:Goodbye wait:400");
    assert!(s.sys.log.contains(&Logged::Power(PowerRequest::PowerOff)));
    let s = sim("hold:power down*2 ok expect-screen:Home");
    assert!(s.shell.gate.locked, "Lock keys");
    let mut s = sim("hold:power down*4 ok");
    assert!(s.shell.dialog_open(), "start original firmware asks first");
    run(&mut s, "next ok");
    assert!(s.sys.log.contains(&Logged::Power(PowerRequest::StartOriginal)));
    let s = sim("set:dual=0 hold:power down*4 ok next ok");
    assert!(s.sys.log.contains(&Logged::Power(PowerRequest::Recovery)), "no dual boot: row 5 is recovery");
}

#[test]
fn quick_panel_changes_values_in_row() {
    let s = sim("hold:menu expect-screen:QuickPanel down*4 next next");
    assert_eq!(s.sys.settings.get(id::BRIGHTNESS), 8);
    let s = sim("hold:menu down*2 prev");
    assert_eq!(s.sys.settings.get(id::OUTPUT), 1, "choices wrap");
    let s = sim("hold:menu down ok");
    assert_eq!(s.sys.settings.get(id::BLUETOOTH), 1);
}

#[test]
fn home_grid_and_hidden_apps() {
    let mut s = sim("set:card=absent ok expect-screen:Notice back");
    // With a card, Music opens (UI-4); Radio opens (UI-5a); the Recorder
    // opens (UI-5b).
    run(
        &mut s,
        "set:card=ready ok expect-screen:Music back next*2 ok expect-screen:Radio back prev*2 \
         down ok expect-screen:Recorder back up",
    );
    // Hide Radio (bit 2) in Settings > Display > Home screen.
    run(&mut s, "prev ok down*2 ok down*11 ok expect-screen:HomeApps down*2 ok");
    assert_eq!(s.sys.settings.get(id::HOME_APPS) & (1 << 2), 0);
    // Settings cannot be hidden.
    run(&mut s, "down*8 ok");
    run(&mut s, "hold:back expect-screen:Home");
    // Ten apps now: Prev from the first is Settings (the tenth).
    run(&mut s, "prev ok expect-screen:Settings");
}

#[test]
fn picker_sets_the_clock() {
    let s = sim("prev ok down*4 ok ok up next up up ok");
    let t = s.sys.clock;
    assert_eq!((t.hour, t.minute), (13, 43));
    let s = sim("prev ok down*4 ok down ok down ok");
    assert_eq!(s.sys.clock.day, 23);
}

#[test]
fn language_switch_changes_strings() {
    let mut s = sim("lang:cyrillic");
    assert_eq!(s.sys.lang.code(), "ru");
    run(&mut s, "prev ok down*9 ok ok down ok");
    assert_eq!(s.sys.lang.code(), "ja", "Language choice list: English, Русский, 日本語");
    assert_eq!(settings::lang_code("ja"), s.sys.settings.get(id::LANGUAGE));
}

#[test]
fn boot_goes_home() {
    let s = sim("goto:Boot expect-screen:Boot wait:400 expect-screen:Home");
    assert!(s.at_home());
}

#[test]
fn charging_screen_starts_player() {
    let s = sim("set:charge=charging goto:Charging ok expect-screen:Home");
    assert!(s.sys.log.contains(&Logged::Power(PowerRequest::StartFromCharging)));
}

#[test]
fn every_app_has_an_icon_and_label() {
    for a in App::ALL {
        assert_eq!(a.icon().data.len(), 24 * 24 / 4);
        assert!(!orpheus_apps::Lang::ENGLISH.get(a.label()).is_empty());
    }
    assert!(settings::def(SettingId(0x0304)).is_some());
}

#[test]
fn toast_over_own_strip_is_repainted() {
    let mut s = sim("set:card=absent ok expect-screen:Notice wait:100");
    let with_toast = s.fb.clone();
    run(&mut s, "wait:2100");
    let strip = |fb: &[u16]| fb[110 * 128..].to_vec();
    assert_ne!(strip(&with_toast), strip(&s.fb), "toast gone");
    // Same pixels as a notice that never had a toast.
    let fresh = sim("set:card=absent wait:2200 ok expect-screen:Notice wait:100");
    assert_eq!(strip(&fresh.fb), strip(&s.fb));
}

/// D-4: Settings › System › Performance is a Normal / Maximum choice,
/// stored like any other setting.
#[test]
fn performance_choice() {
    let s = sim("prev ok down*11 ok down*6 ok expect-screen:Choice down ok expect-screen:SettingsGroup");
    assert_eq!(s.sys.settings.get(id::PERFORMANCE), 1);
    let d = settings::def(id::PERFORMANCE).unwrap();
    assert_eq!((d.default, d.validate(2)), (0, 0), "Normal by default, out of range is Normal");
}

/// PWR-5: the Developer row is hidden until OK is pressed five times in
/// About; it starts and stops the battery logger.
#[test]
fn developer_rows_unlock_and_logger() {
    // Without the unlock the last System row is Performance.
    let mut s = sim("prev ok down*11 ok up ok expect-screen:Choice");
    assert!(!s.sys.developer);
    run(&mut s, "back back ok ok expect-screen:Info ok*4");
    assert!(!s.sys.developer, "four presses are not enough");
    run(&mut s, "ok");
    assert!(s.sys.developer);
    run(&mut s, "back up ok expect-screen:Developer ok");
    assert!(s.sys.drain.on && !s.sys.drain.stay);
    assert_eq!(s.sys.drain.file, 1);
    run(&mut s, "down ok");
    assert!(s.sys.drain.on && s.sys.drain.stay);
    run(&mut s, "up ok");
    assert!(!s.sys.drain.on && s.sys.drain.stay);
    run(&mut s, "ok");
    assert_eq!(s.sys.drain.file, 2, "a new log is a new file");
}
