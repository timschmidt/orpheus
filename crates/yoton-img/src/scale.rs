//! The scaler: a [`RowSink`] that fits delivered rows into a box of an RGB565
//! buffer (the frame buffer, a thumbnail) — 04 §7 "Scaler".
//!
//! * Box down-scaling with one accumulator per output column (a group of
//!   source rows is summed, then emitted as one output line), bilinear
//!   up-scaling (two resampled rows kept) — chosen per axis.
//! * Orientation: EXIF orientation and a user rotation compose into one
//!   [`Orient`]; the scaler works in the order rows arrive and places each
//!   output line as a row or a column of the box, so rotating by 90° costs
//!   nothing extra.
//! * Fit modes: [`Fit::Contain`] (letterbox), [`Fit::Cover`] (crop to fill),
//!   [`Fit::Actual`] (1:1 with a pan offset).
//! * Output: RGB565 (native endian), optionally ordered-dithered with a
//!   4 × 4 Bayer matrix.
//!
//! Memory: [`Scaler::need_words`] `u32` words, 6 per pixel of the longest box
//! side (3 KiB for a 128 × 128 box).

use crate::{Flow, Frame, Rgb, RowSink};

/// An element of the dihedral group of the square: how stored pixels map to
/// displayed ones. Applied as: transpose (if `swap`), then mirror the
/// columns (`flip_x`) and/or the rows (`flip_y`) of the result.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Orient {
    pub swap: bool,
    pub flip_x: bool,
    pub flip_y: bool,
}

impl Orient {
    pub const NORMAL: Self = Self { swap: false, flip_x: false, flip_y: false };
    /// Rotate 90° clockwise.
    pub const CW90: Self = Self { swap: true, flip_x: true, flip_y: false };
    pub const R180: Self = Self { swap: false, flip_x: true, flip_y: true };
    /// Rotate 90° counter-clockwise.
    pub const CCW90: Self = Self { swap: true, flip_x: false, flip_y: true };
    pub const MIRROR_Y: Self = Self { swap: false, flip_x: false, flip_y: true };

    /// From the EXIF/TIFF `Orientation` tag (1–8; anything else is normal).
    #[must_use]
    pub const fn from_exif(v: u16) -> Self {
        let (swap, flip_x, flip_y) = match v {
            2 => (false, true, false),
            3 => (false, true, true),
            4 => (false, false, true),
            5 => (true, false, false),
            6 => (true, true, false),
            7 => (true, true, true),
            8 => (true, false, true),
            _ => (false, false, false),
        };
        Self { swap, flip_x, flip_y }
    }

    /// The EXIF value (1–8).
    #[must_use]
    pub const fn to_exif(self) -> u16 {
        match (self.swap, self.flip_x, self.flip_y) {
            (false, false, false) => 1,
            (false, true, false) => 2,
            (false, true, true) => 3,
            (false, false, true) => 4,
            (true, false, false) => 5,
            (true, true, false) => 6,
            (true, true, true) => 7,
            (true, false, true) => 8,
        }
    }

    /// As a matrix on centred coordinates: `F · S`.
    const fn mat(self) -> [[i8; 2]; 2] {
        let sx = if self.flip_x { -1 } else { 1 };
        let sy = if self.flip_y { -1 } else { 1 };
        if self.swap { [[0, sx], [sy, 0]] } else { [[sx, 0], [0, sy]] }
    }

    const fn from_mat(m: [[i8; 2]; 2]) -> Self {
        if m[0][0] != 0 {
            Self { swap: false, flip_x: m[0][0] < 0, flip_y: m[1][1] < 0 }
        } else {
            Self { swap: true, flip_x: m[0][1] < 0, flip_y: m[1][0] < 0 }
        }
    }

    /// `self` first, then `next`.
    #[must_use]
    pub const fn then(self, next: Self) -> Self {
        let a = self.mat();
        let b = next.mat();
        let mut m = [[0i8; 2]; 2];
        let mut i = 0;
        while i < 2 {
            let mut j = 0;
            while j < 2 {
                m[i][j] = b[i][0] * a[0][j] + b[i][1] * a[1][j];
                j += 1;
            }
            i += 1;
        }
        Self::from_mat(m)
    }

