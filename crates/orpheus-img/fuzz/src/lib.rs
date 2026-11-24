//! Fuzz-target bodies of `orpheus-img` (07 §3.4), shared by the cargo-fuzz
//! targets in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`; VER-4).

#![forbid(unsafe_code)]

use orpheus_img::thumb::{CacheFile, THUMBS, ThumbKey, ThumbMeta};
use orpheus_img::{Decoder, Fit, Options, Orient, Placement, Scaler, SliceSource, Step, Target};

/// A cache bucket file in memory.
pub struct Mem(pub Vec<u8>);

impl CacheFile for Mem {
    type Error = ();
    fn read_at(&mut self, off: u32, buf: &mut [u8]) -> Result<usize, ()> {
        let off = (off as usize).min(self.0.len());
        let n = buf.len().min(self.0.len() - off);
        buf[..n].copy_from_slice(&self.0[off..off + n]);
        Ok(n)
    }
    fn write_at(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
        let end = off as usize + data.len();
        if end > 1 << 22 {
            return Err(());
        }
        if self.0.len() < end {
            self.0.resize(end, 0);
        }
        self.0[off as usize..end].copy_from_slice(data);
        Ok(())
    }
}

/// Any bytes through the format probe, a decoder and the scaler.
///
/// The first byte picks the feeding (whole buffer, or a simulated ring
/// refilled in steps), the arena size and the placement; the rest is the
/// picture. Seeds: the pictures from `cargo test -p orpheus-img` in
/// `target/tmp/orpheus-img/data-*/` (prefix each with one byte, or not: any
/// byte is a valid selector).
pub fn decode(data: &[u8]) {
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
    let Ok(mut sc) = Scaler::new(&mut work, p, Target { buf: &mut fb, stride: 40, x: 0, y: 0 }) else {
        return;
    };
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
}

/// Thumbnail cache lookups and stores on arbitrary bucket-file contents
/// never panic, and a store followed by a lookup finds the thumbnail.
pub fn thumb(data: &[u8]) {
    let kind = THUMBS;
    let mut f = Mem(data.to_vec());
    let key = ThumbKey { file_size: data.len() as u64, w: kind.w, h: kind.h, ..Default::default() };
    let mut out = vec![0u16; usize::from(kind.w) * usize::from(kind.h)];
    let _ = kind.lookup(&mut f, &key, &mut out);
    let px: Vec<u16> = (0..out.len()).map(|i| i as u16).collect();
    let meta = ThumbMeta { orig_w: 1, orig_h: 2, approximate: false };
    if kind.store(&mut f, &key, &meta, &px).is_ok() {
        assert_eq!(kind.lookup(&mut f, &key, &mut out), Ok(Some(meta)));
        assert_eq!(out, px);
    }
}
