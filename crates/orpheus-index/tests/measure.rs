//! Figures for the report: RAM, card I/O per build and per step, on-card
//! size, host throughput. Run with
//! `cargo test -p orpheus-index --release --test measure -- --ignored --nocapture`.

mod common;

use std::time::Instant;

use common::*;
use orpheus_index::{Builder, Index, Progress, Row, Signature, SortBy, Spec, Suspended, Verifier};

#[test]
#[ignore = "prints measurements"]
fn report() {
    use core::mem::size_of;
    println!(
        "RAM: Builder {} B (+ borrowed scratch), Suspended {} B, Index {} B, Row {} B (32-row window {} B), Verifier {} B, Signature {} B",
        size_of::<Builder<'static>>(),
        size_of::<Suspended>(),
        size_of::<Index>(),
        size_of::<Row>(),
        32 * size_of::<Row>(),
        size_of::<Verifier>(),
        size_of::<Signature>()
    );
    let mut r = Rng(0xBEEF);
    let spec = Spec { sort: SortBy::Name, show_hidden: true, audio_only: false, images_only: false };
    let cases =
        [(1000usize, false), (5000, false), (10_000, false), (50_000, false), (5000, true), (10_000, true)];
    for (n, album) in cases {
        let entries = if album {
            let names = (0..n).map(|i| prefixed_name(&mut r, i)).collect();
            make_dir(names, &mut r)
        } else {
            random_dir(&mut r, n)
        };
        let name_bytes: usize = entries.iter().map(|e| e.name.len()).sum();
        for scratch in [4096usize, 8192] {
            let mut store = MemStore::new();
            let mut src = MockDir::new(entries.clone());
            let mut buf = vec![0u8; scratch];
            let t = Instant::now();
            let mut b = Builder::new(&mut buf, DIR, spec, 0).unwrap();
            let (mut steps, mut passes, mut max_step_io) = (0u32, 0u8, 0u64);
            let ix = loop {
                let before = store.read_bytes + store.write_bytes;
                let p = b.step(&mut store, &mut src, 64).unwrap();
                max_step_io = max_step_io.max(store.read_bytes + store.write_bytes - before);
                steps += 1;
                match p {
                    Progress::Done(ix) => break ix,
                    Progress::Merging { pass, .. } => passes = passes.max(pass + 1),
                    _ => {}
                }
            };
            let dt = t.elapsed();
            let tmp_peak =
                store.peak.iter().filter(|(k, _)| k.ends_with(".tmp")).map(|(_, v)| *v).max().unwrap_or(0);
            println!(
                "n={n:6} names {:5.1} B avg, scratch {scratch}: {:7.1} ms host ({:8.0} entries/s), passes {passes}, steps {steps} (budget 64), \
                 card I/O: read {} KiB in {} calls, wrote {} KiB in {} calls, max {} KiB per step; scratch file peak {} KiB; \
                 index {} B = {:.0} B per 1000 rows, letter runs {}",
                name_bytes as f64 / n as f64,
                dt.as_secs_f64() * 1e3,
                n as f64 / dt.as_secs_f64(),
                store.read_bytes / 1024,
                store.reads,
                store.write_bytes / 1024,
                store.writes,
                max_step_io / 1024,
                tmp_peak / 1024,
                ix.file_len(),
                (ix.file_len() as f64 - 512.0) * 1000.0 / n as f64,
                ix.letter_runs(),
            );
            // Query costs.
            store.reset_counters();
            let mut win = [ix.row(&mut store, 0).unwrap().unwrap(); 32];
            store.reset_counters();
            ix.rows(&mut store, (n as u32 / 2) & !31, &mut win).unwrap();
            let (wr, wb) = (store.reads, store.read_bytes);
            store.reset_counters();
            ix.seek_letter(&mut store, n as u32 / 3, true).unwrap();
            let (sr, sb) = (store.reads, store.read_bytes);
            store.reset_counters();
            let t = Instant::now();
            ix.position_of(&mut store, entries[n / 2].loc).unwrap();
            let pt = t.elapsed();
            let (pr, pb) = (store.reads, store.read_bytes);
            if scratch == 4096 {
                println!(
                    "    query: 32-row window {wr} reads / {wb} B; seek_letter {sr} reads / {sb} B; position_of (linear) {pr} reads / {pb} B, {:.2} ms host",
                    pt.as_secs_f64() * 1e3
                );
            }
        }
    }
    // Staleness check cost: one directory pass.
    let entries = random_dir(&mut r, 10_000);
    let mut src = MockDir::new(entries);
    let mut v = Verifier::new();
    let mut name = vec![0u8; orpheus_index::MAX_NAME];
    let t = Instant::now();
    while v.step(&mut src, &mut name, 256).unwrap().is_none() {}
    println!("signature of 10 000 entries: {:.2} ms host", t.elapsed().as_secs_f64() * 1e3);
}
