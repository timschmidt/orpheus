//! GIF decoder: the first frame, composed on the logical screen (areas
//! outside the frame and transparent pixels show the background).
//!
//! Non-interlaced frames stream row by row. Interlaced frames are collected
//! as palette indices when `width × height` bytes fit the arena, otherwise
//! only interlace pass 1 (every 8th row) of the frame is delivered.
//! Animation is not decoded.
//!
//! Arena: 768 bytes palette + 16 KiB LZW tables + `3 × screen width` row
//! (+ `frame width × height` for a stored interlaced frame).

use crate::inflate::Input;
use crate::{
    Arena, ByteSource, Error, Flow, Format, Frame, Info, Options, Orient, Rgb, RowSink, Step, View,
    check_dims, peek, tryp,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Header,
    Blocks,
    /// Skipping data sub-blocks of an extension.
    SkipSub,
    /// Background rows above the frame.
    Lead,
    Lzw,
    /// Rows of a stored interlaced frame, then background rows below.
    Tail,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Stream,
    Store,
    /// Interlace pass 1 only.
    Reduced,
}

const NONE: u16 = u16::MAX;
/// Prefix, suffix and output stack of the LZW dictionary (+ alignment).
const LZW_NEED: usize = 4096 * 4 + 1;
const ROWS_PER_STEP: u32 = 8;

pub struct Gif<'a> {
    arena: Option<Arena<'a>>,
    used: usize,
    opts: Options,
    info: Option<Info>,
    phase: Phase,
    pal: &'a mut [Rgb],
    gct_len: usize,
    trans: Option<u8>,
    lw: u32,
    lh: u32,
    fx0: u32,
    fy0: u32,
    fw: u32,
    fh: u32,
    interlaced: bool,
    mode: Mode,
    row: &'a mut [Rgb],
    store: &'a mut [u8],
    prefix: &'a mut [u16],
    suffix: &'a mut [u8],
    stack: &'a mut [u8],
    // LZW state.
    min_size: u32,
    size: u32,
    next: u16,
    prev: u16,
    first: u8,
    bits: u32,
    nbits: u32,
    sub_left: u32,
    sub_done: bool,
    // Output position.
    x: u32,
    /// Frame rows completed (in file order).
    r: u32,
    /// Next logical row to deliver.
    y: u32,
    /// The LZW data ended early: deliver what there is, then report it.
    truncated: bool,
}

/// Frame row of the `r`-th row in file order of an interlaced frame.
fn interlaced_row(r: u32, h: u32) -> u32 {
    let p1 = h.div_ceil(8);
    let p2 = if h > 4 { (h - 4).div_ceil(8) } else { 0 };
    let p3 = if h > 2 { (h - 2).div_ceil(4) } else { 0 };
    if r < p1 {
        r * 8
    } else if r < p1 + p2 {
        (r - p1) * 8 + 4
    } else if r < p1 + p2 + p3 {
        (r - p1 - p2) * 4 + 2
    } else {
        (r - p1 - p2 - p3) * 2 + 1
    }
}

struct SubIn<'v> {
    v: View<'v>,
    pos: usize,
    left: u32,
    done: bool,
    src_end: bool,
}

impl Input for SubIn<'_> {
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
            let n = self.v.get(self.pos)?;
            self.pos += 1;
            if n == 0 {
                self.done = true;
                return None;
            }
            self.left = u32::from(n);
        }
    }

    fn ended(&self) -> bool {
        self.done || (self.src_end && self.pos >= self.v.len())
    }
}

