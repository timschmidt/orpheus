//! Fuzz-target body of `orpheus-settings` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`settings_mount`] — the store mounted over arbitrary contents of its four
//! NOR sectors, then sets, removes, remounts and power cuts. Oracles: no
//! panic; mount always succeeds; every listed key reads back, once; the
//! store never programs over bytes that are not erased (NOR can only clear
//! bits, so that would corrupt what is there) and only aligned words; a
//! value read back is the value last written; after a remount the store
//! holds exactly what it held before; after a power cut at any write step
//! the interrupted key holds its old or its new value and every other key
//! is unchanged.

#![forbid(unsafe_code)]

use std::collections::BTreeMap;

use embassy_futures::block_on;
use orpheus_settings::{Error, MAX_KEYS, MAX_VALUE, Nor, SECTOR, Store};

/// Sectors of the firmware's settings region (`0x7DA000..0x7DE000`).
pub const SECTORS: usize = 4;

/// NOR model with an optional power cut after `budget` more write steps
/// (the step that hits it is applied halfway).
#[derive(Clone)]
pub struct SimNor {
    pub mem: Vec<u8>,
    pub budget: Option<u32>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Cut;

impl SimNor {
    fn tick(&mut self) -> Result<bool, Cut> {
        match &mut self.budget {
            None => Ok(false),
            Some(0) => Err(Cut),
            Some(1) => {
                self.budget = Some(0);
                Ok(true)
            }
            Some(b) => {
                *b -= 1;
                Ok(false)
            }
        }
    }
}

impl Nor for SimNor {
    type Error = Cut;
    async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), Cut> {
        let o = off as usize;
        buf.copy_from_slice(self.mem.get(o..o + buf.len()).expect("read inside the region"));
        Ok(())
    }
    async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), Cut> {
        assert!(off % 4 == 0 && data.len() % 4 == 0, "unaligned program {off:#x} +{}", data.len());
        let o = off as usize;
        let old = self.mem.get(o..o + data.len()).expect("program inside the region");
        for (i, (d, s)) in old.iter().zip(data).enumerate() {
            assert_eq!(d & s, *s, "program over non-erased bits at {:#x}: {d:#04x} <- {s:#04x}", o + i);
        }
        let torn = self.tick()?;
        let n = if torn { data.len() / 2 } else { data.len() };
        for (d, s) in self.mem[o..o + n].iter_mut().zip(&data[..n]) {
            *d &= *s;
        }
        if torn { Err(Cut) } else { Ok(()) }
    }
    async fn erase(&mut self, s: u32) -> Result<(), Cut> {
        assert!(s % SECTOR == 0 && (s as usize) < self.mem.len(), "erase {s:#x}");
        let torn = self.tick()?;
        let end = if torn { s + SECTOR / 3 } else { s + SECTOR };
        self.mem[s as usize..end as usize].fill(0xFF);
        if torn { Err(Cut) } else { Ok(()) }
    }
}

type S = Store<SimNor, SECTORS>;

fn get(s: &mut S, k: u16) -> Option<Vec<u8>> {
    let mut b = [0u8; MAX_VALUE];
    block_on(s.get(k, &mut b)).expect("get").map(|n| b[..n].to_vec())
}

/// Everything the store holds, checking the key list on the way.
fn contents(s: &mut S) -> BTreeMap<u16, Vec<u8>> {
    let keys: Vec<u16> = s.keys().collect();
    assert!(keys.len() <= MAX_KEYS);
    let mut m = BTreeMap::new();
    for k in keys {
        let v = get(s, k).unwrap_or_else(|| panic!("listed key {k} does not read"));
        assert!(v.len() <= MAX_VALUE);
        assert!(m.insert(k, v).is_none(), "key {k} listed twice");
    }
    m
}

fn mount(nor: SimNor) -> S {
    block_on(S::mount(nor)).expect("mount never fails without a power cut")
}

fn value(k: u16, len: usize, salt: u8) -> Vec<u8> {
    (0..len).map(|i| (i as u8).wrapping_mul(31) ^ salt ^ k as u8).collect()
}

/// Input: `[n][n operation bytes][sector images…]` (padded with `0xFF`).
pub fn settings_mount(data: &[u8]) {
    let Some((&n, rest)) = data.split_first() else { return };
    let (ops, img) = rest.split_at(usize::from(n).min(rest.len()));
    let mut mem = vec![0xFF; SECTORS * SECTOR as usize];
    let k = img.len().min(mem.len());
    mem[..k].copy_from_slice(&img[..k]);
    let mut s = mount(SimNor { mem, budget: None });
    let mut model = contents(&mut s);
    // Mounting again changes nothing.
    s = mount(s.into_inner());
    assert_eq!(contents(&mut s), model, "second mount");

    let mut it = ops.iter().copied();
    while let Some(op) = it.next() {
        let mut b = || it.next().unwrap_or(0);
        let key = u16::from(b() % 16);
        match op % 6 {
            0..=2 => {
                let len = usize::from(b()) * if op & 0x80 != 0 { 4 } else { 1 };
                let v = value(key, len, op);
                match block_on(s.set(key, &v)) {
                    Ok(()) => {
                        model.insert(key, v);
                    }
                    Err(Error::Full | Error::TooManyKeys) => {}
                    Err(e) => panic!("set: {e:?}"),
                }
            }
            3 => match block_on(s.remove(key)) {
                Ok(()) => {
                    model.remove(&key);
                }
                // A full store cannot even write the deletion marker
                // (finding F-SET-2, open): the value stays.
                Err(Error::Full) => {}
                Err(e) => panic!("remove: {e:?}"),
            },
            4 => s = mount(s.into_inner()),
            _ => {
                // A power cut during a set, then a remount.
                let steps = u32::from(b() % 12);
                let len = usize::from(b());
                let v = value(key, len, !op);
                let mut nor = s.into_inner();
                nor.budget = Some(steps);
                // The region has an active sector: mounting only reads, so
                // the budget counts the steps of the set.
                let mut cut = mount(nor);
                let r = block_on(cut.set(key, &v));
                let mut nor = cut.into_inner();
                nor.budget = None;
                s = mount(nor);
                let got = contents(&mut s);
                match r {
                    Ok(()) => {
                        model.insert(key, v);
                    }
                    Err(Error::Full | Error::TooManyKeys) => {}
                    Err(Error::Nor(Cut)) => {
                        let now = got.get(&key);
                        assert!(
                            now == model.get(&key) || now == Some(&v),
                            "cut after {steps} steps: key {key} is {now:?}"
                        );
                        match now {
                            Some(x) => model.insert(key, x.clone()),
                            None => model.remove(&key),
                        };
                    }
                    Err(e) => panic!("set: {e:?}"),
                }
            }
        }
        assert_eq!(contents(&mut s), model, "after op {op}");
    }
    s = mount(s.into_inner());
    assert_eq!(contents(&mut s), model, "after the final remount");
}
