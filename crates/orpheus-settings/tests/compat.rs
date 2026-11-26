//! On-disk compatibility of the settings store across index sizes (VER-4,
//! 2026-11-26: `MAX_KEYS` 128 → 192 with a 4-byte index slot).
//!
//! `tests/data/store-128keys-c3041aeb.bin` is the whole four-sector region
//! as the code at commit `c3041aeb` (`MAX_KEYS` 128, 8-byte slots) left it
//! after [`workload`]: 125 two-byte settings, a 6-byte Bluetooth address,
//! the radio presets (332 bytes) and the retired Brightness key, all 128
//! of the old index; then churn with several compactions and deletions.
//! `ORPHEUS_WRITE_FIXTURE=1 cargo test -p orpheus-settings --test compat`
//! rewrites it (only meaningful with the old code checked out).

#[allow(dead_code)]
#[path = "../fuzz/src/lib.rs"]
mod bodies;

use std::collections::BTreeMap;

use bodies::{SECTORS, SimNor};
use embassy_futures::block_on;
use orpheus_settings::{Error, MAX_KEYS, MAX_VALUE, SECTOR, Store};

const FIXTURE: &str = "tests/data/store-128keys-c3041aeb.bin";

/// The settings keys of the workload: 125 two-byte values over the
/// firmware's high bytes (none equal to the other three keys).
fn setting_keys() -> Vec<u16> {
    (0..125u16).map(|i| 0x0100 * (1 + i / 10) + 0x20 + i % 10).collect()
}
const BT: u16 = 0x06F0;
const PRESETS: u16 = 0x0710;
const RETIRED: u16 = 0x0301;

/// One step of a small LCG (deterministic churn).
fn lcg(s: &mut u32) -> u32 {
    *s = s.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
    *s >> 8
}

/// The operations: `(key, Some(value))` sets, `(key, None)` removes.
pub fn workload() -> Vec<(u16, Option<Vec<u8>>)> {
    let keys = setting_keys();
    let mut ops = Vec::new();
    // A factory reset: every key written once.
    for (i, &k) in keys.iter().enumerate() {
        ops.push((k, Some((i as u16 * 7).to_le_bytes().to_vec())));
    }
    ops.push((BT, Some(vec![0x11, 0x22, 0x33, 0x44, 0x55, 0x66])));
    ops.push((PRESETS, Some((0..332).map(|i| (i * 3) as u8).collect())));
    ops.push((RETIRED, Some(10u16.to_le_bytes().to_vec())));
    // Churn: settings changes, preset and Bluetooth saves, removes.
    let mut s = 0x0E4F_u32;
    for _ in 0..700 {
        let r = lcg(&mut s);
        let k = keys[(r % keys.len() as u32) as usize];
        match lcg(&mut s) % 20 {
            0 => ops.push((PRESETS, Some((0..2 + r % 331).map(|i| (i ^ r) as u8).collect()))),
            1 => ops.push((BT, Some(r.to_le_bytes().iter().chain(&[1, 2]).copied().collect()))),
            2 => ops.push((k, None)),
            _ => ops.push((k, Some(((r >> 4) as u16).to_le_bytes().to_vec()))),
        }
    }
    ops
}

/// What the store must hold after [`workload`].
fn model() -> BTreeMap<u16, Vec<u8>> {
    let mut m = BTreeMap::new();
    for (k, v) in workload() {
        match v {
            Some(v) => m.insert(k, v),
            None => m.remove(&k),
        };
    }
    m
}

fn erased() -> SimNor {
    SimNor { mem: vec![0xFF; SECTORS * SECTOR as usize], budget: None }
}

/// The region after the workload, written by the code under test.
fn run() -> Vec<u8> {
    let mut s = block_on(Store::<SimNor, SECTORS>::mount(erased())).unwrap();
    for (k, v) in workload() {
        match v {
            Some(v) => block_on(s.set(k, &v)).unwrap(),
            None => block_on(s.remove(k)).unwrap(),
        }
    }
    assert!(s.erases >= 4, "the workload compacts: {} erases", s.erases);
    s.into_inner().mem
}

fn read_all<const K: usize>(s: &mut Store<SimNor, SECTORS, K>) -> BTreeMap<u16, Vec<u8>> {
    let keys: Vec<u16> = s.keys().collect();
    let mut m = BTreeMap::new();
    for k in keys {
        let mut b = [0u8; MAX_VALUE];
        let n = block_on(s.get(k, &mut b)).unwrap().unwrap();
        assert!(m.insert(k, b[..n].to_vec()).is_none(), "key {k:#06x} listed twice");
    }
    m
}

