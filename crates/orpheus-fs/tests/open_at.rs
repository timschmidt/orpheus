//! `Volume::open_located` and `Volume::open_at`: a file reopened from its
//! directory entry without the path walk (the index files of a listing,
//! AU-PERF 2026-11-25), on every format. The entry is read again each
//! time: a file rewritten in place opens with its new size and clusters; a
//! renamed, removed or replaced one, or a directory, is not opened.

mod common;

use common::*;
use orpheus_fs::host::block_on;
use orpheus_fs::{Error, OpenMode};

fn read_at(vol: &mut Vol<'_>, loc: orpheus_fs::Locator, name: &str) -> Result<Vec<u8>, FsErr> {
    let mut f = block_on(vol.open_at(loc, name))?;
    let mut out = vec![0u8; f.size() as usize];
    let n = block_on(vol.read_exact_or_eof(&mut f, &mut out))?;
    out.truncate(n);
    Ok(out)
}

fn scenario(fmt: Fmt) {
    let dir = scratch(&format!("open-at-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    let mut rng = Rng(0x0BE7_A700);
    block_on(vol.create_dir("/.orpheus")).unwrap();
    block_on(vol.create_dir("/.orpheus/index")).unwrap();
    for i in 0..40 {
        write_all(&mut vol, &format!("/.orpheus/index/0000{i:04}-n0.idx"), &[i as u8; 100], &mut rng)
            .unwrap();
    }
    let path = "/.orpheus/index/00000033-n0.idx";
    let (f, loc) = block_on(vol.open_located(path)).unwrap();
    assert_eq!(f.size(), 100);
    // Found again from the entry; the name compares as a walk does.
    assert_eq!(read_at(&mut vol, loc, "00000033-n0.idx").unwrap(), vec![33u8; 100]);
    assert_eq!(read_at(&mut vol, loc, "00000033-N0.IDX").unwrap(), vec![33u8; 100]);
    // Another name at that entry: not this file.
    assert_eq!(read_at(&mut vol, loc, "00000034-n0.idx"), Err(Error::NotFound));
    // Directories are not files.
    assert_eq!(block_on(vol.open_located("/.orpheus/index")).err(), Some(Error::IsADirectory));
    let (_, dloc) = block_on(vol.open_located("/.orpheus/index/00000000-n0.idx")).unwrap();
    let mut e = orpheus_fs::DirEntry::new();
    block_on(vol.stat("/.orpheus", &mut e)).unwrap();
    assert_eq!(block_on(vol.open_at(e.locator(), ".orpheus")).err(), Some(Error::IsADirectory));
    assert_eq!(block_on(vol.open_located("/.orpheus/index/missing.idx")).err(), Some(Error::NotFound));

    // Rewritten in place, longer (new clusters): the new contents.
    let mut w = block_on(vol.open(path, OpenMode::Truncate)).unwrap();
    let big = vec![0xA5u8; 70_000];
    block_on(vol.write(&mut w, &big)).unwrap();
    block_on(vol.close(w)).unwrap();
    assert_eq!(read_at(&mut vol, loc, "00000033-n0.idx").unwrap(), big);

    // Renamed: not found under its old name.
    block_on(vol.rename(path, "/.orpheus/index/00000033-n0.old")).unwrap();
    assert_eq!(read_at(&mut vol, loc, "00000033-n0.idx"), Err(Error::NotFound));
    // Removed, and its slots taken by a new file of the same length name.
    block_on(vol.remove("/.orpheus/index/00000033-n0.old")).unwrap();
    assert_eq!(read_at(&mut vol, loc, "00000033-n0.idx"), Err(Error::NotFound));
    write_all(&mut vol, "/.orpheus/index/00000099-n0.idx", b"other", &mut rng).unwrap();
    let (_, nloc) = block_on(vol.open_located("/.orpheus/index/00000099-n0.idx")).unwrap();
    assert_eq!(read_at(&mut vol, loc, "00000033-n0.idx"), Err(Error::NotFound));
    assert_eq!(read_at(&mut vol, nloc, "00000099-n0.idx").unwrap(), b"other");
    // The untouched file is still found where it was.
    assert_eq!(read_at(&mut vol, dloc, "00000000-n0.idx").unwrap(), vec![0u8; 100]);
    unmount(vol);
    assert_fsck(&img, fmt.is_exfat());
}

#[test]
fn open_at_on_every_format() {
    for fmt in matrix() {
        if fmt.is_exfat() && !have("mkfs.exfat") {
            continue;
        }
        scenario(fmt);
    }
}