impl<'a> Gif<'a> {
    #[must_use]
    pub fn new(arena: Arena<'a>, opts: Options) -> Self {
        Self {
            arena: Some(arena),
            used: 0,
            opts,
            info: None,
            phase: Phase::Header,
            pal: &mut [],
            gct_len: 0,
            trans: None,
            lw: 0,
            lh: 0,
            fx0: 0,
            fy0: 0,
            fw: 0,
            fh: 0,
            interlaced: false,
            mode: Mode::Stream,
            row: &mut [],
            store: &mut [],
            prefix: &mut [],
            suffix: &mut [],
            stack: &mut [],
            min_size: 0,
            size: 0,
            next: 0,
            prev: NONE,
            first: 0,
            bits: 0,
            nbits: 0,
            sub_left: 0,
            sub_done: false,
            x: 0,
            r: 0,
            y: 0,
            truncated: false,
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

    fn header(&mut self, src: &mut dyn ByteSource) -> Result<Step, Error> {
        let v = tryp!(peek(src, 13));
        if !(v.starts_with_at(0, b"GIF87a") || v.starts_with_at(0, b"GIF89a")) {
            return Err(Error::UnknownFormat);
        }
        self.lw = u32::from(v.u16_le(6).unwrap_or(0));
        self.lh = u32::from(v.u16_le(8).unwrap_or(0));
        let packed = v.byte(10);
        let n = if packed & 0x80 != 0 { 2usize << (packed & 7) } else { 0 };
        let v = tryp!(peek(src, 13 + 3 * n));
        let arena = self.arena.as_mut().ok_or(Error::Corrupt)?;
        // Palette, LZW tables and one row of the logical screen.
        crate::reserve(arena, 768 + LZW_NEED + 3 * self.lw as usize)?;
        self.pal = arena.slice::<Rgb>(256)?;
        self.used = arena.used();
        for i in 0..n {
            self.pal[i] = [v.byte(13 + 3 * i), v.byte(14 + 3 * i), v.byte(15 + 3 * i)];
        }
        self.gct_len = n;
        src.consume(13 + 3 * n);
        self.phase = Phase::Blocks;
        Ok(Step::Progress)
    }

    fn blocks(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        let v = tryp!(peek(src, 1));
        match v.byte(0) {
            0x21 => {
                let v = tryp!(peek(src, 2));
                if v.byte(1) == 0xF9 {
                    let v = tryp!(peek(src, 7));
                    if v.byte(2) >= 4 && v.byte(3) & 1 != 0 {
                        self.trans = Some(v.byte(6));
                    }
                }
                src.consume(2);
                self.phase = Phase::SkipSub;
                Ok(Step::Progress)
            }
            0x2C => {
                let v = tryp!(peek(src, 10));
                let packed = v.byte(9);
                let n = if packed & 0x80 != 0 { 2usize << (packed & 7) } else { 0 };
                let v = tryp!(peek(src, 11 + 3 * n));
                self.fx0 = u32::from(v.u16_le(1).unwrap_or(0));
                self.fy0 = u32::from(v.u16_le(3).unwrap_or(0));
                self.fw = u32::from(v.u16_le(5).unwrap_or(0));
                self.fh = u32::from(v.u16_le(7).unwrap_or(0));
                self.interlaced = packed & 0x40 != 0;
                for i in 0..n {
                    self.pal[i] = [v.byte(10 + 3 * i), v.byte(11 + 3 * i), v.byte(12 + 3 * i)];
                }
                if n == 0 && self.gct_len == 0 {
                    // No palette at all: grey ramp (as most decoders do).
                    for (i, p) in self.pal.iter_mut().enumerate() {
                        *p = [i as u8; 3];
                    }
                }
                let min = u32::from(v.byte(10 + 3 * n));
                if !(1..=11).contains(&min) {
                    return Err(Error::Corrupt);
                }
                src.consume(11 + 3 * n);
                self.start_frame(min, sink)?;
                Ok(Step::Progress)
            }
            _ => Err(Error::Corrupt),
        }
    }

    fn start_frame(&mut self, min: u32, sink: &mut dyn RowSink) -> Result<(), Error> {
        check_dims(self.fw, self.fh)?;
        self.lw = self.lw.max(self.fx0 + self.fw);
        self.lh = self.lh.max(self.fy0 + self.fh);
        check_dims(self.lw, self.lh)?;
        let mut arena = self.arena.take().ok_or(Error::Corrupt)?;
        crate::reserve(&arena, LZW_NEED + 3 * self.lw.max(self.fw) as usize)?;
        self.prefix = arena.slice::<u16>(4096)?;
        self.suffix = arena.slice::<u8>(4096)?;
        self.stack = arena.slice::<u8>(4096)?;
        self.mode = if !self.interlaced {
            Mode::Stream
        } else if (self.fw as usize * self.fh as usize) + 3 * self.lw as usize + 8 <= arena.remaining() {
            Mode::Store
        } else {
            Mode::Reduced
        };
        let row_w = if self.mode == Mode::Reduced { self.fw } else { self.lw };
        self.row = arena.slice::<Rgb>(row_w as usize)?;
        if self.mode == Mode::Store {
            self.store = arena.slice::<u8>(self.fw as usize * self.fh as usize)?;
        }
        self.used = arena.used();
        let (w, h, fw, fh) = if self.mode == Mode::Reduced {
            (self.fw, self.fh.div_ceil(8), self.fw, self.fh)
        } else {
            (self.lw, self.lh, self.lw, self.lh)
        };
        self.info = Some(Info {
            format: Format::Gif,
            width: self.lw,
            height: self.lh,
            orient: Orient::NORMAL,
            interlaced: self.interlaced,
            alpha: self.trans.is_some(),
        });
        sink.begin(&Frame {
            width: w,
            height: h,
            full_width: fw,
            full_height: fh,
            orient: Orient::NORMAL,
            bottom_up: false,
            approximate: self.mode == Mode::Reduced,
        });
        self.min_size = min.max(2);
        self.reset_codes();
        self.prev = NONE;
        self.phase = if self.mode == Mode::Stream { Phase::Lead } else { Phase::Lzw };
        self.row.fill(self.opts.background);
        Ok(())
    }

    fn reset_codes(&mut self) {
        self.size = self.min_size + 1;
        self.next = (1u16 << self.min_size) + 2;
        self.prev = NONE;
    }

    fn color(&self, idx: u8) -> Rgb {
        if self.trans == Some(idx) { self.opts.background } else { self.pal[usize::from(idx)] }
    }

    /// One pixel of the frame in file order. Returns `true` when the sink
    /// wants no more rows.
    fn put(&mut self, idx: u8, sink: &mut dyn RowSink) -> bool {
        if self.r >= self.fh {
            return false;
        }
        let x = self.x;
        match self.mode {
            Mode::Stream => {
                let lx = (self.fx0 + x) as usize;
                if let Some(p) = self.row.get_mut(lx) {
                    *p = if self.trans == Some(idx) {
                        self.opts.background
                    } else {
                        self.pal[usize::from(idx)]
                    };
                }
            }
            Mode::Store => {
                let fy = interlaced_row(self.r, self.fh);
                self.store[(fy * self.fw + x) as usize] = idx;
            }
            Mode::Reduced => {
                if self.r < self.fh.div_ceil(8) {
                    self.row[x as usize] = self.color(idx);
                }
            }
        }
        self.x += 1;
        if self.x < self.fw {
            return false;
        }
        self.x = 0;
        self.r += 1;
        let mut stop = false;
        match self.mode {
            Mode::Stream => {
                stop = sink.row(self.y, self.row) == Flow::Stop;
                self.y += 1;
                self.row.fill(self.opts.background);
            }
            Mode::Reduced => {
                if self.r <= self.fh.div_ceil(8) {
                    stop = sink.row(self.r - 1, self.row) == Flow::Stop;
                }
                if self.r >= self.fh.div_ceil(8) {
                    stop = true;
                }
            }
            Mode::Store => {}
        }
        if stop {
            self.r = self.fh;
            self.y = self.lh;
        }
        stop
    }

    fn lzw(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        let src_end = src.at_end();
        let mut inp = SubIn { v: View::of(src), pos: 0, left: self.sub_left, done: self.sub_done, src_end };
        let clear = 1u16 << self.min_size;
        let start_r = self.r;
        let mut result = Ok(Step::Progress);
        while self.r < self.fh && self.r - start_r < ROWS_PER_STEP {
            while self.nbits < self.size {
                match inp.byte() {
                    Some(b) => {
                        self.bits |= u32::from(b) << self.nbits;
                        self.nbits += 8;
                    }
                    None => break,
                }
            }
            if self.nbits < self.size {
                result = if inp.ended() {
                    self.truncated = true;
                    self.r = self.fh;
                    Ok(Step::Progress)
                } else {
                    Ok(Step::NeedData(inp.v.len() - inp.pos + 1))
                };
                break;
            }
            let code = (self.bits & ((1 << self.size) - 1)) as u16;
            self.bits >>= self.size;
            self.nbits -= self.size;
            if code == clear {
                self.reset_codes();
                continue;
            }
            if code == clear + 1 {
                // End of information before the frame is complete.
                self.truncated = true;
                self.r = self.fh;
                break;
            }
            // Expand `code` onto the stack (reversed).
            let mut sp = 0usize;
            let mut c = if self.prev == NONE {
                if code >= clear {
                    return Err(Error::Corrupt);
                }
                code
            } else if code < self.next {
                code
            } else if code == self.next {
                self.stack[0] = self.first;
                sp = 1;
                self.prev
            } else {
                return Err(Error::Corrupt);
            };
            while c >= clear {
                if sp >= self.stack.len() {
                    return Err(Error::Corrupt);
                }
                self.stack[sp] = self.suffix[usize::from(c)];
                sp += 1;
                c = self.prefix[usize::from(c)];
            }
            if sp >= self.stack.len() {
                return Err(Error::Corrupt);
            }
            self.stack[sp] = c as u8;
            sp += 1;
            let first = c as u8;
            if self.prev != NONE && self.next < 4096 {
                self.prefix[usize::from(self.next)] = self.prev;
                self.suffix[usize::from(self.next)] = first;
                self.next += 1;
                if u32::from(self.next) == 1 << self.size && self.size < 12 {
                    self.size += 1;
                }
            }
            self.prev = code;
            self.first = first;
            while sp > 0 {
                sp -= 1;
                let idx = self.stack[sp];
                if self.put(idx, sink) {
                    break;
                }
            }
        }
        self.sub_left = inp.left;
        self.sub_done = inp.done;
        let pos = inp.pos;
        src.consume(pos);
        if self.r >= self.fh {
            match self.mode {
                Mode::Reduced => {
                    self.phase = Phase::Done;
                    return if self.truncated { Err(Error::Truncated) } else { Ok(Step::Done) };
                }
                Mode::Stream => {
                    // A truncated frame still gets the rows below it.
                    self.phase = Phase::Tail;
                    return result;
                }
                Mode::Store => {
                    self.y = 0;
                    self.phase = Phase::Tail;
                    return result;
                }
            }
        }
        result
    }

    fn rows(&mut self, sink: &mut dyn RowSink, until: u32) -> Step {
        let mut n = 0;
        while self.y < until && n < ROWS_PER_STEP * 2 {
            let y = self.y;
            self.row.fill(self.opts.background);
            if self.mode == Mode::Store && y >= self.fy0 && y < self.fy0 + self.fh {
                let fy = y - self.fy0;
                for fx in 0..self.fw {
                    let idx = self.store[(fy * self.fw + fx) as usize];
                    let c = self.color(idx);
                    if let Some(p) = self.row.get_mut((self.fx0 + fx) as usize) {
                        *p = c;
                    }
                }
            }
            self.y += 1;
            n += 1;
            if sink.row(y, self.row) == Flow::Stop {
                self.y = self.lh;
                self.phase = Phase::Done;
                return Step::Done;
            }
        }
        Step::Progress
    }

    pub fn step(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        match self.phase {
            Phase::Header => self.header(src),
            Phase::Blocks => self.blocks(src, sink),
            Phase::SkipSub => loop {
                let v = tryp!(peek(src, 1));
                let n = usize::from(v.byte(0));
                if n == 0 {
                    src.consume(1);
                    self.phase = Phase::Blocks;
                    return Ok(Step::Progress);
                }
                tryp!(peek(src, 1 + n));
                src.consume(1 + n);
            },
            Phase::Lead => {
                let s = self.rows(sink, self.fy0);
                if self.y >= self.fy0 && self.phase == Phase::Lead {
                    self.phase = Phase::Lzw;
                }
                Ok(s)
            }
            Phase::Lzw => self.lzw(src, sink),
            Phase::Tail => {
                let s = self.rows(sink, self.lh);
                if self.y >= self.lh {
                    self.phase = Phase::Done;
                    return if self.truncated { Err(Error::Truncated) } else { Ok(Step::Done) };
                }
                Ok(s)
            }
            Phase::Done => Ok(Step::Done),
        }
    }
}
