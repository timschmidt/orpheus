//! `Volume::walk_step`, the path walk in stretches (the firmware's index
//! lookup while music plays, AU-PERF 2026-11-25), on every format: it
//! finds what `open_located` finds, at any stretch length, says which
//! component is missing, and never searches more than its stretch; and
//! the walk cache survives file removals (every index build removes its
//! scratch files, and the next listing searched the card's root again).

mod common;

use common::*;
use orpheus_fs::host::block_on;
use orpheus_fs::{Error, Locator, OpenMode, PathWalk, Volume};

/// A walk in stretches of `sets` sets: the result and the stretches.
fn walk<D: orpheus_fs::BlockDevice>(
    vol: &mut Volume<'_, D>,
    path: &str,
    sets: u32,
) -> (Result<Locator, orpheus_fs::Error<D::Error>>, u32) {
    let mut w = PathWalk::START;
    let mut n = 0;
    loop {
        n += 1;
        match block_on(vol.walk_step(&mut w, path, sets)) {
            Ok(Some(loc)) => {
                assert!(w.found());
                // Asked again: the same, no I/O.
                let steps = vol.dir_steps();
                assert_eq!(block_on(vol.walk_step(&mut w, path, sets)).ok(), Some(Some(loc)));
                assert_eq!(vol.dir_steps(), steps);
                return (Ok(loc), n);
            }
            Ok(None) => assert!(n < 100_000, "{path}: no end"),
            Err(e) => return (Err(e), n),
        }
    }
}

