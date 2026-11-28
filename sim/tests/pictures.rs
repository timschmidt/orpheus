//! UI-PIC (2026-11-28, owner's request): Pictures opens at the card's
//! pictures folder, the first of `/Pictures`, `/Photos`, `/DCIM` the card
//! has (names compared without case, as FAT and exFAT do, so `/PICTURES`
//! is `/Pictures`), else at the root; Back from it is Home. The simulator
//! looks the folders up as the firmware does (`Volume::stat` over each of
//! `orpheus_apps::browse::PICTURE_DIRS` in order), on real FAT32 and exFAT
//! images.

use std::sync::Arc;

use orpheus_apps::browse::{DirId, PICTURE_DIRS};
use orpheus_apps::services::{BrowseService, PictureService};
use orpheus_fs::host::{Base, RamDisk, block_on};
use orpheus_fs::{CacheSlot, FsKind, MountOptions, OpenMode, Volume};
use orpheus_sim::apps::HostServices;
use orpheus_sim::fallback::HexFont;
use orpheus_sim::media::Media;
use orpheus_sim::{AppSim, Sim};

/// A card with these folders (each with one picture named after it) and
/// these files, and a file `/a.png`.
fn card_with(kind: FsKind, dirs: &[&str], files: &[&str]) -> Media {
    let mut disk = RamDisk::new(80 * 2048);
    let mut buf = [[0u8; 512]; 32];
    block_on(orpheus_fs::format::format(&mut disk, Some(kind), 0x1234_5678, &mut buf)).expect("format");
    let mut cache = vec![CacheSlot::EMPTY; 16];
    let mut vol = block_on(Volume::mount(disk, &mut cache, MountOptions::default())).expect("mount");
    let put = |vol: &mut Volume<'_, RamDisk>, path: &str| {
        let f = block_on(vol.open(path, OpenMode::CreateNew)).unwrap_or_else(|e| panic!("{path}: {e:?}"));
        block_on(vol.close(f)).expect("close");
    };
    for d in dirs {
        block_on(vol.create_dir(d)).unwrap_or_else(|e| panic!("{d}: {e:?}"));
        put(&mut vol, &format!("{d}/{}.png", d.rsplit('/').next().unwrap_or("")));
    }
    for f in files.iter().chain(&["/a.png"]) {
        put(&mut vol, f);
    }
    block_on(vol.flush()).expect("flush");
    let (disk, _) = vol.into_parts();
    Media::with_card(Base::Shared(Arc::from(disk.data.into_boxed_slice()))).expect("the card mounts")
}

fn card(kind: FsKind, dirs: &[&str]) -> Media {
    card_with(kind, dirs, &[])
}

fn sim(media: Media) -> AppSim {
    Sim::with_sys(HostServices::with_media(media), Box::new(HexFont::subset()))
}

fn run(s: &mut AppSim, script: &str) {
    s.run_script(script).unwrap_or_else(|e| panic!("{script}: {e:#}"));
}

/// The folder Pictures opens at on a card with `dirs`, by its name
/// (`""`: the root), and the names it lists.
fn opens_at(kind: FsKind, dirs: &[&str]) -> (String, Vec<String>) {
    let mut s = sim(card(kind, dirs));
    run(&mut s, "next*5 ok wait:100 expect-screen:Pictures");
    let l = s.sys.listing();
    let names = (0..l.count).filter_map(|i| s.sys.entry(i).map(|e| e.name.to_string())).collect();
    let name = if l.dir == DirId::ROOT { String::new() } else { s.sys.dir_name(l.dir).to_string() };
    (name, names)
}

#[test]
fn pictures_opens_at_the_first_pictures_folder() {
    assert_eq!(PICTURE_DIRS, ["/Pictures", "/Photos", "/DCIM"], "the order of 05 §7.6");
    for kind in [FsKind::Fat32, FsKind::ExFat] {
        let at = |dirs: &[&str]| opens_at(kind, dirs);
        // The first of the list wins, whatever the order on the card.
        assert_eq!(at(&["/DCIM", "/Photos", "/Pictures"]).0, "Pictures", "{kind:?}");
        assert_eq!(at(&["/DCIM", "/Photos"]).0, "Photos", "{kind:?}");
        assert_eq!(at(&["/Music", "/DCIM"]), ("DCIM".into(), vec!["DCIM.png".into()]), "{kind:?}");
        // Names compared without case; listed under the card's own name.
        assert_eq!(at(&["/PICTURES"]), ("PICTURES".into(), vec!["PICTURES.png".into()]), "{kind:?}");
        assert_eq!(at(&["/photos", "/dcim"]).0, "photos", "{kind:?}");
        // None of them: the root (folders and pictures).
        let (name, rows) = at(&["/Music", "/Pics"]);
        assert_eq!(name, "", "{kind:?}");
        assert_eq!(rows, ["Music", "Pics", "a.png"], "{kind:?}");
    }
}

/// A file named like a folder is not the folder: the next one counts.
#[test]
fn a_file_named_pictures_is_not_the_folder() {
    let mut m = card_with(FsKind::Fat32, &["/Photos"], &["/Pictures"]);
    assert_eq!(m.find_root_dir("/Pictures"), None);
    let mut s = HostServices::with_media(m);
    let d = s.pictures_dir();
    assert_ne!(d, DirId::ROOT);
    assert_eq!(s.dir_name(d), "Photos");
}

/// Back from the pictures folder is Home, not the card's root; a folder
/// inside it goes back to it. Coming back from Home opens it again.
#[test]
fn back_from_the_pictures_folder_is_home() {
    let mut s = sim(card(FsKind::Fat32, &["/Pictures", "/Pictures/Trip"]));
    run(&mut s, "next*5 ok wait:100 expect-screen:Pictures");
    let top = s.sys.listing().dir;
    assert_eq!(s.sys.entry(0).map(|e| e.name.to_string()).as_deref(), Some("Trip"));
    run(&mut s, "ok wait:100 expect-screen:Pictures");
    assert_ne!(s.sys.listing().dir, top, "into Trip");
    run(&mut s, "back wait:100 expect-screen:Pictures");
    assert_eq!(s.sys.listing().dir, top);
    run(&mut s, "back expect-screen:Home ok wait:100 expect-screen:Pictures");
    assert_eq!(s.sys.listing().dir, top);
}
