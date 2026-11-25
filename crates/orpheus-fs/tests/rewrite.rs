//! `Volume::rewrite` (a file's content replaced inside its own clusters),
//! the next-free hint left by a free, and `Volume::find_free_step` (the
//! allocation search in bounded stretches). The player's games saves,
//! 2026-11-25: a truncating open freed the save's cluster and the write
//! allocated one, on the owner's exFAT card a search of 452 bitmap sectors
//! from the start (exFAT keeps no next-free hint): one 41 ms stretch while
//! music played. Every image ends fsck-clean on FAT16, FAT32 and exFAT.

mod common;

use common::*;
use orpheus_fs::host::{FaultyDisk, RamDisk, block_on};
use orpheus_fs::{BlockDevice, CacheSlot, MountOptions, OpenMode, Volume, WalkCache};

const FMTS: [Fmt; 3] = [Fmt::Fat(16, 1, 16), Fmt::Fat(32, 1, 64), Fmt::ExFat(131_072, 192)];
const SAVE: &str = "/.orpheus/games/a.sav";

fn tools(fmt: Fmt) -> bool {
    let ok = have(if fmt.is_exfat() { "mkfs.exfat" } else { "mkfs.fat" });
    if !ok {
        eprintln!("{}: mkfs missing, skipped", fmt.label());
    }
    ok
}

fn vol_on<D: BlockDevice>(disk: D, c: &mut [CacheSlot]) -> Volume<'_, D>
where
    D::Error: core::fmt::Debug,
{
    let mut vol = block_on(Volume::mount(disk, c, MountOptions::default())).expect("mount");
    vol.set_walk_cache(Box::leak(Box::new(WalkCache::EMPTY)));
    vol.set_time(orpheus_fs::DateTime::from_fat(0x5B79, 0x6000, 0));
    vol
}

