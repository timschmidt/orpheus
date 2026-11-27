//! Settings › Storage and System › Diagnostics in the simulator (VER-13,
//! DOC-2 findings 2026-11-27): Clear caches, Check card and Format card do
//! their card work through `orpheus-fs` on the simulated card, as the
//! firmware's card task does (`fw/firmware/src/care.rs`); Diagnostics is a
//! read-only page.

use orpheus_apps::services::{BrowseService, CardState, StorageOutcome, StorageRequest, StorageService};
use orpheus_fs::host::block_on;
use orpheus_fs::{DirEntry, FsKind, OpenMode};
use orpheus_sim::AppSim;
use orpheus_ui::a11y::{Desc, Layer};

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

fn sim(script: &str) -> AppSim {
    let mut s = AppSim::new();
    run(&mut s, script);
    s
}

fn describe(s: &mut AppSim) -> Desc {
    let mut d = Desc::new();
    s.shell.describe(&mut s.sys, &mut d);
    d
}

/// Settings › Storage (the eleventh group): Card info, Rebuild library,
/// Clear caches, Check card, Format card….
const STORAGE: &str = "goto:Settings wait:50 down*10 ok wait:50";

/// Files under `/.orpheus/cache` and `/.orpheus/index`, and some that are
/// not caches.
fn seed(s: &mut AppSim) {
    let v = s.sys.media.volume();
    for d in ["/.orpheus", "/.orpheus/cache", "/.orpheus/cache/art", "/.orpheus/index", "/.orpheus/library"] {
        let _ = block_on(v.create_dir(d));
    }
    for p in [
        "/.orpheus/cache/art/A0001.bin",
        "/.orpheus/cache/W.bin",
        "/.orpheus/index/1.idx",
        "/.orpheus/library/lib.ydb",
        "/.orpheus/queue.bin",
    ] {
        let mut f = block_on(v.open(p, OpenMode::CreateNew)).unwrap();
        block_on(v.write(&mut f, &[3u8; 4000])).unwrap();
        block_on(v.close(f)).unwrap();
    }
}

fn exists(s: &mut AppSim, p: &str) -> bool {
    block_on(s.sys.media.volume().exists(p)).unwrap()
}

fn root_names(s: &mut AppSim) -> Vec<String> {
    let v = s.sys.media.volume();
    let mut cur = v.root();
    let mut e = DirEntry::new();
    let mut out = Vec::new();
    while block_on(v.read_dir(&mut cur, &mut e)).unwrap() {
        out.push(e.name().to_string());
    }
    out
}

#[test]
fn clear_caches_removes_the_caches_only() {
    let mut s = sim(STORAGE);
    seed(&mut s);
    let music = root_names(&mut s);
    assert!(music.iter().any(|n| n == "Music"), "{music:?}");
    // Clear caches asks first (Clear focused); Cancel does nothing.
    run(&mut s, "down*2 ok");
    assert!(s.shell.dialog_open());
    run(&mut s, "prev ok wait:50");
    assert!(!s.shell.dialog_open());
    assert!(exists(&mut s, "/.orpheus/cache/art/A0001.bin"));
    run(&mut s, "ok ok wait:50");
    assert!(!exists(&mut s, "/.orpheus/cache"));
    assert!(!exists(&mut s, "/.orpheus/index"));
    for keep in ["/.orpheus/library/lib.ydb", "/.orpheus/queue.bin"] {
        assert!(exists(&mut s, keep), "{keep} stays");
    }
    assert_eq!(root_names(&mut s), music, "nothing outside /.orpheus changes");
    let o = s.sys.storage_outcome();
    assert_eq!((o.req, o.busy, o.ok), (Some(StorageRequest::ClearCaches), false, true));
}

#[test]
fn check_card_reports_and_writes_nothing() {
    let mut s = sim(STORAGE);
    let dirty = s.sys.media.volume().device().dirty_sectors();
    run(&mut s, "down*3 ok wait:50");
    assert_eq!(s.sys.media.volume().device().dirty_sectors(), dirty, "read-only");
    let o = s.sys.storage_outcome();
    assert_eq!((o.req, o.ok, o.problems, o.stopped), (Some(StorageRequest::CheckCard), true, 0, false));
    assert!(o.dirs > 5 && o.files > 20, "{o:?}");
    // The result, in a dialog over Storage.
    let d = describe(&mut s);
    assert_eq!(d.layer, Layer::Dialog);
    assert_eq!(d.title.as_str(), "No problems found");
    for want in [format!("Folders {}", o.dirs), format!("Files {}", o.files)] {
        assert!(d.summary.as_str().contains(&want), "{}", d.summary.as_str());
    }
    run(&mut s, "ok");
    assert!(!s.shell.dialog_open());
}

