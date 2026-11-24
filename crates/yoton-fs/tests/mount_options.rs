//! Partition tables, superfloppy, write prefix, read-only, non-standard
//! exFAT up-case table, small caches.

mod common;

use common::*;
use yoton_fs::host::{RamDisk, block_on};
use yoton_fs::{CacheSlot, Error, FsKind, MountOptions, OpenMode, Volume};

fn put_part(mbr: &mut [u8], idx: usize, ty: u8, start: u32, count: u32) {
    let e = &mut mbr[446 + 16 * idx..446 + 16 * idx + 16];
    e[4] = ty;
    e[8..12].copy_from_slice(&start.to_le_bytes());
    e[12..16].copy_from_slice(&count.to_le_bytes());
}

#[test]
fn mbr_partitions() {
    let dir = scratch("mbr");
    let fat = dir.join("fat.img");
    let ex = dir.join("ex.img");
    make_image(&fat, Fmt::Fat(32, 1, 64));
    make_image(&ex, Fmt::ExFat(4096, 32));
    let fat_b = std::fs::read(&fat).unwrap();
    let ex_b = std::fs::read(&ex).unwrap();
    let p0 = 2048u32;
    let p1 = p0 + (fat_b.len() / 512) as u32 + 2048;
    let total = p1 as usize * 512 + ex_b.len() + 4096;
    let mut img = vec![0u8; total];
    put_part(&mut img, 0, 0x0C, p0, (fat_b.len() / 512) as u32);
    put_part(&mut img, 1, 0x07, p1, (ex_b.len() / 512) as u32);
    img[510] = 0x55;
    img[511] = 0xAA;
    // filler between partitions that must never change
    for (i, b) in img[p0 as usize * 512 - 4096..p0 as usize * 512].iter_mut().enumerate() {
        *b = i as u8 | 1;
    }
    img[p0 as usize * 512..p0 as usize * 512 + fat_b.len()].copy_from_slice(&fat_b);
    img[p1 as usize * 512..p1 as usize * 512 + ex_b.len()].copy_from_slice(&ex_b);
    let disk = RamDisk::from_bytes(img.clone());
    let mut c = vec![CacheSlot::EMPTY; 4];
    // Default: first FAT/exFAT partition.
    let mut vol = block_on(Volume::mount(disk, &mut c, MountOptions::default())).unwrap();
    assert_eq!(vol.kind(), FsKind::Fat32);
    assert_eq!(vol.base_lba(), p0);
    assert_eq!(vol.serial(), 0x1234_ABCD);
    block_on(async {
        let mut f = vol.open("/in fat partition.txt", OpenMode::CreateNew).await.unwrap();
        vol.write(&mut f, &[7u8; 9000]).await.unwrap();
        vol.close(f).await.unwrap();
    });
    let (disk, r) = block_on(vol.unmount());
    r.unwrap();
    // Second partition.
    let mut vol =
        block_on(Volume::mount(disk, &mut c, MountOptions { partition: Some(1), ..Default::default() }))
            .unwrap();
    assert_eq!(vol.kind(), FsKind::ExFat);
    block_on(async {
        let mut f = vol.open("/in exfat partition.txt", OpenMode::CreateNew).await.unwrap();
        vol.write(&mut f, &[9u8; 9000]).await.unwrap();
        vol.close(f).await.unwrap();
    });
    let (disk, r) = block_on(vol.unmount());
    r.unwrap();
    let out = disk.data;
    assert_eq!(&out[..p0 as usize * 512], &img[..p0 as usize * 512], "MBR/gap changed");
    let fat_end = p0 as usize * 512 + fat_b.len();
    assert_eq!(&out[fat_end..p1 as usize * 512], &img[fat_end..p1 as usize * 512]);
    std::fs::write(&fat, &out[p0 as usize * 512..fat_end]).unwrap();
    std::fs::write(&ex, &out[p1 as usize * 512..p1 as usize * 512 + ex_b.len()]).unwrap();
    assert_fsck(&fat, false);
    assert_fsck(&ex, true);
    assert!(mread(&fat, "/in fat partition.txt") == vec![7u8; 9000]);

    // GPT (protective MBR) is refused.
    let mut gpt = vec![0u8; 1 << 20];
    put_part(&mut gpt, 0, 0xEE, 1, 2047);
    gpt[510] = 0x55;
    gpt[511] = 0xAA;
    let r = block_on(Volume::mount(RamDisk::from_bytes(gpt), &mut c, MountOptions::default()));
    assert!(matches!(r, Err(Error::Unsupported)));
    // Blank media.
    let r = block_on(Volume::mount(RamDisk::new(4096), &mut c, MountOptions::default()));
    assert!(matches!(r, Err(Error::NoFilesystem)));
}

