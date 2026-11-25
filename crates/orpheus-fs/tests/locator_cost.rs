//! What one `open_locator` costs: sector reads (cold cache, as after the
//! playback loop's file reads went through the 8 slots, and warm) and the
//! slot steps of the seek, by entry position, folder size, name length and
//! directory layout (contiguous, or fragmented so the walk follows the
//! FAT), on FAT32 and exFAT (AU-PERF, 2026-11-25: Up next titles took
//! 70–120 ms each on the player).
//!
//! `measure_open_locator` prints the table (`--ignored --nocapture`);
//! the `regress_` tests bound the cost so it cannot creep back.

mod common;

use common::*;
use orpheus_fs::host::{FileDisk, HostError, block_on};
use orpheus_fs::{BlockDevice, CacheSlot, DirEntry, Locator, MountOptions, OpenMode, Sector, Volume};

/// A device that counts sector reads.
struct Counting {
    d: FileDisk,
    reads: u64,
}

impl BlockDevice for Counting {
    type Error = HostError;
    async fn read(&mut self, lba: u32, buf: &mut [Sector]) -> Result<(), HostError> {
        self.reads += buf.len() as u64;
        self.d.read(lba, buf).await
    }
    async fn write(&mut self, lba: u32, buf: &[Sector]) -> Result<(), HostError> {
        self.d.write(lba, buf).await
    }
    fn sector_count(&self) -> u32 {
        self.d.sector_count()
    }
}

fn long_name(i: usize, long: bool) -> String {
    if long {
        // 60 characters: 5 LFN slots (FAT), 4 name entries (exFAT).
        format!("{:03} - Some Artist Name - A Rather Long Track Title {i:05}.flac", i % 997)
    } else {
        format!("T{i:05}.MP3")
    }
}

/// Make `/big` with `n` files (and, `fragment`, `/other` grown in step and
/// one data cluster after every 8 files, so `/big`'s clusters are spread
/// over the volume and its chain is followed through the FAT).
fn make(fmt: Fmt, n: usize, long: bool, fragment: bool) -> (std::path::PathBuf, Vec<Locator>) {
    let tag = format!("loccost-{}-{n}-{long}-{fragment}", fmt.label());
    let dir = scratch(&tag);
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    block_on(async {
        vol.create_dir("/big").await.unwrap();
        if fragment {
            vol.create_dir("/other").await.unwrap();
        }
        let blob = vec![0x5Au8; 4096];
        for i in 0..n {
            let p = format!("/big/{}", long_name(i, long));
            let f = vol.open(&p, OpenMode::CreateNew).await.unwrap();
            vol.close(f).await.unwrap();
            if fragment {
                let p = format!("/other/{}", long_name(i, long));
                let mut f = vol.open(&p, OpenMode::CreateNew).await.unwrap();
                if i % 8 == 0 {
                    vol.write(&mut f, &blob).await.unwrap();
                }
                vol.close(f).await.unwrap();
            }
        }
    });
    unmount(vol);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    let mut locs = Vec::new();
    block_on(async {
        let mut cur = vol.open_dir("/big").await.unwrap();
        let mut e = DirEntry::new();
        while vol.read_dir(&mut cur, &mut e).await.unwrap() {
            locs.push(e.locator());
        }
    });
    assert_eq!(locs.len(), n);
    (img, locs)
}

/// (cold reads, slot steps, warm reads, host µs) of `open_locator` for
/// `loc`.
fn cost(vol: &mut Volume<'_, Counting>, loc: Locator) -> (u64, u32, u64, f64) {
    let mut e = DirEntry::new();
    vol.invalidate_cached(0, u32::MAX);
    let r0 = vol.device().reads;
    let s0 = vol.dir_steps();
    block_on(vol.open_locator(loc, &mut e)).unwrap();
    let cold = vol.device().reads - r0;
    let steps = vol.dir_steps() - s0;
    let r0 = vol.device().reads;
    block_on(vol.open_locator(loc, &mut e)).unwrap();
    let warm = vol.device().reads - r0;
    let t = std::time::Instant::now();
    let reps = 20;
    for _ in 0..reps {
        block_on(vol.open_locator(loc, &mut e)).unwrap();
    }
    (cold, steps, warm, t.elapsed().as_secs_f64() * 1e6 / f64::from(reps))
}

