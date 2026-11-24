//! PNG decoder: every colour type and bit depth, tRNS, Adam7.
//!
//! Non-interlaced pictures stream one scan line at a time. Adam7 pictures
//! are reduced: the decoder picks the coarsest pixel grid (every 8th, 4th,
//! 2nd or every pixel) that is still at least the size the sink wants and
//! fits the arena; the 1/8 grid is pass 1 alone and streams, finer grids
//! are collected from passes 1–3, 1–5 or 1–7 into an RGB buffer of the grid
//! size, then delivered. Gamma, colour profiles and ancillary chunks are
//! ignored; alpha and tRNS blend over the background. CRCs and the Adler-32
//! are not checked (corrupt data still cannot crash the decoder).
//!
//! Arena: 32 KiB window + 3.6 KiB tables + 1 KiB palette + 2 scan lines
//! (filter byte + `width × bytes per pixel`) + `3 × width` RGB row
//! (+ `3 × (w/L) × (h/L)` for a stored Adam7 grid).

use crate::inflate::{Inflate, Input, Status};
use crate::{
    Arena, ByteSource, Error, Flow, Format, Frame, Info, Options, Orient, Rgb, RowSink, Step, View, blend,
    check_dims, peek, tryp,
};

const ADAM7: [(u32, u32, u32, u32); 7] =
    [(0, 0, 8, 8), (4, 0, 8, 8), (0, 4, 4, 8), (2, 0, 4, 4), (0, 2, 2, 4), (1, 0, 2, 2), (0, 1, 1, 2)];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Signature,
    Chunks,
    /// Skipping the rest of a chunk (data and CRC).
    Skip(u32),
    Idat,
    /// Delivering a stored Adam7 grid.
    Grid(u32),
    Done,
}

#[derive(Debug, Clone, Copy, Default)]
struct Hdr {
    w: u32,
    h: u32,
    depth: u8,
    ct: u8,
    interlace: bool,
}

impl Hdr {
    fn channels(&self) -> u32 {
        match self.ct {
            2 => 3,
            4 => 2,
            6 => 4,
            _ => 1,
        }
    }
    fn row_bytes(&self, w: u32) -> usize {
        (w as usize * self.channels() as usize * usize::from(self.depth)).div_ceil(8)
    }
    fn bpp(&self) -> usize {
        ((self.channels() as usize * usize::from(self.depth)) / 8).max(1)
    }
}

pub struct Png<'a> {
    arena: Option<Arena<'a>>,
    used: usize,
    opts: Options,
    info: Option<Info>,
    phase: Phase,
    hdr: Hdr,
    pal: &'a mut [[u8; 4]],
    pal_len: usize,
    /// tRNS colour key (raw samples) for grey/RGB.
    key: Option<[u16; 3]>,
    inflate: Option<Inflate<'a>>,
    cur: &'a mut [u8],
    prev: &'a mut [u8],
    rgb: &'a mut [Rgb],
    grid: &'a mut [Rgb],
    /// Bytes left in the current IDAT chunk; no more IDAT chunks.
    idat_left: u32,
    idat_done: bool,
    /// Adam7 grid step (1, 2, 4, 8); 0 for non-interlaced.
    level: u32,
    pass: usize,
    /// Row within the pass and bytes of the current line filled.
    row: u32,
    filled: usize,
    /// Rows delivered.
    out_y: u32,
}

/// Bytes from the IDAT chunks of the source view.
struct IdatIn<'v> {
    v: View<'v>,
    pos: usize,
    left: u32,
    done: bool,
    src_end: bool,
}

impl Input for IdatIn<'_> {
    fn byte(&mut self) -> Option<u8> {
        loop {
            if self.left > 0 {
                let b = self.v.get(self.pos)?;
                self.pos += 1;
                self.left -= 1;
                return Some(b);
            }
            if self.done {
                return None;
            }
            // CRC of this chunk, then the next chunk's length and type.
            let len = self.v.u32_be(self.pos + 4)?;
            if !self.v.starts_with_at(self.pos + 8, b"IDAT") {
                if self.v.len() >= self.pos + 12 {
                    self.done = true;
                }
                return None;
            }
            self.pos += 12;
            self.left = len;
        }
    }

    fn ended(&self) -> bool {
        self.done
            || (self.src_end && (self.pos >= self.v.len() || self.left == 0 && self.v.len() < self.pos + 12))
    }
}

