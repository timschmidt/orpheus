//! Adversarial card contents (VER-3, 07 §3.7), beyond the random-patch
//! `fuzz_smoke`/`mount_walk`: cyclic and broken FAT chains under
//! directories and files (every operation must end, in bounded time),
//! hostile file names created and listed back, locators from damaged
//! index/queue files, and boot-sector fields at their edge values.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
mod common;

use std::sync::mpsc;
use std::time::Duration;

use adversarial::{config, edges, hostile_names, put_le};
use common::*;
use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, DirEntry, Error, Locator, MountOptions, OpenMode, Volume};
use proptest::prelude::*;

type RVol<'c> = Volume<'c, RamDisk>;

/// Run `f` on a thread; fail if it does not finish within `secs` (a hang
/// on the player is a watchdog reset).
fn bounded<T: Send + 'static>(what: &str, secs: u64, f: impl FnOnce() -> T + Send + 'static) -> T {
    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(f());
    });
    match rx.recv_timeout(Duration::from_secs(secs)) {
        Ok(v) => v,
        Err(_) => panic!("{what}: did not finish within {secs} s"),
    }
}

/// Geometry of a FAT12/16/32 image, read from its boot sector.
struct Fat {
    bits: u8,
    fat_start: usize,
    fat_bytes: usize,
    fats: usize,
}

fn fat_geometry(img: &[u8], bits: u8) -> Fat {
    let rd16 = |o: usize| usize::from(u16::from_le_bytes([img[o], img[o + 1]]));
    let rd32 = |o: usize| u32::from_le_bytes(img[o..o + 4].try_into().unwrap()) as usize;
    let reserved = rd16(14);
    let fats = usize::from(img[16]);
    let fatsz = if rd16(22) != 0 { rd16(22) } else { rd32(36) };
    Fat { bits, fat_start: reserved * 512, fat_bytes: fatsz * 512, fats }
}

/// Set FAT entry `c` to `v` in every FAT copy.
fn set_fat(img: &mut [u8], g: &Fat, c: u32, v: u32) {
    for k in 0..g.fats {
        let base = g.fat_start + k * g.fat_bytes;
        match g.bits {
            12 => {
                let o = base + (c as usize * 3) / 2;
                let mut w = u16::from_le_bytes([img[o], img[o + 1]]);
                w = if c & 1 == 0 {
                    (w & 0xF000) | (v as u16 & 0x0FFF)
                } else {
                    (w & 0x000F) | ((v as u16) << 4)
                };
                img[o..o + 2].copy_from_slice(&w.to_le_bytes());
            }
            16 => put_le(img, base + c as usize * 2, 2, u64::from(v)),
            _ => put_le(img, base + c as usize * 4, 4, u64::from(v & 0x0FFF_FFFF)),
        }
    }
}

fn get_fat(img: &[u8], g: &Fat, c: u32) -> u32 {
    let base = g.fat_start;
    match g.bits {
        12 => {
            let o = base + (c as usize * 3) / 2;
            let w = u16::from_le_bytes([img[o], img[o + 1]]);
            u32::from(if c & 1 == 0 { w & 0x0FFF } else { w >> 4 })
        }
        16 => u32::from(u16::from_le_bytes([img[base + c as usize * 2], img[base + c as usize * 2 + 1]])),
        _ => {
            u32::from_le_bytes(img[base + c as usize * 4..base + c as usize * 4 + 4].try_into().unwrap())
                & 0x0FFF_FFFF
        }
    }
}

/// A FAT image with `/d` (enough names for several clusters) and
/// `/f.bin` (several clusters); returns the bytes and both first clusters.
fn prepared(fmt: Fmt, name: &str) -> (Vec<u8>, u32, u32) {
    let dir = scratch(name);
    let path = dir.join("img");
    make_image(&path, fmt);
    let mut c = cache();
    let mut vol = mount(&path, &mut c);
    let (d, f) = block_on(async {
        vol.create_dir("/d").await.unwrap();
        for i in 0..40 {
            let mut f =
                vol.open(&format!("/d/a fairly long file name {i:03}.txt"), OpenMode::Create).await.unwrap();
            vol.write(&mut f, b"x").await.unwrap();
            vol.close(f).await.unwrap();
        }
        let mut f = vol.open("/f.bin", OpenMode::Create).await.unwrap();
        vol.write(&mut f, &[0xA5; 5000]).await.unwrap();
        vol.close(f).await.unwrap();
        let mut e = DirEntry::new();
        vol.stat("/d", &mut e).await.unwrap();
        let d = e.first_cluster();
        vol.stat("/f.bin", &mut e).await.unwrap();
        (d, e.first_cluster())
    });
    unmount(vol);
    (std::fs::read(&path).unwrap(), d, f)
}