    /// The orientation that undoes `self`.
    #[must_use]
    pub const fn inverse(self) -> Self {
        let m = self.mat();
        // Signed permutation matrices are orthogonal: the inverse is the
        // transpose.
        Self::from_mat([[m[0][0], m[1][0]], [m[0][1], m[1][1]]])
    }

    /// Size after applying to a `w` × `h` image.
    #[must_use]
    pub const fn size(self, w: u32, h: u32) -> (u32, u32) {
        if self.swap { (h, w) } else { (w, h) }
    }

    /// Where pixel `(x, y)` of a `w` × `h` image goes.
    #[must_use]
    #[inline]
    pub const fn apply(self, x: u32, y: u32, w: u32, h: u32) -> (u32, u32) {
        let (mut x, mut y, w, h) = if self.swap { (y, x, h, w) } else { (x, y, w, h) };
        if self.flip_x {
            x = w - 1 - x;
        }
        if self.flip_y {
            y = h - 1 - y;
        }
        (x, y)
    }
}

/// How the picture fills the box.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Fit {
    /// Whole picture, aspect kept, centred, letterboxed with the background.
    Contain,
    /// Box filled, aspect kept, the excess cropped evenly.
    Cover,
    /// One image pixel per box pixel; the visible window starts at
    /// (`x`, `y`) of the oriented picture (clamped to the picture).
    Actual { x: u32, y: u32 },
}

/// Where and how a picture is drawn.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placement {
    /// Box size in pixels.
    pub w: u16,
    pub h: u16,
    pub fit: Fit,
    /// User rotation, applied after EXIF orientation.
    pub rotate: Orient,
    /// Honour the picture's EXIF orientation.
    pub exif: bool,
    /// Allow enlarging pictures smaller than the box (`Contain`, `Cover`).
    pub upscale: bool,
    /// Letterbox colour.
    pub background: Rgb,
    /// Ordered dithering to RGB565 (otherwise rounded).
    pub dither: bool,
}

impl Placement {
    /// Contain-fit in a `w` × `h` box, EXIF honoured, dithered, black bars,
    /// small pictures enlarged.
    #[must_use]
    pub const fn fit(w: u16, h: u16) -> Self {
        Self {
            w,
            h,
            fit: Fit::Contain,
            rotate: Orient::NORMAL,
            exif: true,
            upscale: true,
            background: [0, 0, 0],
            dither: true,
        }
    }
}

/// An RGB565 destination: a `w` × `h` box at (`x`, `y`) in a buffer of rows
/// of `stride` pixels.
#[derive(Debug)]
pub struct Target<'f> {
    pub buf: &'f mut [u16],
    pub stride: usize,
    pub x: usize,
    pub y: usize,
}

/// The result of laying a picture out in a box.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Layout {
    /// Output rectangle inside the box (oriented).
    pub dx: u32,
    pub dy: u32,
    pub ow: u32,
    pub oh: u32,
    /// Visible part of the oriented, full-size picture.
    pub rx: u32,
    pub ry: u32,
    pub rw: u32,
    pub rh: u32,
}

/// Lay out an oriented picture of `fw` × `fh` in the box.
#[must_use]
pub fn layout(p: &Placement, fw: u32, fh: u32) -> Layout {
    let (bw, bh) = (u64::from(p.w.max(1)), u64::from(p.h.max(1)));
    let (w, h) = (u64::from(fw.max(1)), u64::from(fh.max(1)));
    let small = w <= bw && h <= bh;
    let fit = match p.fit {
        Fit::Cover if small && !p.upscale => Fit::Contain,
        f => f,
    };
    let (ow, oh, rx, ry, rw, rh) = match fit {
        Fit::Contain => {
            let (ow, oh) = if small && !p.upscale {
                (w, h)
            } else if w * bh >= h * bw {
                (bw, ((h * bw + w / 2) / w).clamp(1, bh))
            } else {
                (((w * bh + h / 2) / h).clamp(1, bw), bh)
            };
            (ow, oh, 0, 0, w, h)
        }
        Fit::Cover => {
            if w * bh >= h * bw {
                let rw = ((bw * h + bh / 2) / bh).clamp(1, w);
                (bw, bh, (w - rw) / 2, 0, rw, h)
            } else {
                let rh = ((bh * w + bw / 2) / bw).clamp(1, h);
                (bw, bh, 0, (h - rh) / 2, w, rh)
            }
        }
        Fit::Actual { x, y } => {
            let ow = bw.min(w);
            let oh = bh.min(h);
            let rx = u64::from(x).min(w - ow);
            let ry = u64::from(y).min(h - oh);
            (ow, oh, rx, ry, ow, oh)
        }
    };
    // All values are bounded by the box or the picture (u32).
    let c = |v: u64| v as u32;
    Layout {
        dx: c((bw - ow) / 2),
        dy: c((bh - oh) / 2),
        ow: c(ow),
        oh: c(oh),
        rx: c(rx),
        ry: c(ry),
        rw: c(rw),
        rh: c(rh),
    }
}

