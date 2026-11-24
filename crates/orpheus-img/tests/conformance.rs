//! Decoded pixels against reference decodes (exact for lossless formats,
//! PSNR for JPEG), every reduced resolution, every way of feeding the
//! bytes, and the arena accounting.

mod common;

use common::*;
use orpheus_img::{Error, Frame};

/// Compare a capture with the reference; returns the PSNR (∞ when exact).
fn check(c: &Case, cap: &Capture) -> f64 {
    let f: Frame = cap.frame.unwrap_or_else(|| panic!("{}: no frame", c.name));
    assert_eq!((f.full_width, f.full_height), (c.w, c.h), "{}: full size", c.name);
    assert_eq!(cap.nrows, f.height, "{}: rows", c.name);
    let got = cap.top_down();
    let r = c.reference();
    if (f.width, f.height) == (c.w, c.h) {
        if c.exact {
            for (i, (a, b)) in got.iter().zip(&r).enumerate() {
                let close = (0..3).all(|k| a[k].abs_diff(b[k]) <= c.tol);
                assert!(close, "{}: pixel {},{}: {a:?} vs {b:?}", c.name, i as u32 % c.w, i as u32 / c.w);
            }
            return f64::INFINITY;
        }
        return psnr(&got, &to_f(&r));
    }
    // The reduction factor of an axis: the largest of 1, 2, 4, 8 that gives
    // the delivered size.
    let factor = |full: u32, got: u32| {
        [8u32, 4, 2, 1].into_iter().find(|&l| full.div_ceil(l) == got).unwrap_or_else(|| panic!("{}", c.name))
    };
    if c.is(".jpg") {
        let s = factor(c.w, f.width);
        let (dw, dh, b) = box_down(&r, c.w, c.h, s);
        assert_eq!((dw, dh), (f.width, f.height), "{}: scaled size", c.name);
        return psnr(&got, &b);
    }
    // Point-sampled reductions: Adam7 grids, GIF interlace pass 1.
    let sx = factor(c.w, f.width);
    let sy = factor(c.h, f.height);
    let mut want = Vec::new();
    for y in 0..f.height {
        for x in 0..f.width {
            want.push(r[((y * sy) * c.w + x * sx) as usize]);
        }
    }
    assert_eq!(got, want, "{}: reduced {}x{}", c.name, f.width, f.height);
    f64::INFINITY
}

#[test]
fn full_size_matches_reference() {
    let mut report = String::from("\nfile                       size        delivered   arena B   PSNR dB\n");
    for c in cases() {
        let mut cap = Capture::default();
        let o = decode(&c.data, ARENA, &mut cap, Feed::All);
        o.result.unwrap_or_else(|e| panic!("{}: {e:?}", c.name));
        let p = check(&c, &cap);
        let f = cap.frame.unwrap();
        report += &format!(
            "{:<26} {:>5}x{:<5} {:>5}x{:<5} {:>7}   {}\n",
            c.name,
            c.w,
            c.h,
            f.width,
            f.height,
            o.used,
            if p.is_infinite() { "exact".to_string() } else { format!("{p:.1}") }
        );
        if !c.exact {
            let min = if f.approximate { 28.0 } else { 30.0 };
            assert!(p >= min, "{}: PSNR {p:.1} dB", c.name);
        }
        let info = o.info.unwrap();
        assert_eq!((info.width, info.height), (c.w, c.h));
    }
    println!("{report}");
}

#[test]
fn jpeg_every_scale() {
    let mut report = String::from(
        "\nfile                        1/1    1/2    1/4    1/8  (PSNR dB vs box-filtered reference)\n",
    );
    for c in cases().iter().filter(|c| c.is(".jpg")) {
        report += &format!("{:<26}", c.name);
        for s in [1u32, 2, 4, 8] {
            // Partial edge blocks dominate pictures only a few pixels wide.
            if c.w.div_ceil(s) < 8 || c.h.div_ceil(s) < 8 {
                report += "      -";
                continue;
            }
            let mut cap = Capture { want: Some((c.w.div_ceil(s), c.h.div_ceil(s))), ..Default::default() };
            // A large arena so the requested scale is the one delivered.
            let o = decode(&c.data, 64 << 20, &mut cap, Feed::All);
            o.result.unwrap_or_else(|e| panic!("{} 1/{s}: {e:?}", c.name));
            let f = cap.frame.unwrap();
            assert_eq!((f.width, f.height), (c.w.div_ceil(s), c.h.div_ceil(s)), "{} 1/{s}", c.name);
            let p = check(c, &cap);
            report += &format!(" {p:6.1}");
            assert!(p >= 30.0, "{} 1/{s}: PSNR {p:.1}", c.name);
        }
        report += "\n";
    }
    println!("{report}");
}

#[test]
fn adam7_levels() {
    for c in cases().iter().filter(|c| c.name.starts_with("p_i_")) {
        for l in [1u32, 2, 4, 8] {
            let want = (c.w.div_ceil(l), c.h.div_ceil(l));
            let mut cap = Capture { want: Some(want), ..Default::default() };
            let o = decode(&c.data, 8 << 20, &mut cap, Feed::All);
            o.result.unwrap_or_else(|e| panic!("{} L{l}: {e:?}", c.name));
            let f = cap.frame.unwrap();
            // The coarsest grid at least the wanted size.
            assert!(f.width >= want.0 && f.height >= want.1, "{} L{l}", c.name);
            check(c, &cap);
        }
    }
}

#[test]
fn ring_and_trickle_give_the_same_rows() {
    for c in cases() {
        if c.data.len() > 3_000_000 {
            continue;
        }
        let mut a = Capture::default();
        decode(&c.data, ARENA, &mut a, Feed::All).result.unwrap();
        for feed in [Feed::Ring(32 * 1024), Feed::Ring(9000), Feed::Trickle(16 * 1024, 997)] {
            if matches!(feed, Feed::Trickle(..)) && c.data.len() > 400_000 {
                continue;
            }
            let mut b = Capture::default();
            let o = decode(&c.data, ARENA, &mut b, feed);
            o.result.unwrap_or_else(|e| panic!("{} {feed:?}: {e:?}", c.name));
            assert_eq!(a.frame, b.frame, "{} {feed:?}", c.name);
            assert!(a.rows == b.rows, "{} {feed:?}: rows differ", c.name);
        }
    }
}

#[test]
fn sink_stop_ends_decoding() {
    for c in cases() {
        let mut cap = Capture { stop_after: Some(3), ..Default::default() };
        let o = decode(&c.data, ARENA, &mut cap, Feed::All);
        assert_eq!(o.result, Ok(()), "{}", c.name);
        assert!(cap.nrows <= 3, "{}: {} rows after stop", c.name, cap.nrows);
    }
}

#[test]
fn small_arena_reports_its_need() {
    for c in cases() {
        let mut cap = Capture::default();
        let o = decode(&c.data, 1024, &mut cap, Feed::All);
        match o.result {
            Ok(()) => assert!(o.used <= 1024),
            Err(Error::Arena { needed, capacity }) => {
                assert_eq!(capacity, 1024);
                assert!(needed > 1024, "{}", c.name);
                // That much is enough (at the smallest reduction).
                let mut cap = Capture::default();
                let o = decode(&c.data, needed, &mut cap, Feed::All);
                o.result.unwrap_or_else(|e| panic!("{} with {needed} B: {e:?}", c.name));
                assert!(o.used <= needed);
                check(&c, &cap);
            }
            Err(e) => panic!("{}: {e:?}", c.name),
        }
    }
}