/// Everything a card task does with a directory and a file, on a damaged
/// image: each must end (with any result).
fn exercise_all(img: Vec<u8>) -> u32 {
    let mut slots = vec![CacheSlot::EMPTY; 8];
    let Ok(mut vol): Result<RVol<'_>, _> =
        block_on(Volume::mount(RamDisk::from_bytes(img), &mut slots, MountOptions::default()))
    else {
        return 0;
    };
    block_on(async {
        let mut listed = 0u32;
        for dir in ["/", "/d"] {
            if let Ok(mut cur) = vol.open_dir(dir).await {
                let mut e = DirEntry::new();
                while let Ok(true) = vol.read_dir(&mut cur, &mut e).await {
                    listed += 1;
                    assert!(listed < 200_000, "a directory listing does not end");
                }
            }
        }
        let mut e = DirEntry::new();
        let _ = vol.stat("/d/not there.txt", &mut e).await;
        let _ = vol.exists("/d/a fairly long file name 039.txt").await;
        if let Ok(mut f) = vol.open("/f.bin", OpenMode::Read).await {
            let mut buf = vec![0u8; 8192];
            let _ = vol.read_exact_or_eof(&mut f, &mut buf).await;
            let mut it = vol.extents(&f);
            let mut n = 0;
            while let Ok(Some(_)) = vol.next_extent(&mut it).await {
                n += 1;
                assert!(n < 100_000, "the extent list does not end");
            }
            let _ = vol.is_contiguous(&f).await;
        }
        let _ = vol.create_dir("/d/new").await;
        if let Ok(mut f) = vol.open("/d/grow.bin", OpenMode::Create).await {
            let _ = vol.write(&mut f, &[1; 3000]).await;
            let _ = vol.close(f).await;
        }
        if let Ok(mut f) = vol.open("/f.bin", OpenMode::ReadWrite).await {
            let _ = vol.set_len(&mut f, 100).await;
            let _ = vol.set_len(&mut f, 20_000).await;
            let _ = vol.close(f).await;
        }
        let _ = vol.remove("/f.bin").await;
        let _ = vol.remove("/d/a fairly long file name 000.txt").await;
        let _ = vol.rename("/d", "/e").await;
        let _ = vol.free_clusters().await;
        let _ = vol.flush().await;
        listed
    })
}

#[test]
fn cyclic_and_broken_chains_end() {
    for (fmt, bits) in [(Fmt::Fat(16, 1, 16), 16u8), (Fmt::Fat(32, 1, 64), 32), (Fmt::Fat(12, 1, 2), 12)] {
        let (img, d, f) = prepared(fmt, &format!("chains-{}", fmt.label()));
        let g = fat_geometry(&img, bits);
        let d2 = get_fat(&img, &g, d);
        let f2 = get_fat(&img, &g, f);
        let max = match bits {
            12 => 0xFFF,
            16 => 0xFFFF,
            _ => 0x0FFF_FFFF,
        };
        // (entry, value): self loops, two-cycles, a chain into free,
        // reserved, out-of-range, bad and end-of-chain values, and a loop
        // into the other's chain.
        let cases = [
            (d, d),
            (d2, d),
            (f, f),
            (f2, f),
            (d, f),
            (f, d),
            (d, 0),
            (d, 1),
            (f, 0),
            (f2, 1),
            (d, max - 8),
            (f, max - 8),
            (d, 0x00FF_FFF0 & max),
            (f, 0x0123_4567 & max),
            (d2, 2),
        ];
        for (at, v) in cases {
            if at < 2 || at >= max - 8 {
                continue;
            }
            let mut b = img.clone();
            set_fat(&mut b, &g, at, v);
            let what = format!("{}: FAT[{at}] = {v:#x}", fmt.label());
            bounded(&what, 20, move || exercise_all(b));
        }
    }
}

