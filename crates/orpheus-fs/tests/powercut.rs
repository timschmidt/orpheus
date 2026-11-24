//! Power-cut simulation: a workload runs on a device that stops accepting
//! writes after N sectors (multi-sector writes are torn). Afterwards the
//! volume must mount, every file must be readable, no two files may share a
//! cluster, and `fsck -n` may report at most leaked clusters / stale counts.

mod common;

use common::*;
use orpheus_fs::host::{FaultyDisk, HostError, RamDisk, block_on};
use orpheus_fs::{CacheSlot, MountOptions, OpenMode, Volume};

type FVol<'c> = Volume<'c, FaultyDisk<RamDisk>>;

async fn workload(vol: &mut FVol<'_>, seed: u64) -> Result<(), orpheus_fs::Error<HostError>> {
    let mut rng = Rng(seed);
    vol.create_dir("/.orpheus").await?;
    vol.create_dir("/.orpheus/rec").await?;
    for i in 0..6 {
        let p = format!("/.orpheus/rec/take {i} – Ünïcødé.wav");
        let mut f = vol.open(&p, OpenMode::CreateNew).await?;
        let n = 1000 + rng.below(40_000) as usize;
        let data = rng.bytes(n);
        let mut off = 0;
        while off < n {
            let k = (1 + rng.below(9000) as usize).min(n - off);
            vol.write(&mut f, &data[off..off + k]).await?;
            off += k;
            if rng.below(4) == 0 {
                vol.flush_file(&mut f).await?;
            }
        }
        vol.close(f).await?;
    }
    vol.rename("/.orpheus/rec/take 1 – Ünïcødé.wav", "/.orpheus/renamed.wav").await?;
    vol.remove("/.orpheus/rec/take 2 – Ünïcødé.wav").await?;
    let mut f = vol.open("/.orpheus/rec/take 3 – Ünïcødé.wav", OpenMode::ReadWrite).await?;
    vol.set_len(&mut f, 100).await?;
    f.set_position(50_000);
    vol.write(&mut f, b"after gap").await?;
    vol.close(f).await?;
    for i in 0..40 {
        let f = vol.open(&format!("/.orpheus/rec/many file name {i}.idx"), OpenMode::CreateNew).await?;
        vol.close(f).await?;
    }
    vol.flush().await
}

/// Check the image after a cut; returns fsck output for reporting.
fn check_after(disk: RamDisk, exfat: bool, path: &std::path::Path) -> String {
    let mut c = vec![CacheSlot::EMPTY; 8];
    let mut vol = block_on(Volume::mount(disk, &mut c, MountOptions::default())).expect("remount after cut");
    orpheus_fs::host::exercise(&mut vol, 100_000).expect("walk after cut");
    // No two files share a cluster (no cross-links).
    let mut runs: Vec<(u32, u32, String)> = Vec::new();
    block_on(async {
        let mut stack = vec![vol.root()];
        let mut e = orpheus_fs::DirEntry::new();
        while let Some(mut cur) = stack.pop() {
            while vol.read_dir(&mut cur, &mut e).await.unwrap() {
                if e.is_dir() {
                    stack.push(vol.dir_cursor(&e).unwrap());
                } else {
                    let f = vol.open_entry(&e).unwrap();
                    let mut it = vol.extents(&f);
                    while let Some(x) = vol.next_extent(&mut it).await.unwrap() {
                        runs.push((x.lba, x.lba + x.sectors, e.name().to_string()));
                    }
                }
            }
        }
    });
    runs.sort();
    for w in runs.windows(2) {
        assert!(w[0].1 <= w[1].0, "cross-linked: {:?} and {:?}", w[0], w[1]);
    }
    let disk = vol.into_device();
    disk.save(path).unwrap();
    let (ok, out) = fsck(path, exfat);
    if !ok {
        // Only leak-class findings are acceptable after a power cut.
        const ALLOWED: &[&str] = &[
            "Truncating file to",
            "File size is",
            "Reclaimed",
            "Free cluster summary wrong",
            "Auto-correcting",
            "FATs differ but appear to be intact",
            "Using first FAT",
            "Orphaned long file name part",
            "Auto-deleting",
            "Dirty bit is set",
            "Automatically removing dirty bit",
            "Leaving filesystem unchanged",
            "more clusters are allocated",
            "fsck.fat",
            "exfatprogs version",
            "cut.img",
        ];
        for l in out.lines() {
            let t = l.trim();
            if t.is_empty() || t.starts_with('/') || ALLOWED.iter().any(|a| t.contains(a)) {
                continue;
            }
            panic!("fsck after power cut: unexpected finding '{t}':\n{out}");
        }
    }
    out
}

fn scenario(fmt: Fmt, seed: u64, cuts: usize) {
    let dir = scratch(&format!("powercut-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let base = RamDisk::load(&img).unwrap();
    // Total writes of an uninterrupted run.
    let total = {
        let mut c = vec![CacheSlot::EMPTY; 4];
        let dev = FaultyDisk::new(base.clone(), None);
        let mut vol = block_on(Volume::mount(dev, &mut c, MountOptions::default())).unwrap();
        block_on(workload(&mut vol, seed)).unwrap();
        let d = vol.into_device();
        let total = d.writes_done;
        let out = check_after(d.inner, fmt.is_exfat(), &dir.join("full.img"));
        assert!(fsck(&dir.join("full.img"), fmt.is_exfat()).0, "uninterrupted run not clean:\n{out}");
        total
    };
    let mut rng = Rng(seed ^ 0xABCDEF);
    let mut points: Vec<u64> = (0..cuts).map(|_| rng.below(total)).collect();
    points.extend([0, 1, 2, 3, total - 1]);
    let mut seen = std::collections::BTreeSet::new();
    for &n in &points {
        let mut c = vec![CacheSlot::EMPTY; 4];
        let dev = FaultyDisk::new(base.clone(), Some(n));
        let mut vol = block_on(Volume::mount(dev, &mut c, MountOptions::default())).unwrap();
        let r = block_on(workload(&mut vol, seed));
        assert!(r.is_err(), "cut at {n} of {total} not hit");
        let d = vol.into_device();
        let out = check_after(d.inner, fmt.is_exfat(), &dir.join("cut.img"));
        for l in out.lines() {
            if !l.contains("cut.img") && !l.contains("version") && !l.starts_with("fsck.fat") {
                seen.insert(l.to_string());
            }
        }
    }
    eprintln!("{}: fsck lines seen:", fmt.label());
    for l in &seen {
        eprintln!("  {l}");
    }
}

#[test]
fn powercut_fat12() {
    scenario(Fmt::Fat(12, 1, 2), 1, 40);
}

#[test]
fn powercut_fat16() {
    scenario(Fmt::Fat(16, 2, 32), 2, 40);
}

#[test]
fn powercut_fat32() {
    scenario(Fmt::Fat(32, 1, 64), 3, 60);
}

#[test]
fn powercut_exfat() {
    scenario(Fmt::ExFat(4096, 64), 4, 60);
}

#[test]
fn powercut_exfat_512() {
    scenario(Fmt::ExFat(512, 16), 5, 40);
}