#[test]
fn check_card_finds_a_broken_file() {
    let mut s = sim(&format!("set:cardbad=1 {STORAGE} down*3 ok wait:50"));
    let o = s.sys.storage_outcome();
    assert_eq!((o.ok, o.problems), (true, 1), "{o:?}");
    let d = describe(&mut s);
    assert_eq!(d.title.as_str(), "Problems found");
    assert!(d.summary.as_str().contains("Problems 1"), "{}", d.summary.as_str());
    assert!(d.summary.as_str().contains("Check the card on a computer."), "{}", d.summary.as_str());
}

#[test]
fn format_card_asks_then_empties_the_card() {
    // The confirmation names what goes; Cancel is focused and does nothing.
    let mut s = sim(&format!("set:0x030D=9 {STORAGE} down*4 ok"));
    let d = describe(&mut s);
    assert_eq!((d.layer, d.title.as_str()), (Layer::Dialog, "Format the card?"));
    assert!(
        d.summary.as_str().contains("All music and files on the card are erased"),
        "{}",
        d.summary.as_str()
    );
    run(&mut s, "ok wait:50");
    assert!(root_names(&mut s).iter().any(|n| n == "Music"));
    assert!(!s.sys.log.contains(&orpheus_sim::apps::Logged::Storage(StorageRequest::Format)));
    // Format.
    run(&mut s, "ok next ok wait:50");
    let o = s.sys.storage_outcome();
    assert_eq!(
        o,
        StorageOutcome { req: Some(StorageRequest::Format), ok: true, ..StorageOutcome::default() }
    );
    assert_eq!(s.sys.card(), CardState::Ready);
    assert!(root_names(&mut s).is_empty(), "{:?}", root_names(&mut s));
    // The simulated card is 16 MiB: too small for FAT32, so exFAT.
    assert_eq!(s.sys.media.volume().kind(), FsKind::ExFat);
    let ci = s.sys.card_info().unwrap();
    assert_eq!(ci.fs, orpheus_apps::services::FsKind::ExFat);
    assert!(ci.free_mib >= 14, "{ci:?}");
    // Settings are in the player, not on the card.
    assert_eq!(s.sys.settings.get(orpheus_apps::settings::id::BRIGHTNESS), 9);
    // The card works: a file written and read back, Files shows it empty.
    let v = s.sys.media.volume();
    block_on(v.create_dir("/Music")).unwrap();
    assert!(block_on(v.exists("/Music")).unwrap());
    run(&mut s, "back back back wait:50 goto:Files wait:100");
    assert_eq!(s.sys.listing().count, 1);
}

/// The card work refuses a second request while one runs (the firmware's
/// is asynchronous; the simulator's ends at once, so this asks while a
/// `busy` outcome is set by hand).
#[test]
fn busy_card_work_is_not_started_twice() {
    let mut s = sim(STORAGE);
    s.sys.storage_out =
        StorageOutcome { req: Some(StorageRequest::CheckCard), busy: true, ..StorageOutcome::default() };
    let n = s.sys.log.len();
    run(&mut s, "down*3 ok wait:50");
    assert_eq!(s.sys.log.len(), n, "not asked again");
}

/// Regression (DOC-2 finding, 2026-11-27): System › Diagnostics opened a
/// "Not available yet" notice. It is a read-only page.
#[test]
fn regress_diagnostics_was_not_available() {
    let mut s = sim("goto:Settings wait:50 down*11 ok wait:50");
    let d = describe(&mut s);
    assert_eq!(d.title.as_str(), "System");
    run(&mut s, "down*2 ok wait:50");
    assert_eq!(s.screen(), "Info");
    let d = describe(&mut s);
    assert_eq!(d.title.as_str(), "Diagnostics");
    let f = d.focus.expect("rows");
    assert_eq!((f.label.as_str(), f.count), ("Uptime", 5));
    // Read-only: OK changes nothing, Back leaves.
    let before = s.sys.log.len();
    run(&mut s, "ok down ok back wait:50");
    assert_eq!(s.sys.log.len(), before);
    assert_eq!(s.screen(), "SettingsGroup");
}