#[test]
fn exfat_with_mkfs_mbr() {
    let dir = scratch("exfat-mbr");
    let img = dir.join("fs.img");
    std::fs::File::create(&img).unwrap().set_len(64 << 20).unwrap();
    let ok = std::process::Command::new("mkfs.exfat")
        .args(["-q", "-P", "mbr"])
        .arg(&img)
        .status()
        .unwrap()
        .success();
    assert!(ok);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    assert_eq!(vol.kind(), FsKind::ExFat);
    // (mkfs.exfat writes no MBR into a regular file; this checks that the
    // option does not break anything, the MBR path is tested above.)
    write_all(&mut vol, "/a.txt", b"hello", &mut Rng(1)).unwrap();
    unmount(vol);
}

#[test]
fn write_prefix() {
    let dir = scratch("prefix");
    for fmt in [Fmt::Fat(32, 1, 64), Fmt::ExFat(4096, 32)] {
        let img = dir.join(format!("{}.img", fmt.label()));
        make_image(&img, fmt);
        // Prepare an outside file without a prefix.
        {
            let mut c = cache();
            let mut vol = mount(&img, &mut c);
            write_all(&mut vol, "/music.mp3", b"owner's music", &mut Rng(1)).unwrap();
            unmount(vol);
        }
        let mut c = cache();
        let dev = yoton_fs::host::FileDisk::open(&img).unwrap();
        let mut vol = block_on(Volume::mount(
            dev,
            &mut c,
            MountOptions { write_prefix: Some("/.yoton"), ..Default::default() },
        ))
        .unwrap();
        block_on(async {
            assert_eq!(vol.create_dir("/other").await, Err(Error::OutsidePrefix));
            assert_eq!(vol.create_dir("/.yotonx").await, Err(Error::OutsidePrefix));
            vol.create_dir("/.yoton").await.unwrap();
            vol.create_dir("/.YOTON/sub").await.unwrap();
            let f = vol.open("/.yoton/sub/ok.bin", OpenMode::CreateNew).await.unwrap();
            vol.close(f).await.unwrap();
            assert_eq!(vol.open("/new.bin", OpenMode::Create).await.err(), Some(Error::OutsidePrefix));
            assert_eq!(vol.open("/music.mp3", OpenMode::ReadWrite).await.err(), Some(Error::OutsidePrefix));
            assert_eq!(vol.remove("/music.mp3").await, Err(Error::OutsidePrefix));
            assert_eq!(vol.rename("/music.mp3", "/.yoton/stolen.mp3").await, Err(Error::OutsidePrefix));
            assert_eq!(vol.rename("/.yoton/sub/ok.bin", "/out.bin").await, Err(Error::OutsidePrefix));
            assert_eq!(vol.create_dir("/./.yoton/../x").await, Err(Error::InvalidName));
            // Reading outside is fine.
            let mut f = vol.open("/music.mp3", OpenMode::Read).await.unwrap();
            let mut b = [0u8; 13];
            vol.read(&mut f, &mut b).await.unwrap();
            assert_eq!(&b, b"owner's music");
            assert!(!f.is_writable());
            vol.rename("/.yoton/sub/ok.bin", "/.yoton/renamed.bin").await.unwrap();
        });
        let (_, r) = block_on(vol.unmount());
        r.unwrap();
        assert_fsck(&img, fmt.is_exfat());
    }
}

#[test]
fn path_rules() {
    let dir = scratch("paths");
    let img = dir.join("fs.img");
    make_image(&img, Fmt::Fat(16, 2, 32));
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    block_on(async {
        vol.create_dir("/a").await.unwrap();
        assert_eq!(vol.create_dir("/a/../b").await, Err(Error::InvalidName));
        assert_eq!(vol.open("/a/bad?.txt", OpenMode::Create).await.err(), Some(Error::InvalidName));
        assert_eq!(vol.open("/a/trailing.", OpenMode::Create).await.err(), Some(Error::InvalidName));
        let long: String = std::iter::repeat_n('é', 256).collect();
        assert_eq!(vol.open(&format!("/a/{long}"), OpenMode::Create).await.err(), Some(Error::NameTooLong));
        let ok: String = std::iter::repeat_n('é', 255).collect();
        let f = vol.open(&format!("/a/{ok}"), OpenMode::Create).await.unwrap();
        vol.close(f).await.unwrap();
        assert!(vol.exists(&format!("//a/./{}", ok.to_uppercase())).await.unwrap());
        let f = vol.open("a/x.txt", OpenMode::Create).await.unwrap();
        vol.close(f).await.unwrap();
        assert!(vol.exists("/A/X.TXT").await.unwrap());
        assert_eq!(vol.open("/a/x.txt/y", OpenMode::Read).await.err(), Some(Error::NotADirectory));
        let mut e = yoton_fs::DirEntry::new();
        vol.stat(&format!("/a/{ok}"), &mut e).await.unwrap();
        assert_eq!(e.name(), ok);
        // Root directory of FAT16 is fixed: fill it.
        let mut n = 0;
        loop {
            match vol.open(&format!("/root file number {n}.txt"), OpenMode::CreateNew).await {
                Ok(f) => vol.close(f).await.unwrap(),
                Err(Error::DirectoryFull) => break,
                Err(e) => panic!("{e:?}"),
            }
            n += 1;
        }
        assert!(n > 50);
    });
    unmount(vol);
    assert_fsck(&img, false);
}

