//! Files created by Linux tools (mkfs.fat + mtools) read back byte-exact,
//! including a fragmented file mapped through the extent API.
//!
//! exFAT: no user-space tool on this host writes files into an exFAT image
//! (exfatprogs has none, no FUSE driver installed, loop mounts need root), so
//! exFAT is covered by round trips through yoton-fs plus `fsck.exfat` and
//! `dump.exfat` listings in the other tests.

mod common;

use std::collections::BTreeMap;

use common::*;
use yoton_fs::host::block_on;
use yoton_fs::{BlockDevice, OpenMode};

fn read_extents(vol: &mut Vol<'_>, path: &str) -> (Vec<u8>, usize) {
    block_on(async {
        let f = vol.open(path, OpenMode::Read).await.unwrap();
        let mut it = vol.extents(&f);
        let mut out = Vec::new();
        let mut runs = 0;
        let mut expect_off = 0u64;
        while let Some(e) = vol.next_extent(&mut it).await.unwrap() {
            assert_eq!(e.offset, expect_off);
            let mut buf = vec![[0u8; 512]; e.sectors as usize];
            vol.device().read(e.lba, &mut buf).await.unwrap();
            out.extend_from_slice(buf.as_flattened());
            expect_off += u64::from(e.sectors) * 512;
            runs += 1;
        }
        out.truncate(f.size() as usize);
        (out, runs)
    })
}

fn scenario(fmt: Fmt) {
    let dir = scratch(&format!("linux-{}", fmt.label()));
    let img = dir.join("fs.img");
    let host = dir.join("host");
    std::fs::create_dir_all(&host).unwrap();
    make_image(&img, fmt);
    let mut rng = Rng(0xDEAD_BEEF);
    let mut model: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    mmd(&img, "/Music");
    mmd(&img, "/Music/Артист — Альбом");
    mmd(&img, "/docs");
    let sizes = [0usize, 1, 100, 511, 512, 513, 4095, 4096, 4097, 33_000, 70_000];
    for (i, &sz) in sizes.iter().enumerate() {
        let name = match i % 4 {
            0 => unicode_name(i),
            1 => format!("SHORT{i}.TXT"),
            2 => format!("lower{i}.txt"),
            _ => format!("Mixed Case {i}.Ogg"),
        };
        let dest = match i % 3 {
            0 => format!("/Music/Артист — Альбом/{name}"),
            1 => format!("/docs/{name}"),
            _ => format!("/{name}"),
        };
        let data = rng.bytes(sz);
        let hp = host.join(format!("f{i}"));
        std::fs::write(&hp, &data).unwrap();
        mcopy_in(&img, &hp, &dest);
        model.insert(dest, data);
    }
    // Fragment free space: 30 small files, delete every other one, then
    // copy a big file that has to fill the holes.
    let cs = {
        let mut c = cache();
        let vol = mount(&img, &mut c);
        vol.cluster_size() as usize
    };
    for i in 0..30 {
        let hp = host.join(format!("small{i}"));
        std::fs::write(&hp, rng.bytes(cs + 1)).unwrap();
        mcopy_in(&img, &hp, &format!("/frag{i:02}.bin"));
    }
    for i in (0..30).step_by(2) {
        mdel(&img, &format!("/frag{i:02}.bin"));
    }
    for i in (1..30).step_by(2) {
        let p = format!("/frag{i:02}.bin");
        model.insert(p.clone(), std::fs::read(host.join(format!("small{i}"))).unwrap());
    }
    if matches!(fmt, Fmt::Fat(32, ..)) {
        // mtools allocates from the FSINFO next-free hint; point it at the
        // start so that the holes get used.
        let mut bytes = std::fs::read(&img).unwrap();
        let fsi = usize::from(u16::from_le_bytes([bytes[48], bytes[49]])) * 512;
        bytes[fsi + 492..fsi + 496].copy_from_slice(&2u32.to_le_bytes());
        std::fs::write(&img, &bytes).unwrap();
    }
    let big = rng.bytes(40 * cs + 77);
    let hp = host.join("big");
    std::fs::write(&hp, &big).unwrap();
    mcopy_in(&img, &hp, "/fragmented.bin");
    model.insert("/fragmented.bin".into(), big.clone());
    assert_fsck(&img, false);

    let before = std::fs::read(&img).unwrap();
    let mut c = cache();
    let mut dev = yoton_fs::host::FileDisk::open(&img).unwrap();
    dev.elide_zero_writes = false;
    let mut vol = block_on(yoton_fs::Volume::mount(
        dev,
        &mut c,
        yoton_fs::MountOptions { read_only: true, ..Default::default() },
    ))
    .unwrap();
    for (p, data) in &model {
        let got = read_all(&mut vol, p).unwrap_or_else(|e| panic!("{p}: {e:?}"));
        assert!(got == *data, "{p}: content differs");
    }
    // Directory listings contain exactly the names mtools wrote.
    let docs: Vec<String> = list(&mut vol, "/docs").into_iter().map(|x| x.0).collect();
    let want: Vec<String> =
        model.keys().filter_map(|k| k.strip_prefix("/docs/").map(str::to_string)).collect();
    let mut d2 = docs.clone();
    d2.sort();
    let mut w2 = want.clone();
    w2.sort();
    assert_eq!(d2, w2);
    // Extents of the fragmented file.
    let (raw, runs) = read_extents(&mut vol, "/fragmented.bin");
    assert!(raw == big, "extent read differs");
    assert!(runs > 1, "expected fragmentation, got {runs} run(s)");
    let f = block_on(vol.open("/fragmented.bin", OpenMode::Read)).unwrap();
    assert!(!block_on(vol.is_contiguous(&f)).unwrap());
    let f = block_on(vol.open("/frag01.bin", OpenMode::Read)).unwrap();
    assert!(block_on(vol.is_contiguous(&f)).unwrap());
    // Read-only mode refuses writes and changes nothing.
    assert_eq!(block_on(vol.open("/new", OpenMode::Create)).err(), Some(yoton_fs::Error::ReadOnly));
    assert_eq!(block_on(vol.create_dir("/x")), Err(yoton_fs::Error::ReadOnly));
    let (_, r) = block_on(vol.unmount());
    r.unwrap();
    assert!(std::fs::read(&img).unwrap() == before, "read-only mount modified the image");
}

#[test]
fn linux_fat12() {
    scenario(Fmt::Fat(12, 4, 4));
}

#[test]
fn linux_fat16() {
    scenario(Fmt::Fat(16, 2, 32));
}

#[test]
fn linux_fat32_512() {
    scenario(Fmt::Fat(32, 1, 64));
}

#[test]
fn linux_fat32_4k() {
    scenario(Fmt::Fat(32, 8, 300));
}
