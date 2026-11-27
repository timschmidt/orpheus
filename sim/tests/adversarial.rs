//! Adversarial language packs (VER-3, 07 §3.7): a `.ylang` pack is loaded
//! from the card or the NOR resource area (`orpheus_apps::strings`), so
//! its strings are external data. A pack whose every string is hostile —
//! very long, control characters, combining marks, right-to-left and
//! zero-width characters, characters the fonts do not have, a header
//! naming a long language — drives every screen of the UI fuzzer's list
//! through keys, menus and the screen reader's text, with the fuzzer's
//! oracles: no panic, dirty rectangles inside the panel, navigation depth
//! within its bounds, and a full redraw equal to the incremental frame.
//! A cut or damaged pack header is refused or falls back to English.

#[path = "../../tests/adversarial.rs"]
mod adversarial;

use adversarial::{config, damaged, prefixes};
use orpheus_apps::services::SettingsService;
use orpheus_apps::settings::{id, lang_code};
use orpheus_apps::strings::{Lang, Str};
use orpheus_sim::AppSim;
use orpheus_sim::packs::build;
use orpheus_ui::nav::NAV_DEPTH;
use proptest::prelude::*;

/// Hostile strings, one per id in turn.
fn hostile(i: usize) -> String {
    match i % 9 {
        0 => "W".repeat(400),
        1 => "line\nbreak\ttab\r\u{0}nul\u{7}".to_string(),
        2 => format!("e{}", "\u{301}".repeat(60)),
        3 => "\u{202E}txet desrever\u{202C}\u{200F}".to_string(),
        4 => "\u{200B}\u{200D}\u{FEFF}\u{2060}".repeat(20),
        5 => "\u{1F3B5}\u{10FFFF}\u{E000}\u{FFFD}".repeat(10),
        6 => "%s%d{}{0}\\u{41}&amp;<b>".to_string(),
        7 => " ".repeat(200),
        _ => "Ünïcødé 日本語 Русский العربية ".repeat(8),
    }
}

fn hostile_pack(code: &str, name: &str) -> &'static [u8] {
    let texts: Vec<String> = (0..Str::ALL.len()).map(hostile).collect();
    let entries: Vec<(Str, &str)> = Str::ALL.iter().copied().zip(texts.iter().map(String::as_str)).collect();
    Box::leak(build(code, name, &entries).into_boxed_slice())
}

/// The fuzzer's oracles after a command.
fn check(sim: &mut AppSim) {
    let d = sim.shell.depth();
    assert!((1..=NAV_DEPTH).contains(&d), "navigation depth {d} on {}", sim.screen());
    for r in sim.take_dirty().rects() {
        assert!(
            r.x >= 0 && r.y >= 0 && r.x + r.w <= 128 && r.y + r.h <= 128,
            "dirty {r:?} on {}",
            sim.screen()
        );
    }
    sim.draw();
    let before = sim.fb.clone();
    sim.shell.invalidate();
    sim.draw();
    let overlay = sim.shell.overlay_shown();
    if !overlay && let Some(i) = (0..before.len()).find(|&i| sim.fb[i] != before[i]) {
        panic!("stale frame on {}: a full redraw differs at ({}, {})", sim.screen(), i % 128, i / 128);
    }
}

const SCREENS: [&str; 17] = [
    "Home",
    "Music",
    "Files",
    "NowPlaying",
    "Equaliser",
    "Boot",
    "Settings",
    "PowerMenu",
    "QuickPanel",
    "Lock",
    "Charging",
    "UsbConnect",
    "Goodbye",
    "HomeApps",
    "Bluetooth",
    "BtScan",
    "BtPaired",
];

fn with_pack(pack: &'static [u8]) -> AppSim {
    let mut sim = AppSim::new();
    let lang = Lang::from_pack(pack).expect("a well-formed pack");
    sim.sys.langs.push(lang);
    sim.sys.set_setting(id::LANGUAGE, lang_code(lang.code()));
    assert_eq!(sim.sys.lang.code(), lang.code(), "the pack is in use");
    sim
}

#[test]
fn the_hostile_pack_is_drawn() {
    let mut en = AppSim::new();
    en.run_script("goto:Settings").unwrap();
    en.draw();
    let mut x = with_pack(hostile_pack("xx", "Hostile"));
    x.run_script("goto:Settings").unwrap();
    x.draw();
    assert_ne!(en.fb, x.fb, "the hostile strings change the screen");
}

#[test]
fn every_screen_in_a_hostile_language() {
    for (code, name) in [("xx", "Hostile"), ("zz", "a very long language name that no screen has room for")] {
        let mut sim = with_pack(hostile_pack(code, name));
        check(&mut sim);
        for s in SCREENS {
            for cmd in [
                format!("goto:{s}"),
                "down".into(),
                "down".into(),
                "ok".into(),
                "menu".into(),
                "down".into(),
                "ok".into(),
                "wait:700".into(),
                "back".into(),
                "back".into(),
                "set:battery=3".into(),
                "set:card=unreadable".into(),
                "hold:back:900".into(),
                "set:card=ready".into(),
            ] {
                sim.run_script(&cmd).unwrap_or_else(|e| panic!("`{cmd}` refused: {e:#}"));
                check(&mut sim);
            }
        }
        // Settings, every row and its value list.
        sim.run_script("goto:Settings").unwrap();
        for _ in 0..12 {
            for cmd in ["ok", "down", "down", "ok", "back", "back", "down"] {
                sim.run_script(cmd).unwrap();
                check(&mut sim);
            }
        }
    }
}

#[test]
fn pack_header_cut_and_damaged() {
    let pack = build("ru", "Русский", &[(Str::ALL[0], "Музыка")]);
    prefixes(&pack, |p| {
        let p: &'static [u8] = Box::leak(p.to_vec().into_boxed_slice());
        if let Some(l) = Lang::from_pack(p) {
            let _ = (l.code(), l.name());
            for &s in Str::ALL {
                let _ = l.get(s);
            }
        }
    });
    damaged(&pack[..pack.len().min(64)], |_, d| {
        let mut b = d.to_vec();
        b.extend_from_slice(&pack[d.len()..]);
        let p: &'static [u8] = Box::leak(b.into_boxed_slice());
        if let Some(l) = Lang::from_pack(p) {
            let _ = (l.code(), l.name());
            for &s in Str::ALL {
                let _ = l.get(s);
            }
        }
    });
}

proptest! {
    #![proptest_config(config("sim-language-pack-entries", 24))]

    /// A pack with any table entries (offsets and lengths) and any data
    /// bytes: every string read is UTF-8 or English, and the Home and
    /// Settings screens draw.
    #[test]
    fn any_pack_entries(entries in prop::collection::vec((any::<u16>(), any::<u16>()), 0..64),
                        data in prop::collection::vec(any::<u8>(), 0..512)) {
        let mut b = b"YLN1".to_vec();
        b.extend((entries.len() as u16).to_le_bytes());
        b.extend([2, 3]);
        b.extend(b"qqQqq");
        for (o, l) in &entries {
            b.extend(o.to_le_bytes());
            b.extend(l.to_le_bytes());
        }
        b.extend(&data);
        let p: &'static [u8] = Box::leak(b.into_boxed_slice());
        if let Some(l) = Lang::from_pack(p) {
            for &s in Str::ALL {
                let _ = l.get(s);
            }
            let mut sim = with_pack(p);
            for cmd in ["goto:Home", "down", "goto:Settings", "ok", "down"] {
                sim.run_script(cmd).unwrap();
                check(&mut sim);
            }
        }
    }
}
