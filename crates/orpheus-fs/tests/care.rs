//! Card care (Settings › Storage, VER-13): `Volume::check` reads every
//! folder and follows every file's chain without writing; `remove_tree`
//! removes a tree under the write prefix and nothing else.

mod common;

use common::{Fmt, assert_fsck, have, make_image, mcopy_in, mmd, scratch};
use orpheus_fs::host::{FileDisk, RamDisk, block_on};
use orpheus_fs::{CacheSlot, CheckReport, DirCursor, DirEntry, MountOptions, OpenMode, Volume};

const DIRS: [&str; 5] =
    ["/.orpheus", "/.orpheus/cache", "/.orpheus/cache/art", "/.orpheus/cache/books", "/.orpheus/index"];
const FILES: [&str; 5] = [
    "/.orpheus/cache/art/A1.bin",
    "/.orpheus/cache/books/b.idx",
    "/.orpheus/cache/W.bin",
    "/.orpheus/index/1.idx",
    "/.orpheus/queue.bin",
];

fn fill<D: orpheus_fs::BlockDevice>(v: &mut Volume<'_, D>)
where
    D::Error: core::fmt::Debug,
{
    for d in DIRS {
        block_on(v.create_dir(d)).unwrap();
    }
    for (i, p) in FILES.iter().enumerate() {
        let mut f = block_on(v.open(p, OpenMode::CreateNew)).unwrap();
        block_on(v.write(&mut f, &vec![i as u8; 3000 * (i + 1)])).unwrap();
        block_on(v.close(f)).unwrap();
    }
}

fn ram_volume(fmt: Fmt, name: &str) -> RamDisk {
    let dir = scratch(name);
    let img = dir.join("v.img");
    make_image(&img, fmt);
    mmd(&img, "/Music");
    let song = dir.join("song.mp3");
    std::fs::write(&song, vec![7u8; 100_000]).unwrap();
    mcopy_in(&img, &song, "/Music/song.mp3");
    RamDisk::load(&img).unwrap()
}

fn check(v: &mut Volume<'_, RamDisk>, depth: usize) -> CheckReport {
    let mut stack = vec![v.root(); depth];
    let mut e = DirEntry::new();
    block_on(v.check(&mut stack, &mut e, &mut |_| false)).unwrap()
}

#[test]
fn check_counts_and_writes_nothing() {
    if !have("mkfs.fat") || !have("mcopy") {
        return;
    }
    let d = ram_volume(Fmt::Fat(32, 1, 64), "care-check");
    let mut cache = [CacheSlot::EMPTY; 8];
    let opts = MountOptions { read_only: false, write_prefix: Some("/.orpheus"), partition: None };
    let mut v = block_on(Volume::mount(d, &mut cache, opts)).unwrap();
    fill(&mut v);
    block_on(v.flush()).unwrap();
    let writes = v.device().writes;
    let r = check(&mut v, 8);
    assert_eq!(r, CheckReport { dirs: 6, files: 6, problems: 0, too_deep: 0, stopped: false });
    assert_eq!(v.device().writes, writes, "read-only");
    // Depth 2: the root and one level; deeper folders are counted.
    let r = check(&mut v, 2);
    assert_eq!((r.dirs, r.too_deep), (4, 2), "{r:?}");
    // Stop after the third entry.
    let mut stack = vec![v.root(); 8];
    let mut e = DirEntry::new();
    let mut n = 0;
    let r = block_on(v.check(&mut stack, &mut e, &mut |_| {
        n += 1;
        n > 3
    }))
    .unwrap();
    assert!(r.stopped && r.dirs + r.files <= 3, "{r:?}");
}

/// A file whose chain is cut (its first cluster's link cleared in the
/// FAT) is a problem; the rest still counts.
#[test]
fn check_finds_a_broken_chain() {
    if !have("mkfs.fat") || !have("mcopy") {
        return;
    }
    let mut d = ram_volume(Fmt::Fat(32, 1, 64), "care-broken");
    let first = {
        let mut cache = [CacheSlot::EMPTY; 4];
        let opts = MountOptions { read_only: true, write_prefix: None, partition: None };
        let mut v = block_on(Volume::mount(&mut d, &mut cache, opts)).unwrap();
        let f = block_on(v.open("/Music/song.mp3", OpenMode::Read)).unwrap();
        f.first_cluster()
    };
    let reserved = usize::from(u16::from_le_bytes([d.data[14], d.data[15]]));
    let fats = usize::from(d.data[16]);
    let fat_len = u32::from_le_bytes([d.data[36], d.data[37], d.data[38], d.data[39]]) as usize;
    for k in 0..fats {
        let at = (reserved + k * fat_len) * 512 + first as usize * 4;
        d.data[at..at + 4].copy_from_slice(&0u32.to_le_bytes());
    }
    let mut cache = [CacheSlot::EMPTY; 8];
    let opts = MountOptions { read_only: true, write_prefix: None, partition: None };
    let mut v = block_on(Volume::mount(d, &mut cache, opts)).unwrap();
    let r = check(&mut v, 8);
    assert_eq!((r.dirs, r.files, r.problems), (1, 1, 1), "{r:?}");
}

/// The caches go, everything else stays; the volume stays clean.
#[test]
fn remove_tree_removes_only_the_tree() {
    for fmt in [Fmt::Fat(32, 1, 64), Fmt::ExFat(4096, 64)] {
        if !have(if fmt.is_exfat() { "mkfs.exfat" } else { "mkfs.fat" }) {
            continue;
        }
        let dir = scratch(&format!("care-remove-{}", fmt.label()));
        let img = dir.join("v.img");
        make_image(&img, fmt);
        let mut cache = [CacheSlot::EMPTY; 8];
        let opts = MountOptions { read_only: false, write_prefix: Some("/.orpheus"), partition: None };
        let mut v = block_on(Volume::mount(FileDisk::open(&img).unwrap(), &mut cache, opts)).unwrap();
        fill(&mut v);
        let free0 = block_on(v.free_bytes()).unwrap();
        let mut buf = [0u8; 128];
        let mut e = DirEntry::new();
        assert_eq!(block_on(v.remove_tree("/.orpheus/cache", &mut buf, &mut e)).unwrap(), 3);
        assert_eq!(block_on(v.remove_tree("/.orpheus/index/", &mut buf, &mut e)).unwrap(), 1);
        assert_eq!(block_on(v.remove_tree("/.orpheus/missing", &mut buf, &mut e)).unwrap(), 0);
        assert!(!block_on(v.exists("/.orpheus/cache")).unwrap());
        assert!(!block_on(v.exists("/.orpheus/index")).unwrap());
        assert!(block_on(v.exists("/.orpheus/queue.bin")).unwrap());
        assert!(block_on(v.free_bytes()).unwrap() > free0);
        // Outside the write prefix: refused.
        block_on({
            v.set_write_prefix(None);
            v.create_dir("/Keep")
        })
        .unwrap();
        v.set_write_prefix(Some("/.orpheus"));
        assert!(block_on(v.remove_tree("/Keep", &mut buf, &mut e)).is_err());
        assert!(block_on(v.exists("/Keep")).unwrap());
        // A path too long for the buffer.
        block_on(v.create_dir("/.orpheus/cache")).unwrap();
        block_on(v.create_dir("/.orpheus/cache/a-rather-long-folder-name")).unwrap();
        assert!(block_on(v.remove_tree("/.orpheus/cache", &mut [0u8; 24], &mut e)).is_err());
        let (_, r) = block_on(v.unmount());
        r.unwrap();
        assert_fsck(&img, fmt.is_exfat());
    }
}

/// `DirCursor` is small: a deep stack costs little.
#[test]
fn cursor_size() {
    assert!(core::mem::size_of::<DirCursor>() <= 48, "{}", core::mem::size_of::<DirCursor>());
}
