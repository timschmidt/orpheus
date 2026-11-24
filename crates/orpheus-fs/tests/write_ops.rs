//! Mutating operations on every format of the matrix, checked against an
//! in-memory model, `fsck -n`, and (FAT) mtools reading the files back.

mod common;

use std::collections::BTreeMap;

use common::*;
use orpheus_fs::host::block_on;
use orpheus_fs::{Error, OpenMode, SeekFrom};

fn verify(vol: &mut Vol<'_>, model: &BTreeMap<String, Vec<u8>>, dirs: &[&str]) {
    for (p, data) in model {
        let got = read_all(vol, p).unwrap_or_else(|e| panic!("read {p}: {e:?}"));
        assert!(got == *data, "content mismatch for {p} ({} vs {} bytes)", got.len(), data.len());
    }
    // Every listed file is in the model and vice versa.
    for d in dirs {
        for (name, is_dir, size) in list(vol, d) {
            if is_dir {
                continue;
            }
            let p = if *d == "/" { format!("/{name}") } else { format!("{d}/{name}") };
            let m = model.get(&p).unwrap_or_else(|| panic!("unexpected file {p}"));
            assert_eq!(m.len() as u64, size, "{p}");
        }
    }
}

fn scenario(fmt: Fmt) {
    let dir = scratch(&format!("ops-{}", fmt.label()));
    let img = dir.join("fs.img");
    make_image(&img, fmt);
    let mut rng = Rng(0x9E37_79B9_7F4A_7C15);
    let mut model: BTreeMap<String, Vec<u8>> = BTreeMap::new();
    let dirs = ["/", "/music", "/music/Пётр Ильич", "/.orpheus", "/.orpheus/cache", "/moved"];
    let cs;
    {
        let mut c = cache();
        let mut vol = mount(&img, &mut c);
        cs = vol.cluster_size() as usize;
        for d in &dirs[1..] {
            block_on(vol.create_dir(d)).unwrap_or_else(|e| panic!("mkdir {d}: {e:?}"));
        }
        assert_eq!(block_on(vol.create_dir("/music")), Err(Error::AlreadyExists));
        assert_eq!(block_on(vol.create_dir("/MUSIC")), Err(Error::AlreadyExists));
        assert_eq!(block_on(vol.create_dir("/nope/sub")), Err(Error::NotFound));
        let sizes = [0, 1, 511, 512, 513, cs - 1, cs, cs + 1, 3 * cs + 17, 100_000, 300_000];
        for (i, &sz) in sizes.iter().enumerate() {
            let p = match i % 3 {
                0 => format!("/music/Пётр Ильич/{}", unicode_name(i)),
                1 => format!("/FILE{i}.BIN"),
                _ => format!("/.orpheus/cache/data {i}.bin"),
            };
            let data = rng.bytes(sz);
            write_all(&mut vol, &p, &data, &mut rng).unwrap_or_else(|e| panic!("write {p}: {e:?}"));
            model.insert(p, data);
        }
        // lower-case 8.3 name (stored with NT case flags on FAT)
        let data = rng.bytes(1234);
        write_all(&mut vol, "/readme.txt", &data, &mut rng).unwrap();
        model.insert("/readme.txt".into(), data);

        block_on(async {
            // Overwrite inside a file.
            let p = "/music/Пётр Ильич/".to_string() + &unicode_name(9);
            let mut f = vol.open(&p, OpenMode::ReadWrite).await.unwrap();
            let patch = rng.bytes(5000);
            f.seek(SeekFrom::Start(777));
            vol.write(&mut f, &patch).await.unwrap();
            vol.close(f).await.unwrap();
            model.get_mut(&p).unwrap()[777..777 + 5000].copy_from_slice(&patch);

            // Write past the end: the gap reads as zeros.
            let mut f = vol.open("/FILE1.BIN", OpenMode::ReadWrite).await.unwrap();
            f.seek(SeekFrom::End(3000));
            let tail = rng.bytes(100);
            vol.write(&mut f, &tail).await.unwrap();
            vol.close(f).await.unwrap();
            let m = model.get_mut("/FILE1.BIN").unwrap();
            m.extend(std::iter::repeat_n(0u8, 3000));
            m.extend_from_slice(&tail);

            // Truncate and extend.
            let p = "/.orpheus/cache/data 8.bin";
            let mut f = vol.open(p, OpenMode::ReadWrite).await.unwrap();
            vol.set_len(&mut f, 10).await.unwrap();
            vol.set_len(&mut f, 2 * cs as u64 + 5).await.unwrap();
            vol.close(f).await.unwrap();
            let m = model.get_mut(p).unwrap();
            m.truncate(10);
            m.resize(2 * cs + 5, 0);

            let p = "/music/Пётр Ильич/".to_string() + &unicode_name(6);
            let mut f = vol.open(&p, OpenMode::ReadWrite).await.unwrap();
            vol.set_len(&mut f, 0).await.unwrap();
            vol.close(f).await.unwrap();
            model.get_mut(&p).unwrap().clear();

            // Open modes.
            assert_eq!(vol.open("/FILE1.BIN", OpenMode::CreateNew).await.err(), Some(Error::AlreadyExists));
            assert_eq!(vol.open("/missing", OpenMode::Read).await.err(), Some(Error::NotFound));
            assert_eq!(vol.open("/music", OpenMode::Read).await.err(), Some(Error::IsADirectory));
            let f = vol.open("/empty.dat", OpenMode::Create).await.unwrap();
            vol.close(f).await.unwrap();
            model.insert("/empty.dat".into(), Vec::new());

            // Case-insensitive lookup.
            let mut f = vol.open("/file4.bin", OpenMode::Read).await.unwrap();
            assert_eq!(f.size(), model["/FILE4.BIN"].len() as u64);
            let mut b = [0u8; 4];
            vol.read(&mut f, &mut b).await.unwrap();
            let up = "/MUSIC/ПЁТР ИЛЬИЧ/".to_string() + &unicode_name(3).to_uppercase();
            assert!(vol.exists(&up).await.unwrap(), "{up}");

            // Renames: within a directory, across directories, case only,
            // and over an existing file.
            vol.rename("/FILE7.BIN", "/renamed seven.bin").await.unwrap();
            let d = model.remove("/FILE7.BIN").unwrap();
            model.insert("/renamed seven.bin".into(), d);
            vol.rename("/FILE10.BIN", "/moved/ten.bin").await.unwrap();
            let d = model.remove("/FILE10.BIN").unwrap();
            model.insert("/moved/ten.bin".into(), d);
            vol.rename("/readme.txt", "/README.txt").await.unwrap();
            let d = model.remove("/readme.txt").unwrap();
            model.insert("/README.txt".into(), d);
            vol.rename("/FILE4.BIN", "/moved/ten.bin").await.unwrap();
            let d = model.remove("/FILE4.BIN").unwrap();
            model.insert("/moved/ten.bin".into(), d);
            // Move a directory.
            vol.create_dir("/music/sub").await.unwrap();
            vol.open("/music/sub/inner.txt", OpenMode::Create).await.unwrap();
            vol.rename("/music/sub", "/moved/sub2").await.unwrap();
            model.insert("/moved/sub2/inner.txt".into(), Vec::new());
            assert_eq!(vol.rename("/moved", "/moved/sub2/x").await, Err(Error::InvalidName));

            // Deletes.
            vol.remove("/FILE1.BIN").await.unwrap();
            model.remove("/FILE1.BIN");
            assert_eq!(vol.remove("/music").await, Err(Error::DirectoryNotEmpty));
            vol.create_dir("/gone").await.unwrap();
            vol.remove("/gone").await.unwrap();
            assert_eq!(vol.remove("/gone").await, Err(Error::NotFound));

            // Re-use freed space.
            let data = rng.bytes(4 * cs + 3);
            let mut f = vol.open("/after delete.bin", OpenMode::CreateNew).await.unwrap();
            vol.write(&mut f, &data).await.unwrap();
            vol.close(f).await.unwrap();
            model.insert("/after delete.bin".into(), data);
            vol.flush().await.unwrap();
        });
        let free_before = block_on(vol.free_clusters()).unwrap();
        verify(&mut vol, &model, &dirs);
        unmount(vol);
        // Free count after a fresh mount equals the running count.
        let mut c = cache();
        let mut vol = mount(&img, &mut c);
        let fresh = block_on(vol.free_clusters()).unwrap();
        assert_eq!(fresh, free_before, "free cluster accounting");
        unmount(vol);
    }
    assert_fsck(&img, fmt.is_exfat());
    let mut c = cache();
    let mut vol = mount(&img, &mut c);
    let mut all_dirs: Vec<&str> = dirs.to_vec();
    all_dirs.push("/moved/sub2");
    verify(&mut vol, &model, &all_dirs);
    unmount(vol);
    if !fmt.is_exfat() {
        // Independent reader.
        for (p, data) in &model {
            assert!(mread(&img, p) == *data, "mtools reads {p} differently");
        }
    } else {
        let listing = exfat_dump_dir(&img, "/music/Пётр Ильич");
        assert!(listing.contains(&unicode_name(0)), "dump.exfat listing:\n{listing}");
    }
}

macro_rules! fmt_tests {
    ($($name:ident: $fmt:expr,)*) => {$(
        #[test]
        fn $name() {
            scenario($fmt);
        }
    )*};
}

fmt_tests! {
    ops_fat12_s1: Fmt::Fat(12, 1, 2),
    ops_fat12_s8: Fmt::Fat(12, 8, 8),
    ops_fat16_s1: Fmt::Fat(16, 1, 16),
    ops_fat16_s8: Fmt::Fat(16, 8, 64),
    ops_fat32_s1: Fmt::Fat(32, 1, 64),
    ops_fat32_s8: Fmt::Fat(32, 8, 300),
    ops_exfat_512: Fmt::ExFat(512, 16),
    ops_exfat_4k: Fmt::ExFat(4096, 64),
    ops_exfat_32k: Fmt::ExFat(32768, 128),
}
