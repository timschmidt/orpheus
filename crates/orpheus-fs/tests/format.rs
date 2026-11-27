//! `orpheus_fs::format`: FAT32 and exFAT made on images (superfloppy and
//! MBR-partitioned), checked by `fsck -n`, mounted and written by
//! `orpheus-fs`, the partition table left as it was, a cut format
//! formattable again (Settings › Storage › Format card, VER-13).

mod common;

use common::{assert_fsck, have, scratch};
use orpheus_fs::format::{Formatted, LABEL, format};
use orpheus_fs::host::{FaultyDisk, FileDisk, RamDisk, block_on};
use orpheus_fs::{BlockDevice, CacheSlot, FsKind, MountOptions, OpenMode, Sector, Volume};
use std::path::Path;

const MIB: u32 = 2048;

/// An image of `mib` MiB with an MBR whose first partition (type `ty`)
/// starts at sector 2048 and fills the rest; the bytes before the
/// partition are a pattern a format must not touch.
fn mbr_image(path: &Path, mib: u32, ty: u8) {
    let f = std::fs::File::create(path).unwrap();
    f.set_len(u64::from(mib) << 20).unwrap();
    drop(f);
    let mut d = FileDisk::open(path).unwrap();
    let mut s: Sector = [0; 512];
    for (i, b) in s.iter_mut().enumerate().take(446) {
        *b = i as u8;
    }
    s[446 + 4] = ty;
    s[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
    s[446 + 12..446 + 16].copy_from_slice(&(mib * MIB - 2048).to_le_bytes());
    s[510] = 0x55;
    s[511] = 0xAA;
    block_on(d.write(0, &[s])).unwrap();
    let pat = [0xA5u8; 512];
    for l in 1..2048 {
        block_on(d.write(l, &[pat])).unwrap();
    }
}

/// The first `n` bytes of an image (the images can be sparse and large).
fn head(path: &Path, n: usize) -> Vec<u8> {
    use std::io::Read;
    let mut b = vec![0u8; n];
    std::fs::File::open(path).unwrap().read_exact(&mut b).unwrap();
    b
}

/// The partition's bytes as an image of their own (for fsck; small
/// images only).
fn extract(path: &Path, out: &Path, f: &Formatted) {
    let b = std::fs::read(path).unwrap();
    let s = f.start as usize * 512;
    std::fs::write(out, &b[s..s + f.sectors as usize * 512]).unwrap();
}

/// Mount, write a file under `/.orpheus`, read it back, unmount.
fn use_volume<D: BlockDevice>(dev: D, kind: FsKind) -> D
where
    D::Error: core::fmt::Debug,
{
    let mut cache = [CacheSlot::EMPTY; 8];
    let opts = MountOptions { read_only: false, write_prefix: Some("/.orpheus"), partition: None };
    let mut v = block_on(Volume::mount(dev, &mut cache, opts)).unwrap();
    assert_eq!(v.kind(), kind);
    let free = block_on(v.free_bytes()).unwrap();
    assert!(free > 0);
    block_on(v.create_dir("/.orpheus")).unwrap();
    let mut f = block_on(v.open("/.orpheus/hello.txt", OpenMode::CreateNew)).unwrap();
    block_on(v.write(&mut f, b"formatted by Orpheus")).unwrap();
    block_on(v.close(f)).unwrap();
    let mut f = block_on(v.open("/.orpheus/hello.txt", OpenMode::Read)).unwrap();
    let mut b = [0u8; 64];
    let n = block_on(v.read(&mut f, &mut b)).unwrap();
    assert_eq!(&b[..n], b"formatted by Orpheus");
    // Nothing but the label (FAT) or the system entries (exFAT) and ours.
    let mut cur = v.root();
    let mut e = orpheus_fs::DirEntry::new();
    let mut names = Vec::new();
    while block_on(v.read_dir(&mut cur, &mut e)).unwrap() {
        names.push(e.name().to_string());
    }
    assert_eq!(names, [".orpheus"]);
    let (d, r) = block_on(v.unmount());
    r.unwrap();
    d
}

fn check(kind: Option<FsKind>, want: FsKind, mib: u32, name: &str) {
    let dir = scratch(name);
    let img = dir.join("card.img");
    mbr_image(&img, mib, if want == FsKind::ExFat { 0x07 } else { 0x0C });
    let before = head(&img, 2048 * 512);
    let mut d = FileDisk::open(&img).unwrap();
    let mut buf = [[0u8; 512]; 32];
    let f = block_on(format(&mut d, kind, 0x1234_ABCD, &mut buf)).unwrap();
    assert_eq!((f.kind, f.start, f.sectors), (want, 2048, mib * MIB - 2048));
    drop(d);
    assert_eq!(head(&img, 2048 * 512), before, "the MBR and the gap are untouched");
    // fsck on a copy of the partition (images up to 1 GiB).
    let fsck = mib <= 1024 && have(if want == FsKind::ExFat { "fsck.exfat" } else { "fsck.fat" });
    let part = dir.join("part.img");
    if fsck {
        extract(&img, &part, &f);
        assert_fsck(&part, want == FsKind::ExFat);
    }
    let d = use_volume(FileDisk::open(&img).unwrap(), want);
    drop(d);
    if fsck {
        extract(&img, &part, &f);
        assert_fsck(&part, want == FsKind::ExFat);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn fat32_on_a_partition() {
    check(None, FsKind::Fat32, 64, "format-fat32");
}

#[test]
fn exfat_forced_on_a_small_partition() {
    check(Some(FsKind::ExFat), FsKind::ExFat, 64, "format-exfat");
}

/// Above 32 GiB the size rule picks exFAT (SDXC), 128 KiB clusters.
#[test]
fn exfat_by_size_above_32_gib() {
    check(None, FsKind::ExFat, 33 * 1024, "format-exfat-33g");
}

/// The label tools see.
#[test]
fn label_is_orpheus() {
    if !have("fatlabel") {
        return;
    }
    let dir = scratch("format-label");
    let img = dir.join("card.img");
    mbr_image(&img, 64, 0x0C);
    let mut d = FileDisk::open(&img).unwrap();
    let f = block_on(format(&mut d, None, 1, &mut [[0u8; 512]; 4])).unwrap();
    drop(d);
    let part = dir.join("part.img");
    extract(&img, &part, &f);
    let o = std::process::Command::new("fatlabel").arg(&part).output().unwrap();
    assert_eq!(String::from_utf8_lossy(&o.stdout).trim(), LABEL);
}

/// A superfloppy card (the boot sector at 0, as mkfs makes it): the whole
/// device; the old files are gone.
#[test]
fn superfloppy_is_formatted_whole() {
    if !have("mkfs.fat") {
        return;
    }
    let dir = scratch("format-superfloppy");
    let img = dir.join("card.img");
    common::make_image(&img, common::Fmt::Fat(32, 1, 64));
    common::mmd(&img, "/OLD");
    let mut d = FileDisk::open(&img).unwrap();
    let f = block_on(format(&mut d, None, 7, &mut [[0u8; 512]; 8])).unwrap();
    assert_eq!((f.start, f.sectors), (0, 64 * MIB));
    drop(d);
    assert_fsck(&img, false);
    drop(use_volume(FileDisk::open(&img).unwrap(), FsKind::Fat32));
}

/// Sector 0 that is neither a boot sector nor an MBR (nor all zero): the
/// card is refused, nothing written.
#[test]
fn unknown_layout_is_refused() {
    let mut d = RamDisk::new(64 * MIB);
    d.data[..512].fill(0x5A);
    let r = block_on(format(&mut d, None, 1, &mut [[0u8; 512]; 1]));
    assert_eq!(r, Err(orpheus_fs::Error::NoFilesystem));
    assert_eq!(d.writes, 0);
    // A GPT's protective MBR: refused too (no repartitioning, no GPT).
    let mut d = RamDisk::new(64 * MIB);
    d.data[446 + 4] = 0xEE;
    d.data[446 + 8] = 1;
    d.data[446 + 12] = 0xFF;
    d.data[510] = 0x55;
    d.data[511] = 0xAA;
    let r = block_on(format(&mut d, None, 1, &mut [[0u8; 512]; 1]));
    assert_eq!(r, Err(orpheus_fs::Error::Unsupported));
    assert_eq!(d.writes, 0);
}

/// A format cut at any write leaves a card that does not mount as the old
/// file system and formats again (superfloppy and partitioned).
#[test]
fn a_cut_format_formats_again() {
    for (kind, mbr) in [(FsKind::Fat32, true), (FsKind::ExFat, true), (FsKind::Fat32, false)] {
        let mut base = RamDisk::new(64 * MIB);
        if mbr {
            let s = &mut base.data[..512];
            s[446 + 4] = 0x0C;
            s[446 + 8..446 + 12].copy_from_slice(&2048u32.to_le_bytes());
            s[446 + 12..446 + 16].copy_from_slice(&(64 * MIB - 2048).to_le_bytes());
            s[510] = 0x55;
            s[511] = 0xAA;
        }
        // A first, whole format to count the writes.
        let mut full = FaultyDisk::new(base.clone(), None);
        block_on(format(&mut full, Some(kind), 1, &mut [[0u8; 512]; 16])).unwrap();
        let total = full.writes_done;
        for cut in [1, total / 3, total / 2, total - 3, total - 1] {
            let mut d = FaultyDisk::new(base.clone(), Some(cut));
            let _ = block_on(format(&mut d, Some(kind), 1, &mut [[0u8; 512]; 16]));
            let mut cache = [CacheSlot::EMPTY; 4];
            let opts = MountOptions { read_only: true, write_prefix: None, partition: None };
            let mounted = block_on(Volume::mount(&mut d.inner, &mut cache, opts)).is_ok();
            assert!(!mounted, "{kind:?} mbr={mbr} cut after {cut} of {total}: mounts half made");
            let mut again = d.inner;
            block_on(format(&mut again, Some(kind), 1, &mut [[0u8; 512]; 16])).unwrap();
            drop(use_volume(again, kind));
        }
    }
}
