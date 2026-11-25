//! UI-8: list roll-around on every list screen, in both settings
//! (Settings › Display › List roll-around, key `0x030C`, on by default).
//!
//! For each screen, reached with its first row focused: with roll-around on,
//! Up rolls round to the last row, Down from there comes back to the first,
//! Prev pages round to the last row and Next back (where Prev/Next page the
//! list), and a held Down (20 s: enough for 5000 rows) runs to the last row
//! and stops there. With roll-around off, Up and Prev on the first row
//! stay, a held Down ends on the same last row, and Down there stays.
//!
//! Where a list is is read from the screen's resume record (focus and
//! scroll; the laps' scroll for the stopwatch), and from the frame for the
//! context menu, which is not a screen.

use orpheus_apps::AppScreen;
use orpheus_apps::settings::id;
use orpheus_sim::AppSim;
use orpheus_sim::apps::HostServices;
use orpheus_ui::gfx::CONTENT;
use orpheus_ui::nav::Screen;

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
    s
}

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

#[derive(Clone, Copy)]
enum Probe {
    /// Focus and scroll from the resume record.
    Resume,
    /// The stopwatch laps' scroll.
    Laps,
    /// The content area's pixels (the menu sheet).
    Frame,
}

/// Where the list is.
fn pos(s: &AppSim, p: Probe) -> Vec<u32> {
    let r = <AppScreen as Screen<HostServices>>::save(s.shell.top());
    match p {
        Probe::Resume => vec![r.focus, r.top],
        Probe::Laps => vec![r.handle >> 16 & 0xFF],
        Probe::Frame => {
            let (y, h) = (usize::try_from(CONTENT.y).unwrap(), usize::try_from(CONTENT.h).unwrap());
            s.fb[y * 128..(y + h) * 128].iter().map(|&p| u32::from(p)).collect()
        }
    }
}

/// A list screen: how to reach it with its first row focused, what to let
/// happen after a move (windows loading), whether Prev/Next page it.
struct Case {
    name: String,
    path: String,
    settle: &'static str,
    paging: bool,
    probe: Probe,
}

fn case(name: &str, path: impl Into<String>, settle: &'static str, paging: bool) -> Case {
    Case { name: name.into(), path: path.into(), settle, paging, probe: Probe::Resume }
}

const LIB: &str = "set:library=ready ok wait:100";
const RECORDER: &str = "next*4 ok wait:200 expect-screen:Recorder";
const STOPWATCH: &str = "next*7 ok wait:100 expect-screen:Stopwatch";
const BT: &str = "next*3 ok wait:100 expect-screen:Bluetooth";

fn cases() -> Vec<Case> {
    let mut v = vec![
        case("settings", "prev ok expect-screen:Settings", "", true),
        case("choice", "prev ok down*2 ok down*3 ok expect-screen:Choice", "", true),
        case("home_apps", "prev ok down*2 ok down*11 ok expect-screen:HomeApps", "", true),
        case("about", "prev ok down*11 ok ok expect-screen:Info", "", true),
        case("power_menu", "hold:power expect-screen:PowerMenu", "", true),
        // Prev/Next change the focused row's value here.
        case("quick_panel", "hold:menu expect-screen:QuickPanel", "", false),
        case("files", "next ok wait:100 expect-screen:Files", "wait:300", true),
        case("files_5000", "next ok down*2 ok ok wait:600 expect-screen:Files", "wait:300", true),
        case("music", format!("{LIB} expect-screen:Music"), "", true),
        case(
            "library_artists",
            format!("{LIB} down ok wait:100 expect-screen:MusicLibrary"),
            "wait:100",
            true,
        ),
        case("library_songs", format!("{LIB} down*3 ok wait:100"), "wait:100", true),
        case("bluetooth", format!("set:bt=on {BT}"), "", true),
        case("bt_found", format!("set:bt=on {BT} down*2 ok wait:6000 expect-screen:BtScan"), "", true),
        // Opens on the preset playing: a held Up takes it to the first.
        case(
            "radio_presets",
            "set:presets=demo set:radio=94700 next*2 ok wait:1100 menu down ok wait:100 \
             expect-screen:RadioPresets hold:up:3000",
            "",
            true,
        ),
        case(
            "recordings",
            format!(
                "{RECORDER} ok wait:1500 hold:ok next ok wait:200 set:clock=12:45:10 ok wait:1500 hold:ok next ok \
                 wait:200 menu down*8 ok wait:200 expect-screen:Recordings"
            ),
            "wait:100",
            true,
        ),
        case("authenticator", "set:vault=unlocked goto:Authenticator wait:100", "", true),
        // Prev/Next switch the stopwatch's tabs.
        case("alarms", format!("{STOPWATCH} prev"), "", false),
        case(
            "alarm_days",
            format!("{STOPWATCH} prev menu down ok wait:100 expect-screen:AlarmDays"),
            "",
            true,
        ),
        Case {
            name: "laps".into(),
            path: format!(
                "{STOPWATCH} ok wait:1000 next wait:1000 next wait:1000 next wait:1000 next wait:1000 next ok"
            ),
            settle: "",
            paging: false,
            probe: Probe::Laps,
        },
        Case {
            name: "menu".into(),
            path: "next ok wait:100 menu".into(),
            settle: "",
            paging: true,
            probe: Probe::Frame,
        },
    ];
    // Every Settings group with more than one row (Language has one).
    for g in [0, 1, 2, 3, 4, 5, 6, 7, 8, 10, 11] {
        v.push(case(
            &format!("group {g}"),
            format!("prev ok down*{g} ok expect-screen:SettingsGroup"),
            "",
            true,
        ));
    }
    v
}