fn read<D: BlockDevice>(vol: &mut Volume<'_, D>, path: &str) -> Vec<u8>
where
    D::Error: core::fmt::Debug,
{
    block_on(async {
        let mut f = vol.open(path, OpenMode::Read).await.unwrap();
        let mut out = vec![0u8; f.size() as usize];
        vol.read_exact_or_eof(&mut f, &mut out).await.unwrap();
        out
    })
}

fn first_cluster<D: BlockDevice>(vol: &mut Volume<'_, D>, path: &str) -> u32
where
    D::Error: core::fmt::Debug,
{
    block_on(vol.open(path, OpenMode::Read)).unwrap().first_cluster()
}

/// Truncate and write `data` as `path`, closed.
fn put<D: BlockDevice>(vol: &mut Volume<'_, D>, path: &str, data: &[u8])
where
    D::Error: core::fmt::Debug,
{
    block_on(async {
        let mut f = vol.open(path, OpenMode::Truncate).await.unwrap();
        vol.write(&mut f, data).await.unwrap();
        vol.close(f).await.unwrap();
    });
}

/// `rewrite` then close; whether it was done in place, and the sector
/// reads and writes `rewrite` itself made.
fn rewrite(vol: &mut Volume<'_, RamDisk>, path: &str, data: &[u8]) -> (bool, u64, u64) {
    block_on(async {
        let mut f = vol.open(path, OpenMode::ReadWrite).await.unwrap();
        let (r0, w0) = (vol.device().reads, vol.device().writes);
        let done = vol.rewrite(&mut f, data).await.unwrap();
        let io = (vol.device().reads - r0, vol.device().writes - w0);
        if done {
            vol.close(f).await.unwrap();
        }
        (done, io.0, io.1)
    })
}

fn bytes(len: usize, seed: usize) -> Vec<u8> {
    (0..len).map(|k| (k * 7 + seed * 13 + 1) as u8).collect()
}

fn fresh(fmt: Fmt, name: &str) -> (std::path::PathBuf, RamDisk) {
    let dir = scratch(&format!("rewrite-{name}-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let disk = RamDisk::load(&img).unwrap();
    (img, disk)
}

fn games_dirs<D: BlockDevice>(vol: &mut Volume<'_, D>)
where
    D::Error: core::fmt::Debug,
{
    block_on(vol.create_dir("/.orpheus")).unwrap();
    block_on(vol.create_dir("/.orpheus/games")).unwrap();
}

fn finish(vol: Volume<'_, RamDisk>, img: &std::path::Path, fmt: Fmt) {
    let (disk, r) = block_on(vol.unmount());
    r.unwrap();
    disk.save(img).unwrap();
    assert_fsck(img, fmt.is_exfat());
}

/// In place: the same cluster, no data sector read, nothing allocated;
/// a size that needs another cluster count is refused untouched and
/// written the usual way.
#[test]
fn rewrite_in_place() {
    for fmt in FMTS {
        if !tools(fmt) {
            continue;
        }
        let (img, disk) = fresh(fmt, "in-place");
        let mut c = cache();
        let mut vol = vol_on(disk, &mut c);
        games_dirs(&mut vol);
        put(&mut vol, SAVE, &bytes(300, 0));
        let first = first_cluster(&mut vol, SAVE);
        for (i, len) in [487usize, 12, 512, 300, 511].into_iter().enumerate() {
            let data = bytes(len, i + 1);
            let (done, r, w) = rewrite(&mut vol, SAVE, &data);
            assert!(done, "{}: {len} B not in place", fmt.label());
            // At most the boot sector (exFAT's dirty flag) read, no data
            // sector; the dirty flag written (FAT: FAT[1] in both copies)
            // and a whole data sector at once (512 B).
            assert!(
                r <= 1 && w <= 2 + u64::from(len == 512),
                "{}: rewrite of {len} B: {r} reads, {w} writes",
                fmt.label()
            );
            assert_eq!(read(&mut vol, SAVE), data, "{}: {len} B", fmt.label());
            assert_eq!(first_cluster(&mut vol, SAVE), first, "{}: moved", fmt.label());
        }
        // 1,200 B: three 512 B clusters (FAT here), still one on exFAT.
        let big = bytes(1200, 9);
        let before = read(&mut vol, SAVE);
        let (done, ..) = rewrite(&mut vol, SAVE, &big);
        assert_eq!(done, fmt.is_exfat(), "{}", fmt.label());
        if !done {
            assert_eq!(read(&mut vol, SAVE), before, "{}: refused, yet changed", fmt.label());
            put(&mut vol, SAVE, &big);
        }
        assert_eq!(read(&mut vol, SAVE), big);
        // Back to one cluster: refused on FAT (three held), done on exFAT.
        let small = bytes(100, 10);
        let (done, ..) = rewrite(&mut vol, SAVE, &small);
        assert_eq!(done, fmt.is_exfat(), "{}", fmt.label());
        if !done {
            put(&mut vol, SAVE, &small);
        }
        assert_eq!(read(&mut vol, SAVE), small);
        // An empty file has no cluster to write into.
        put(&mut vol, "/.orpheus/games/empty.sav", &[]);
        assert!(!rewrite(&mut vol, "/.orpheus/games/empty.sav", b"x").0);
        finish(vol, &img, fmt);
    }
}

/// A freed chain is where the next allocation goes, without a search: a
/// file truncated and written again keeps its cluster; a file made after
/// a removal takes the removed one's.
#[test]
fn freed_clusters_taken_first() {
    for fmt in FMTS {
        if !tools(fmt) {
            continue;
        }
        let (img, disk) = fresh(fmt, "freed");
        let mut c = cache();
        let mut vol = vol_on(disk, &mut c);
        games_dirs(&mut vol);
        for k in 0..4 {
            put(&mut vol, &format!("/.orpheus/games/{k}.sav"), &bytes(400, k));
        }
        let a = first_cluster(&mut vol, "/.orpheus/games/1.sav");
        put(&mut vol, "/.orpheus/games/1.sav", &bytes(450, 7));
        assert_eq!(first_cluster(&mut vol, "/.orpheus/games/1.sav"), a, "{}: truncated", fmt.label());
        let b = first_cluster(&mut vol, "/.orpheus/games/2.sav");
        block_on(vol.remove("/.orpheus/games/2.sav")).unwrap();
        block_on(vol.flush()).unwrap();
        let w0 = vol.device().reads;
        put(&mut vol, "/.orpheus/games/new.sav", &bytes(300, 8));
        assert_eq!(first_cluster(&mut vol, "/.orpheus/games/new.sav"), b, "{}: removed", fmt.label());
        assert!(vol.device().reads - w0 <= 4, "{}: {} reads", fmt.label(), vol.device().reads - w0);
        for k in [0, 1, 3] {
            assert_eq!(read(&mut vol, &format!("/.orpheus/games/{k}.sav")).len(), [400, 450, 0, 400][k]);
        }
        finish(vol, &img, fmt);
    }
}

/// After a mount with no usable hint (exFAT always, FAT32 with FSINFO's
/// next-free unknown), `find_free_step` searches in stretches of at most
/// the sectors asked for, and the allocation after it reads nothing more.
#[test]
fn find_free_in_steps() {
    for fmt in [Fmt::Fat(32, 1, 64), Fmt::ExFat(4096, 192)] {
        if !tools(fmt) {
            continue;
        }
        let (img, disk) = fresh(fmt, "find-free");
        let mut c = cache();
        let mut vol = vol_on(disk, &mut c);
        games_dirs(&mut vol);
        // Three quarters of the clusters in use from the start.
        let fill = u64::from(vol.cluster_count()) * 3 / 4 * u64::from(vol.cluster_size());
        block_on(async {
            let mut f = vol.open("/fill.bin", OpenMode::Create).await.unwrap();
            vol.set_len(&mut f, fill).await.unwrap();
            vol.close(f).await.unwrap();
        });
        let (mut disk, r) = block_on(vol.unmount());
        r.unwrap();
        if !fmt.is_exfat() {
            let bs = &disk.data[..512];
            let fsi = usize::from(u16::from_le_bytes([bs[48], bs[49]]));
            disk.data[fsi * 512 + 492..fsi * 512 + 496].fill(0xFF);
        }
        let mut c = cache();
        let mut vol = vol_on(disk, &mut c);
        block_on(vol.warm_walk("/.orpheus/games")).unwrap();
        let mut steps = 0;
        loop {
            let r0 = vol.device().reads;
            let found = block_on(vol.find_free_step(4)).unwrap();
            let r = vol.device().reads - r0;
            assert!(r <= 4, "{}: a step read {r} sectors", fmt.label());
            steps += 1;
            assert!(steps < 10_000, "{}: no end", fmt.label());
            if found {
                break;
            }
        }
        assert!(steps > 2, "{}: {steps} steps (the fill spans more sectors)", fmt.label());
        let r0 = vol.device().reads;
        block_on(async {
            let mut f = vol.open(SAVE, OpenMode::Truncate).await.unwrap();
            vol.write(&mut f, &bytes(300, 1)).await.unwrap();
            vol.write_entry(&mut f).await.unwrap();
        });
        let r = vol.device().reads - r0;
        assert!(r <= 2, "{}: the allocation after the search read {r} sectors", fmt.label());
        eprintln!("{}: {steps} steps of at most 4 sectors", fmt.label());
        finish(vol, &img, fmt);
    }
}

/// Power cut during a rewrite (and its close), after every write count:
/// the file is the old one, the new one, or the new bytes under the old
/// length; the volume needs no repair (fsck finds at most the dirty flag).
#[test]
fn rewrite_power_cut() {
    for fmt in FMTS {
        if !tools(fmt) {
            continue;
        }
        let (img, disk) = fresh(fmt, "cut");
        let (old, new) = (bytes(300, 1), bytes(480, 2));
        let base = {
            let mut c = cache();
            let mut vol = vol_on(disk, &mut c);
            games_dirs(&mut vol);
            put(&mut vol, SAVE, &old);
            let (d, r) = block_on(vol.unmount());
            r.unwrap();
            d
        };
        let run = |cut: Option<u64>| {
            let mut c = cache();
            let mut vol = vol_on(FaultyDisk::new(base.clone(), cut), &mut c);
            let r = block_on(async {
                let mut f = vol.open(SAVE, OpenMode::ReadWrite).await?;
                assert!(vol.rewrite(&mut f, &new).await?);
                vol.close(f).await
            });
            (r, vol.into_device())
        };
        let (r, d) = run(None);
        r.unwrap();
        let total = d.writes_done;
        let mut seen = std::collections::BTreeSet::new();
        for n in 0..=total {
            let (_, d) = run(Some(n));
            let mut c = cache();
            let mut vol = vol_on(d.inner, &mut c);
            let got = read(&mut vol, SAVE);
            let what = if got == old {
                "old"
            } else if got == new {
                "new"
            } else {
                assert_eq!(got.len(), old.len(), "{}: cut at {n}: size", fmt.label());
                assert_eq!(got, new[..old.len()], "{}: cut at {n}: content", fmt.label());
                "new bytes, old length"
            };
            seen.insert(what);
            let disk = vol.into_device();
            disk.save(&img).unwrap();
            let (ok, out) = fsck(&img, fmt.is_exfat());
            if !ok {
                for l in out.lines() {
                    let t = l.trim();
                    let fine = t.is_empty()
                        || t.contains("version")
                        || t.contains("fs.img")
                        || t.starts_with("fsck.fat")
                        || t.contains("irty bit")
                        // The dirty flag in FAT[1] written to one copy.
                        || t.contains("FATs differ but appear to be intact")
                        || t.contains("Using first FAT")
                        || t.contains("Leaving filesystem unchanged");
                    assert!(fine, "{}: cut at {n}: fsck: '{t}'\n{out}", fmt.label());
                }
            }
        }
        eprintln!("{}: {total} writes; after a cut: {seen:?}", fmt.label());
        assert!(seen.contains("old") && seen.contains("new"), "{}: {seen:?}", fmt.label());
    }
}
