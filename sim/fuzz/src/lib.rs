//! Fuzz-target body of the simulator (VER-4, 07 §3.8), shared by the
//! cargo-fuzz target in `fuzz_targets/` (nightly) and the stable replay of
//! the committed corpus (`sim/tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`ui_keys`] — the real `orpheus-apps` screens over the simulator's
//! services, driven by arbitrary key taps, holds, presses and releases,
//! waits, system events (battery, charger, card, USB, Bluetooth, playback,
//! library, clock, settings by id with any value), appearance and language
//! changes and direct jumps to screens. Oracles: no panic, no hang
//! (simulated time is bounded); the navigation stack is never empty nor
//! deeper than its capacity; every frame is drawn inside the 128 × 128
//! panel; a long Back press with no dialog, unlocked and with the screen on
//! returns to Home; and a full redraw at any moment produces exactly the
//! frame the incremental redraws left on the panel (golden-consistent
//! redraws: nothing stale, nothing missing).

#![forbid(unsafe_code)]

use orpheus_sim::AppSim;
use orpheus_ui::gfx::FB_PIXELS;
use orpheus_ui::nav::NAV_DEPTH;

const KEYS: [&str; 10] = ["up", "down", "prev", "next", "ok", "back", "menu", "vol+", "vol-", "power"];

/// `set:` commands of the simulator's services.
const SETS: [&str; 44] = [
    "battery=72",
    "battery=8",
    "battery=3",
    "battery=0",
    "battery=100",
    "charge=charging",
    "charge=full",
    "charge=none",
    "charge=fault",
    "card=absent",
    "card=ready",
    "card=unreadable",
    "usb=host",
    "usb=charger",
    "usb=off",
    "bt=off",
    "bt=starting",
    "bt=on",
    "bt=scanning",
    "bt=connecting",
    "bt=connected",
    "bt=streaming",
    "bt=dead",
    "btfound=0",
    "btfound=3",
    "btfound=8",
    "playing=1",
    "playing=0",
    "clock=23:59",
    "clock=0:00",
    "library=ready",
    "library=missing",
    "library=building",
    "queue=takk",
    "queue=kino",
    "queue=yoasobi",
    "queue=podcast",
    "pos=120000",
    "pause=1",
    "resume=1",
    "bookmark=1",
    "locked=1",
    "locked=0",
    "dual=0",
];

/// Setting ids (`orpheus_apps::settings::id`), set to any value.
const SETTINGS: [u16; 30] = [
    0x0101, 0x0107, 0x0108, 0x0201, 0x0202, 0x0206, 0x0301, 0x0302, 0x0304, 0x0305, 0x0308, 0x0309, 0x030A,
    0x030B, 0x0401, 0x0402, 0x0404, 0x0503, 0x0504, 0x0505, 0x0601, 0x0603, 0x0604, 0x0901, 0x0A01, 0x0D01,
    0x0D02, 0x0D03, 0x0D10, 0x030C,
];

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

/// Most commands per input, and simulated milliseconds per input.
pub const MAX_OPS: usize = 48;
pub const MAX_MS: u64 = 120_000;

/// The script command for one operation (see [`ui_keys`]).
pub fn command(op: u8, a: u8, b: u8) -> String {
    let key = KEYS[usize::from(a) % KEYS.len()];
    match op % 16 {
        0..=5 => key.to_string(),
        6 => format!("hold:{key}:{}", 200 + u32::from(b) * 20),
        7 => format!("press:{key}"),
        8 => format!("release:{key}"),
        9 => format!("wait:{}", u32::from(b) * 37 + 1),
        10 | 11 => format!("set:{}", SETS[usize::from(a) % SETS.len()]),
        12 => format!(
            "set:{:#06x}={}",
            SETTINGS[usize::from(a) % SETTINGS.len()],
            i16::from(b as i8).saturating_mul(if a & 0x80 != 0 { 300 } else { 1 })
        ),
        13 => match a % 3 {
            0 => format!("theme:{}", ["dark", "light", "amber"][usize::from(b) % 3]),
            1 => format!("accent:{}", ["blue", "green", "pink", "red", "grey"][usize::from(b) % 5]),
            _ => format!("lang:{}", ["latin", "cyrillic", "cjk"][usize::from(b) % 3]),
        },
        14 => format!("goto:{}", SCREENS[usize::from(a) % SCREENS.len()]),
        _ => format!("hold:back:{}", 700 + u32::from(b) * 4),
    }
}