/// 4 × 4 Bayer matrix (0..16).
const BAYER: [[u8; 4]; 4] = [[0, 8, 2, 10], [12, 4, 14, 6], [3, 11, 1, 9], [15, 7, 13, 5]];

/// Pack a pixel with 8 fractional bits per channel (`0..=255 << 8`) to RGB565.
#[inline]
fn pack(r: u32, g: u32, b: u32, x: u32, y: u32, dither: bool) -> u16 {
    if dither {
        let t = u32::from(BAYER[(y & 3) as usize][(x & 3) as usize]) * 16 + 8;
        let q = |v: u32, m: u32| ((v * m / 255 + t) >> 8).min(m);
        ((q(r, 31) << 11) | (q(g, 63) << 5) | q(b, 31)) as u16
    } else {
        let q = |v: u32, m: u32| ((v * m / 255 + 128) >> 8).min(m);
        ((q(r, 31) << 11) | (q(g, 63) << 5) | q(b, 31)) as u16
    }
}

/// RGB888 to RGB565 with rounding.
#[must_use]
pub fn rgb565(c: Rgb) -> u16 {
    pack(u32::from(c[0]) << 8, u32::from(c[1]) << 8, u32::from(c[2]) << 8, 0, 0, false)
}

/// RGB565 back to RGB888 (bits replicated into the low bits).
#[must_use]
pub fn rgb888(c: u16) -> Rgb {
    let (r, g, b) = ((c >> 11) & 31, (c >> 5) & 63, c & 31);
    [((r << 3) | (r >> 2)) as u8, ((g << 2) | (g >> 4)) as u8, ((b << 3) | (b >> 2)) as u8]
}

/// Feed an RGB565 picture (`w` pixels per row, rows top first, upright) to
/// a sink — e.g. a [`Scaler`] that makes a smaller copy of a picture
/// already decoded. `row` is work memory for at least `w` pixels. Returns
/// false if `w`, `row` or `px` do not fit together.
pub fn feed_rgb565(px: &[u16], w: usize, sink: &mut dyn RowSink, row: &mut [Rgb]) -> bool {
    if w == 0 || row.len() < w || px.len() % w != 0 || px.is_empty() {
        return false;
    }
    let h = px.len() / w;
    let (fw, fh) = (w as u32, h as u32);
    sink.begin(&Frame {
        width: fw,
        height: fh,
        full_width: fw,
        full_height: fh,
        orient: Orient::NORMAL,
        bottom_up: false,
        approximate: false,
    });
    for (y, line) in px.chunks_exact(w).enumerate() {
        for (d, &s) in row.iter_mut().zip(line) {
            *d = rgb888(s);
        }
        if sink.row(y as u32, &row[..w]) == Flow::Stop {
            break;
        }
    }
    true
}

/// See the module documentation.
pub struct Scaler<'b, 'f> {
    p: Placement,
    out: Target<'f>,
    work: &'b mut [u32],
    // Set by `begin`.
    active: bool,
    orient: Orient,
    lay: Layout,
    /// Delivered columns and rows that are used.
    cx0: u32,
    cw: u32,
    cy0: u32,
    ch: u32,
    /// Output size in arrival orientation (line length, line count).
    ow: u32,
    oh: u32,
    /// Next output line.
    line: u32,
    /// Next delivered row (relative to `cy0`).
    k: u32,
}

