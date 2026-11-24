//! Album art helpers (04 §4.4, 05 §7.1): which picture file in a folder is
//! the cover, and a sink that feeds one decode to two scalers (the small
//! and the big art are made in one pass over the picture).

use crate::{Flow, Frame, Orient, Rgb, RowSink};

/// How good a cover a picture file of this name is: lower is better,
/// `None` if the name is not one of the usual cover names (`cover`,
/// `folder`, `front`, `album`, Windows Media Player's `AlbumArt…`, in any
/// case, with a picture extension the decoders read).
#[must_use]
pub fn cover_rank(name: &str) -> Option<u8> {
    let (stem, ext) = name.rsplit_once('.')?;
    let pic = ["jpg", "jpeg", "png", "bmp", "gif", "qoi"].iter().any(|e| ext.eq_ignore_ascii_case(e));
    if !pic || stem.len() > 64 {
        return None;
    }
    let mut low = [0u8; 64];
    for (d, s) in low.iter_mut().zip(stem.bytes()) {
        *d = s.to_ascii_lowercase();
    }
    let stem = &low[..stem.len()];
    match stem {
        b"cover" => Some(0),
        b"folder" => Some(1),
        b"front" => Some(2),
        b"album" => Some(3),
        s if s.starts_with(b"albumart") && s.ends_with(b"small") => Some(5),
        s if s.starts_with(b"albumart") => Some(4),
        _ => None,
    }
}

/// Delivers every row to two sinks. Asks for the larger of the two wanted
/// sizes per axis; stops when both have stopped (a sink that stopped gets
/// no more rows).
pub struct Tee<'a, 'b> {
    a: &'a mut dyn RowSink,
    b: &'b mut dyn RowSink,
    a_done: bool,
    b_done: bool,
}

impl<'a, 'b> Tee<'a, 'b> {
    pub fn new(a: &'a mut dyn RowSink, b: &'b mut dyn RowSink) -> Self {
        Self { a, b, a_done: false, b_done: false }
    }
}

impl RowSink for Tee<'_, '_> {
    fn wanted(&self, full_w: u32, full_h: u32, orient: Orient) -> (u32, u32) {
        let (aw, ah) = self.a.wanted(full_w, full_h, orient);
        let (bw, bh) = self.b.wanted(full_w, full_h, orient);
        (aw.max(bw), ah.max(bh))
    }

    fn begin(&mut self, frame: &Frame) {
        self.a.begin(frame);
        self.b.begin(frame);
        self.a_done = false;
        self.b_done = false;
    }

    fn row(&mut self, y: u32, px: &[Rgb]) -> Flow {
        if !self.a_done {
            self.a_done = self.a.row(y, px) == Flow::Stop;
        }
        if !self.b_done {
            self.b_done = self.b.row(y, px) == Flow::Stop;
        }
        if self.a_done && self.b_done { Flow::Stop } else { Flow::Continue }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scale::rgb565;
    use crate::{Decoder, Options, Placement, Scaler, SliceSource, Target};
    use std::vec::Vec;

    #[test]
    fn cover_names() {
        assert_eq!(cover_rank("cover.jpg"), Some(0));
        assert_eq!(cover_rank("Folder.JPG"), Some(1));
        assert_eq!(cover_rank("FRONT.png"), Some(2));
        assert_eq!(cover_rank("AlbumArt_{1234}_Large.jpg"), Some(4));
        assert_eq!(cover_rank("AlbumArtSmall.jpg"), Some(5));
        assert_eq!(cover_rank("cover.txt"), None);
        assert_eq!(cover_rank("back.jpg"), None);
        assert_eq!(cover_rank("cover"), None);
        assert_eq!(cover_rank("covers.jpg"), None);
    }

    /// A 24-bit BMP, `w` × `h`, left half red, right half blue.
    fn bmp(w: u32, h: u32) -> Vec<u8> {
        let row = (w * 3).div_ceil(4) * 4;
        let mut v = Vec::new();
        v.extend_from_slice(b"BM");
        v.extend_from_slice(&(54 + row * h).to_le_bytes());
        v.extend_from_slice(&[0; 4]);
        v.extend_from_slice(&54u32.to_le_bytes());
        v.extend_from_slice(&40u32.to_le_bytes());
        v.extend_from_slice(&w.to_le_bytes());
        v.extend_from_slice(&h.to_le_bytes());
        v.extend_from_slice(&1u16.to_le_bytes());
        v.extend_from_slice(&24u16.to_le_bytes());
        v.extend_from_slice(&[0; 24]);
        for _ in 0..h {
            for x in 0..w {
                v.extend_from_slice(if x < w / 2 { &[0, 0, 255] } else { &[255, 0, 0] });
            }
            v.resize(v.len() + (row - w * 3) as usize, 0);
        }
        v
    }

    #[test]
    fn tee_makes_two_sizes_in_one_pass() {
        let data = bmp(64, 64);
        let mut small = [0u16; 16 * 16];
        let mut big = [0u16; 32 * 32];
        let mut w1 = [0u32; Scaler::need_words(16)];
        let mut w2 = [0u32; Scaler::need_words(32)];
        let p1 = Placement { dither: false, ..Placement::fit(16, 16) };
        let p2 = Placement { dither: false, ..Placement::fit(32, 32) };
        let mut s1 = Scaler::new(&mut w1, p1, Target { buf: &mut small, stride: 16, x: 0, y: 0 }).unwrap();
        let mut s2 = Scaler::new(&mut w2, p2, Target { buf: &mut big, stride: 32, x: 0, y: 0 }).unwrap();
        assert_eq!(Tee::new(&mut s1, &mut s2).wanted(64, 64, Orient::NORMAL), (32, 32));
        let mut arena = [0u8; 4096];
        let mut dec = Decoder::new(&mut arena, Options::default());
        let mut src = SliceSource::new(&data);
        dec.run(&mut src, &mut Tee::new(&mut s1, &mut s2)).unwrap();
        assert!(s1.complete() && s2.complete());
        let (red, blue) = (rgb565([255, 0, 0]), rgb565([0, 0, 255]));
        assert_eq!((small[0], small[15], small[255]), (red, blue, blue));
        assert_eq!((big[0], big[31], big[32 * 31]), (red, blue, red));
    }
}