#[test]
fn exfat_root_chain_loop_ends() {
    // exFAT's root directory has no length: its FAT chain made a loop.
    let dir = scratch("exfat-root-loop");
    let path = dir.join("img");
    make_image(&path, Fmt::ExFat(4096, 16));
    let img = std::fs::read(&path).unwrap();
    let rd32 = |o: usize| u32::from_le_bytes(img[o..o + 4].try_into().unwrap());
    let fat = rd32(80) as usize * 512;
    let root = rd32(96);
    for v in [root, 2, 0, 1, 0xFFFF_FFF7] {
        let mut b = img.clone();
        put_le(&mut b, fat + root as usize * 4, 4, u64::from(v));
        bounded(&format!("exFAT FAT[root {root}] = {v:#x}"), 20, move || exercise_all(b));
    }
}

#[test]
fn hostile_names_are_refused_or_listed_back() {
    for fmt in [Fmt::Fat(16, 1, 16), Fmt::ExFat(4096, 16)] {
        let dir = scratch(&format!("names-{}", fmt.label()));
        let path = dir.join("img");
        make_image(&path, fmt);
        let mut c = cache();
        let mut vol = mount(&path, &mut c);
        let names = hostile_names();
        let made: Vec<String> = block_on(async {
            vol.create_dir("/.orpheus").await.unwrap();
            let mut made = Vec::new();
            for n in &names {
                let p = format!("/.orpheus/{n}");
                match vol.open(&p, OpenMode::CreateNew).await {
                    Ok(mut f) => {
                        vol.write(&mut f, n.as_bytes()).await.unwrap();
                        vol.close(f).await.unwrap();
                        made.push(n.clone());
                    }
                    Err(Error::InvalidName | Error::NameTooLong | Error::AlreadyExists | Error::NotFound) => {
                    }
                    Err(e) => panic!("{fmt:?} {n:?}: {e:?}"),
                }
                // Directories and renames with the same names.
                let _ = vol.create_dir(&format!("/.orpheus/d{n}")).await;
                let _ = vol.rename("/.orpheus/d", &format!("/.orpheus/r{n}")).await;
            }
            made
        });
        // Every name made is listed back, and opens by that name.
        let listed: Vec<String> = list(&mut vol, "/.orpheus").into_iter().map(|e| e.0).collect();
        for n in &made {
            let found = listed.iter().any(|l| {
                l == n
                    || l.eq_ignore_ascii_case(n)
                    || l.trim_end_matches(['.', ' ']) == n.trim_end_matches(['.', ' '])
            });
            assert!(found, "{fmt:?}: {n:?} created but not listed ({} listed)", listed.len());
            let p = format!("/.orpheus/{n}");
            let data = read_all(&mut vol, &p).unwrap_or_else(|e| panic!("{fmt:?}: {n:?}: {e:?}"));
            assert_eq!(data, n.as_bytes(), "{fmt:?}: {n:?}");
        }
        unmount(vol);
        assert_fsck(&path, fmt.is_exfat());
        // Hostile paths never write outside the prefix.
        let mut c = cache();
        let mut dev = orpheus_fs::host::FileDisk::open(&path).unwrap();
        dev.elide_zero_writes = true;
        let opts = MountOptions { write_prefix: Some("/.orpheus"), ..MountOptions::default() };
        let mut vol = block_on(Volume::mount(dev, &mut c, opts)).unwrap();
        for p in [
            "/.orpheus/../x",
            "/.orpheus/./../y",
            "/.ORPHEUS/../z",
            "/x/../.orpheus/w",
            "//.orpheus/../v",
            "/.orpheusx/a",
            "/.orpheus\\..\\u",
        ] {
            let r = block_on(vol.open(p, OpenMode::CreateNew));
            if let Ok(f) = r {
                block_on(vol.close(f)).unwrap();
            }
        }
        let root: Vec<String> = list(&mut vol, "/").into_iter().map(|e| e.0).collect();
        assert!(
            root.iter().all(|n| n == ".orpheus" || n == "System Volume Information"),
            "{fmt:?}: written outside the prefix: {root:?}"
        );
        unmount(vol);
    }
}