impl<'b, 'f> Scaler<'b, 'f> {
    /// Work words for a box whose longer side is `side` pixels.
    #[must_use]
    pub const fn need_words(side: usize) -> usize {
        6 * side
    }

    /// A scaler drawing into `out`; `work` must hold
    /// `need_words(max(w, h))` words (fewer: `Err` with that number).
    pub fn new(work: &'b mut [u32], p: Placement, out: Target<'f>) -> Result<Self, usize> {
        let need = Self::need_words(usize::from(p.w.max(p.h)));
        let rows = (out.y + usize::from(p.h)).saturating_sub(1);
        let fits_out = p.w > 0
            && p.h > 0
            && out.x + usize::from(p.w) <= out.stride
            && rows * out.stride + out.x + usize::from(p.w) <= out.buf.len();
        if work.len() < need || !fits_out {
            return Err(need);
        }
        Ok(Self {
            p,
            out,
            work,
            active: false,
            orient: Orient::NORMAL,
            lay: Layout::default(),
            cx0: 0,
            cw: 1,
            cy0: 0,
            ch: 1,
            ow: 1,
            oh: 1,
            line: 0,
            k: 0,
        })
    }

    /// Where the picture went (valid after `begin`).
    #[must_use]
    pub fn layout(&self) -> Layout {
        self.lay
    }

    /// Whether every output line was drawn.
    #[must_use]
    pub fn complete(&self) -> bool {
        self.active && self.line >= self.oh
    }

    /// Fill the box with the background.
    pub fn clear(&mut self) {
        let c = rgb565(self.p.background);
        for yy in 0..usize::from(self.p.h) {
            let o = (self.out.y + yy) * self.out.stride + self.out.x;
            self.out.buf[o..o + usize::from(self.p.w)].fill(c);
        }
    }

    fn total_orient(&self, frame_orient: Orient, bottom_up: bool) -> Orient {
        let base = if bottom_up { Orient::MIRROR_Y } else { Orient::NORMAL };
        let base = if self.p.exif { base.then(frame_orient) } else { base };
        base.then(self.p.rotate)
    }

    /// Horizontally resample delivered row `px` to `ow` values (8.8 fixed
    /// point per channel), handing each to `put(index, value)`.
    ///
    /// Shrinking is an exact area average: in units of 1/(`cw`·`ow`) of the
    /// visible width, source pixel `x` covers `[x·ow, (x+1)·ow)` and output
    /// pixel `i` covers `[i·cw, (i+1)·cw)`; a source pixel straddling two
    /// outputs is split by its overlap. Enlarging interpolates linearly
    /// between pixel centres.
    fn hresample(&self, px: &[Rgb], mut put: impl FnMut(usize, u32)) {
        let (cx0, cw, ow) = (self.cx0 as usize, self.cw as usize, self.ow as usize);
        let px = &px[cx0.min(px.len())..(cx0 + cw).min(px.len())];
        if px.is_empty() {
            return;
        }
        let cw = px.len();
        if ow <= cw {
            let (owu, cwu) = (ow as u32, cw as u32);
            let mut s = [0u32; 3];
            let mut i = 0usize;
            let mut edge = cwu;
            let mut x1 = 0u32;
            for p in px {
                let x0 = x1;
                x1 += owu;
                if x1 <= edge {
                    for c in 0..3 {
                        s[c] += u32::from(p[c]) * owu;
                    }
                } else {
                    let a = edge - x0;
                    for c in 0..3 {
                        s[c] += u32::from(p[c]) * a;
                        put(i * 3 + c, ((s[c] << 8) + cwu / 2) / cwu);
                        s[c] = u32::from(p[c]) * (x1 - edge);
                    }
                    i += 1;
                    edge += cwu;
                }
                if x1 == edge {
                    for (c, sc) in s.iter_mut().enumerate() {
                        put(i * 3 + c, ((*sc << 8) + cwu / 2) / cwu);
                        *sc = 0;
                    }
                    i += 1;
                    edge += cwu;
                }
            }
        } else {
            let max = ((cw - 1) as u64) << 16;
            for i in 0..ow {
                let pos = ((2 * i as u64 + 1) * cw as u64 * 65536 / (2 * ow as u64)).saturating_sub(32768);
                let pos = pos.min(max);
                let i0 = (pos >> 16) as usize;
                let i1 = (i0 + 1).min(cw - 1);
                let f = ((pos >> 8) & 255) as u32;
                let (a, b) = (px[i0], px[i1]);
                for c in 0..3 {
                    put(i * 3 + c, u32::from(a[c]) * (256 - f) + u32::from(b[c]) * f);
                }
            }
        }
    }

