//! Any bytes through the format probe, a decoder and the scaler.
//!
//! The first byte picks the feeding (whole buffer, or a simulated ring
//! refilled in steps), the arena size and the placement; the rest is the
//! picture. Seeds: the pictures from `cargo test -p orpheus-img` in
//! `target/tmp/orpheus-img/data-*/` (prefix each with one byte, or not: any
//! byte is a valid selector).
#![no_main]

use libfuzzer_sys::fuzz_target;
use orpheus_img::{Decoder, Fit, Options, Orient, Placement, Scaler, SliceSource, Step, Target};

fuzz_target!(|data: &[u8]| {
    let Some((&sel, pic)) = data.split_first() else { return };
    let arena_size = [104 * 1024, 40 * 1024, 12 * 1024, 3 * 1024][usize::from(sel & 3)];
    let mut mem = vec![0u8; arena_size];
    let fit = match (sel >> 2) & 3 {
        0 => Fit::Contain,
        1 => Fit::Cover,
        _ => Fit::Actual { x: 7, y: 3 },
    };
    let rotate = Orient::from_exif(u16::from((sel >> 4) & 7) + 1);
    let p = Placement { fit, rotate, ..Placement::fit(40, 30) };
    let mut fb = [0u16; 40 * 30];
    let mut work = [0u32; Scaler::need_words(40)];
    let Ok(mut sc) = Scaler::new(&mut work, p, Target { buf: &mut fb, stride: 40, x: 0, y: 0 }) else { return };
    let mut dec = Decoder::new(&mut mem, Options::default());
    let ring = sel & 0x80 != 0;
    let mut src = if ring { SliceSource::ring(pic, 9000) } else { SliceSource::new(pic) };
    for _ in 0..10_000_000 {
        match dec.step(&mut src, &mut sc) {
            Ok(Step::Done) | Err(_) => break,
            Ok(Step::Progress) => {}
            Ok(Step::NeedData(n)) => {
                assert!(n <= orpheus_img::MAX_PEEK);
                if !ring {
                    panic!("NeedData with everything buffered");
                }
                src.trickle(1 + n % 3000);
            }
        }
    }
    assert!(dec.arena_used() <= arena_size);
});