fn mount<const K: usize>(mem: Vec<u8>) -> Store<SimNor, SECTORS, K> {
    block_on(Store::<SimNor, SECTORS, K>::mount(SimNor { mem, budget: None })).unwrap()
}

fn fixture() -> Vec<u8> {
    std::fs::read(std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE)).unwrap()
}

#[test]
fn write_fixture() {
    if std::env::var_os("ORPHEUS_WRITE_FIXTURE").is_none() {
        return;
    }
    let img = run();
    assert_eq!(read_all(&mut mount::<MAX_KEYS>(img.clone())), model());
    let p = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join(FIXTURE);
    std::fs::create_dir_all(p.parent().unwrap()).unwrap();
    std::fs::write(p, img).unwrap();
}

/// The image the old code (128-key index, 8-byte slots) wrote mounts with
/// the new index and with one of the old size, gives exactly the
/// workload's values, and the mount writes nothing; the store then keeps
/// working through a compaction.
#[test]
fn old_image_mounts_with_identical_values() {
    let img = fixture();
    assert_eq!(img.len(), SECTORS * SECTOR as usize);
    let mut want = model();
    let mut s = mount::<MAX_KEYS>(img.clone());
    assert_eq!(read_all(&mut s), want);
    assert!(s.into_inner().mem == img, "the mount wrote");
    assert_eq!(read_all(&mut mount::<128>(img.clone())), want);
    let mut s = mount::<MAX_KEYS>(img);
    for i in 0..300u16 {
        let k = setting_keys()[usize::from(i % 125)];
        block_on(s.set(k, &i.to_le_bytes())).unwrap();
        want.insert(k, i.to_le_bytes().to_vec());
    }
    assert!(s.erases >= 1);
    assert_eq!(read_all(&mut mount::<MAX_KEYS>(s.into_inner().mem)), want);
}

/// The format did not change: given the same operations, the new code
/// leaves the same bytes in NOR as the old code did.
#[test]
fn new_code_writes_the_old_format_byte_for_byte() {
    assert!(run() == fixture(), "the image differs from the one the 128-key code wrote");
}

/// An older firmware (128-key index) mounting a store written with more
/// keys: the first 128 keys in log order read with their values, the
/// others are not seen, and its own writes and compactions leave those
/// 128 intact (the other keys are dropped by its first compaction).
#[test]
fn smaller_index_reads_a_bigger_store() {
    let mut s = mount::<MAX_KEYS>(erased().mem);
    let mut want = BTreeMap::new();
    for k in 0..180u16 {
        let v = (k * 3).to_le_bytes().to_vec();
        block_on(s.set(0x0100 + k, &v)).unwrap();
        want.insert(0x0100 + k, v);
    }
    let mut old = mount::<128>(s.into_inner().mem);
    let mut seen = read_all(&mut old);
    assert_eq!(seen.len(), 128);
    for (k, v) in &seen {
        assert_eq!(want.get(k), Some(v));
        assert!(*k < 0x0100 + 128, "the first 128 in log order");
    }
    for i in 0..400u16 {
        let k = 0x0100 + i % 128;
        block_on(old.set(k, &i.to_le_bytes())).unwrap();
        seen.insert(k, i.to_le_bytes().to_vec());
    }
    assert!(old.erases >= 2);
    assert_eq!(read_all(&mut mount::<MAX_KEYS>(old.into_inner().mem)), seen);
}