    /// Draw output line `j` from 8.8 values: `a` and `b` mixed with weight
    /// `f`/256 of `b`.
    fn emit(
        out: &mut Target<'_>,
        p: &Placement,
        lay: &Layout,
        orient: Orient,
        (ow, oh): (u32, u32),
        j: u32,
        (a, b, f): (&[u32], &[u32], u32),
    ) {
        for i in 0..ow {
            let (x, y) = orient.apply(i, j, ow, oh);
            let (bx, by) = (lay.dx + x, lay.dy + y);
            let (sx, sy) = (out.x + bx as usize, out.y + by as usize);
            let k = i as usize * 3;
            let v =
                |c: usize| if f == 0 { a[k + c] } else { (a[k + c] * (256 - f) + b[k + c] * f + 128) >> 8 };
            let c = pack(v(0), v(1), v(2), sx as u32, sy as u32, p.dither);
            if let Some(d) = out.buf.get_mut(sy * out.stride + sx) {
                *d = c;
            }
        }
    }

    /// Source row (relative) feeding output line `j` when enlarging rows:
    /// position in 16.16.
    fn vpos(&self, j: u32) -> u64 {
        let (ch, oh) = (u64::from(self.ch), u64::from(self.oh));
        ((2 * u64::from(j) + 1) * ch * 65536 / (2 * oh)).saturating_sub(32768).min((ch - 1) << 16)
    }
}

impl RowSink for Scaler<'_, '_> {
    fn wanted(&self, full_w: u32, full_h: u32, orient: Orient) -> (u32, u32) {
        let o = if self.p.exif { orient.then(self.p.rotate) } else { self.p.rotate };
        let (fw, fh) = o.size(full_w, full_h);
        let lay = layout(&self.p, fw, fh);
        // Delivered pixels per output pixel ≥ 1 along each stored axis.
        let need = |out: u32, full: u32, vis: u32| -> u32 {
            let v = (u64::from(out) * u64::from(full)).div_ceil(u64::from(vis.max(1)));
            v.clamp(1, u64::from(full)) as u32
        };
        let (nw, nh) = (need(lay.ow, fw, lay.rw), need(lay.oh, fh, lay.rh));
        if o.swap { (nh, nw) } else { (nw, nh) }
    }

    fn begin(&mut self, f: &Frame) {
        let orient = self.total_orient(f.orient, f.bottom_up);
        let (fw, fh) = (f.full_width.max(1), f.full_height.max(1));
        let (ofw, ofh) = orient.size(fw, fh);
        let lay = layout(&self.p, ofw, ofh);
        // Visible rectangle back in arrival orientation (full pixels).
        let inv = orient.inverse();
        let (ax0, ay0) = inv.apply(lay.rx, lay.ry, ofw, ofh);
        let (ax1, ay1) = inv.apply(lay.rx + lay.rw - 1, lay.ry + lay.rh - 1, ofw, ofh);
        let (ax0, ax1) = (ax0.min(ax1), ax0.max(ax1) + 1);
        let (ay0, ay1) = (ay0.min(ay1), ay0.max(ay1) + 1);
        // Full to delivered pixels.
        let (dw, dh) = (u64::from(f.width.max(1)), u64::from(f.height.max(1)));
        let lo = |a: u32, d: u64, full: u32| (u64::from(a) * d / u64::from(full)) as u32;
        let hi = |a: u32, d: u64, full: u32| (u64::from(a) * d).div_ceil(u64::from(full)) as u32;
        self.cx0 = lo(ax0, dw, fw);
        self.cw = hi(ax1, dw, fw).saturating_sub(self.cx0).max(1);
        self.cy0 = lo(ay0, dh, fh);
        self.ch = hi(ay1, dh, fh).saturating_sub(self.cy0).max(1);
        let (ow, oh) = orient.inverse().size(lay.ow, lay.oh);
        self.ow = ow.max(1);
        self.oh = oh.max(1);
        self.orient = orient;
        self.lay = lay;
        self.line = 0;
        self.k = 0;
        self.active = true;
        self.clear();
        self.work.fill(0);
    }