fn scenario(fmt: Fmt) {
    let dir = scratch(&format!("walk-step-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    let mut rng = Rng(0x57E9_0001);
    // A root of long-named folders (more than a FAT12/16 root holds is
    // not needed), `/.orpheus/index/X` after them.
    let roots = if matches!(fmt, Fmt::Fat(12 | 16, ..)) { 40 } else { 120 };
    for i in 0..roots {
        block_on(vol.create_dir(&format!("/{i:03} Some Artist Name With A Longer Title"))).unwrap();
    }
    for d in ["/.orpheus", "/.orpheus/index", "/.orpheus/index/3"] {
        block_on(vol.create_dir(d)).unwrap();
    }
    for i in 0..30u32 {
        let p = format!("/.orpheus/index/3/{:08x}-n0.idx", i * 16 + 3);
        write_all(&mut vol, &p, &i.to_le_bytes(), &mut rng).unwrap();
    }
    unmount(vol);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    for (i, path) in [(0u32, "/.orpheus/index/3/00000003-n0.idx"), (29, "/.orpheus/index/3/000001d3-n0.idx")]
    {
        let (_, want) = block_on(vol.open_located(path)).unwrap();
        for sets in [1, 2, 16, 1000] {
            // From the root (another cache each time).
            vol.set_walk_cache(Box::leak(Box::new(orpheus_fs::WalkCache::EMPTY)));
            let (got, stretches) = walk(&mut vol, path, sets);
            assert_eq!(got, Ok(want), "{} {path} sets {sets}", fmt.label());
            if sets == 1 {
                assert!(stretches > roots, "{stretches}: one set per stretch");
            }
            let f = block_on(vol.open_at(want, path.rsplit('/').next().unwrap())).unwrap();
            let mut b = [0u8; 4];
            block_on(vol.read_exact_or_eof(&mut { f }, &mut b)).unwrap();
            assert_eq!(b, i.to_le_bytes());
            // The directories passed are cached: the next walk starts at
            // the shard (one stretch finds a file among 30).
            let (got, stretches) = walk(&mut vol, path, 100);
            assert_eq!((got, stretches), (Ok(want), 1));
        }
    }
    // Case as a walk compares it.
    let (got, _) = walk(&mut vol, "/.ORPHEUS/Index/3/00000003-N0.IDX", 4);
    assert!(got.is_ok());
    // Missing: the file, a directory on the way; a file used as one.
    let missing = ["/.orpheus/index/3/00000004-n0.idx", "/.orpheus/index/4/00000004-n0.idx", "/nothing/x"];
    for p in missing {
        for sets in [1, 7] {
            vol.set_walk_cache(Box::leak(Box::new(orpheus_fs::WalkCache::EMPTY)));
            assert_eq!(walk(&mut vol, p, sets).0, Err(Error::NotFound), "{p}");
        }
    }
    assert_eq!(walk(&mut vol, "/.orpheus/index/3/00000003-n0.idx/x", 4).0, Err(Error::NotADirectory));
    assert_eq!(walk(&mut vol, "/.orpheus/../x", 4).0, Err(Error::InvalidName));
    // A stretch looks at no more than its sets: from the root, one set is
    // a few slots (these names: 5 FAT slots, 6 exFAT entries; a volume
    // label or a free slot before them).
    vol.set_walk_cache(Box::leak(Box::new(orpheus_fs::WalkCache::EMPTY)));
    let mut w = PathWalk::START;
    let s0 = vol.dir_steps();
    assert_eq!(block_on(vol.walk_step(&mut w, "/.orpheus/index/3/00000003-n0.idx", 2)), Ok(None));
    assert!(vol.dir_steps() - s0 <= 16, "{} steps", vol.dir_steps() - s0);
    unmount(vol);
    assert_fsck(&img, fmt.is_exfat());
}

#[test]
fn walk_step_on_every_format() {
    for fmt in matrix() {
        if fmt.is_exfat() && !have("mkfs.exfat") {
            continue;
        }
        scenario(fmt);
    }
}

/// Removing or renaming a *file* keeps the walk cache: before, every
/// removal emptied it, each index build's scratch files went at its end,
/// and the next index lookup while music played searched the card's root
/// (102–334 sector reads on owner-like images, `job:index` 52–60 ms on the
/// player, 2026-11-25). A removed or renamed *directory* still empties it
/// (`walk_cache.rs`).
#[test]
fn regress_removing_a_file_keeps_the_walk_cache() {
    for fmt in [Fmt::Fat(32, 1, 64), Fmt::ExFat(32768, 128)] {
        if fmt.is_exfat() && !have("mkfs.exfat") {
            continue;
        }
        let dir = scratch(&format!("walk-keep-{}", fmt.label()));
        let img = dir.join("fs.img");
        make_image(&img, fmt);
        let mut c = cache();
        let mut vol = mount(&img, &mut c);
        let mut rng = Rng(0x57E9_0002);
        for i in 0..200 {
            block_on(vol.create_dir(&format!("/{i:03} Some Artist Name With A Longer Title"))).unwrap();
        }
        for d in ["/.orpheus", "/.orpheus/index", "/.orpheus/index/3"] {
            block_on(vol.create_dir(d)).unwrap();
        }
        write_all(&mut vol, "/.orpheus/index/3/00000003-n0.idx", b"index", &mut rng).unwrap();
        block_on(vol.warm_walk("/.orpheus/index")).unwrap();
        let cost = |vol: &mut Vol<'_>| {
            let s = vol.dir_steps();
            assert_eq!(read_all(vol, "/.orpheus/index/3/00000003-n0.idx").unwrap(), b"index");
            vol.dir_steps() - s
        };
        let warm = cost(&mut vol);
        // A build's scratch files: made, written, removed; one renamed.
        for round in 0..3 {
            for t in ["sort-a.tmp", "sort-b.tmp"] {
                write_all(&mut vol, &format!("/.orpheus/index/{t}"), &[round; 3000], &mut rng).unwrap();
            }
            block_on(vol.rename("/.orpheus/index/sort-b.tmp", "/.orpheus/index/sort-c.tmp")).unwrap();
            block_on(vol.remove("/.orpheus/index/sort-a.tmp")).unwrap();
            block_on(vol.remove("/.orpheus/index/sort-c.tmp")).unwrap();
            let after = cost(&mut vol);
            assert!(
                after <= warm + 8,
                "{} round {round}: {after} steps after a file removal, {warm} warm",
                fmt.label()
            );
        }
        // A directory removed: emptied (the root is searched again), and
        // nothing stale is served.
        block_on(vol.create_dir("/.orpheus/index/tmpdir")).unwrap();
        block_on(vol.remove("/.orpheus/index/tmpdir")).unwrap();
        let cold = cost(&mut vol);
        assert!(cold > warm + 200, "{cold} vs {warm}: the root searched after a directory went");
        let f = block_on(vol.open("/.orpheus/index/tmpdir", OpenMode::Read));
        assert_eq!(f.err(), Some(Error::NotFound));
        unmount(vol);
        assert_fsck(&img, fmt.is_exfat());
    }
}
