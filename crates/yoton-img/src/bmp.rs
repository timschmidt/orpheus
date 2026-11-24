//! BMP decoder: OS/2 core and Windows INFO/V4/V5 headers, 1/2/4/8-bit
//! palette, 16/24/32-bit (default masks or bit fields, with alpha), RLE8 and
//! RLE4. Rows stream in file order (bottom row first unless the height is
//! negative); the scaler flips them.
//!
//! Arena: `3 × width` bytes (one RGB row) + 768 bytes of palette.

use crate::{
    Arena, ByteSource, Error, Flow, Format, Frame, Info, Options, Orient, Rgb, RowSink, Step, Unsupported,
    View, blend, check_dims, peek, tryp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Comp {
    Raw,
    Rle8,
    Rle4,
}

#[derive(Debug, Clone, Copy, Default)]
struct Mask {
    mask: u32,
    shift: u32,
    max: u32,
}

impl Mask {
    fn new(mask: u32) -> Self {
        if mask == 0 {
            return Self::default();
        }
        let shift = mask.trailing_zeros();
        let max = mask >> shift;
        // Only contiguous masks are meaningful; others are taken as given.
        Self { mask, shift, max }
    }

    #[inline]
    fn get(&self, v: u32) -> u8 {
        if self.max == 0 {
            return 0;
        }
        let x = (v & self.mask) >> self.shift;
        ((u64::from(x) * 255 + u64::from(self.max) / 2) / u64::from(self.max)) as u8
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Header,
    Seek,
    Rows,
    Done,
}

pub struct Bmp<'a> {
    arena: Option<Arena<'a>>,
    used: usize,
    opts: Options,
    info: Option<Info>,
    phase: Phase,
    start: u64,
    data_at: u64,
    row: &'a mut [Rgb],
    pal: &'a mut [Rgb],
    pal_len: usize,
    bpp: u32,
    comp: Comp,
    masks: [Mask; 4],
    stride: u32,
    /// Pixel and byte position inside the current row.
    x: u32,
    xb: u32,
    y: u32,
    /// RLE: remaining rows are blank (end of bitmap seen).
    rle_end: bool,
}

const ROWS_PER_STEP: u32 = 8;

impl<'a> Bmp<'a> {
    #[must_use]
    pub fn new(arena: Arena<'a>, opts: Options) -> Self {
        Self {
            arena: Some(arena),
            used: 0,
            opts,
            info: None,
            phase: Phase::Header,
            start: 0,
            data_at: 0,
            row: &mut [],
            pal: &mut [],
            pal_len: 0,
            bpp: 0,
            comp: Comp::Raw,
            masks: [Mask::default(); 4],
            stride: 0,
            x: 0,
            xb: 0,
            y: 0,
            rle_end: false,
        }
    }

    #[must_use]
    pub fn info(&self) -> Option<Info> {
        self.info
    }

    #[must_use]
    pub fn arena_used(&self) -> usize {
        self.used
    }

    fn header(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        let v = tryp!(peek(src, 18));
        if !v.starts_with_at(0, b"BM") {
            return Err(Error::UnknownFormat);
        }
        let off = v.u32_le(10).unwrap_or(0);
        let dib = v.u32_le(14).unwrap_or(0) as usize;
        if !(12..=124).contains(&dib) {
            return Err(Error::Corrupt);
        }
        let v = tryp!(peek(src, 14 + dib + 12));
        let core = dib < 40;
        let (w, h, bpp, compression, clr_used) = if core {
            let w = u32::from(v.u16_le(18).unwrap_or(0));
            let h = i32::from(v.u16_le(20).unwrap_or(0));
            (w as i32, h, u32::from(v.u16_le(24).unwrap_or(0)), 0, 0)
        } else {
            (
                v.u32_le(18).unwrap_or(0) as i32,
                v.u32_le(22).unwrap_or(0) as i32,
                u32::from(v.u16_le(28).unwrap_or(0)),
                v.u32_le(30).unwrap_or(0),
                v.u32_le(46).unwrap_or(0),
            )
        };
        let bottom_up = h > 0;
        let (w, h) = (w.unsigned_abs(), h.unsigned_abs());
        check_dims(w, h)?;
        let comp = match (compression, bpp) {
            (0, 1 | 2 | 4 | 8 | 16 | 24 | 32) => Comp::Raw,
            (3 | 6, 16 | 32) => Comp::Raw,
            (1, 8) => Comp::Rle8,
            (2, 4) => Comp::Rle4,
            (0 | 3 | 6, _) | (1 | 2, _) => return Err(Error::Corrupt),
            _ => return Err(Error::Unsupported(Unsupported::Compression)),
        };
        // Colour masks.
        let mut masks = match bpp {
            16 => [0x7C00, 0x03E0, 0x001F, 0],
            32 => [0x00FF_0000, 0x0000_FF00, 0x0000_00FF, 0],
            _ => [0; 4],
        };
        let mut hdr_end = 14 + dib;
        if compression == 3 || compression == 6 {
            if dib == 40 {
                // Masks follow the 40-byte header.
                let n = if compression == 6 { 4 } else { 3 };
                let v = tryp!(peek(src, 14 + 40 + 16));
                for (i, m) in masks.iter_mut().enumerate().take(n) {
                    *m = v.u32_le(54 + 4 * i).unwrap_or(0);
                }
                hdr_end += 4 * n;
            } else if dib >= 52 {
                let n = if dib >= 56 { 4 } else { 3 };
                for (i, m) in masks.iter_mut().enumerate().take(n) {
                    *m = v.u32_le(54 + 4 * i).unwrap_or(0);
                }
            }
        } else if bpp == 32 && dib >= 56 && compression == 0 {
            // V4/V5 alpha mask for BI_RGB is not used (as most readers do).
        }
        // Palette.
        let (pal_len, entry) = if bpp <= 8 {
            let max = 1usize << bpp;
            let n = if clr_used == 0 || clr_used as usize > max { max } else { clr_used as usize };
            (n, if core { 3 } else { 4 })
        } else {
            (0, 4)
        };
        let pal_bytes = pal_len * entry;
        let v = tryp!(peek(src, hdr_end + pal_bytes));
        let mut arena = self.arena.take().ok_or(Error::Corrupt)?;
        let pal_n = if bpp <= 8 { 256 } else { 0 };
        crate::reserve(&arena, 3 * (pal_n + w as usize) + 2)?;
        self.pal = arena.slice::<Rgb>(pal_n)?;
        self.row = arena.slice::<Rgb>(w as usize)?;
        self.used = arena.used();
        for i in 0..pal_len {
            let o = hdr_end + i * entry;
            self.pal[i] = [v.byte(o + 2), v.byte(o + 1), v.byte(o)];
        }
        self.pal_len = pal_len;
        self.masks = [Mask::new(masks[0]), Mask::new(masks[1]), Mask::new(masks[2]), Mask::new(masks[3])];
        self.bpp = bpp;
        self.comp = comp;
        self.stride = (w * bpp).div_ceil(32) * 4;
        self.start = src.position();
        let data = if (off as usize) < hdr_end + pal_bytes { hdr_end + pal_bytes } else { off as usize };
        self.data_at = self.start + data as u64;
        let alpha = self.masks[3].max != 0;
        let info = Info {
            format: Format::Bmp,
            width: w,
            height: h,
            orient: Orient::NORMAL,
            interlaced: false,
            alpha,
        };
        self.info = Some(info);
        sink.begin(&Frame {
            width: w,
            height: h,
            full_width: w,
            full_height: h,
            orient: Orient::NORMAL,
            bottom_up,
            approximate: false,
        });
        self.phase = Phase::Seek;
        Ok(Step::Progress)
    }

    fn pal(&self, i: u32) -> Rgb {
        self.pal.get(i as usize).copied().unwrap_or([0, 0, 0])
    }

    /// Deliver the current row; `true` when the sink wants no more.
    fn emit(&mut self, sink: &mut dyn RowSink) -> bool {
        let y = self.y;
        self.y += 1;
        self.x = 0;
        self.xb = 0;
        let stop = sink.row(y, self.row) == Flow::Stop;
        if self.comp != Comp::Raw {
            let c = self.pal(0);
            self.row.fill(c);
        }
        stop
    }

    fn raw_rows(
        &mut self,
        src: &mut dyn ByteSource,
        sink: &mut dyn RowSink,
        w: u32,
        h: u32,
    ) -> Result<Step, Error> {
        let bg = self.opts.background;
        let v = View::of(src);
        let mut pos = 0usize;
        let mut rows = 0;
        let bpp = self.bpp;
        let used_bytes = (w * bpp).div_ceil(8);
        let mut result = Ok(Step::Progress);
        'rows: while self.y < h {
            // Pixels.
            while self.x < w {
                if bpp < 8 {
                    let Some(b) = v.get(pos) else { break 'rows };
                    let per = 8 / bpp;
                    let mask = (1u32 << bpp) - 1;
                    for k in 0..per {
                        if self.x >= w {
                            break;
                        }
                        let idx = (u32::from(b) >> (8 - bpp * (k + 1))) & mask;
                        self.row[self.x as usize] = self.pal(idx);
                        self.x += 1;
                    }
                    pos += 1;
                    self.xb += 1;
                } else {
                    let n = (bpp / 8) as usize;
                    if v.len() - pos < n {
                        break 'rows;
                    }
                    let px = match bpp {
                        8 => self.pal(u32::from(v.byte(pos))),
                        24 => [v.byte(pos + 2), v.byte(pos + 1), v.byte(pos)],
                        _ => {
                            let val = if bpp == 16 {
                                u32::from(v.u16_le(pos).unwrap_or(0))
                            } else {
                                v.u32_le(pos).unwrap_or(0)
                            };
                            let m = &self.masks;
                            let c = [m[0].get(val), m[1].get(val), m[2].get(val)];
                            if m[3].max != 0 { blend(c, m[3].get(val), bg) } else { c }
                        }
                    };
                    self.row[self.x as usize] = px;
                    self.x += 1;
                    pos += n;
                    self.xb += n as u32;
                }
            }
            // Padding.
            let pad = (self.stride - used_bytes.max(self.xb).min(self.stride)) as usize;
            let skip = pad.min(v.len() - pos);
            pos += skip;
            self.xb += skip as u32;
            if self.xb < self.stride && self.y + 1 < h {
                break;
            }
            rows += 1;
            if self.emit(sink) {
                self.y = h;
            }
            if rows >= ROWS_PER_STEP {
                break;
            }
        }
        let rest = v.len() - pos;
        if self.y < h && rows == 0 {
            result = if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(rest + 1)) };
        }
        src.consume(pos);
        result
    }

    fn rle_rows(
        &mut self,
        src: &mut dyn ByteSource,
        sink: &mut dyn RowSink,
        w: u32,
        h: u32,
    ) -> Result<Step, Error> {
        let v = View::of(src);
        let mut pos = 0usize;
        let mut rows = 0;
        let rle4 = self.comp == Comp::Rle4;
        let mut result = Ok(Step::Progress);
        while self.y < h && rows < ROWS_PER_STEP {
            if self.rle_end {
                rows += 1;
                if self.emit(sink) {
                    self.y = h;
                }
                continue;
            }
            if v.len() - pos < 2 {
                result = if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(2)) };
                break;
            }
            let (n, c) = (u32::from(v.byte(pos)), v.byte(pos + 1));
            if n > 0 {
                for k in 0..n {
                    if self.x >= w {
                        break;
                    }
                    let idx = if rle4 { if k & 1 == 0 { c >> 4 } else { c & 15 } } else { c };
                    self.row[self.x as usize] = self.pal(u32::from(idx));
                    self.x += 1;
                }
                pos += 2;
                continue;
            }
            match c {
                0 => {
                    pos += 2;
                    rows += 1;
                    if self.emit(sink) {
                        self.y = h;
                    }
                }
                1 => {
                    pos += 2;
                    self.rle_end = true;
                    rows += 1;
                    if self.emit(sink) {
                        self.y = h;
                    }
                }
                2 => {
                    if v.len() - pos < 4 {
                        result = if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(4)) };
                        break;
                    }
                    let (dx, dy) = (u32::from(v.byte(pos + 2)), u32::from(v.byte(pos + 3)));
                    pos += 4;
                    let x = self.x + dx;
                    for _ in 0..dy {
                        rows += 1;
                        if self.emit(sink) || self.y >= h {
                            self.y = h;
                            break;
                        }
                    }
                    self.x = x.min(w);
                }
                k => {
                    let k = u32::from(k);
                    let bytes = if rle4 { k.div_ceil(2) } else { k };
                    let bytes = (bytes + 1) & !1;
                    let need = 2 + bytes as usize;
                    if v.len() - pos < need {
                        result = if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(need)) };
                        break;
                    }
                    for i in 0..k {
                        if self.x >= w {
                            break;
                        }
                        let idx = if rle4 {
                            let b = v.byte(pos + 2 + (i / 2) as usize);
                            if i & 1 == 0 { b >> 4 } else { b & 15 }
                        } else {
                            v.byte(pos + 2 + i as usize)
                        };
                        self.row[self.x as usize] = self.pal(u32::from(idx));
                        self.x += 1;
                    }
                    pos += need;
                }
            }
        }
        src.consume(pos);
        result
    }

    pub fn step(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        match self.phase {
            Phase::Header => self.header(src, sink),
            Phase::Seek => {
                let here = src.position();
                if here < self.data_at {
                    let k = usize::try_from(self.data_at - here).unwrap_or(usize::MAX).min(src.available());
                    src.consume(k);
                    if src.position() < self.data_at {
                        return if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(1)) };
                    }
                }
                if self.comp != Comp::Raw {
                    let c = self.pal(0);
                    self.row.fill(c);
                }
                self.phase = Phase::Rows;
                Ok(Step::Progress)
            }
            Phase::Rows => {
                let Some(info) = self.info else { return Err(Error::Corrupt) };
                let r = if self.comp == Comp::Raw {
                    self.raw_rows(src, sink, info.width, info.height)
                } else {
                    self.rle_rows(src, sink, info.width, info.height)
                };
                if self.y >= info.height {
                    self.phase = Phase::Done;
                    return Ok(Step::Done);
                }
                r
            }
            Phase::Done => Ok(Step::Done),
        }
    }
}
