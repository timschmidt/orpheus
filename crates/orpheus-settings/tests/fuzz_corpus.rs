//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-settings --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/settings_mount/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use bodies::{SECTORS, SimNor};
use embassy_futures::block_on;
use orpheus_settings::{SECTOR, Store};

#[test]
fn settings_mount_corpus() {
    replay::replay("settings_mount", bodies::settings_mount);
}

/// The region after `sets` (key, value length) and `removes`, trailing
/// erased bytes cut (the target pads them back).
fn image(sets: &[(u16, usize)], removes: &[u16], cut: Option<u32>) -> Vec<u8> {
    let nor = SimNor { mem: vec![0xFF; SECTORS * SECTOR as usize], budget: None };
    let mut s = block_on(Store::<SimNor, SECTORS>::mount(nor)).unwrap();
    for (i, &(k, n)) in sets.iter().enumerate() {
        block_on(s.set(k, &vec![(i % 251) as u8; n])).unwrap();
    }
    for &k in removes {
        block_on(s.remove(k)).unwrap();
    }
    if let Some(c) = cut {
        let mut nor = s.into_inner();
        nor.budget = Some(c);
        s = block_on(Store::mount(nor)).unwrap();
        let _ = block_on(s.set(0x0304, &[9; 300]));
    }
    let mut m = s.into_inner().mem;
    while m.last() == Some(&0xFF) {
        m.pop();
    }
    m
}

fn seeds() -> Vec<(String, Vec<u8>)> {
    let with = |ops: &[u8], img: Vec<u8>| {
        let mut d = vec![ops.len() as u8];
        d.extend_from_slice(ops);
        d.extend(img);
        d
    };
    let ops: &[u8] = &[0, 1, 20, 1, 2, 200, 0x80, 3, 50, 3, 1, 4, 5, 2, 3, 40, 5, 1, 5, 9, 0x82, 4, 60, 4];
    // Firmware-like keys: settings ids, Bluetooth record, resume data.
    let fw: Vec<(u16, usize)> = vec![(0x0101, 2), (0x0304, 2), (0x0603, 2), (0x0700, 120), (0x0800, 64)];
    let churn: Vec<(u16, usize)> = (0..400).map(|i| ((i % 9) as u16, 1 + (i * 37) % 180)).collect();
    // More keys than the 128-key index held (VER-4, 2026-11-26), the
    // fuzzer's keys 0..16 among them.
    let many: Vec<(u16, usize)> = (0..180u16).map(|k| (k, 2)).collect();
    vec![
        ("erased-ops".into(), with(ops, Vec::new())),
        ("fw".into(), with(&[], image(&fw, &[], None))),
        ("fw-ops".into(), with(ops, image(&fw, &[0x0603], None))),
        ("churn".into(), with(ops, image(&churn, &[3, 4], None))),
        (
            "big".into(),
            with(&[1, 9, 255, 0x81, 10, 255], image(&[(1, 1024), (2, 1024), (3, 900)], &[], None)),
        ),
        ("torn".into(), with(ops, image(&fw, &[], Some(2)))),
        ("torn-compact".into(), with(&[], image(&churn[..60], &[], Some(5)))),
        ("many-keys".into(), with(ops, image(&many, &[3, 100], None))),
    ]
}

#[test]
fn write_seeds() {
    let s = seeds();
    for (_, d) in &s {
        assert!(d.len() <= 64 * 1024);
        bodies::settings_mount(d);
    }
    replay::write_seeds("settings_mount", &s);
}
