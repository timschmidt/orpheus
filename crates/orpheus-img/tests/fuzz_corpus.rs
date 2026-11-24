//! The fuzz targets' committed corpora and regression inputs, replayed on
//! stable through the same bodies the cargo-fuzz targets run (VER-4); and
//! the seed inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-img --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/<target>/seed-*`): the
//! synthetic pictures of `tests/gen.py` and bucket files the thumbnail
//! cache writes.

mod common;

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use bodies::Mem;
use orpheus_img::thumb::{ART, THUMBS, ThumbKey, ThumbMeta};

#[test]
fn decode_corpus() {
    replay::replay("decode", bodies::decode);
}

#[test]
fn thumb_corpus() {
    replay::replay("thumb", bodies::thumb);
}

/// Pictures under 32 KiB, each with a selector byte that cycles through
/// the feedings, arenas and placements; at most eight per format.
fn decode_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    let mut per_ext = std::collections::HashMap::<String, usize>::new();
    for (i, c) in common::cases().into_iter().enumerate() {
        let ext = c.name.rsplit('.').next().unwrap_or("").to_string();
        let n = per_ext.entry(ext).or_default();
        if c.data.len() > 32 * 1024 || *n >= 8 {
            continue;
        }
        *n += 1;
        let sel = [0x00u8, 0x85, 0x16, 0xA2][i % 4];
        let mut d = vec![sel];
        d.extend_from_slice(&c.data);
        v.push((c.name.replace('/', "_"), d));
    }
    v
}

fn thumb_seeds() -> Vec<(String, Vec<u8>)> {
    let mut v = Vec::new();
    for (k, kind) in [THUMBS, ART].into_iter().enumerate() {
        let mut f = Mem(Vec::new());
        let px = vec![0x5A5Au16; usize::from(kind.w) * usize::from(kind.h)];
        for n in 0..3u8 {
            let key = ThumbKey {
                locator: [n; 16],
                file_size: 1000 + u64::from(n),
                mtime: 7,
                w: kind.w,
                h: kind.h,
                rotation: 1,
                ..Default::default()
            };
            kind.store(&mut f, &key, &ThumbMeta { orig_w: 640, orig_h: 480, approximate: n == 1 }, &px)
                .unwrap();
            if f.0.len() <= 64 * 1024 {
                v.push((format!("{k}-{n}"), f.0.clone()));
            }
        }
    }
    v
}

#[test]
fn seeds() {
    let d = decode_seeds();
    assert!(d.len() >= 10, "pictures for the decode seeds: {}", d.len());
    for (_, x) in &d {
        bodies::decode(x);
    }
    let t = thumb_seeds();
    for (_, x) in &t {
        bodies::thumb(x);
    }
    replay::write_seeds("decode", &d);
    replay::write_seeds("thumb", &t);
}