/// Up-case table that is valid but not the recommended one (stored
/// uncompressed): the name hash must come from the volume's table.
#[test]
fn exfat_nonstandard_upcase() {
    let dir = scratch("upcase");
    let table = dir.join("upcase.bin");
    let mut t = Vec::with_capacity(131072);
    // Like the standard mapping, except that U+00E9 'é' stays lower case,
    // so a hash computed with the built-in table would differ.
    let up = |u: u16| if u == 0xE9 { u } else { yoton_fs::unicode::upcase16(u) };
    for u in 0..=0xFFFFu32 {
        t.extend_from_slice(&up(u as u16).to_le_bytes());
    }
    std::fs::write(&table, &t).unwrap();
    let img = dir.join("fs.img");
    std::fs::File::create(&img).unwrap().set_len(64 << 20).unwrap();
    let st = std::process::Command::new("mkfs.exfat")
        .args(["-q", "-P", "none"])
        .arg(format!("--upcase={}", table.display()))
        .arg(&img)
        .output()
        .unwrap();
    if !st.status.success() {
        eprintln!("mkfs.exfat --upcase not usable: {}", String::from_utf8_lossy(&st.stderr));
        return;
    }
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    let names = ["Ünïcødé", "Пётр", "Σίσυφος", "plain"];
    for n in names {
        write_all(&mut vol, &format!("/{n}.txt"), n.as_bytes(), &mut Rng(3)).unwrap();
    }
    unmount(vol);
    assert_fsck(&img, true);
    let listing = exfat_dump_dir(&img, "/");
    for n in names {
        assert!(listing.contains(n), "{n} missing from dump:\n{listing}");
        // Name hash as stored, compared with the spec algorithm over the
        // volume's (non-standard) table.
        let full = format!("{n}.txt");
        let out = std::process::Command::new("dump.exfat")
            .arg("-d")
            .arg(format!("/{full}"))
            .arg(&img)
            .output()
            .unwrap();
        let text = String::from_utf8_lossy(&out.stdout);
        let line = text.lines().find(|l| l.contains("NameHash")).expect("NameHash line");
        let hex = line.rsplit("0x").next().unwrap().trim();
        let stored = u16::from_str_radix(hex, 16).unwrap();
        let mut h: u16 = 0;
        for u in full.encode_utf16() {
            for b in up(u).to_le_bytes() {
                h = (if h & 1 != 0 { 0x8000u16 } else { 0 }).wrapping_add(h >> 1).wrapping_add(u16::from(b));
            }
        }
        assert_eq!(stored, h, "name hash of {full}");
    }
}

/// Two cache slots (the minimum) still work.
#[test]
fn tiny_cache() {
    let dir = scratch("tinycache");
    for fmt in [Fmt::Fat(12, 1, 2), Fmt::ExFat(512, 16)] {
        let img = dir.join(format!("{}.img", fmt.label()));
        make_image(&img, fmt);
        let mut c = vec![CacheSlot::EMPTY; 2];
        let dev = yoton_fs::host::FileDisk::open(&img).unwrap();
        let mut vol = block_on(Volume::mount(dev, &mut c, MountOptions::default())).unwrap();
        let mut rng = Rng(11);
        block_on(vol.create_dir("/d")).unwrap();
        for i in 0..30 {
            let data = rng.bytes(i * 333);
            write_all(&mut vol, &format!("/d/{}", unicode_name(i)), &data, &mut rng).unwrap();
            assert!(read_all(&mut vol, &format!("/d/{}", unicode_name(i))).unwrap() == data);
        }
        let (_, r) = block_on(vol.unmount());
        r.unwrap();
        assert_fsck(&img, fmt.is_exfat());
        let mut one = vec![CacheSlot::EMPTY; 1];
        let dev = yoton_fs::host::FileDisk::open(&img).unwrap();
        assert!(matches!(
            block_on(Volume::mount(dev, &mut one, MountOptions::default())),
            Err(Error::BufferTooSmall)
        ));
    }
}