#[test]
fn every_list_rolls_round_with_the_setting_on_and_clamps_off() {
    let mut fails = Vec::new();
    let mut check = |ok: bool, msg: String| {
        if !ok {
            fails.push(msg);
        }
    };
    for c in cases() {
        let what = format!("{} ({})", c.name, c.path);
        let (p, settle) = (c.probe, c.settle);
        // On (the default).
        let s0 = sim(&c.path);
        check(s0.sys.settings.get(id::LIST_ROLL) == 1, format!("{what}: on by default"));
        let first = pos(&s0, p);
        let mut s = sim(&c.path);
        run(&mut s, &format!("up {settle}"));
        let last = pos(&s, p);
        check(first != last, format!("{what}: Up on the first row moved nowhere"));
        run(&mut s, &format!("down {settle}"));
        check(pos(&s, p) == first, format!("{what}: Down on the last row is not the first row"));
        if c.paging {
            let mut s = sim(&c.path);
            run(&mut s, &format!("prev {settle}"));
            check(pos(&s, p) == last, format!("{what}: Prev on the first page is not the last row"));
            run(&mut s, &format!("next {settle}"));
            check(pos(&s, p) == first, format!("{what}: Next on the last page is not the first row"));
        }
        let mut s = sim(&c.path);
        run(&mut s, &format!("hold:down:20000 {settle}"));
        check(pos(&s, p) == last, format!("{what}: a held Down did not stop on the last row"));
        run(&mut s, &format!("down {settle}"));
        check(pos(&s, p) == first, format!("{what}: a fresh Down after the hold did not roll round"));

        // Off.
        let path = format!("set:0x030c=0 {}", c.path);
        let mut s = sim(&path);
        check(s.sys.settings.get(id::LIST_ROLL) == 0, format!("{what}: off"));
        check(pos(&s, p) == first, format!("{what}: off starts elsewhere"));
        run(&mut s, &format!("up {settle}"));
        check(pos(&s, p) == first, format!("{what}: Up on the first row moved with roll-around off"));
        if c.paging {
            run(&mut s, &format!("prev {settle}"));
            check(pos(&s, p) == first, format!("{what}: Prev on the first page moved with roll-around off"));
        }
        run(&mut s, &format!("hold:down:20000 {settle}"));
        check(pos(&s, p) == last, format!("{what}: off, a held Down did not end on the last row"));
        run(&mut s, &format!("down {settle}"));
        check(pos(&s, p) == last, format!("{what}: Down on the last row moved with roll-around off"));
        if c.paging {
            run(&mut s, &format!("next {settle}"));
            check(pos(&s, p) == last, format!("{what}: Next on the last page moved with roll-around off"));
        }
    }
    assert!(fails.is_empty(), "{}", fails.join("\n"));
}

/// The setting is a row of Settings › Display, flipped with OK, stored, and
/// read by the next key.
#[test]
fn display_row_switches_it() {
    let mut s = sim("prev ok down*2 ok down*10 ok");
    assert_eq!(s.sys.settings.get(id::LIST_ROLL), 0);
    // Back on the group's first row, Up stays …
    run(&mut s, "back ok up");
    let r = <AppScreen as Screen<HostServices>>::save(s.shell.top());
    assert_eq!(r.focus, 1, "row 0 is the group's title");
    // … and with it on again, Up rolls round to Home screen.
    run(&mut s, "down*10 ok");
    assert_eq!(s.sys.settings.get(id::LIST_ROLL), 1);
    run(&mut s, "up*10 up");
    let r = <AppScreen as Screen<HostServices>>::save(s.shell.top());
    assert_eq!(r.focus, 12, "the last row");
}
