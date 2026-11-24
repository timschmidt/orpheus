//! QOI ("Quite OK Image") decoder: streamed op by op, one row buffer.
//!
//! Arena: `3 × width` bytes (one RGB row).

use crate::{
    Arena, ByteSource, Error, Flow, Format, Frame, Info, Options, Orient, Rgb, RowSink, Step, View, blend,
    check_dims, peek, tryp,
};

pub struct Qoi<'a> {
    arena: Option<Arena<'a>>,
    used: usize,
    opts: Options,
    info: Option<Info>,
    row: &'a mut [Rgb],
    index: [[u8; 4]; 64],
    px: [u8; 4],
    run: u32,
    x: u32,
    y: u32,
    done: bool,
}

/// Rows delivered per `step` at most.
const ROWS_PER_STEP: u32 = 8;

impl<'a> Qoi<'a> {
    #[must_use]
    pub fn new(arena: Arena<'a>, opts: Options) -> Self {
        Self {
            arena: Some(arena),
            used: 0,
            opts,
            info: None,
            row: &mut [],
            index: [[0; 4]; 64],
            px: [0, 0, 0, 255],
            run: 0,
            x: 0,
            y: 0,
            done: false,
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
        let v = tryp!(peek(src, 14));
        if !v.starts_with_at(0, b"qoif") {
            return Err(Error::UnknownFormat);
        }
        let w = v.u32_be(4).unwrap_or(0);
        let h = v.u32_be(8).unwrap_or(0);
        let ch = v.byte(12);
        check_dims(w, h)?;
        if ch != 3 && ch != 4 {
            return Err(Error::Corrupt);
        }
        let mut arena = self.arena.take().ok_or(Error::Corrupt)?;
        self.row = arena.slice::<Rgb>(w as usize)?;
        self.used = arena.used();
        src.consume(14);
        let info = Info {
            format: Format::Qoi,
            width: w,
            height: h,
            orient: Orient::NORMAL,
            interlaced: false,
            alpha: ch == 4,
        };
        self.info = Some(info);
        sink.begin(&Frame {
            width: w,
            height: h,
            full_width: w,
            full_height: h,
            orient: Orient::NORMAL,
            bottom_up: false,
            approximate: false,
        });
        Ok(Step::Progress)
    }

    pub fn step(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        if self.done {
            return Ok(Step::Done);
        }
        let Some(info) = self.info else { return self.header(src, sink) };
        let (w, h) = (info.width, info.height);
        let bg = self.opts.background;
        let v = View::of(src);
        let mut pos = 0usize;
        let mut rows = 0;
        let mut result = Ok(Step::Progress);
        while self.y < h {
            if self.run == 0 {
                let b0 = v.byte(pos);
                let len = match b0 {
                    0xFE => 4,
                    0xFF => 5,
                    _ if b0 >> 6 == 2 => 2,
                    _ => 1,
                };
                if v.len() - pos < len {
                    result = if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(len)) };
                    break;
                }
                let px = &mut self.px;
                match b0 {
                    0xFE => {
                        px[0] = v.byte(pos + 1);
                        px[1] = v.byte(pos + 2);
                        px[2] = v.byte(pos + 3);
                    }
                    0xFF => {
                        px[0] = v.byte(pos + 1);
                        px[1] = v.byte(pos + 2);
                        px[2] = v.byte(pos + 3);
                        px[3] = v.byte(pos + 4);
                    }
                    _ => match b0 >> 6 {
                        0 => *px = self.index[usize::from(b0 & 63)],
                        1 => {
                            px[0] = px[0].wrapping_add(((b0 >> 4) & 3).wrapping_sub(2));
                            px[1] = px[1].wrapping_add(((b0 >> 2) & 3).wrapping_sub(2));
                            px[2] = px[2].wrapping_add((b0 & 3).wrapping_sub(2));
                        }
                        2 => {
                            let b1 = v.byte(pos + 1);
                            let dg = (b0 & 63).wrapping_sub(32);
                            px[0] = px[0].wrapping_add(dg.wrapping_sub(8).wrapping_add(b1 >> 4));
                            px[1] = px[1].wrapping_add(dg);
                            px[2] = px[2].wrapping_add(dg.wrapping_sub(8).wrapping_add(b1 & 15));
                        }
                        _ => self.run = u32::from(b0 & 63),
                    },
                }
                pos += len;
                let p = *px;
                let hash = (usize::from(p[0]) * 3
                    + usize::from(p[1]) * 5
                    + usize::from(p[2]) * 7
                    + usize::from(p[3]) * 11)
                    % 64;
                self.index[hash] = p;
            } else {
                self.run -= 1;
            }
            let p = self.px;
            self.row[self.x as usize] = blend([p[0], p[1], p[2]], p[3], bg);
            self.x += 1;
            if self.x == w {
                self.x = 0;
                let y = self.y;
                self.y += 1;
                if sink.row(y, self.row) == Flow::Stop {
                    self.y = h;
                }
                rows += 1;
                if rows >= ROWS_PER_STEP {
                    break;
                }
            }
        }
        src.consume(pos);
        if self.y >= h {
            self.done = true;
            return Ok(Step::Done);
        }
        result
    }
}