    fn row(&mut self, y: u32, px: &[Rgb]) -> Flow {
        if !self.active || self.line >= self.oh {
            return Flow::Stop;
        }
        if y < self.cy0 {
            return Flow::Continue;
        }
        let k = y - self.cy0;
        if k >= self.ch {
            return Flow::Stop;
        }
        self.k = k + 1;
        let n = self.ow as usize * 3;
        let work = core::mem::take(&mut self.work);
        if self.oh <= self.ch {
            // Shrinking rows: area weights as across a row (units of
            // 1/(ch·oh)); a row straddling two output lines adds to the
            // current line's accumulators and the next one's.
            let (acc, rest) = work.split_at_mut(n);
            let (next, _) = rest.split_at_mut(n);
            let (oh, ch) = (self.oh, self.ch);
            let r0 = k * oh;
            let r1 = r0 + oh;
            let edge = (self.line + 1) * ch;
            let w1 = r1.min(edge) - r0;
            let w2 = r1 - r0 - w1;
            self.hresample(px, |i, v| {
                acc[i] += v * w1;
                next[i] += v * w2;
            });
            if r1 >= edge {
                for v in acc.iter_mut() {
                    *v = (*v + ch / 2) / ch;
                }
                Self::emit(
                    &mut self.out,
                    &self.p,
                    &self.lay,
                    self.orient,
                    (self.ow, oh),
                    self.line,
                    (acc, acc, 0),
                );
                acc.copy_from_slice(next);
                next.fill(0);
                self.line += 1;
            }
        } else {
            // Enlarging rows: keep the previous and current resampled rows.
            let (prev, rest) = work.split_at_mut(n);
            let (cur, _) = rest.split_at_mut(n);
            if k > 0 {
                prev.copy_from_slice(cur);
            }
            self.hresample(px, |i, v| cur[i] = v);
            if k == 0 {
                prev.copy_from_slice(cur);
            }
            while self.line < self.oh {
                let pos = self.vpos(self.line);
                let k0 = (pos >> 16) as u32;
                let ready = k >= (k0 + 1).min(self.ch - 1);
                if !ready {
                    break;
                }
                let f = ((pos >> 8) & 255) as u32;
                let (a, b): (&[u32], &[u32]) = if k0 + 1 == k { (prev, cur) } else { (cur, cur) };
                let o = (self.ow, self.oh);
                Self::emit(&mut self.out, &self.p, &self.lay, self.orient, o, self.line, (a, b, f));
                self.line += 1;
            }
        }
        self.work = work;
        if self.line >= self.oh { Flow::Stop } else { Flow::Continue }
    }
}

#[cfg(test)]
#[allow(clippy::drop_non_drop)] // ends the scaler's borrow of the buffer before reading it
mod tests {
    use super::*;

    #[test]
    fn orient_group() {
        for v in 1..=8u16 {
            let o = Orient::from_exif(v);
            assert_eq!(o.to_exif(), v);
            assert_eq!(o.then(o.inverse()), Orient::NORMAL);
            // apply/inverse round trip on a 5×3 image.
            let (w, h) = (5, 3);
            let (ow, oh) = o.size(w, h);
            for y in 0..h {
                for x in 0..w {
                    let (a, b) = o.apply(x, y, w, h);
                    assert!(a < ow && b < oh);
                    assert_eq!(o.inverse().apply(a, b, ow, oh), (x, y));
                }
            }
        }
        // CW90 maps the top-left corner to the top-right one.
        assert_eq!(Orient::CW90.apply(0, 0, 4, 2), (1, 0));
        assert_eq!(Orient::CW90.then(Orient::CW90), Orient::R180);
        assert_eq!(Orient::CW90.then(Orient::CCW90), Orient::NORMAL);
        // Composition matches applying one after the other.
        for a in 1..=8u16 {
            for b in 1..=8u16 {
                let (oa, ob) = (Orient::from_exif(a), Orient::from_exif(b));
                let c = oa.then(ob);
                let (w, h) = (4, 3);
                let (w1, h1) = oa.size(w, h);
                for y in 0..h {
                    for x in 0..w {
                        let (x1, y1) = oa.apply(x, y, w, h);
                        assert_eq!(ob.apply(x1, y1, w1, h1), c.apply(x, y, w, h));
                    }
                }
            }
        }
    }