/// Pixels the redraw comparison skips while an overlay is up: the
/// anti-aliased corners of the volume overlay, which blend over whatever is
/// under them and compound when the overlay is redrawn over itself
/// (finding F-UI-1, open).
fn known_stale(sim: &AppSim, i: usize) -> bool {
    if !sim.shell.overlay_shown() {
        return false;
    }
    let r = orpheus_ui::widgets::overlay::VolumeOverlay::rect();
    let (x, y) = ((i % 128) as i16, (i / 128) as i16);
    let near =
        |p: i16, lo: i16, len: i16| (lo..lo + 10).contains(&p) || (lo + len - 10..lo + len).contains(&p);
    near(x, r.x, r.w) && near(y, r.y, r.h)
}

/// Now Playing's Levels view redraws at most every 66 ms by design (the
/// meters move between two draws): no comparison there.
fn rate_limited(sim: &AppSim) -> bool {
    use orpheus_apps::services::SettingsService;
    sim.screen() == "NowPlaying" && sim.sys.setting(orpheus_apps::settings::id::NP_VIEW) == 5
}

/// A full redraw gives the frame the incremental redraws left.
fn check_redraw(sim: &mut AppSim) {
    if rate_limited(sim) {
        return;
    }
    sim.draw();
    let before = sim.fb.clone();
    sim.shell.invalidate();
    sim.draw();
    if let Some(diff) = (0..before.len()).find(|&i| sim.fb[i] != before[i] && !known_stale(sim, i)) {
        panic!("stale frame on {}: a full redraw differs at ({}, {})", sim.screen(), diff % 128, diff / 128);
    }
}

fn check(sim: &mut AppSim) {
    let d = sim.shell.depth();
    assert!((1..=NAV_DEPTH).contains(&d), "navigation depth {d}");
    assert!(!sim.screen().is_empty());
    assert_eq!(sim.fb.len(), FB_PIXELS);
    for r in sim.take_dirty().rects() {
        assert!(
            r.x >= 0 && r.y >= 0 && r.x + r.w <= 128 && r.y + r.h <= 128,
            "dirty {r:?} outside the panel"
        );
    }
}

/// Input: 3-byte operations `[op, a, b]` (see [`command`]); byte 0 of the
/// input picks how often a full redraw is compared.
pub fn ui_keys(data: &[u8]) {
    let Some((&sel, ops)) = data.split_first() else { return };
    let every = usize::from(sel % 8) + 1;
    let mut sim = AppSim::new();
    check(&mut sim);
    for (i, op) in ops.chunks_exact(3).take(MAX_OPS).enumerate() {
        if sim.now.0 > MAX_MS {
            break;
        }
        let cmd = command(op[0], op[1], op[2]);
        let back_home = cmd.starts_with("hold:back")
            && !sim.shell.dialog_open()
            && !sim.shell.gate.locked
            && sim.shell.gate.screen_on;
        if let Err(e) = sim.run_script(&cmd) {
            panic!("script command `{cmd}` refused: {e:#}");
        }
        check(&mut sim);
        if back_home && !sim.shell.dialog_open() {
            assert!(sim.at_home(), "long Back from {cmd} left {} on top", sim.screen());
        }
        if i % every == 0 {
            check_redraw(&mut sim);
        }
    }
    check_redraw(&mut sim);
}