/// A full index (191 settings: 190 of two bytes and one rewritten with a
/// presets-sized 332-byte value; and the 6-byte Bluetooth address: 192
/// keys) still leaves the sector room to compact: churn never meets
/// `Full`, and a 193rd key is refused without touching the others.
#[test]
fn full_index_before_full_sector() {
    let mut s = mount::<MAX_KEYS>(erased().mem);
    let mut want = BTreeMap::new();
    let n = (MAX_KEYS - 1) as u16;
    for k in 0..n {
        block_on(s.set(0x0100 + k, &k.to_le_bytes())).unwrap();
        want.insert(0x0100 + k, k.to_le_bytes().to_vec());
    }
    block_on(s.set(BT, &[9; 6])).unwrap();
    want.insert(BT, vec![9; 6]);
    assert_eq!(s.indexed(), MAX_KEYS);
    assert_eq!(block_on(s.set(0x7000, &[1, 2])), Err(Error::TooManyKeys));
    let big = 0x0107;
    let mut seed = 7u32;
    for i in 0..2000u32 {
        let r = lcg(&mut seed);
        let k = 0x0100 + (r % u32::from(n)) as u16;
        let v: Vec<u8> = if i % 25 == 0 {
            (0..332).map(|j| (j ^ r) as u8).collect()
        } else if k == big {
            continue;
        } else {
            (r as u16).to_le_bytes().to_vec()
        };
        let k = if i % 25 == 0 { big } else { k };
        block_on(s.set(k, &v)).unwrap();
        want.insert(k, v);
    }
    assert!(s.erases >= 20, "{} erases", s.erases);
    assert_eq!(read_all(&mut mount::<MAX_KEYS>(s.into_inner().mem)), want);
}

type Op = (u16, Option<Vec<u8>>);

fn apply(s: &mut Store<SimNor, SECTORS>, op: &Op) -> Result<(), Error<bodies::Cut>> {
    match &op.1 {
        Some(v) => block_on(s.set(op.0, v)),
        None => block_on(s.remove(op.0)),
    }
}

/// A power cut at every write step while a store of 180 keys (more than
/// the old index held) takes new values, six new keys and removes
/// through compactions: after the remount every key holds its value
/// before or after the interrupted operation, and the store keeps
/// working. The NOR model also asserts nothing is programmed over bits
/// that are not erased.
#[test]
fn power_cut_at_every_step_with_many_keys() {
    let mut s = mount::<MAX_KEYS>(erased().mem);
    let mut base_model = BTreeMap::new();
    for k in 0..180u16 {
        let v = k.to_le_bytes().to_vec();
        block_on(s.set(0x0100 + k, &v)).unwrap();
        base_model.insert(0x0100 + k, v);
    }
    let base = s.into_inner().mem;
    let mut seed = 0x00C0_FFEEu32;
    let ops: Vec<Op> = (0..200u32)
        .map(|i| {
            let r = lcg(&mut seed);
            let k = 0x0100 + (r % 186) as u16;
            match i % 17 {
                0 => (k, None),
                1 => (k, Some(vec![(r >> 3) as u8; 1 + (r % 60) as usize])),
                _ => (k, Some((r as u16).to_le_bytes().to_vec())),
            }
        })
        .collect();
    let total = {
        let nor = SimNor { mem: base.clone(), budget: Some(u32::MAX) };
        let mut s = block_on(Store::<SimNor, SECTORS>::mount(nor)).unwrap();
        for op in &ops {
            apply(&mut s, op).unwrap();
        }
        assert!(s.erases >= 2, "the workload compacts ({} erases)", s.erases);
        u32::MAX - s.into_inner().budget.unwrap()
    };
    for cut in 0..total {
        let nor = SimNor { mem: base.clone(), budget: Some(cut) };
        let mut s = block_on(Store::<SimNor, SECTORS>::mount(nor)).unwrap();
        let mut model = base_model.clone();
        let mut pending = None;
        for op in &ops {
            match apply(&mut s, op) {
                Ok(()) => {
                    match &op.1 {
                        Some(v) => model.insert(op.0, v.clone()),
                        None => model.remove(&op.0),
                    };
                }
                Err(Error::Nor(bodies::Cut)) => {
                    pending = Some(op.clone());
                    break;
                }
                Err(e) => panic!("cut {cut}: {e:?}"),
            }
        }
        let pending = pending.unwrap_or_else(|| panic!("cut {cut} of {total} never hit"));
        let mut nor = s.into_inner();
        nor.budget = None;
        let mut s = block_on(Store::<SimNor, SECTORS>::mount(nor)).unwrap();
        let got = read_all(&mut s);
        let mut after = model.clone();
        match &pending.1 {
            Some(v) => after.insert(pending.0, v.clone()),
            None => after.remove(&pending.0),
        };
        assert!(got == model || got == after, "cut {cut}: key {:#06x} op {:?}", pending.0, pending.1);
        block_on(s.set(0x0F00, b"after")).unwrap();
        let mut s = mount::<MAX_KEYS>(s.into_inner().mem);
        assert_eq!(read_all(&mut s).get(&0x0F00).map(Vec::as_slice), Some(&b"after"[..]));
    }
}