const ROWS_PER_STEP: u32 = 8;

impl<'a> Png<'a> {
    #[must_use]
    pub fn new(arena: Arena<'a>, opts: Options) -> Self {
        Self {
            arena: Some(arena),
            used: 0,
            opts,
            info: None,
            phase: Phase::Signature,
            hdr: Hdr::default(),
            pal: &mut [],
            pal_len: 0,
            key: None,
            inflate: None,
            cur: &mut [],
            prev: &mut [],
            rgb: &mut [],
            grid: &mut [],
            idat_left: 0,
            idat_done: false,
            level: 0,
            pass: 0,
            row: 0,
            filled: 0,
            out_y: 0,
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

    /// Size of pass `p` (or the whole picture for non-interlaced).
    fn pass_size(&self, p: usize) -> (u32, u32) {
        let (w, h) = (self.hdr.w, self.hdr.h);
        if self.level == 0 {
            return (w, h);
        }
        let (x0, y0, dx, dy) = ADAM7[p];
        let f = |n: u32, o: u32, d: u32| if n > o { (n - o).div_ceil(d) } else { 0 };
        (f(w, x0, dx), f(h, y0, dy))
    }

    /// Passes making up grid level `l`.
    fn passes(l: u32) -> usize {
        match l {
            8 => 1,
            4 => 3,
            2 => 5,
            _ => 7,
        }
    }

    fn need(&self, level: u32) -> usize {
        let hdr = &self.hdr;
        let line_w = if level == 8 { hdr.w.div_ceil(8) } else { hdr.w };
        let line = hdr.row_bytes(line_w) + 1;
        let grid = if level == 0 || level == 8 {
            0
        } else {
            hdr.w.div_ceil(level) as usize * hdr.h.div_ceil(level) as usize * 3
        };
        Inflate::NEED + 2 * line + 3 * line_w as usize + grid + 16
    }

    fn start_idat(&mut self, sink: &mut dyn RowSink) -> Result<(), Error> {
        let hdr = self.hdr;
        let mut arena = self.arena.take().ok_or(Error::Corrupt)?;
        let cap = arena.remaining();
        let level = if hdr.interlace {
            let (ww, wh) = sink.wanted(hdr.w, hdr.h, Orient::NORMAL);
            let mut l = 8;
            for cand in [8u32, 4, 2, 1] {
                l = cand;
                if hdr.w.div_ceil(cand) >= ww && hdr.h.div_ceil(cand) >= wh {
                    break;
                }
            }
            while l < 8 && self.need(l) > cap {
                l *= 2;
            }
            l
        } else {
            0
        };
        let need = self.need(level);
        if need > cap {
            return Err(Error::Arena { needed: arena.used() + need, capacity: arena.capacity() });
        }
        self.level = level;
        let line_w = if level == 8 { hdr.w.div_ceil(8) } else { hdr.w };
        let line = hdr.row_bytes(line_w) + 1;
        self.inflate = Some(Inflate::new(&mut arena)?);
        self.cur = arena.slice::<u8>(line)?;
        self.prev = arena.slice::<u8>(line)?;
        self.rgb = arena.slice::<Rgb>(line_w as usize)?;
        if level != 0 && level != 8 {
            self.grid =
                arena.slice::<Rgb>(hdr.w.div_ceil(level) as usize * hdr.h.div_ceil(level) as usize)?;
        }
        self.used = arena.used();
        let (dw, dh) =
            if level == 0 { (hdr.w, hdr.h) } else { (hdr.w.div_ceil(level), hdr.h.div_ceil(level)) };
        sink.begin(&Frame {
            width: dw,
            height: dh,
            full_width: hdr.w,
            full_height: hdr.h,
            orient: Orient::NORMAL,
            bottom_up: false,
            approximate: level >= 2,
        });
        self.pass = 0;
        self.row = 0;
        self.filled = 0;
        self.skip_empty_passes();
        Ok(())
    }

    fn skip_empty_passes(&mut self) {
        if self.level == 0 {
            return;
        }
        while self.pass < Self::passes(self.level) {
            let (pw, ph) = self.pass_size(self.pass);
            if pw > 0 && ph > 0 {
                break;
            }
            self.pass += 1;
        }
    }

    fn chunks(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        let v = tryp!(peek(src, 8));
        let len = v.u32_be(0).unwrap_or(0);
        let ty = v.array::<4>(4).unwrap_or([0; 4]);
        let have_hdr = self.info.is_some();
        match &ty {
            b"IHDR" | b"PLTE" | b"tRNS" => {
                if len as usize > 768 + 4 {
                    return Err(Error::Corrupt);
                }
                let v = tryp!(peek(src, 8 + len as usize + 4));
                let d = v.skip(8).take(len as usize);
                match &ty {
                    b"IHDR" => {
                        if have_hdr || len < 13 {
                            return Err(Error::Corrupt);
                        }
                        let hdr = Hdr {
                            w: d.u32_be(0).unwrap_or(0),
                            h: d.u32_be(4).unwrap_or(0),
                            depth: d.byte(8),
                            ct: d.byte(9),
                            interlace: d.byte(12) == 1,
                        };
                        check_dims(hdr.w, hdr.h)?;
                        let ok = match hdr.ct {
                            0 => matches!(hdr.depth, 1 | 2 | 4 | 8 | 16),
                            3 => matches!(hdr.depth, 1 | 2 | 4 | 8),
                            2 | 4 | 6 => matches!(hdr.depth, 8 | 16),
                            _ => false,
                        };
                        if !ok || d.byte(10) != 0 || d.byte(11) != 0 || d.byte(12) > 1 {
                            return Err(Error::Corrupt);
                        }
                        self.hdr = hdr;
                        // Palette plus the least the image data needs.
                        let least = self.need(if hdr.interlace { 8 } else { 0 });
                        let arena = self.arena.as_mut().ok_or(Error::Corrupt)?;
                        crate::reserve(arena, 1024 + least)?;
                        self.pal = arena.slice::<[u8; 4]>(256)?;
                        self.pal.fill([0, 0, 0, 255]);
                        self.used = arena.used();
                        self.info = Some(Info {
                            format: Format::Png,
                            width: hdr.w,
                            height: hdr.h,
                            orient: Orient::NORMAL,
                            interlaced: hdr.interlace,
                            alpha: hdr.ct == 4 || hdr.ct == 6,
                        });
                    }
                    b"PLTE" => {
                        if !have_hdr {
                            return Err(Error::Corrupt);
                        }
                        let n = (len as usize / 3).min(256);
                        for i in 0..n {
                            self.pal[i] = [d.byte(3 * i), d.byte(3 * i + 1), d.byte(3 * i + 2), 255];
                        }
                        self.pal_len = n;
                    }
                    _ => {
                        if let Some(info) = &mut self.info {
                            match self.hdr.ct {
                                3 => {
                                    for i in 0..(len as usize).min(256) {
                                        self.pal[i][3] = d.byte(i);
                                    }
                                    info.alpha = true;
                                }
                                0 => {
                                    let k = d.u16_be(0).unwrap_or(0);
                                    self.key = Some([k, k, k]);
                                    info.alpha = true;
                                }
                                2 => {
                                    self.key = Some([
                                        d.u16_be(0).unwrap_or(0),
                                        d.u16_be(2).unwrap_or(0),
                                        d.u16_be(4).unwrap_or(0),
                                    ]);
                                    info.alpha = true;
                                }
                                _ => {}
                            }
                        }
                    }
                }
                src.consume(8 + len as usize + 4);
                Ok(Step::Progress)
            }
            b"IDAT" => {
                if !have_hdr || (self.hdr.ct == 3 && self.pal_len == 0) {
                    return Err(Error::Corrupt);
                }
                src.consume(8);
                self.idat_left = len;
                self.start_idat(sink)?;
                self.phase = Phase::Idat;
                Ok(Step::Progress)
            }
            b"IEND" => Err(Error::Corrupt),
            _ => {
                if !have_hdr {
                    return Err(Error::Corrupt);
                }
                src.consume(8);
                self.phase = Phase::Skip(len.saturating_add(4));
                Ok(Step::Progress)
            }
        }
    }

    /// Undo the filter of the current line (`cur[0]` is the filter type).
    fn unfilter(&mut self, n: usize) -> Result<(), Error> {
        let bpp = self.hdr.bpp();
        let first = self.row == 0;
        let (cur, prev) = (&mut self.cur[..=n], &self.prev[..=n]);
        let ft = cur[0];
        let cur = &mut cur[1..];
        let prev = &prev[1..];
        match ft {
            0 => {}
            1 => {
                for i in bpp..n {
                    cur[i] = cur[i].wrapping_add(cur[i - bpp]);
                }
            }
            2 => {
                if !first {
                    for i in 0..n {
                        cur[i] = cur[i].wrapping_add(prev[i]);
                    }
                }
            }
            3 => {
                for i in 0..n {
                    let a = if i >= bpp { u16::from(cur[i - bpp]) } else { 0 };
                    let b = if first { 0 } else { u16::from(prev[i]) };
                    cur[i] = cur[i].wrapping_add(((a + b) / 2) as u8);
                }
            }
            4 => {
                for i in 0..n {
                    let a = if i >= bpp { i16::from(cur[i - bpp]) } else { 0 };
                    let b = if first { 0 } else { i16::from(prev[i]) };
                    let c = if i >= bpp && !first { i16::from(prev[i - bpp]) } else { 0 };
                    let p = a + b - c;
                    let (pa, pb, pc) = ((p - a).abs(), (p - b).abs(), (p - c).abs());
                    let pr = if pa <= pb && pa <= pc {
                        a
                    } else if pb <= pc {
                        b
                    } else {
                        c
                    };
                    cur[i] = cur[i].wrapping_add(pr as u8);
                }
            }
            _ => return Err(Error::Corrupt),
        }
        Ok(())
    }

    /// Convert the current (unfiltered) line of `w` pixels into `rgb`.
    fn convert(&mut self, w: u32) {
        let hdr = self.hdr;
        let bg = self.opts.background;
        let line = &self.cur[1..];
        let d = u32::from(hdr.depth);
        let sample = |i: usize| -> u16 {
            match d {
                16 => u16::from_be_bytes([line[2 * i], line[2 * i + 1]]),
                8 => u16::from(line[i]),
                _ => {
                    let bit = i * d as usize;
                    let b = line[bit / 8];
                    u16::from((b >> (8 - d as usize - bit % 8)) & ((1u8 << d) - 1))
                }
            }
        };
        let to8 = |s: u16| -> u8 {
            match d {
                16 => (s >> 8) as u8,
                8 => s as u8,
                _ => (u32::from(s) * 255 / ((1 << d) - 1)) as u8,
            }
        };
        for x in 0..w as usize {
            let (c, a) = match hdr.ct {
                0 => {
                    let s = sample(x);
                    let g = to8(s);
                    let a = if self.key.is_some_and(|k| k[0] == s) { 0 } else { 255 };
                    ([g, g, g], a)
                }
                2 => {
                    let s = [sample(3 * x), sample(3 * x + 1), sample(3 * x + 2)];
                    let a = if self.key == Some(s) { 0 } else { 255 };
                    ([to8(s[0]), to8(s[1]), to8(s[2])], a)
                }
                3 => {
                    let i = usize::from(sample(x));
                    let p = if i < self.pal_len { self.pal[i] } else { [0, 0, 0, 255] };
                    ([p[0], p[1], p[2]], p[3])
                }
                4 => {
                    let g = to8(sample(2 * x));
                    ([g, g, g], to8(sample(2 * x + 1)))
                }
                _ => {
                    let c = [to8(sample(4 * x)), to8(sample(4 * x + 1)), to8(sample(4 * x + 2))];
                    (c, to8(sample(4 * x + 3)))
                }
            };
            self.rgb[x] = blend(c, a, bg);
        }
    }

    fn idat(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        let src_end = src.at_end();
        let mut inp =
            IdatIn { v: View::of(src), pos: 0, left: self.idat_left, done: self.idat_done, src_end };
        let mut rows = 0;
        let mut result = Ok(Step::Progress);
        let npass = if self.level == 0 { 1 } else { Self::passes(self.level) };
        while rows < ROWS_PER_STEP {
            if self.pass >= npass {
                break;
            }
            let (pw, ph) = self.pass_size(self.pass);
            let n = self.hdr.row_bytes(pw);
            let Some(inf) = self.inflate.as_mut() else { return Err(Error::Corrupt) };
            let (k, st) = match inf.run(&mut inp, &mut self.cur[self.filled..=n]) {
                Ok(r) => r,
                Err(e) => {
                    result = Err(e);
                    break;
                }
            };
            self.filled += k;
            if self.filled <= n {
                result = match st {
                    Status::NeedInput if !inp.ended() => Ok(Step::NeedData(inp.v.len() - inp.pos + 1)),
                    _ => Err(Error::Truncated),
                };
                break;
            }
            if let Err(e) = self.unfilter(n) {
                result = Err(e);
                break;
            }
            self.convert(pw);
            rows += 1;
            let mut stop = false;
            if self.level == 0 || self.level == 8 {
                stop = sink.row(self.out_y, &self.rgb[..pw as usize]) == Flow::Stop;
                self.out_y += 1;
            } else {
                let l = self.level;
                let gw = self.hdr.w.div_ceil(l) as usize;
                let (x0, y0, dx, dy) = ADAM7[self.pass];
                let y = y0 + self.row * dy;
                if y % l == 0 {
                    for i in 0..pw {
                        let x = x0 + i * dx;
                        if x % l == 0 {
                            self.grid[(y / l) as usize * gw + (x / l) as usize] = self.rgb[i as usize];
                        }
                    }
                }
            }
            core::mem::swap(&mut self.cur, &mut self.prev);
            self.filled = 0;
            self.row += 1;
            if stop {
                self.pass = npass;
            } else if self.row >= ph {
                self.row = 0;
                self.pass += 1;
                self.skip_empty_passes();
            }
        }
        self.idat_left = inp.left;
        self.idat_done = inp.done;
        let pos = inp.pos;
        src.consume(pos);
        if self.pass >= npass {
            self.phase = if self.level == 0 || self.level == 8 { Phase::Done } else { Phase::Grid(0) };
            return Ok(if self.phase == Phase::Done { Step::Done } else { Step::Progress });
        }
        result
    }

    pub fn step(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        match self.phase {
            Phase::Signature => {
                let v = tryp!(peek(src, 8));
                if !v.starts_with_at(0, b"\x89PNG\r\n\x1a\n") {
                    return Err(Error::UnknownFormat);
                }
                src.consume(8);
                self.phase = Phase::Chunks;
                Ok(Step::Progress)
            }
            Phase::Chunks => self.chunks(src, sink),
            Phase::Skip(n) => {
                let k = (n as usize).min(src.available());
                src.consume(k);
                let left = n - k as u32;
                if left == 0 {
                    self.phase = Phase::Chunks;
                    Ok(Step::Progress)
                } else {
                    self.phase = Phase::Skip(left);
                    if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(1)) }
                }
            }
            Phase::Idat => self.idat(src, sink),
            Phase::Grid(y) => {
                let l = self.level;
                let gw = self.hdr.w.div_ceil(l) as usize;
                let gh = self.hdr.h.div_ceil(l);
                let mut y = y;
                let end = (y + 16).min(gh);
                while y < end {
                    let r = &self.grid[y as usize * gw..(y as usize + 1) * gw];
                    if sink.row(y, r) == Flow::Stop {
                        y = gh;
                        break;
                    }
                    y += 1;
                }
                if y >= gh {
                    self.phase = Phase::Done;
                    Ok(Step::Done)
                } else {
                    self.phase = Phase::Grid(y);
                    Ok(Step::Progress)
                }
            }
            Phase::Done => Ok(Step::Done),
        }
    }
}
