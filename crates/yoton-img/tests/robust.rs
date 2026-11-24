//! Truncated and corrupted pictures never panic or hang, and keep the
//! sink contract (begin once, rows in order, never more than announced).

mod common;

use common::*;
use yoton_img::Error;

fn lcg(seed: &mut u64) -> u64 {
    *seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1_442_695_040_888_963_407);
    *seed >> 33
}

/// Pictures small enough to mutate many times.
fn small_cases() -> Vec<Case> {
    cases().into_iter().filter(|c| c.data.len() < 300_000).collect()
}

#[test]
fn truncated_everywhere() {
    for c in small_cases() {
        let n = c.data.len();
        let step = (n / 97).max(1);
        let mut cut = 0;
        while cut < n {
            let mut cap = Capture::default();
            let o = decode(&c.data[..cut], ARENA, &mut cap, Feed::All);
            match o.result {
                Ok(()) | Err(Error::Truncated | Error::Corrupt | Error::UnknownFormat) => {}
                Err(e) => panic!("{} cut at {cut}: {e:?}", c.name),
            }
            // Through the ring as well (exercises NeedData at the cut).
            let mut cap = Capture::default();
            let _ = decode(&c.data[..cut], ARENA, &mut cap, Feed::Ring(4096));
            cut += step;
        }
    }
}

#[test]
fn corrupted_bytes() {
    let mut seed = 0x5EED;
    for c in small_cases() {
        for round in 0..60 {
            let mut d = c.data.clone();
            let flips = 1 + lcg(&mut seed) as usize % 8;
            for _ in 0..flips {
                // Headers matter most: bias half the flips to the first 1 KiB.
                let range = if round % 2 == 0 { d.len().min(1024) } else { d.len() };
                let i = lcg(&mut seed) as usize % range;
                d[i] = lcg(&mut seed) as u8;
            }
            let mut cap = Capture::default();
            let _ = decode(&d, ARENA, &mut cap, Feed::All);
            let mut cap = Capture::default();
            let _ = decode(&d, 20 * 1024, &mut cap, Feed::Ring(5000));
        }
    }
}

#[test]
fn garbage_after_valid_headers() {
    let mut seed = 77;
    for c in small_cases() {
        let keep = c.data.len().min(600);
        for _ in 0..20 {
            let mut d = c.data[..keep].to_vec();
            for _ in 0..2000 {
                d.push(lcg(&mut seed) as u8);
            }
            let mut cap = Capture::default();
            let _ = decode(&d, ARENA, &mut cap, Feed::All);
        }
    }
}

#[test]
fn hostile_headers() {
    // Huge declared sizes with almost no data finish quickly.
    let jpeg_huge: &[u8] = &[
        0xFF, 0xD8, 0xFF, 0xC2, 0, 11, 8, 0xFF, 0xFF, 0xFF, 0xFF, 1, 1, 0x11, 0, 0xFF, 0xC4, 0, 20, 0, 0, 1,
        0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0xFF, 0xDA, 0, 8, 1, 1, 0, 0, 0, 0, 0xFF, 0xD9,
    ];
    let png_huge = {
        let mut v = b"\x89PNG\r\n\x1a\n\0\0\0\x0dIHDR\0\0\x7f\xff\0\0\x7f\xff\x08\x06\0\0\0\0\0\0\0".to_vec();
        v.extend_from_slice(b"\0\0\0\x02IDAT\x78\x01\0\0\0\0");
        v
    };
    let gif_huge: &[u8] =
        b"GIF89a\xff\xff\xff\xff\x00\x00\x00,\x00\x00\x00\x00\xff\xff\xff\xff\x00\x08\x01\x00\x00;";
    let bmp_huge: &[u8] = b"BM\0\0\0\0\0\0\0\0\x36\0\0\0\x28\0\0\0\xff\x7f\0\0\xff\x7f\0\0\x01\0\x18\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0\0";
    let qoi_huge: &[u8] = b"qoif\0\0\x7f\xff\0\0\x7f\xff\x04\x00\xfe\x01\x02\x03";
    for d in [jpeg_huge, &png_huge, gif_huge, bmp_huge, qoi_huge] {
        let mut cap = Capture::default();
        let t = std::time::Instant::now();
        let o = decode(d, ARENA, &mut cap, Feed::All);
        assert!(o.result.is_err(), "{:?}", o.result);
        assert!(t.elapsed().as_secs() < 5);
    }
}
