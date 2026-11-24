//! Stable-toolchain smoke run of the fuzz harness (`fuzz/`): mutated images
//! must never panic or hang. Also writes the seed images the cargo-fuzz
//! target loads (`target/tmp/orpheus-fs/fuzz-seeds/*.img`) and replays the
//! regression inputs in `fuzz/regress/`.

mod common;

use common::*;
use orpheus_fs::host::{RamDisk, block_on, fuzz_one};
use orpheus_fs::{CacheSlot, MountOptions, OpenMode, Volume};

fn seed(fmt: Fmt, size_kib: u64, name: &str, out: &std::path::Path) -> Vec<u8> {
    let path = out.join(format!("{name}.img"));
    std::fs::File::create(&path).unwrap().set_len(size_kib << 10).unwrap();
    let ok = match fmt {
        Fmt::Fat(bits, spc, _) => std::process::Command::new("mkfs.fat")
            .args(["-F", &bits.to_string(), "-s", &spc.to_string(), "-r", "32"])
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success(),
        Fmt::ExFat(c, _) => std::process::Command::new("mkfs.exfat")
            .args(["-q", "-c", &c.to_string()])
            .arg(&path)
            .output()
            .unwrap()
            .status
            .success(),
    };
    assert!(ok, "mkfs {name}");
    // Populate: nested dirs, long names, a fragmented file.
    let disk = RamDisk::load(&path).unwrap();
    let mut c = vec![CacheSlot::EMPTY; 4];
    let mut vol = block_on(Volume::mount(disk, &mut c, MountOptions::default())).unwrap();
    block_on(async {
        vol.create_dir("/Music").await.unwrap();
        vol.create_dir("/Music/Пётр Ильич").await.unwrap();
        let mut a = vol.open("/Music/Пётр Ильич/frag a.bin", OpenMode::CreateNew).await.unwrap();
        let mut b = vol.open("/frag b.bin", OpenMode::CreateNew).await.unwrap();
        for k in 0..6u8 {
            vol.write(&mut a, &[k; 700]).await.unwrap();
            vol.write(&mut b, &[!k; 900]).await.unwrap();
        }
        vol.close(a).await.unwrap();
        vol.close(b).await.unwrap();
        for i in 0..12 {
            let f = vol.open(&format!("/Music/{}", unicode_name(i)), OpenMode::CreateNew).await.unwrap();
            vol.close(f).await.unwrap();
        }
        let f = vol.open("/README.TXT", OpenMode::CreateNew).await.unwrap();
        vol.close(f).await.unwrap();
    });
    let (disk, r) = block_on(vol.unmount());
    r.unwrap();
    disk.save(&path).unwrap();
    assert_fsck(&path, fmt.is_exfat());
    disk.data
}

#[test]
fn fuzz_smoke() {
    let out = std::path::PathBuf::from(env!("CARGO_TARGET_TMPDIR")).join("orpheus-fs").join("fuzz-seeds");
    std::fs::create_dir_all(&out).unwrap();
    let bases = [
        seed(Fmt::Fat(12, 1, 0), 128, "fat12", &out),
        seed(Fmt::Fat(16, 1, 0), 4096, "fat16", &out),
        seed(Fmt::Fat(32, 1, 0), 33 * 1024, "fat32", &out),
        seed(Fmt::ExFat(512, 0), 3072, "exfat", &out),
    ];
    let refs: Vec<&[u8]> = bases.iter().map(Vec::as_slice).collect();
    // Unmutated bases walk cleanly.
    for b in &refs {
        let mut d = orpheus_fs::host::OverlayDisk::new(b);
        let mut c = vec![CacheSlot::EMPTY; 3];
        let mut v = block_on(Volume::mount(&mut d, &mut c, MountOptions::default())).unwrap();
        let (_, files, _) = orpheus_fs::host::exercise(&mut v, 100_000).unwrap();
        assert!(files >= 14);
    }
    // Regression inputs.
    let regress = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("fuzz").join("regress");
    if let Ok(rd) = std::fs::read_dir(&regress) {
        for e in rd.flatten() {
            let data = std::fs::read(e.path()).unwrap();
            fuzz_one(&refs, &data);
        }
    }
    // Directed: hostile cluster numbers and sizes in real directory entries.
    let edge32: [u32; 7] = [0, 1, 2, 3, 0x0FFF_FFF7, 0x0FFF_FFFF, 0xFFFF_FFFF];
    let mut directed = [0usize; 4];
    for (bi, b) in refs.iter().enumerate() {
        let window = b.len().min(1 << 24);
        for off in (0..window).step_by(32) {
            let e = &b[off..off + 32];
            let fat_entry = bi < 3
                && e[0] > b' '
                && e[0] != 0xE5
                && e[11] & 0x0F != 0x0F
                && e[11] & 0xC8 == 0
                && e[..11].iter().all(|&c| c >= b' ');
            let ex_stream = bi == 3 && e[0] == 0xC0;
            if !fat_entry && !ex_stream {
                continue;
            }
            directed[bi] += 1;
            for &v in &edge32 {
                for field in 0..2 {
                    let mut data = vec![bi as u8 | 0x80];
                    let mut patch = |o: usize, x: u8| {
                        let p = (off + o) as u32;
                        data.extend_from_slice(&[p as u8, (p >> 8) as u8, (p >> 16) as u8, x]);
                    };
                    let bytes = v.to_le_bytes();
                    if ex_stream {
                        let base = if field == 0 { 20 } else { 24 };
                        for (k, &x) in bytes.iter().enumerate() {
                            patch(base + k, x);
                        }
                    } else if field == 0 {
                        patch(26, bytes[0]);
                        patch(27, bytes[1]);
                        patch(20, bytes[2]);
                        patch(21, bytes[3]);
                    } else {
                        for (k, &x) in bytes.iter().enumerate() {
                            patch(28 + k, x);
                        }
                    }
                    fuzz_one(&refs, &data);
                }
            }
        }
    }
    assert!(directed.iter().all(|&n| n >= 3), "directed cases per image: {directed:?}");
    // Random mutations, biased to the first sectors and directory areas.
    let mut rng = Rng(0x5EED);
    let iters: usize =
        std::env::var("ORPHEUS_FS_FUZZ_ITERS").ok().and_then(|s| s.parse().ok()).unwrap_or(3000);
    for _ in 0..iters {
        let mut data = vec![rng.next() as u8];
        let patches = 1 + rng.below(24) as usize;
        for _ in 0..patches {
            let off = match rng.below(3) {
                0 => rng.below(1024),
                1 => rng.below(64 * 1024),
                _ => rng.below(1 << 24),
            } as u32;
            let val = match rng.below(4) {
                0 => 0,
                1 => 0xFF,
                _ => rng.next() as u8,
            };
            data.extend_from_slice(&[off as u8, (off >> 8) as u8, (off >> 16) as u8, val]);
        }
        fuzz_one(&refs, &data);
    }
}