fn open<'c>(img: &std::path::Path, c: &'c mut [CacheSlot]) -> Volume<'c, Counting> {
    let d = Counting { d: FileDisk::open(img).unwrap(), reads: 0 };
    block_on(Volume::mount(d, c, MountOptions::default())).unwrap()
}

fn formats() -> Vec<Fmt> {
    let mut v = vec![Fmt::Fat(32, 8, 300)];
    if have("mkfs.exfat") {
        v.push(Fmt::ExFat(32768, 256));
        v.push(Fmt::ExFat(131_072, 512));
    }
    v
}

#[test]
#[ignore = "measurement: prints the table (--ignored --nocapture)"]
fn measure_open_locator() {
    println!("format              files long  frag   pos   slot  steps  cold warm  host-us");
    for fmt in formats() {
        for (n, long, frag) in
            [(200, true, false), (2000, false, false), (2000, true, false), (2000, true, true)]
        {
            let (img, locs) = make(fmt, n, long, frag);
            let mut c = vec![CacheSlot::EMPTY; 8];
            let mut vol = open(&img, &mut c);
            for k in [0, n / 4, n / 2, 3 * n / 4, n - 1] {
                let (cold, steps, warm, us) = cost(&mut vol, locs[k]);
                println!(
                    "{:<19} {n:>5} {:>5} {:>5} {k:>5} {:>6} {steps:>6} {cold:>5} {warm:>4} {us:>8.1}",
                    fmt.label(),
                    long,
                    frag,
                    locs[k].index & !(1 << 31),
                );
            }
        }
    }
}

/// Regression (AU-PERF, 2026-11-25): `open_locator` stepped slot by slot to
/// its entry (3,507 steps for one entry measured here, 4,095 possible in a
/// 128 KiB cluster) and an Up next title took 70–120 ms on the player. Now
/// a seek takes one step per cluster crossed: every entry of a large folder
/// opens within a few steps and at most 3 sector reads from a cold cache.
#[test]
fn regress_open_locator_steps_per_cluster() {
    let mut fmts = vec![Fmt::Fat(32, 8, 300)];
    if have("mkfs.exfat") {
        fmts.push(Fmt::ExFat(131_072, 512));
    }
    for fmt in fmts {
        let (img, locs) = make(fmt, 600, true, false);
        let mut c = vec![CacheSlot::EMPTY; 8];
        let mut vol = open(&img, &mut c);
        let cluster_slots = vol.cluster_size() / 32;
        let mut worst = (0, 0);
        for (i, &l) in locs.iter().enumerate() {
            let (cold, steps, _, _) = cost(&mut vol, l);
            let index = l.index & !(1 << 31);
            // Clusters crossed, plus the set's own slots (at most 21).
            let bound = index / cluster_slots + 22;
            assert!(steps <= bound, "{}: entry {i} (slot {index}): {steps} steps > {bound}", fmt.label());
            assert!(cold <= 3, "{}: entry {i}: {cold} sector reads", fmt.label());
            worst = (worst.0.max(steps), worst.1.max(cold));
            let mut e = DirEntry::new();
            block_on(vol.open_locator(l, &mut e)).unwrap();
            assert_eq!(e.name(), long_name(i, true));
        }
        println!("{}: worst {} steps, {} reads", fmt.label(), worst.0, worst.1);
    }
}

