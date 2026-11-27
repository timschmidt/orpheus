//! Adversarial pictures (VER-3, 07 §3.7), beyond `robust.rs` (cuts every
//! n/97 bytes, random corruption, huge declared sizes): every picture cut
//! at every byte of its first KiB; the arena at exactly what a decode
//! used, one byte less, and reused (another picture's bytes left in it,
//! as on the player); width/height/depth fields at their edges in every
//! format, with the PNG chunk CRC made right so the check is passed.

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
mod common;

use adversarial::{config, edges, put_be, put_le};
use common::*;
use orpheus_img::{Decoder, Error, Options, SliceSource, Step};
use proptest::prelude::*;

fn small_cases() -> Vec<Case> {
    cases().into_iter().filter(|c| c.data.len() < 300_000).collect()
}

/// The verdicts a damaged or cut picture may get.
fn acceptable(r: &Result<(), Error>) -> bool {
    matches!(
        r,
        Ok(())
            | Err(Error::Truncated
                | Error::Corrupt
                | Error::UnknownFormat
                | Error::Unsupported(_)
                | Error::Arena { .. })
    )
}

#[test]
fn every_picture_cut_at_every_header_byte() {
    for c in small_cases() {
        for cut in 0..c.data.len().min(1024) {
            let mut cap = Capture::default();
            let o = decode(&c.data[..cut], ARENA, &mut cap, Feed::All);
            match o.result {
                Ok(()) | Err(Error::Truncated | Error::Corrupt | Error::UnknownFormat) => {}
                Err(e) => panic!("{} cut at {cut}: {e:?}", c.name),
            }
            if cut % 7 == 0 {
                let mut cap = Capture::default();
                let _ = decode(&c.data[..cut], ARENA, &mut cap, Feed::Trickle(4096, 13));
            }
        }
    }
}

/// `decode` with the arena prefilled with `fill`.
fn decode_filled(data: &[u8], arena: usize, fill: u8) -> (Result<(), Error>, Vec<orpheus_img::Rgb>, usize) {
    let mut mem = vec![fill; arena];
    let mut dec = Decoder::new(&mut mem, Options { background: BG });
    let mut src = SliceSource::ring(data, 9000);
    let mut cap = Capture::default();
    let mut result = Ok(());
    for _ in 0..50_000_000u64 {
        match dec.step(&mut src, &mut cap) {
            Ok(Step::Done) => break,
            Ok(Step::Progress) => {}
            Ok(Step::NeedData(_)) => src.refill(),
            Err(e) => {
                result = Err(e);
                break;
            }
        }
    }
    let used = dec.arena_used();
    assert!(used <= arena);
    (result, cap.rows, used)
}

#[test]
fn arena_exact_one_short_and_reused() {
    for c in small_cases() {
        let (r, rows, used) = decode_filled(&c.data, ARENA, 0);
        if r.is_err() || used == 0 {
            continue;
        }
        // Reused: another picture's bytes in the arena change nothing.
        for fill in [0xFF, 0xA5] {
            let (r2, rows2, _) = decode_filled(&c.data, ARENA, fill);
            assert_eq!((&r2, &rows2), (&r, &rows), "{}: arena filled with {fill:#x}", c.name);
        }
        // Exactly what it used: the decoders check their need before they
        // allocate (with slack: PNG adds 16 bytes and a palette reserve,
        // JPEG falls back to a cheaper mode), so this may be refused, but
        // only with `Arena`.
        let (r3, _, _) = decode_filled(&c.data, used, 0x33);
        assert!(matches!(r3, Ok(()) | Err(Error::Arena { .. })), "{}: exactly {used} bytes: {r3:?}", c.name);
        // One byte less: refused, or a coarser reduction that fits; never
        // a panic or a write past it.
        for short in [used - 1, used / 2, 64, 0] {
            let (r4, _, used4) = decode_filled(&c.data, short, 0x77);
            assert!(acceptable(&r4), "{} with {short} bytes: {r4:?}", c.name);
            assert!(used4 <= short);
        }
    }
}

fn crc32(data: &[u8]) -> u32 {
    let mut c = !0u32;
    for &b in data {
        c ^= u32::from(b);
        for _ in 0..8 {
            c = if c & 1 != 0 { (c >> 1) ^ 0xEDB8_8320 } else { c >> 1 };
        }
    }
    !c
}

fn check_all(d: &[u8]) {
    // A thumbnail-sized sink: the decoders take their cheapest scale, so
    // hundreds of variants stay fast; the header paths are the same.
    let mut cap = Capture { want: Some((64, 48)), ..Capture::default() };
    let t = std::time::Instant::now();
    let o = decode(d, ARENA, &mut cap, Feed::All);
    assert!(acceptable(&o.result));
    assert!(t.elapsed().as_secs() < 10, "slow: {:?}", t.elapsed());
    let mut cap = Capture { want: Some((64, 48)), ..Capture::default() };
    let _ = decode(d, 12 * 1024, &mut cap, Feed::Trickle(4096, 1000));
}

