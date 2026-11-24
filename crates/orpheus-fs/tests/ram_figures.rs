//! Future (async state machine) sizes of the main calls, for the README.
//! Run with `--nocapture` to see them. Measured with a RAM device whose own
//! futures are small, so the numbers are what orpheus-fs itself adds.

mod common;

use std::mem::size_of_val;

fn sz<T>(t: T) -> usize {
    size_of_val(&t)
}

use common::*;
use orpheus_fs::host::{RamDisk, block_on};
use orpheus_fs::{CacheSlot, DirEntry, MountOptions, OpenMode, Volume};

#[test]
fn future_sizes() {
    let dir = scratch("ramfig");
    let img = dir.join("fs.img");
    make_image(&img, Fmt::Fat(16, 2, 32));
    let disk = RamDisk::load(&img).unwrap();
    let mut c = vec![CacheSlot::EMPTY; 4];
    let mfut = Volume::mount(disk.clone(), &mut c, MountOptions::default());
    let mount = size_of_val(&mfut);
    drop(mfut);
    let mut c2 = vec![CacheSlot::EMPTY; 4];
    let mut vol = block_on(Volume::mount(disk, &mut c2, MountOptions::default())).unwrap();
    let mut f = block_on(vol.open("/a", OpenMode::Create)).unwrap();
    let mut buf = [0u8; 16];
    let mut e = DirEntry::new();
    let mut cur = vol.root();
    let sizes = [
        ("mount", mount),
        ("open", sz(vol.open("/a/b/c", OpenMode::Read))),
        ("read", sz(vol.read(&mut f, &mut buf))),
        ("write", sz(vol.write(&mut f, &buf))),
        ("set_len", sz(vol.set_len(&mut f, 0))),
        ("flush_file", sz(vol.flush_file(&mut f))),
        ("read_dir", sz(vol.read_dir(&mut cur, &mut e))),
        ("create_dir", sz(vol.create_dir("/x"))),
        ("remove", sz(vol.remove("/x"))),
        ("rename", sz(vol.rename("/x", "/y"))),
        ("flush", sz(vol.flush())),
    ];
    for (n, s) in sizes {
        println!("future {n}: {s} B");
        assert!(s < 4096, "{n} future is {s} bytes");
    }
}
