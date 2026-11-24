//! exFAT files larger than 4 GiB (sparse image; all-zero writes are elided
//! by the host device so the image stays small on disk). FAT refuses them.

mod common;

use common::*;
use yoton_fs::host::block_on;
use yoton_fs::{Error, OpenMode, SeekFrom};

const MIB: u64 = 1 << 20;
const SIZE: u64 = 4 * 1024 * MIB + 300 * MIB + 12345; // > 4 GiB, odd tail

fn marker(mib: u64) -> [u8; 512] {
    let mut s = [0u8; 512];
    s[..8].copy_from_slice(&mib.to_le_bytes());
    s[8..16].copy_from_slice(b"YOTONMIB");
    s[511] = 0xA5;
    s
}

#[test]
fn exfat_file_over_4gib() {
    let dir = scratch("large");
    let img = dir.join("fs.img");
    make_image(&img, Fmt::ExFat(131072, 12 * 1024));
    let zeros = vec![0u8; MIB as usize - 512];
    {
        let mut c = cache();
        let mut vol = mount(&img, &mut c);
        block_on(async {
            let mut f = vol.open("/big.img", OpenMode::CreateNew).await.unwrap();
            let mut mib = 0;
            while f.position() + MIB <= SIZE {
                vol.write(&mut f, &marker(mib)).await.unwrap();
                vol.write(&mut f, &zeros).await.unwrap();
                mib += 1;
            }
            let tail = (SIZE - f.position()) as usize;
            let mut t = vec![0x5Au8; tail];
            t[..512].copy_from_slice(&marker(mib));
            vol.write(&mut f, &t).await.unwrap();
            assert_eq!(f.size(), SIZE);
            vol.close(f).await.unwrap();

            // Extended by allocation only: reads as zeros past the valid length.
            let mut g = vol.open("/prealloc.bin", OpenMode::CreateNew).await.unwrap();
            vol.set_len(&mut g, 5 * 1024 * MIB).await.unwrap();
            vol.write(&mut g, b"head").await.unwrap();
            vol.close(g).await.unwrap();
        });
        unmount(vol);
    }
    assert_fsck(&img, true);
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    block_on(async {
        let mut f = vol.open("/BIG.IMG", OpenMode::Read).await.unwrap();
        assert_eq!(f.size(), SIZE);
        for mib in [0u64, 1, 4095, 4096, 4097, 4300] {
            f.seek(SeekFrom::Start(mib * MIB));
            let mut s = [0u8; 512];
            assert_eq!(vol.read(&mut f, &mut s).await.unwrap(), 512);
            assert_eq!(s, marker(mib), "marker at {mib} MiB");
            let mut z = [1u8; 1000];
            vol.read(&mut f, &mut z).await.unwrap();
            assert!(z.iter().all(|&b| b == 0));
        }
        // Unaligned read across the 4 GiB boundary.
        f.seek(SeekFrom::Start(4096 * MIB - 3));
        let mut s = [0u8; 20];
        vol.read(&mut f, &mut s).await.unwrap();
        assert_eq!(&s[3..11], &4096u64.to_le_bytes());
        // Tail.
        f.seek(SeekFrom::End(-10));
        let mut s = [0u8; 64];
        assert_eq!(vol.read(&mut f, &mut s).await.unwrap(), 10);
        assert!(s[..10].iter().all(|&b| b == 0x5A));
        // One contiguous run.
        assert!(vol.is_contiguous(&f).await.unwrap());
        let mut it = vol.extents(&f);
        let e = vol.next_extent(&mut it).await.unwrap().unwrap();
        assert_eq!(u64::from(e.sectors), SIZE.div_ceil(512));
        assert!(vol.next_extent(&mut it).await.unwrap().is_none());

        let mut g = vol.open("/prealloc.bin", OpenMode::Read).await.unwrap();
        assert_eq!(g.size(), 5 * 1024 * MIB);
        let mut h = [0u8; 4];
        vol.read(&mut g, &mut h).await.unwrap();
        assert_eq!(&h, b"head");
        g.seek(SeekFrom::Start(4500 * MIB));
        let mut z = [9u8; 4096];
        assert_eq!(vol.read(&mut g, &mut z).await.unwrap(), 4096);
        assert!(z.iter().all(|&b| b == 0));
    });
    unmount(vol);
    let on_disk = std::fs::metadata(&img).unwrap();
    assert!(on_disk.len() > 8 << 30);
}

#[test]
fn fat32_refuses_4gib() {
    let dir = scratch("large-fat");
    let img = dir.join("fs.img");
    make_image(&img, Fmt::Fat(32, 8, 300));
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    block_on(async {
        let mut f = vol.open("/x", OpenMode::CreateNew).await.unwrap();
        assert_eq!(vol.set_len(&mut f, 1 << 32).await, Err(Error::FileTooLarge));
        f.seek(SeekFrom::Start(u64::from(u32::MAX)));
        assert_eq!(vol.write(&mut f, b"x").await, Err(Error::FileTooLarge));
        vol.close(f).await.unwrap();
    });
    unmount(vol);
    assert_fsck(&img, false);
}
