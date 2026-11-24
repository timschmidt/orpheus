//! A directory with 5000 long Unicode names: create, list with a resumable
//! cursor in windows, look up case-insensitively, delete half, fsck.

mod common;

use std::collections::BTreeSet;

use common::*;
use orpheus_fs::host::block_on;
use orpheus_fs::{DirEntry, OpenMode};

const N: usize = 5000;

fn scenario(fmt: Fmt) {
    let dir = scratch(&format!("bigdir-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let names: Vec<String> = (0..N).map(unicode_name).collect();
    {
        let mut c = cache();
        let mut vol = mount(&img, &mut c);
        block_on(async {
            vol.create_dir("/big").await.unwrap();
            for (i, n) in names.iter().enumerate() {
                let p = format!("/big/{n}");
                let mut f = vol.open(&p, OpenMode::CreateNew).await.unwrap_or_else(|e| panic!("{p}: {e:?}"));
                if i % 500 == 0 {
                    vol.write(&mut f, n.as_bytes()).await.unwrap();
                }
                vol.close(f).await.unwrap();
            }
        });
        unmount(vol);
    }
    assert_fsck(&img, fmt.is_exfat());

    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    block_on(async {
        // Window through the directory 37 entries at a time, keeping only
        // the cursor between windows (as a UI list would).
        let mut seen = BTreeSet::new();
        let mut cur = vol.open_dir("/big").await.unwrap();
        let mut e = DirEntry::new();
        let mut pages = Vec::new();
        loop {
            pages.push(cur);
            let mut k = 0;
            while k < 37 && vol.read_dir(&mut cur, &mut e).await.unwrap() {
                assert!(seen.insert(e.name().to_string()), "duplicate {}", e.name());
                k += 1;
            }
            if k < 37 {
                break;
            }
        }
        assert_eq!(seen.len(), N);
        assert_eq!(seen, names.iter().cloned().collect::<BTreeSet<_>>());

        // Jump back to a stored page and to an entry index.
        let mut cur = pages[60];
        vol.read_dir(&mut cur, &mut e).await.unwrap();
        let first_of_page = e.name().to_string();
        let idx = e.index();
        let mut cur2 = vol.open_dir("/big").await.unwrap();
        vol.seek_dir(&mut cur2, idx).await.unwrap();
        vol.read_dir(&mut cur2, &mut e).await.unwrap();
        assert_eq!(e.name(), first_of_page);
        // Locator round trip.
        let loc = e.locator();
        let mut e2 = DirEntry::new();
        vol.open_locator(loc, &mut e2).await.unwrap();
        assert_eq!(e2.name(), first_of_page);

        // Case-insensitive lookups of random names.
        let mut rng = Rng(77);
        for _ in 0..200 {
            let n = &names[rng.below(N as u64) as usize];
            let variant: String = if rng.below(2) == 0 { n.to_uppercase() } else { n.to_lowercase() };
            let p = format!("/big/{variant}");
            // "ß".to_uppercase() is "SS" (full mapping), which simple folding
            // does not equate; skip those.
            if variant.chars().count() != n.chars().count() {
                continue;
            }
            assert!(vol.exists(&p).await.unwrap(), "lookup {p}");
        }
        let f = vol.open(&format!("/big/{}", names[500]), OpenMode::Read).await.unwrap();
        assert_eq!(f.size(), names[500].len() as u64);

        // Remove every other entry.
        for n in names.iter().step_by(2) {
            vol.remove(&format!("/big/{n}")).await.unwrap();
        }
        vol.flush().await.unwrap();
        let left = list(&mut vol, "/big").len();
        assert_eq!(left, N / 2);
    });
    unmount(vol);
    assert_fsck(&img, fmt.is_exfat());
    // `dump.exfat` lists non-ASCII names only from exfatprogs 1.4 (the
    // 1.2 of Ubuntu 24.04 does not); `fsck` above already validated it.
    if fmt.is_exfat() && common::exfatprogs_at_least(1, 4) {
        let listing = exfat_dump_dir(&img, "/big");
        assert!(listing.contains(&names[1]), "dump.exfat does not list {}", names[1]);
    }
}

#[test]
fn bigdir_fat32() {
    scenario(Fmt::Fat(32, 8, 300));
}

#[test]
fn bigdir_fat16() {
    scenario(Fmt::Fat(16, 4, 64));
}

#[test]
fn bigdir_exfat() {
    scenario(Fmt::ExFat(4096, 64));
}