#[test]
fn dimensions_and_depths_at_their_edges() {
    // The smallest two pictures of each format (the variants decode in
    // full; the header paths are the same for every size).
    let mut cs = small_cases();
    cs.sort_by_key(|c| c.data.len());
    let mut per: std::collections::HashMap<String, usize> = std::collections::HashMap::new();
    cs.retain(|c| {
        let n = per.entry(c.name.rsplit('.').next().unwrap_or("").to_string()).or_default();
        *n += 1;
        *n <= 2
    });
    for c in cs {
        let d = &c.data;
        if c.is("png") && d.len() > 33 {
            // IHDR: width, height (big-endian 32), depth, colour type,
            // interlace; CRC over type + data made right.
            for (off, n, real) in
                [(16, 4, u64::from(c.w)), (20, 4, u64::from(c.h)), (24, 1, 8), (25, 1, 6), (28, 1, 0)]
            {
                for v in edges(8 * n as u32, real).into_iter().chain([32768, 32769, 0x8000_0000]) {
                    if n == 1 && v > 255 {
                        continue;
                    }
                    let mut b = d.clone();
                    put_be(&mut b, off, n, v);
                    let crc = crc32(&b[12..29]);
                    b[29..33].copy_from_slice(&crc.to_be_bytes());
                    check_all(&b);
                }
            }
        }
        if c.is("bmp") && d.len() > 30 {
            for (off, n, real) in [
                (18, 4, u64::from(c.w)),
                (22, 4, u64::from(c.h)),
                (28, 2, 24),
                (30, 4, 0),
                (10, 4, 54),
                (14, 4, 40),
            ] {
                for v in edges(8 * n as u32, real)
                    .into_iter()
                    .chain([0x8000_0000, u64::from((-i64::from(c.h)) as u32)])
                {
                    if n == 2 && v > 0xFFFF {
                        continue;
                    }
                    let mut b = d.clone();
                    put_le(&mut b, off, n, v);
                    check_all(&b);
                }
            }
        }
        if c.is("gif") && d.len() > 13 {
            for off in [6, 8] {
                for v in edges(16, 1) {
                    let mut b = d.clone();
                    put_le(&mut b, off, 2, v);
                    check_all(&b);
                }
            }
            // The packed field (global colour table size) and every image
            // descriptor's size and position.
            for v in [0u64, 0x7F, 0x80, 0xF7, 0xFF] {
                let mut b = d.clone();
                b[10] = v as u8;
                check_all(&b);
            }
            if let Some(p) = d.iter().position(|&x| x == b',') {
                for f in 0..4 {
                    for v in edges(16, 1) {
                        let mut b = d.clone();
                        if p + 3 + 2 * f <= b.len() {
                            put_le(&mut b, p + 1 + 2 * f, 2, v);
                            check_all(&b);
                        }
                    }
                }
            }
        }
        if c.is("qoi") && d.len() > 14 {
            for off in [4, 8] {
                for v in edges(32, 1).into_iter().chain([32768, 32769]) {
                    let mut b = d.clone();
                    put_be(&mut b, off, 4, v);
                    check_all(&b);
                }
            }
            for v in [0u64, 2, 3, 4, 5, 0xFF] {
                let mut b = d.clone();
                b[12] = v as u8;
                check_all(&b);
            }
        }
        if c.is("jpg") || c.is("jpeg") {
            // Walk the marker segments up to the first scan: every SOFn's
            // precision, height, width and component count, and every
            // segment length, at their edges.
            let mut i = 2;
            while i + 4 <= d.len() && d[i] == 0xFF && d[i + 1] != 0xDA {
                let m = d[i + 1];
                let len = usize::from(u16::from_be_bytes([d[i + 2], d[i + 3]]));
                if (0xC0..=0xCF).contains(&m) && ![0xC4, 0xC8, 0xCC].contains(&m) {
                    for (f, n) in [(4, 1), (5, 2), (7, 2), (9, 1)] {
                        for v in edges(8 * n as u32, 1).into_iter().chain([32768, 32769]) {
                            if (n == 1 && v > 255) || i + f + n > d.len() {
                                continue;
                            }
                            let mut b = d.clone();
                            put_be(&mut b, i + f, n, v);
                            check_all(&b);
                        }
                    }
                }
                for v in edges(16, len as u64) {
                    let mut b = d.clone();
                    put_be(&mut b, i + 2, 2, v);
                    check_all(&b);
                }
                i += 2 + len;
            }
        }
    }
}

proptest! {
    #![proptest_config(config("img-header-splices", 64))]

    /// A real picture's first bytes, then any bytes (a header that
    /// promises much, followed by noise), in any feeding.
    #[test]
    fn real_header_then_noise(case in 0usize..64, keep in 0usize..2048, noise in prop::collection::vec(any::<u8>(), 0..4096)) {
        let cs = small_cases();
        let c = &cs[case % cs.len()];
        let mut d = c.data[..keep.min(c.data.len())].to_vec();
        d.extend_from_slice(&noise);
        check_all(&d);
    }
}
