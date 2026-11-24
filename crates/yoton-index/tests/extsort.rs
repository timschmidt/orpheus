//! The generic external sort against `sort` of a `Vec`, with suspensions
//! (the scratch overwritten in between) at random points.

mod common;

use std::cmp::Ordering;

use common::MemStore;
use yoton_index::{ExtSort, FileName, MIN_SORT_SCRATCH};

fn rng(seed: u64) -> impl FnMut() -> u64 {
    let mut s = seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1;
    move || {
        s ^= s << 13;
        s ^= s >> 7;
        s ^= s << 17;
        s
    }
}

fn cmp(a: &[u8], b: &[u8]) -> Ordering {
    a.cmp(b)
}

fn maybe_suspend(
    s: &mut ExtSort,
    st: &mut MemStore,
    scratch: &mut [u8],
    r: &mut dyn FnMut() -> u64,
    every: u64,
) {
    if every > 0 && r() % every == 0 {
        s.suspend(st, scratch, &cmp).unwrap();
        scratch.fill(0xA5);
    }
}

fn run(seed: u64, count: usize, max_len: usize, scratch_len: usize, every: u64) {
    run_with(seed, count, max_len, scratch_len, every, every);
}

/// `every`: suspension odds while collecting; `drain_every`: while
/// merging and reading.
fn run_with(seed: u64, count: usize, max_len: usize, scratch_len: usize, every: u64, drain_every: u64) {
    let mut r = rng(seed);
    let items: Vec<Vec<u8>> = (0..count)
        .map(|_| {
            let n = (r() % (max_len as u64 + 1)) as usize;
            (0..n).map(|_| (r() % 7) as u8 + b'a').collect()
        })
        .collect();
    let mut st = MemStore::new();
    let mut scratch = vec![0u8; scratch_len];
    let mut s = ExtSort::new(FileName::new("x-a.tmp").unwrap(), FileName::new("x-b.tmp").unwrap());
    s.reset(&mut st).unwrap();
    for it in &items {
        s.push(&mut st, &mut scratch, it, &cmp).unwrap();
        maybe_suspend(&mut s, &mut st, &mut scratch, &mut r, every);
    }
    s.seal(&mut st, &mut scratch, &cmp).unwrap();
    let every = drain_every;
    loop {
        maybe_suspend(&mut s, &mut st, &mut scratch, &mut r, every);
        if s.merge(&mut st, &mut scratch, &cmp, 37).unwrap() {
            break;
        }
    }
    let mut got = Vec::new();
    loop {
        maybe_suspend(&mut s, &mut st, &mut scratch, &mut r, every);
        match s.next(&mut st, &mut scratch, &cmp).unwrap() {
            Some(x) => got.push(x.to_vec()),
            None => break,
        }
    }
    assert!(s.is_done());
    let mut want = items.clone();
    want.sort();
    assert_eq!(got.len(), want.len(), "seed {seed}");
    assert!(got == want, "seed {seed} count {count} scratch {scratch_len}");
    s.remove(&mut st).unwrap();
    assert!(st.files.is_empty());
}

#[test]
fn in_memory_and_on_file() {
    for (i, &(count, max_len, scratch, every)) in [
        (0, 10, MIN_SORT_SCRATCH, 0),
        (1, 10, MIN_SORT_SCRATCH, 0),
        (50, 20, 8192, 0),
        (50, 20, 8192, 3),
        (5000, 40, MIN_SORT_SCRATCH, 0),
        (5000, 40, 8192, 0),
        (5000, 40, 8192, 50),
        (20000, 12, 4096, 0),
        (3000, 1024, MIN_SORT_SCRATCH, 0),
        (3000, 700, 8192, 40),
        (100_000, 8, 16384, 997),
    ]
    .iter()
    .enumerate()
    {
        run(i as u64 + 1, count, max_len, scratch, every);
    }
}

#[test]
fn suspended_while_reading() {
    // In memory (no run on file until the suspension) and on file.
    for (i, &(count, scratch)) in [(40, 8192), (200, 8192), (3000, 4096)].iter().enumerate() {
        for every in [1, 2, 7] {
            run_with(100 + i as u64 * 10 + every, count, 30, scratch, 0, every);
        }
    }
}