/// Regression: a locator whose entry was deleted, followed by a long run
/// of deleted entries, fails at once (`read_dir` scanned the run for the
/// next set, one sector read per 16 slots, only to fail).
#[test]
fn regress_stale_locator_fails_fast() {
    let mut fmts = vec![Fmt::Fat(32, 8, 300)];
    if have("mkfs.exfat") {
        fmts.push(Fmt::ExFat(32768, 256));
    }
    for fmt in fmts {
        let (img, locs) = make(fmt, 300, true, false);
        {
            let mut c = cache();
            let mut vol = mount(&img, &mut c);
            block_on(async {
                for i in 0..299 {
                    vol.remove(&format!("/big/{}", long_name(i, true))).await.unwrap();
                }
            });
            unmount(vol);
        }
        let mut c = vec![CacheSlot::EMPTY; 8];
        let mut vol = open(&img, &mut c);
        let mut e = DirEntry::new();
        let cluster_slots = vol.cluster_size() / 32;
        for &l in &locs[..299] {
            vol.invalidate_cached(0, u32::MAX);
            let (r0, s0) = (vol.device().reads, vol.dir_steps());
            assert_eq!(block_on(vol.open_locator(l, &mut e)), Err(orpheus_fs::Error::NotFound));
            let (reads, steps) = (vol.device().reads - r0, vol.dir_steps() - s0);
            // The seek's cluster steps only; the FAT sector and the slot's.
            let bound = (l.index & !(1 << 31)) / cluster_slots + 1;
            assert!(
                reads <= 2 && steps <= bound,
                "{}: stale locator: {reads} reads, {steps} steps (bound {bound})",
                fmt.label()
            );
        }
        // The one left still opens.
        block_on(vol.open_locator(locs[299], &mut e)).unwrap();
        assert_eq!(e.name(), long_name(299, true));
    }
}

/// `seek_dir` to any index lands where stepping through the directory
/// does, on every format of the matrix, in the root (the fixed FAT12/16
/// root too) and in a folder larger than a cluster; forward from a cursor
/// already inside, back again, and past the end.
#[test]
fn seek_dir_matches_sequential_listing() {
    for fmt in matrix() {
        if fmt.is_exfat() && !have("mkfs.exfat") {
            continue;
        }
        let dir = scratch(&format!("seekseq-{}", fmt.label()));
        let img = dir.join("fs.img");
        make_image(&img, fmt);
        let mut c = cache();
        let mut vol = mount(&img, &mut c);
        block_on(async {
            vol.create_dir("/d").await.unwrap();
            for i in 0..150 {
                let p = format!("/d/{}", long_name(i, i % 3 != 0));
                let f = vol.open(&p, OpenMode::CreateNew).await.unwrap();
                vol.close(f).await.unwrap();
            }
            for i in 0..40 {
                let p = format!("/{}", long_name(i, i % 2 == 0));
                let f = vol.open(&p, OpenMode::CreateNew).await.unwrap();
                vol.close(f).await.unwrap();
            }
            for path in ["/", "/d"] {
                let start = vol.open_dir(path).await.unwrap();
                let mut cur = start;
                let mut e = DirEntry::new();
                let mut seen = Vec::new();
                while vol.read_dir(&mut cur, &mut e).await.unwrap() {
                    seen.push((e.index(), e.name().to_string(), e.locator()));
                }
                assert!(seen.len() >= 40, "{path}");
                let mut c2 = start;
                for (k, (index, name, loc)) in seen.iter().enumerate().rev().chain(seen.iter().enumerate()) {
                    vol.seek_dir(&mut c2, *index).await.unwrap();
                    assert_eq!(c2.index(), *index, "{} {path} #{k}", fmt.label());
                    let mut c3 = c2;
                    assert!(vol.read_dir(&mut c3, &mut e).await.unwrap());
                    assert_eq!(e.name(), name, "{} {path} #{k}", fmt.label());
                    vol.open_locator(*loc, &mut e).await.unwrap();
                    assert_eq!(e.name(), name);
                }
                let last = seen.last().unwrap().0;
                let mut c4 = start;
                vol.seek_dir(&mut c4, last + 200_000).await.unwrap();
                assert!(!vol.read_dir(&mut c4, &mut e).await.unwrap());
            }
        });
        unmount(vol);
    }
}
