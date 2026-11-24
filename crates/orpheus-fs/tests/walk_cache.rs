//! The path walk's directory cache: repeated opens under one directory
//! start there instead of at the root, and nothing stale is served after a
//! cached directory grows (many new entries), is renamed, or is removed and
//! made again.

mod common;

use common::*;
use orpheus_fs::host::block_on;
use orpheus_fs::{Error, OpenMode, WalkCache};

fn scenario(fmt: Fmt) {
    let dir = scratch(&format!("walk-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let mut c = cache();
    let mut walks = WalkCache::EMPTY;
    let mut vol = mount(&img, &mut c);
    // A cache of its own (`mount` lends one too).
    vol.set_walk_cache(&mut walks);
    let mut rng = Rng(0x5EED_1234);
    for d in ["/.orpheus", "/.orpheus/cache", "/.orpheus/cache/art", "/.orpheus/library"] {
        block_on(vol.create_dir(d)).unwrap();
    }
    write_all(&mut vol, "/.orpheus/library/lib-a.ydb", b"library", &mut rng).unwrap();
    write_all(&mut vol, "/.orpheus/cache/art/A0000001", b"bucket 1", &mut rng).unwrap();
    let h0 = vol.walk_hits();
    for _ in 0..3 {
        assert_eq!(read_all(&mut vol, "/.orpheus/library/lib-a.ydb").unwrap(), b"library");
        assert_eq!(read_all(&mut vol, "/.orpheus/cache/art/A0000001").unwrap(), b"bucket 1");
    }
    assert!(vol.walk_hits() >= h0 + 4, "the walks start at cached directories");

    // The art directory grows past a cluster (exFAT: a longer directory)
    // after it was cached: every new file is found.
    for i in 0..300u32 {
        let p = format!("/.orpheus/cache/art/B{i:07}");
        write_all(&mut vol, &p, &i.to_le_bytes(), &mut rng).unwrap();
    }
    for i in (0..300u32).step_by(7) {
        let p = format!("/.orpheus/cache/art/B{i:07}");
        assert_eq!(read_all(&mut vol, &p).unwrap(), i.to_le_bytes(), "{p}");
    }

    // A cached directory renamed, and a new one made under the old name.
    block_on(vol.rename("/.orpheus/library", "/.orpheus/old-library")).unwrap();
    assert_eq!(read_all(&mut vol, "/.orpheus/library/lib-a.ydb"), Err(Error::NotFound));
    block_on(vol.create_dir("/.orpheus/library")).unwrap();
    write_all(&mut vol, "/.orpheus/library/lib-a.ydb", b"new one", &mut rng).unwrap();
    assert_eq!(read_all(&mut vol, "/.orpheus/library/lib-a.ydb").unwrap(), b"new one");
    assert_eq!(read_all(&mut vol, "/.orpheus/old-library/lib-a.ydb").unwrap(), b"library");

    // Removed and made again (another first cluster).
    block_on(vol.remove("/.orpheus/old-library/lib-a.ydb")).unwrap();
    block_on(vol.remove("/.orpheus/old-library")).unwrap();
    assert!(block_on(vol.open("/.orpheus/old-library/lib-a.ydb", OpenMode::Read)).is_err());
    block_on(vol.flush()).unwrap();
    drop(vol);
    assert_fsck(&img, fmt.is_exfat());
}

#[test]
fn walk_cache_on_every_format() {
    for fmt in matrix() {
        if fmt.is_exfat() && !have("mkfs.exfat") {
            continue;
        }
        scenario(fmt);
    }
}