    #[test]
    fn layouts() {
        let p = Placement::fit(128, 128);
        assert_eq!(
            layout(&p, 1024, 768),
            Layout { dx: 0, dy: 16, ow: 128, oh: 96, rx: 0, ry: 0, rw: 1024, rh: 768 }
        );
        let c = Placement { fit: Fit::Cover, ..p };
        assert_eq!(
            layout(&c, 1024, 768),
            Layout { dx: 0, dy: 0, ow: 128, oh: 128, rx: 128, ry: 0, rw: 768, rh: 768 }
        );
        let a = Placement { fit: Fit::Actual { x: 2000, y: 10 }, ..p };
        assert_eq!(
            layout(&a, 1024, 768),
            Layout { dx: 0, dy: 0, ow: 128, oh: 128, rx: 896, ry: 10, rw: 128, rh: 128 }
        );
        let n = Placement { upscale: false, ..p };
        assert_eq!(
            layout(&n, 40, 20),
            Layout { dx: 44, dy: 54, ow: 40, oh: 20, rx: 0, ry: 0, rw: 40, rh: 20 }
        );
    }

    struct Img<'a> {
        w: u32,
        h: u32,
        px: &'a [Rgb],
    }

    fn run(s: &mut Scaler<'_, '_>, img: &Img<'_>, bottom_up: bool, orient: Orient) {
        s.begin(&Frame {
            width: img.w,
            height: img.h,
            full_width: img.w,
            full_height: img.h,
            orient,
            bottom_up,
            approximate: false,
        });
        for y in 0..img.h {
            let sy = if bottom_up { img.h - 1 - y } else { y };
            let r = &img.px[(sy * img.w) as usize..((sy + 1) * img.w) as usize];
            if s.row(y, r) == Flow::Stop {
                break;
            }
        }
    }

    #[test]
    fn identity_and_rotation_exact() {
        // A 6×4 picture into a 6×4 box at 1:1: exact copy (no dither).
        let mut px = std::vec::Vec::new();
        for y in 0..4u8 {
            for x in 0..6u8 {
                px.push([x * 40, y * 80, 255 - x * 40]);
            }
        }
        let img = Img { w: 6, h: 4, px: &px };
        let p = Placement { dither: false, ..Placement::fit(6, 4) };
        let mut fb = [0u16; 24];
        let mut work = [0u32; 36];
        let mut s = Scaler::new(&mut work, p, Target { buf: &mut fb, stride: 6, x: 0, y: 0 }).unwrap();
        run(&mut s, &img, false, Orient::NORMAL);
        assert!(s.complete());
        drop(s);
        for (i, c) in fb.iter().enumerate() {
            assert_eq!(*c, rgb565(px[i]));
        }
        // Bottom-up delivery gives the same picture.
        let mut fb2 = [0u16; 24];
        let mut s = Scaler::new(&mut work, p, Target { buf: &mut fb2, stride: 6, x: 0, y: 0 }).unwrap();
        run(&mut s, &img, true, Orient::NORMAL);
        drop(s);
        assert_eq!(fb, fb2);
        // Rotated 90° CW into a 4×6 box.
        let p = Placement { dither: false, rotate: Orient::CW90, ..Placement::fit(4, 6) };
        let mut fb3 = [0u16; 24];
        let mut s = Scaler::new(&mut work, p, Target { buf: &mut fb3, stride: 4, x: 0, y: 0 }).unwrap();
        run(&mut s, &img, false, Orient::NORMAL);
        drop(s);
        for y in 0..4u32 {
            for x in 0..6u32 {
                let (rx, ry) = Orient::CW90.apply(x, y, 6, 4);
                assert_eq!(fb3[(ry * 4 + rx) as usize], rgb565(px[(y * 6 + x) as usize]));
            }
        }
    }

    #[test]
    fn box_down_and_up() {
        // A flat colour stays flat at any scale and in any mode.
        let px = std::vec![[200u8, 100, 50]; 37 * 23];
        let img = Img { w: 37, h: 23, px: &px };
        for &(bw, bh) in &[(10u16, 10u16), (128, 128), (5, 40)] {
            for fit in [Fit::Contain, Fit::Cover, Fit::Actual { x: 3, y: 3 }] {
                let p = Placement { dither: false, fit, background: [0, 0, 255], ..Placement::fit(bw, bh) };
                let mut fb = std::vec![0u16; usize::from(bw) * usize::from(bh)];
                let mut work = std::vec![0u32; Scaler::need_words(usize::from(bw.max(bh)))];
                let mut s =
                    Scaler::new(&mut work, p, Target { buf: &mut fb, stride: usize::from(bw), x: 0, y: 0 })
                        .unwrap();
                run(&mut s, &img, false, Orient::CW90);
                assert!(s.complete());
                let l = s.layout();
                drop(s);
                for y in 0..u32::from(bh) {
                    for x in 0..u32::from(bw) {
                        let inside = x >= l.dx && x < l.dx + l.ow && y >= l.dy && y < l.dy + l.oh;
                        let want = if inside { rgb565([200, 100, 50]) } else { rgb565([0, 0, 255]) };
                        assert_eq!(
                            fb[(y * u32::from(bw) + x) as usize],
                            want,
                            "{bw}x{bh} {fit:?} at {x},{y}"
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn dither_mean() {
        // Dithering a flat mid grey averages to the input over a 4×4 tile.
        let px = std::vec![[127u8, 127, 127]; 16];
        let img = Img { w: 4, h: 4, px: &px };
        let mut fb = [0u16; 16];
        let mut work = [0u32; 24];
        let mut s =
            Scaler::new(&mut work, Placement::fit(4, 4), Target { buf: &mut fb, stride: 4, x: 0, y: 0 })
                .unwrap();
        run(&mut s, &img, false, Orient::NORMAL);
        drop(s);
        let mean_r: f64 = fb.iter().map(|c| f64::from(c >> 11)).sum::<f64>() / 16.0;
        assert!((mean_r - 127.0 * 31.0 / 255.0).abs() < 0.07, "{mean_r}");
    }

    #[test]
    fn rgb565_round_trip_and_reduce() {
        for c in [0u16, 0xFFFF, 0xF800, 0x07E0, 0x001F, 0x8410, 0x1234] {
            assert_eq!(rgb565(rgb888(c)), c);
        }
        // An 8 × 8 picture: left half red, right half blue; reduced to 4 × 4.
        let red = rgb565([255, 0, 0]);
        let blue = rgb565([0, 0, 255]);
        let src: std::vec::Vec<u16> = (0..64).map(|i| if i % 8 < 4 { red } else { blue }).collect();
        let mut fb = [0u16; 16];
        let mut work = [0u32; Scaler::need_words(4)];
        let p = Placement { dither: false, ..Placement::fit(4, 4) };
        let mut s = Scaler::new(&mut work, p, Target { buf: &mut fb, stride: 4, x: 0, y: 0 }).unwrap();
        let mut row = [[0u8; 3]; 8];
        assert!(feed_rgb565(&src, 8, &mut s, &mut row));
        assert!(s.complete());
        drop(s);
        for y in 0..4 {
            assert_eq!(&fb[y * 4..y * 4 + 4], &[red, red, blue, blue]);
        }
        let mut s = Scaler::new(&mut work, p, Target { buf: &mut fb, stride: 4, x: 0, y: 0 }).unwrap();
        assert!(!feed_rgb565(&src, 8, &mut s, &mut row[..4]));
        assert!(!feed_rgb565(&src[..60], 8, &mut s, &mut row));
    }
}