#[test]
fn locators_from_damaged_files() {
    for fmt in [Fmt::Fat(16, 1, 16), Fmt::Fat(32, 1, 64), Fmt::ExFat(4096, 16)] {
        let dir = scratch(&format!("loc-{}", fmt.label()));
        let path = dir.join("img");
        make_image(&path, fmt);
        let mut c = cache();
        let mut vol = mount(&path, &mut c);
        let real = block_on(async {
            vol.create_dir("/m").await.unwrap();
            let f = vol.open("/m/song.mp3", OpenMode::Create).await.unwrap();
            vol.close(f).await.unwrap();
            let (f, loc) = vol.open_located("/m/song.mp3").await.unwrap();
            vol.close(f).await.unwrap();
            loc
        });
        let clusters = u64::from(vol.cluster_count());
        for dir in edges(32, u64::from(real.dir)).into_iter().chain([clusters, clusters + 1, clusters + 2]) {
            for index in
                edges(32, u64::from(real.index)).into_iter().chain([1 << 31, (1 << 31) | 1, 0x7FFF_FFFF])
            {
                let loc = Locator { dir: dir as u32, index: index as u32 };
                let mut e = DirEntry::new();
                let r = block_on(vol.open_locator(loc, &mut e));
                if loc == real {
                    assert!(r.is_ok());
                }
                let _ = block_on(vol.open_at(loc, "song.mp3"));
            }
        }
        unmount(vol);
    }
}

#[test]
fn boot_sector_fields_at_their_edges() {
    for fmt in [Fmt::Fat(12, 1, 2), Fmt::Fat(16, 1, 16), Fmt::Fat(32, 1, 64), Fmt::ExFat(4096, 16)] {
        let (img, _, _) = if fmt.is_exfat() {
            let dir = scratch(&format!("bpb-{}", fmt.label()));
            let path = dir.join("img");
            make_image(&path, fmt);
            (std::fs::read(&path).unwrap(), 0, 0)
        } else {
            prepared(fmt, &format!("bpb-{}", fmt.label()))
        };
        // (offset, width) of every geometry field.
        let fields: &[(usize, usize)] = if fmt.is_exfat() {
            &[
                (64, 8),
                (72, 8),
                (80, 4),
                (84, 4),
                (88, 4),
                (92, 4),
                (96, 4),
                (104, 2),
                (106, 2),
                (108, 1),
                (109, 1),
                (110, 1),
            ]
        } else {
            &[
                (11, 2),
                (13, 1),
                (14, 2),
                (16, 1),
                (17, 2),
                (19, 2),
                (22, 2),
                (32, 4),
                (36, 4),
                (44, 4),
                (48, 2),
                (50, 2),
            ]
        };
        for &(off, n) in fields {
            let real = img[off..off + n].iter().rev().fold(0u64, |a, &b| a << 8 | u64::from(b));
            for v in edges(8 * n as u32, real) {
                let mut b = img.clone();
                put_le(&mut b, off, n, v);
                let what = format!("{}: boot sector +{off} = {v:#x}", fmt.label());
                bounded(&what, 20, move || exercise_all(b));
            }
        }
    }
}

proptest! {
    #![proptest_config(config("fs-fat-entries", 48))]

    /// Any FAT entries of a prepared FAT16 image overwritten with any
    /// values: every operation ends.
    #[test]
    fn any_fat_entries(patches in prop::collection::vec((2u32..400, any::<u16>()), 1..12)) {
        static IMG: std::sync::OnceLock<(Vec<u8>, u32, u32)> = std::sync::OnceLock::new();
        let (img, _, _) = IMG.get_or_init(|| prepared(Fmt::Fat(16, 1, 16), "prop-fat16"));
        let g = fat_geometry(img, 16);
        let mut b = img.clone();
        for (c, v) in &patches {
            set_fat(&mut b, &g, *c, u32::from(*v));
        }
        bounded("patched FAT", 20, move || exercise_all(b));
    }
}
