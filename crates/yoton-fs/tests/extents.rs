//! Extent mapping: files fragmented by interleaved appends, contiguous
//! scratch files used through raw LBAs.

mod common;

use common::*;
use yoton_fs::host::block_on;
use yoton_fs::{BlockDevice, OpenMode};

fn extents_of(vol: &mut Vol<'_>, path: &str) -> Vec<yoton_fs::Extent> {
    block_on(async {
        let f = vol.open(path, OpenMode::Read).await.unwrap();
        let mut it = vol.extents(&f);
        let mut v = Vec::new();
        while let Some(e) = vol.next_extent(&mut it).await.unwrap() {
            v.push(e);
        }
        v
    })
}

fn raw_read(vol: &mut Vol<'_>, ext: &[yoton_fs::Extent], size: usize) -> Vec<u8> {
    block_on(async {
        let mut out = Vec::new();
        for e in ext {
            let mut b = vec![[0u8; 512]; e.sectors as usize];
            vol.device().read(e.lba, &mut b).await.unwrap();
            out.extend_from_slice(b.as_flattened());
        }
        out.truncate(size);
        out
    })
}

fn interleaved(fmt: Fmt) {
    let dir = scratch(&format!("extents-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let mut rng = Rng(5);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    let cs = vol.cluster_size() as usize;
    let a = rng.bytes(20 * cs);
    let b = rng.bytes(20 * cs + 100);
    block_on(async {
        let mut fa = vol.open("/a.bin", OpenMode::CreateNew).await.unwrap();
        let mut fb = vol.open("/b.bin", OpenMode::CreateNew).await.unwrap();
        for k in 0..20 {
            vol.write(&mut fa, &a[k * cs..(k + 1) * cs]).await.unwrap();
            let end = if k == 19 { b.len() } else { (k + 1) * cs };
            vol.write(&mut fb, &b[k * cs..end]).await.unwrap();
        }
        vol.close(fa).await.unwrap();
        vol.close(fb).await.unwrap();
    });
    vol = {
        unmount(vol);
        mount(&img, &mut c)
    };
    let ea = extents_of(&mut vol, "/a.bin");
    let eb = extents_of(&mut vol, "/b.bin");
    assert!(ea.len() >= 10, "a: {} runs", ea.len());
    assert!(eb.len() >= 10, "b: {} runs", eb.len());
    assert!(raw_read(&mut vol, &ea, a.len()) == a);
    assert!(raw_read(&mut vol, &eb, b.len()) == b);
    assert!(read_all(&mut vol, "/a.bin").unwrap() == a);
    assert!(read_all(&mut vol, "/b.bin").unwrap() == b);
    // Last extent is cut to the file size.
    let total: u64 = eb.iter().map(|e| u64::from(e.sectors)).sum();
    assert_eq!(total, (b.len() as u64).div_ceil(512));
    unmount(vol);
    assert_fsck(&img, fmt.is_exfat());
}

/// A contiguous file in /.yoton written through raw LBAs, then read back
/// through the file API (the hardware scratch-area pattern).
fn raw_scratch(fmt: Fmt) {
    let dir = scratch(&format!("rawscratch-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    let len = 256 * 1024usize;
    let pattern: Vec<u8> = (0..len).map(|i| (i * 7 + 3) as u8).collect();
    block_on(async {
        vol.create_dir("/.yoton").await.unwrap();
        let mut f = vol.open("/.yoton/scratch.bin", OpenMode::CreateNew).await.unwrap();
        vol.set_len(&mut f, len as u64).await.unwrap();
        vol.close(f).await.unwrap();
    });
    let ext = extents_of(&mut vol, "/.yoton/scratch.bin");
    assert_eq!(ext.len(), 1, "scratch file not contiguous");
    let e = ext[0];
    assert_eq!(e.sectors as usize, len / 512);
    block_on(async {
        let (chunks, _) = pattern.as_chunks::<512>();
        vol.device().write(e.lba, chunks).await.unwrap();
        vol.invalidate_cached(e.lba, e.sectors);
    });
    let got = read_all(&mut vol, "/.yoton/scratch.bin").unwrap();
    if fmt.is_exfat() {
        // exFAT: extended by allocation only, so the valid-data length is 0
        // and the raw data is not visible through the file API ...
        assert!(got.iter().all(|&b| b == 0));
        // ... until the file is declared valid.
        block_on(async {
            let mut f = vol.open("/.yoton/scratch.bin", OpenMode::ReadWrite).await.unwrap();
            vol.mark_valid(&mut f).await.unwrap();
            vol.close(f).await.unwrap();
        });
        let got = read_all(&mut vol, "/.yoton/scratch.bin").unwrap();
        assert!(got == pattern);
    } else {
        assert!(got == pattern);
    }
    unmount(vol);
    assert_fsck(&img, fmt.is_exfat());
}

#[test]
fn extents_fat32() {
    interleaved(Fmt::Fat(32, 1, 64));
}

#[test]
fn extents_fat16() {
    interleaved(Fmt::Fat(16, 4, 64));
}

#[test]
fn extents_exfat() {
    interleaved(Fmt::ExFat(4096, 64));
}

#[test]
fn extents_exfat_512() {
    interleaved(Fmt::ExFat(512, 16));
}

#[test]
fn raw_scratch_fat32() {
    raw_scratch(Fmt::Fat(32, 8, 300));
}

#[test]
fn raw_scratch_exfat() {
    raw_scratch(Fmt::ExFat(4096, 64));
}
