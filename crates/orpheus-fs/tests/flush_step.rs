//! `Volume::write_entry` and `Volume::flush_step`: a file closed in steps
//! (the player's games saves while music plays, 2026-11-25: one save was a
//! 100 ms piece) leaves the same image as `Volume::close`, every step
//! writes at most one sector (with its FAT copies) or syncs, and the image
//! is clean (fsck) when the steps end.

mod common;

use common::*;
use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{MountOptions, OpenMode, Volume};

type RamVol<'c> = Volume<'c, RamDisk>;

/// Write `data` as `path` (truncated), closing with `close` or in steps.
/// Returns the most sectors one step wrote.
fn save(vol: &mut RamVol<'_>, path: &str, data: &[u8], steps: bool) -> u64 {
    block_on(async {
        let mut f = vol.open(path, OpenMode::Truncate).await.unwrap();
        vol.write(&mut f, data).await.unwrap();
        if !steps {
            vol.close(f).await.unwrap();
            return 0;
        }
        vol.write_entry(&mut f).await.unwrap();
        let mut most = 0;
        let mut n = 0;
        loop {
            let w0 = vol.device().writes;
            let done = vol.flush_step().await.unwrap();
            most = most.max(vol.device().writes - w0);
            n += 1;
            assert!(n < 64, "flush_step never ends");
            if done {
                break;
            }
        }
        assert_eq!(vol.dirty_sectors(), 0);
        most
    })
}

fn scenario(fmt: Fmt) {
    let dir = scratch(&format!("flush-step-{}", fmt.label()));
    let (a, b) = (dir.join("a.img"), dir.join("b.img"));
    make_image(&a, fmt);
    std::fs::copy(&a, &b).unwrap();
    for (img, steps) in [(&a, false), (&b, true)] {
        let mut c = cache();
        let disk = RamDisk::load(img).unwrap();
        let mut vol = block_on(Volume::mount(disk, &mut c, MountOptions::default())).expect("mount");
        vol.set_walk_cache(Box::leak(Box::new(orpheus_fs::WalkCache::EMPTY)));
        vol.set_time(orpheus_fs::DateTime::from_fat(0x5B79, 0x6000, 0));
        block_on(vol.create_dir("/.orpheus")).unwrap();
        block_on(vol.create_dir("/.orpheus/games")).unwrap();
        block_on(vol.flush()).unwrap();
        for (i, len) in [300usize, 512, 487, 1200, 12].into_iter().enumerate() {
            let data: Vec<u8> = (0..len).map(|k| (k * 7 + i) as u8).collect();
            let most = save(&mut vol, "/.orpheus/games/salvo.sav", &data, steps);
            if steps {
                // A FAT sector and its mirror, or the clean flag's sector.
                assert!(most <= 2, "{}: a step wrote {most} sectors", fmt.label());
            }
            let back = block_on(async {
                let mut f = vol.open("/.orpheus/games/salvo.sav", OpenMode::Read).await.unwrap();
                let mut out = vec![0u8; f.size() as usize];
                vol.read_exact_or_eof(&mut f, &mut out).await.unwrap();
                out
            });
            assert_eq!(back, data);
        }
        let (disk, r) = block_on(vol.unmount());
        r.unwrap();
        disk.save(img).unwrap();
        assert_fsck(img, fmt.is_exfat());
    }
    assert!(std::fs::read(&a).unwrap() == std::fs::read(&b).unwrap(), "{}: images differ", fmt.label());
}

#[test]
fn flush_in_steps_equals_close() {
    for fmt in [Fmt::Fat(16, 1, 16), Fmt::Fat(32, 1, 64), Fmt::ExFat(131_072, 192)] {
        if !have(if fmt.is_exfat() { "mkfs.exfat" } else { "mkfs.fat" }) {
            eprintln!("{}: mkfs missing, skipped", fmt.label());
            continue;
        }
        scenario(fmt);
    }
}
