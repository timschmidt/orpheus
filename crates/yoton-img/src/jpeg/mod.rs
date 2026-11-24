//! JPEG decoder: baseline, extended-Huffman and progressive, 8-bit,
//! 1/3/4 components, any sampling factors with ratios 1, 2 or 4, restart
//! markers, EXIF orientation, Adobe RGB/CMYK/YCCK transforms.
//!
//! Output resolution 1/1, 1/2, 1/4 or 1/8, chosen as the smallest that is
//! still at least what the sink wants ([`RowSink::wanted`]) and that fits
//! the arena; the scaled inverse DCT produces it directly (1/8 is the DC
//! coefficient alone). Chroma is decoded at the resolution the output needs
//! (so 4:2:0 at 1/2 runs a full 8 × 8 chroma IDCT and no up-sampling);
//! where it would have to be larger than 8 × 8 it is replicated.
//!
//! Modes, decided at the first scan:
//!
//! * **Stream** (baseline, one interleaved scan): one MCU row of component
//!   planes at the output scale; rows go to the sink as each MCU row
//!   completes.
//! * **Store** (progressive, or sequential with one scan per component):
//!   the coefficients the chosen scale uses (zig-zag prefix of 1, 5, 25 or
//!   64 per block) are kept for the whole picture, plus — for an exact
//!   result — one bit per discarded coefficient so refinement scans stay in
//!   step. Without that bitmap ("approximate"), refinement scans that reach
//!   beyond the kept coefficients are skipped. Rows are produced after the
//!   last scan (or when the data ends: a truncated progressive file shows
//!   what arrived).
//! * **DC stream** (progressive, nothing fits): the first scan, when it is
//!   the DC scan of all components, is streamed at 1/8 with no storage; the
//!   rest of the file is not read. If the first scan is the luma DC alone,
//!   a grey picture is streamed instead.
//!
//! Arena: 7.7 KiB tables + MCU-row planes (`Σ 8·h/s × 8·v/s` per MCU and
//! component) + 3 × output width (+ coefficient storage in Store mode).

mod huff;
mod idct;

use bytemuck::{Pod, Zeroable};
use huff::{BitState, Bits, Huff};

use crate::{
    Arena, ByteSource, Error, Flow, Format, Frame, Info, MAX_PEEK, Options, Orient, Rgb, RowSink, Step,
    Unsupported, View, check_dims, peek, tryp,
};

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Tables {
    /// Quantisation tables in zig-zag order.
    q: [[u16; 64]; 4],
    dc: [Huff; 4],
    ac: [Huff; 4],
}

/// Natural-order index of each zig-zag position.
const ZIGZAG: [u8; 64] = [
    0, 1, 8, 16, 9, 2, 3, 10, 17, 24, 32, 25, 18, 11, 4, 5, 12, 19, 26, 33, 40, 48, 41, 34, 27, 20, 13, 6, 7,
    14, 21, 28, 35, 42, 49, 56, 57, 50, 43, 36, 29, 22, 15, 23, 30, 37, 44, 51, 58, 59, 52, 45, 38, 31, 39,
    46, 53, 60, 61, 54, 47, 55, 62, 63,
];

/// Zig-zag prefix covering the top-left n × n coefficients.
const fn zk_for(n: u8) -> u8 {
    match n {
        1 => 1,
        2 => 5,
        4 => 25,
        _ => 64,
    }
}

/// Worst-case entropy-coded bytes of one block (64 codes of 16 + 11 bits,
/// every byte stuffed).
const BLOCK_WORST: usize = 64 * 27 / 8 * 2;

#[derive(Debug, Clone, Copy, Default)]
struct Comp {
    id: u8,
    h: u8,
    v: u8,
    tq: u8,
    td: u8,
    ta: u8,
    /// Output block size and replication.
    bw: u8,
    bh: u8,
    uh: u8,
    uv: u8,
    zk: u8,
    /// Blocks per row/column, padded to whole MCUs.
    bx: u32,
    by: u32,
    /// Blocks per row/column of the component itself (non-interleaved
    /// scans).
    nbx: u32,
    nby: u32,
    plane: usize,
    stride: usize,
    coef: usize,
    nz: usize,
    pred: i32,
}

#[derive(Debug, Clone, Copy, Default)]
struct Scan {
    n: u8,
    c: [u8; 4],
    ss: u8,
    se: u8,
    ah: u8,
    al: u8,
    /// MCUs in the scan, per row.
    mcus: u32,
    per_row: u32,
    mcu: u32,
    eobrun: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Mode {
    Stream,
    /// `capped`: chroma kept at no more than the luma block size (replicated
    /// on output), to save coefficient storage.
    Store {
        exact: bool,
        capped: bool,
    },
    DcStream,
    GrayDc,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Phase {
    Soi,
    Markers,
    Skip(u32),
    /// Skipping an entropy-coded segment we do not need.
    SkipEcs,
    Scan,
    /// Store mode: produce MCU row `my`.
    Output(u32),
    Done,
}

enum Event {
    Need(usize),
    Budget,
    RowReady,
    ScanEnd,
    Truncated,
}

pub struct Jpeg<'a> {
    arena: Option<Arena<'a>>,
    used: usize,
    info: Option<Info>,
    phase: Phase,
    t: Option<&'a mut Tables>,
    /// Defined tables: bits 0–3 quantisation, 4–7 DC, 8–11 AC.
    defined: u16,
    w: u32,
    h: u32,
    nf: usize,
    comps: [Comp; 4],
    hmax: u32,
    vmax: u32,
    mcux: u32,
    mcuy: u32,
    progressive: bool,
    orient: Orient,
    /// Adobe APP14 transform flag.
    adobe: Option<u8>,
    ri: u32,
    mode: Mode,
    started: bool,
    s: u32,
    out_w: u32,
    out_h: u32,
    planes: &'a mut [u8],
    rgb: &'a mut [Rgb],
    coefs: &'a mut [i16],
    nz: &'a mut [u32],
    scan: Scan,
    bits: BitState,
    out_y: u32,
    /// Stream modes: the MCU row being decoded and the MCU in it.
    my: u32,
    mx: u32,
    truncated: bool,
}

/// MCUs decoded per `step` at most (Store mode).
const MCUS_PER_STEP: u32 = 256;

impl<'a> Jpeg<'a> {
    #[must_use]
    /// JPEG has no transparency, so the options are not used.
    pub fn new(arena: Arena<'a>, _opts: Options) -> Self {
        Self {
            arena: Some(arena),
            used: 0,
            info: None,
            phase: Phase::Soi,
            t: None,
            defined: 0,
            w: 0,
            h: 0,
            nf: 0,
            comps: [Comp::default(); 4],
            hmax: 1,
            vmax: 1,
            mcux: 0,
            mcuy: 0,
            progressive: false,
            orient: Orient::NORMAL,
            adobe: None,
            ri: 0,
            mode: Mode::Stream,
            started: false,
            s: 1,
            out_w: 0,
            out_h: 0,
            planes: &mut [],
            rgb: &mut [],
            coefs: &mut [],
            nz: &mut [],
            scan: Scan::default(),
            bits: BitState::default(),
            out_y: 0,
            my: 0,
            mx: 0,
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

    /// The output scale denominator (1, 2, 4, 8) once decoding started.
    #[must_use]
    pub fn scale(&self) -> u32 {
        self.s
    }

    /// The tables, allocated on first use. When the arena cannot hold them
    /// the header is still parsed (tables are checked, not kept) so the
    /// first scan can report the whole need.
    fn tables(&mut self) -> Option<&mut Tables> {
        if self.t.is_none()
            && let Some(arena) = self.arena.as_mut()
            && let Ok(t) = arena.one::<Tables>()
        {
            self.t = Some(t);
            self.used = arena.used();
        }
        self.t.as_deref_mut()
    }

    // ---- markers -------------------------------------------------------

    fn markers(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        // Find the next marker, skipping stray bytes.
        let mut skipped = 0;
        let m = loop {
            let v = match peek(src, 2) {
                Ok(v) => v,
                Err(crate::Pending::Eof) if self.started && matches!(self.mode, Mode::Store { .. }) => {
                    // Data ended between scans: show what arrived.
                    self.truncated = true;
                    self.phase = Phase::Output(0);
                    return Ok(Step::Progress);
                }
                Err(p) => return p.into(),
            };
            if v.byte(0) == 0xFF && v.byte(1) != 0xFF && v.byte(1) != 0 {
                break v.byte(1);
            }
            src.consume(1);
            skipped += 1;
            if skipped >= 4096 {
                return Ok(Step::Progress);
            }
        };
        match m {
            0xD8 | 0x01 | 0xD0..=0xD7 => {
                src.consume(2);
                return Ok(Step::Progress);
            }
            0xD9 => {
                src.consume(2);
                if self.started && matches!(self.mode, Mode::Store { .. }) {
                    self.phase = Phase::Output(0);
                    return Ok(Step::Progress);
                }
                return Err(if self.started { Error::Truncated } else { Error::Corrupt });
            }
            _ => {}
        }
        let v = tryp!(peek(src, 4));
        let len = usize::from(v.u16_be(2).unwrap_or(0));
        if len < 2 {
            return Err(Error::Corrupt);
        }
        match m {
            0xDB | 0xC4 | 0xC0..=0xC2 | 0xDD | 0xDA | 0xEE => {
                if len + 2 > MAX_PEEK {
                    return Err(Error::Unsupported(Unsupported::HeaderTooLarge));
                }
                let v = tryp!(peek(src, 2 + len));
                let d = v.skip(4).take(len - 2);
                // The view borrows `src`; copy what SOS needs below.
                match m {
                    0xDB => self.dqt(d)?,
                    0xC4 => self.dht(d)?,
                    0xC0..=0xC2 => self.sof(d, m == 0xC2)?,
                    0xDD => self.ri = u32::from(d.u16_be(0).unwrap_or(0)),
                    0xEE => {
                        if d.starts_with_at(0, b"Adobe") && d.len() >= 12 {
                            self.adobe = Some(d.byte(11));
                        }
                    }
                    _ => {
                        let mut sos = [0u8; 16];
                        let n = d.len().min(16);
                        d.copy_to(0, &mut sos[..n]);
                        src.consume(2 + len);
                        return self.sos(&sos[..n], sink);
                    }
                }
                src.consume(2 + len);
                Ok(Step::Progress)
            }
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF => {
                Err(Error::Unsupported(Unsupported::JpegProcess))
            }
            0xE1 => {
                let n = (2 + len).min(MAX_PEEK);
                let v = tryp!(peek(src, n));
                let d = v.skip(4);
                if d.starts_with_at(0, b"Exif\0\0") {
                    self.orient = Orient::from_exif(exif_orientation(d.skip(6)));
                }
                src.consume(n);
                self.phase = Phase::Skip((2 + len - n) as u32);
                Ok(Step::Progress)
            }
            _ => {
                src.consume(4);
                self.phase = Phase::Skip((len - 2) as u32);
                Ok(Step::Progress)
            }
        }
    }

    fn dqt(&mut self, d: View<'_>) -> Result<(), Error> {
        let mut o = 0;
        let mut defined = self.defined;
        let mut t = self.tables();
        while o < d.len() {
            let b = d.byte(o);
            let (pq, tq) = (b >> 4, usize::from(b & 15));
            if tq > 3 || pq > 1 {
                return Err(Error::Corrupt);
            }
            let n = if pq == 1 { 128 } else { 64 };
            if o + 1 + n > d.len() {
                return Err(Error::Corrupt);
            }
            if let Some(t) = t.as_deref_mut() {
                for k in 0..64 {
                    t.q[tq][k] = if pq == 1 {
                        d.u16_be(o + 1 + 2 * k).unwrap_or(1)
                    } else {
                        u16::from(d.byte(o + 1 + k))
                    };
                }
            }
            defined |= 1 << tq;
            o += 1 + n;
        }
        self.defined = defined;
        Ok(())
    }

    fn dht(&mut self, d: View<'_>) -> Result<(), Error> {
        let mut o = 0;
        let mut defined = self.defined;
        let mut t = self.tables();
        while o < d.len() {
            let b = d.byte(o);
            let (tc, th) = (b >> 4, usize::from(b & 15));
            if tc > 1 || th > 3 {
                return Err(Error::Corrupt);
            }
            let Some(counts) = d.array::<16>(o + 1) else { return Err(Error::Corrupt) };
            let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
            let vals = d.skip(o + 17).take(total);
            if total > 256 || vals.len() < total {
                return Err(Error::Corrupt);
            }
            if let Some(t) = t.as_deref_mut() {
                let h = if tc == 0 { &mut t.dc[th] } else { &mut t.ac[th] };
                if !h.build(&counts, vals) {
                    return Err(Error::Corrupt);
                }
            }
            defined |= 1 << (4 + 4 * u16::from(tc) + th as u16);
            o += 17 + total;
        }
        self.defined = defined;
        Ok(())
    }

    fn sof(&mut self, d: View<'_>, progressive: bool) -> Result<(), Error> {
        if self.info.is_some() {
            return Err(Error::Corrupt);
        }
        if d.byte(0) != 8 {
            return Err(Error::Unsupported(Unsupported::JpegProcess));
        }
        let h = u32::from(d.u16_be(1).unwrap_or(0));
        let w = u32::from(d.u16_be(3).unwrap_or(0));
        let nf = usize::from(d.byte(5));
        check_dims(w, h)?;
        if !matches!(nf, 1 | 3 | 4) || d.len() < 6 + 3 * nf {
            return Err(Error::Unsupported(Unsupported::Layout));
        }
        let (mut hmax, mut vmax) = (1u8, 1u8);
        for i in 0..nf {
            let c = &mut self.comps[i];
            c.id = d.byte(6 + 3 * i);
            let hv = d.byte(7 + 3 * i);
            c.h = hv >> 4;
            c.v = hv & 15;
            c.tq = d.byte(8 + 3 * i) & 3;
            if !(1..=4).contains(&c.h) || !(1..=4).contains(&c.v) {
                return Err(Error::Corrupt);
            }
            if nf == 1 {
                c.h = 1;
                c.v = 1;
            }
            hmax = hmax.max(c.h);
            vmax = vmax.max(c.v);
        }
        for c in &self.comps[..nf] {
            let (rh, rv) = (hmax / c.h, vmax / c.v);
            if hmax % c.h != 0 || vmax % c.v != 0 || rh == 3 || rv == 3 {
                return Err(Error::Unsupported(Unsupported::Layout));
            }
        }
        self.w = w;
        self.h = h;
        self.nf = nf;
        self.hmax = u32::from(hmax);
        self.vmax = u32::from(vmax);
        self.mcux = w.div_ceil(8 * self.hmax);
        self.mcuy = h.div_ceil(8 * self.vmax);
        for c in &mut self.comps[..nf] {
            c.bx = self.mcux * u32::from(c.h);
            c.by = self.mcuy * u32::from(c.v);
            c.nbx = (w * u32::from(c.h)).div_ceil(self.hmax).div_ceil(8);
            c.nby = (h * u32::from(c.v)).div_ceil(self.vmax).div_ceil(8);
        }
        self.progressive = progressive;
        self.info = Some(Info {
            format: Format::Jpeg,
            width: w,
            height: h,
            orient: self.orient,
            interlaced: progressive,
            alpha: false,
        });
        Ok(())
    }

    /// Per-component output block geometry at scale `s` (`capped`: no
    /// component block larger than the full-resolution one's).
    fn geometry(&mut self, s: u32, capped: bool) {
        let (hmax, vmax) = (self.hmax, self.vmax);
        let base = (8 / s).max(1);
        for c in &mut self.comps[..self.nf] {
            let fh = 8 * hmax / u32::from(c.h) / s;
            let fv = 8 * vmax / u32::from(c.v) / s;
            let lim = if capped { base } else { 8 };
            c.bw = fh.min(lim) as u8;
            c.bh = fv.min(lim) as u8;
            c.uh = (fh / u32::from(c.bw)) as u8;
            c.uv = (fv / u32::from(c.bh)) as u8;
            c.zk = zk_for(c.bw.max(c.bh));
        }
    }

    fn need(&mut self, mode: Mode, s: u32) -> usize {
        self.geometry(s, matches!(mode, Mode::Store { capped: true, .. }));
        let comps = if mode == Mode::GrayDc { 1 } else { self.nf };
        let mut planes = 0usize;
        let mut coefs = 0usize;
        let mut blocks = 0usize;
        let mut partial = false;
        for c in &self.comps[..comps] {
            planes += c.bx as usize * usize::from(c.bw) * usize::from(c.v) * usize::from(c.bh);
            coefs += c.bx as usize * c.by as usize * usize::from(c.zk);
            blocks += c.bx as usize * c.by as usize;
            partial |= c.zk != 1 && c.zk != 64;
        }
        let out_w = self.w.div_ceil(s) as usize;
        let mut n = planes + 3 * out_w + 16;
        if let Mode::Store { exact, .. } = mode {
            n += 2 * coefs + 2;
            if exact && partial {
                n += 8 * blocks + 4;
            }
        }
        n
    }

    fn sos(&mut self, d: &[u8], sink: &mut dyn RowSink) -> Result<Step, Error> {
        if self.info.is_none() {
            return Err(Error::Corrupt);
        }
        let ns = usize::from(*d.first().unwrap_or(&0));
        if !(1..=4).contains(&ns) || d.len() < 4 + 2 * ns {
            return Err(Error::Corrupt);
        }
        let mut sc = Scan { n: ns as u8, ..Scan::default() };
        for i in 0..ns {
            let id = d[1 + 2 * i];
            let k = self.comps[..self.nf].iter().position(|c| c.id == id).ok_or(Error::Corrupt)?;
            sc.c[i] = k as u8;
            self.comps[k].td = (d[2 + 2 * i] >> 4) & 3;
            self.comps[k].ta = d[2 + 2 * i] & 3;
        }
        sc.ss = d[1 + 2 * ns];
        sc.se = d[2 + 2 * ns];
        sc.ah = d[3 + 2 * ns] >> 4;
        sc.al = d[3 + 2 * ns] & 15;
        if self.progressive {
            let bad = sc.se > 63
                || sc.ss > sc.se
                || (sc.ss == 0 && sc.se != 0)
                || (sc.ss > 0 && ns != 1)
                || sc.al > 13
                || sc.ah > 13;
            if bad {
                return Err(Error::Corrupt);
            }
        } else {
            sc.ss = 0;
            sc.se = 63;
            sc.ah = 0;
            sc.al = 0;
        }
        // Tables used must exist.
        for &k in &sc.c[..ns] {
            let c = &self.comps[usize::from(k)];
            let dc = self.defined & (1 << (4 + c.td)) != 0;
            let ac = self.defined & (1 << (8 + c.ta)) != 0;
            let q = self.defined & (1 << c.tq) != 0;
            let need_dc = sc.ss == 0 && sc.ah == 0;
            let need_ac = sc.se > 0;
            if (need_dc && !dc) || (need_ac && !ac) || !q {
                return Err(Error::Corrupt);
            }
        }
        if !self.started {
            self.start(&sc, sink)?;
        }
        // MCU layout of this scan.
        if ns == 1 {
            let c = &self.comps[usize::from(sc.c[0])];
            sc.per_row = c.nbx;
            sc.mcus = c.nbx * c.nby;
        } else {
            sc.per_row = self.mcux;
            sc.mcus = self.mcux * self.mcuy;
        }
        for c in &mut self.comps {
            c.pred = 0;
        }
        self.bits = BitState::default();
        let skip = match self.mode {
            Mode::Store { exact, .. } if sc.ss > 0 => {
                let zk = self.comps[usize::from(sc.c[0])].zk;
                sc.ss >= zk || (sc.ah > 0 && sc.se >= zk && zk < 64 && !exact)
            }
            _ => false,
        };
        self.scan = sc;
        self.phase = if skip { Phase::SkipEcs } else { Phase::Scan };
        Ok(Step::Progress)
    }

    /// Choose the mode and scale, allocate, start the sink.
    fn start(&mut self, sc: &Scan, sink: &mut dyn RowSink) -> Result<(), Error> {
        let (ww, wh) = sink.wanted(self.w, self.h, self.orient);
        let mut pref = 1;
        for s in [8u32, 4, 2, 1] {
            pref = s;
            if self.w.div_ceil(s) >= ww && self.h.div_ceil(s) >= wh {
                break;
            }
        }
        // Tables that did not fit: nothing fits, but report the whole need.
        let short = if self.t.is_none() { core::mem::size_of::<Tables>() + 4 } else { 0 };
        let cap = if short > 0 { 0 } else { self.arena.as_ref().map_or(0, Arena::remaining) };
        let scales = |p: u32| [p, p * 2, p * 4, p * 8].into_iter().filter(|&s| s <= 8);
        let mut choice = None;
        if !self.progressive && usize::from(sc.n) == self.nf {
            for s in scales(pref) {
                if self.need(Mode::Stream, s) <= cap {
                    choice = Some((Mode::Stream, s));
                    break;
                }
            }
        } else {
            // Finest scale first; at each scale prefer exact refinement,
            // then chroma at luma resolution, then skipped refinements.
            'outer: for s in scales(pref) {
                for (exact, capped) in [(true, false), (true, true), (false, false), (false, true)] {
                    let m = Mode::Store { exact, capped };
                    if self.need(m, s) <= cap {
                        choice = Some((m, s));
                        break 'outer;
                    }
                }
            }
            let dc_first = self.progressive && sc.ss == 0 && sc.ah == 0;
            if choice.is_none()
                && dc_first
                && usize::from(sc.n) == self.nf
                && self.need(Mode::DcStream, 8) <= cap
            {
                choice = Some((Mode::DcStream, 8));
            }
            let c0 = self.comps[0];
            if choice.is_none()
                && dc_first
                && sc.n == 1
                && sc.c[0] == 0
                && u32::from(c0.h) == self.hmax
                && u32::from(c0.v) == self.vmax
                && self.need(Mode::GrayDc, 8) <= cap
            {
                choice = Some((Mode::GrayDc, 8));
            }
        }
        let Some((mode, s)) = choice else {
            let dc_stream = self.progressive && sc.ss == 0 && sc.ah == 0 && usize::from(sc.n) == self.nf;
            let m = if dc_stream {
                Mode::DcStream
            } else if self.progressive || usize::from(sc.n) != self.nf {
                Mode::Store { exact: false, capped: true }
            } else {
                Mode::Stream
            };
            let used = self.arena.as_ref().map_or(0, Arena::used);
            let capacity = self.arena.as_ref().map_or(0, Arena::capacity);
            return Err(Error::Arena { needed: used + short + self.need(m, 8), capacity });
        };
        self.geometry(s, matches!(mode, Mode::Store { capped: true, .. }));
        self.mode = mode;
        self.s = s;
        self.out_w = self.w.div_ceil(s);
        self.out_h = self.h.div_ceil(s);
        let mut arena = self.arena.take().ok_or(Error::Corrupt)?;
        let ncomp = if mode == Mode::GrayDc { 1 } else { self.nf };
        let mut planes = 0usize;
        let mut coefs = 0usize;
        let mut blocks = 0usize;
        let mut partial = false;
        for c in &mut self.comps[..ncomp] {
            c.stride = c.bx as usize * usize::from(c.bw);
            c.plane = planes;
            planes += c.stride * usize::from(c.v) * usize::from(c.bh);
            c.coef = coefs;
            coefs += c.bx as usize * c.by as usize * usize::from(c.zk);
            c.nz = blocks * 2;
            blocks += c.bx as usize * c.by as usize;
            partial |= c.zk != 1 && c.zk != 64;
        }
        self.planes = arena.slice::<u8>(planes)?;
        self.rgb = arena.slice::<Rgb>(self.out_w as usize)?;
        let mut approximate = matches!(mode, Mode::DcStream | Mode::GrayDc);
        if let Mode::Store { exact, .. } = mode {
            self.coefs = arena.slice::<i16>(coefs)?;
            if exact && partial {
                self.nz = arena.slice::<u32>(blocks * 2)?;
            }
            approximate = !exact && partial && self.progressive;
        }
        self.used = arena.used();
        self.started = true;
        sink.begin(&Frame {
            width: self.out_w,
            height: self.out_h,
            full_width: self.w,
            full_height: self.h,
            orient: self.orient,
            bottom_up: false,
            approximate,
        });
        Ok(())
    }

    // ---- entropy-coded data -------------------------------------------

    /// Bytes that must be buffered before one MCU of the current scan.
    fn mcu_need(&self) -> usize {
        let blocks: usize = if self.scan.n == 1 {
            1
        } else {
            self.scan.c[..usize::from(self.scan.n)]
                .iter()
                .map(|&k| {
                    let c = &self.comps[usize::from(k)];
                    usize::from(c.h) * usize::from(c.v)
                })
                .sum()
        };
        let per = if self.scan.se == 0 { 8 } else { BLOCK_WORST };
        blocks * per + 64
    }

    fn decode_some(&mut self, src: &mut dyn ByteSource) -> Event {
        let src_end = src.at_end();
        let need = self.mcu_need();
        let Self { t, comps, scan, bits, mode, planes, coefs, nz, ri, mx, progressive, truncated, .. } = self;
        let Some(t) = t.as_deref() else { return Event::Truncated };
        let stream = !matches!(mode, Mode::Store { .. });
        let mut b = Bits::new(View::of(src), *bits);
        let mut count = 0;
        let ev = loop {
            if scan.mcu >= scan.mcus {
                break Event::ScanEnd;
            }
            if b.v.len() - b.pos < need && !src_end {
                break Event::Need(need);
            }
            if !stream && count >= MCUS_PER_STEP {
                break Event::Budget;
            }
            // Restart interval.
            if *ri > 0 && scan.mcu > 0 && scan.mcu % *ri == 0 {
                b.reset();
                restart(&mut b);
                for c in comps.iter_mut() {
                    c.pred = 0;
                }
                scan.eobrun = 0;
            }
            let n = usize::from(scan.n);
            let (mxx, myy) = (scan.mcu % scan.per_row, scan.mcu / scan.per_row);
            for si in 0..n {
                let k = usize::from(scan.c[si]);
                let c = &mut comps[k];
                let (bh_n, bv_n) = if n == 1 { (1, 1) } else { (u32::from(c.h), u32::from(c.v)) };
                for j in 0..bv_n {
                    for i in 0..bh_n {
                        let (bxi, byi) = if n == 1 { (mxx, myy) } else { (mxx * bh_n + i, myy * bv_n + j) };
                        if stream {
                            let mut blk = [0i32; 64];
                            let q = &t.q[usize::from(c.tq)];
                            if *mode == Mode::Stream {
                                block_seq(
                                    &mut b,
                                    &t.dc[usize::from(c.td)],
                                    &t.ac[usize::from(c.ta)],
                                    q,
                                    c,
                                    &mut blk,
                                );
                            } else {
                                let s = u32::from(b.decode(&t.dc[usize::from(c.td)]));
                                c.pred = (c.pred + b.extend(s)).clamp(-65536, 65536);
                                blk[0] = dq((c.pred << scan.al).clamp(-32768, 32767), q[0]);
                            }
                            // Position inside the MCU-row planes.
                            let (px, py) = if n == 1 { (bxi, 0) } else { (bxi, j) };
                            let o = c.plane
                                + py as usize * usize::from(c.bh) * c.stride
                                + px as usize * usize::from(c.bw);
                            idct::idct(
                                &blk,
                                usize::from(c.bw),
                                usize::from(c.bh),
                                &mut planes[o..],
                                c.stride,
                            );
                        } else {
                            let zk = usize::from(c.zk);
                            let bi = byi as usize * c.bx as usize + bxi as usize;
                            if bxi >= c.bx || byi >= c.by {
                                continue;
                            }
                            let blk = &mut coefs[c.coef + bi * zk..c.coef + (bi + 1) * zk];
                            let nzb = if nz.is_empty() {
                                None
                            } else {
                                Some(&mut nz[c.nz + bi * 2..c.nz + bi * 2 + 2])
                            };
                            block_store(&mut b, t, c, scan, *progressive, blk, nzb);
                        }
                    }
                }
            }
            scan.mcu += 1;
            count += 1;
            if src_end && b.overran() {
                *truncated = true;
            }
            if stream {
                *mx += 1;
                if *mx >= scan.per_row {
                    *mx = 0;
                    break Event::RowReady;
                }
            } else if *truncated {
                break Event::Truncated;
            }
        };
        *bits = b.state();
        let pos = b.pos;
        src.consume(pos);
        ev
    }

    /// Convert and deliver the rows of the MCU row in the planes.
    fn emit_rows(&mut self, sink: &mut dyn RowSink) -> Flow {
        let rows_per = if self.scan.n == 1 && !matches!(self.mode, Mode::Store { .. }) {
            let c = &self.comps[usize::from(self.scan.c[0])];
            u32::from(c.bh) * u32::from(c.uv)
        } else {
            8 * self.vmax / self.s
        };
        let gray = self.nf == 1 || self.mode == Mode::GrayDc;
        let ncomp = if gray { 1 } else { self.nf };
        let rgb_ids =
            self.nf == 3 && self.comps[0].id == b'R' && self.comps[1].id == b'G' && self.comps[2].id == b'B';
        let transform = match self.adobe {
            Some(t) => t,
            None if rgb_ids => 0,
            None => 1,
        };
        let adobe = self.adobe.is_some();
        for r in 0..rows_per {
            if self.out_y >= self.out_h {
                return Flow::Stop;
            }
            // Components at half resolution are up-sampled with the
            // triangle filter (weights 3/4 and 1/4 per axis) inside the MCU
            // row; other ratios are replicated.
            let mut near = [&[][..]; 4];
            let mut far = [&[][..]; 4];
            for (k, c) in self.comps[..ncomp].iter().enumerate() {
                let uv = u32::from(c.uv);
                let nr = r / uv;
                let last = u32::from(c.v) * u32::from(c.bh) - 1;
                let fr = match (uv, r % 2) {
                    (2, 0) => nr.saturating_sub(1),
                    (2, _) => (nr + 1).min(last),
                    _ => nr,
                };
                let o = c.plane + nr as usize * c.stride;
                near[k] = &self.planes[o..o + c.stride];
                let o = c.plane + fr as usize * c.stride;
                far[k] = &self.planes[o..o + c.stride];
            }
            for x in 0..self.out_w as usize {
                let s = |k: usize| -> i32 {
                    let c = &self.comps[k];
                    let (n, f) = (near[k], far[k]);
                    let col = |i: usize| 3 * i32::from(n[i]) + i32::from(f[i]);
                    match (c.uh, c.uv) {
                        (1, 1) => i32::from(n[x]),
                        (2, _) => {
                            let i = x / 2;
                            let j = if x % 2 == 0 { i.saturating_sub(1) } else { (i + 1).min(n.len() - 1) };
                            (3 * col(i) + col(j) + 8) >> 4
                        }
                        (uh, 2) => (col(x / usize::from(uh)) + 2) >> 2,
                        (uh, _) => i32::from(n[x / usize::from(uh)]),
                    }
                };
                self.rgb[x] = if gray {
                    let y = s(0) as u8;
                    [y, y, y]
                } else if self.nf == 3 {
                    if transform == 0 { [s(0) as u8, s(1) as u8, s(2) as u8] } else { ycc(s(0), s(1), s(2)) }
                } else {
                    let (c, m, yy) = if transform == 2 {
                        let p = ycc(s(0), s(1), s(2));
                        (i32::from(p[0]), i32::from(p[1]), i32::from(p[2]))
                    } else {
                        (s(0), s(1), s(2))
                    };
                    let k = s(3);
                    if adobe {
                        // Adobe writes inverted CMYK.
                        [(c * k / 255) as u8, (m * k / 255) as u8, (yy * k / 255) as u8]
                    } else {
                        [
                            ((255 - c) * (255 - k) / 255) as u8,
                            ((255 - m) * (255 - k) / 255) as u8,
                            ((255 - yy) * (255 - k) / 255) as u8,
                        ]
                    }
                };
            }
            let y = self.out_y;
            self.out_y += 1;
            if sink.row(y, self.rgb) == Flow::Stop {
                return Flow::Stop;
            }
        }
        if self.out_y >= self.out_h { Flow::Stop } else { Flow::Continue }
    }

    /// Store mode: inverse-transform MCU row `my` into the planes.
    fn output_row(&mut self, my: u32) {
        let Some(t) = self.t.as_deref() else { return };
        for c in &self.comps[..self.nf] {
            let zk = usize::from(c.zk);
            let q = &t.q[usize::from(c.tq)];
            for j in 0..u32::from(c.v) {
                let by = my * u32::from(c.v) + j;
                for bx in 0..c.bx {
                    let bi = by as usize * c.bx as usize + bx as usize;
                    let src = &self.coefs[c.coef + bi * zk..c.coef + (bi + 1) * zk];
                    let mut blk = [0i32; 64];
                    for (k, &v) in src.iter().enumerate() {
                        blk[usize::from(ZIGZAG[k])] = dq(i32::from(v), q[k]);
                    }
                    let o =
                        c.plane + j as usize * usize::from(c.bh) * c.stride + bx as usize * usize::from(c.bw);
                    idct::idct(&blk, usize::from(c.bw), usize::from(c.bh), &mut self.planes[o..], c.stride);
                }
            }
        }
    }

    pub fn step(&mut self, src: &mut dyn ByteSource, sink: &mut dyn RowSink) -> Result<Step, Error> {
        match self.phase {
            Phase::Soi => {
                let v = tryp!(peek(src, 2));
                if !v.starts_with_at(0, &[0xFF, 0xD8]) {
                    return Err(Error::UnknownFormat);
                }
                src.consume(2);
                let _ = self.tables();
                self.phase = Phase::Markers;
                Ok(Step::Progress)
            }
            Phase::Markers => self.markers(src, sink),
            Phase::Skip(n) => {
                let k = (n as usize).min(src.available());
                src.consume(k);
                let left = n - k as u32;
                if left == 0 {
                    self.phase = Phase::Markers;
                    Ok(Step::Progress)
                } else {
                    self.phase = Phase::Skip(left);
                    if src.at_end() { Err(Error::Truncated) } else { Ok(Step::NeedData(1)) }
                }
            }
            Phase::SkipEcs => {
                let v = View::of(src);
                let mut i = 0;
                let found = loop {
                    if i + 1 >= v.len() {
                        break false;
                    }
                    if v.byte(i) == 0xFF {
                        let m = v.byte(i + 1);
                        if m != 0 && m != 0xFF && !(0xD0..=0xD7).contains(&m) {
                            break true;
                        }
                        i += if m == 0xFF { 1 } else { 2 };
                    } else {
                        i += 1;
                    }
                };
                let end = src.at_end();
                src.consume(i);
                if found {
                    self.phase = Phase::Markers;
                    Ok(Step::Progress)
                } else if end {
                    self.truncated = true;
                    self.phase = Phase::Output(0);
                    Ok(Step::Progress)
                } else {
                    Ok(Step::NeedData(2))
                }
            }
            Phase::Scan => match self.decode_some(src) {
                Event::Need(n) => Ok(Step::NeedData(n)),
                Event::Budget => Ok(Step::Progress),
                Event::RowReady => {
                    self.my += 1;
                    let stop = self.emit_rows(sink) == Flow::Stop;
                    if self.truncated && !stop {
                        self.phase = Phase::Done;
                        return Err(Error::Truncated);
                    }
                    if stop {
                        self.phase = Phase::Done;
                        return Ok(Step::Done);
                    }
                    Ok(Step::Progress)
                }
                Event::ScanEnd => {
                    if matches!(self.mode, Mode::Store { .. }) {
                        let st = self.bits;
                        self.bits = BitState::default();
                        // The reader stops in front of a marker; anything
                        // else is skipped up to the next one.
                        self.phase = if st.marker.is_some() { Phase::Markers } else { Phase::SkipEcs };
                        Ok(Step::Progress)
                    } else {
                        self.phase = Phase::Done;
                        Ok(Step::Done)
                    }
                }
                Event::Truncated => {
                    self.phase = Phase::Output(0);
                    Ok(Step::Progress)
                }
            },
            Phase::Output(my) => {
                if my >= self.mcuy {
                    self.phase = Phase::Done;
                    return if self.truncated { Err(Error::Truncated) } else { Ok(Step::Done) };
                }
                self.output_row(my);
                let flow = self.emit_rows(sink);
                if flow == Flow::Stop && self.out_y < self.out_h {
                    self.phase = Phase::Done;
                    return Ok(Step::Done);
                }
                self.phase = Phase::Output(my + 1);
                Ok(Step::Progress)
            }
            Phase::Done => Ok(Step::Done),
        }
    }
}

/// Dequantise, keeping the result within 16 bits.
#[inline]
fn dq(v: i32, q: u16) -> i32 {
    (v * i32::from(q)).clamp(-32767, 32767)
}

/// Sequential (baseline) block into natural-order dequantised coefficients;
/// coefficients beyond the component's zig-zag prefix are decoded and
/// dropped.
fn block_seq(b: &mut Bits<'_>, dc: &Huff, ac: &Huff, q: &[u16; 64], c: &mut Comp, blk: &mut [i32; 64]) {
    let s = u32::from(b.decode(dc));
    c.pred = (c.pred + b.extend(s)).clamp(-65536, 65536);
    blk[0] = dq(c.pred.clamp(-32768, 32767), q[0]);
    let zk = usize::from(c.zk);
    let mut k = 1usize;
    while k < 64 {
        let rs = b.decode(ac);
        let (r, s) = (usize::from(rs >> 4), u32::from(rs & 15));
        if s == 0 {
            if r == 15 {
                k += 16;
                continue;
            }
            break;
        }
        k += r;
        if k > 63 {
            break;
        }
        let v = b.extend(s);
        if k < zk {
            blk[usize::from(ZIGZAG[k])] = dq(v, q[k]);
        }
        k += 1;
    }
}

/// Store mode: one block of the current scan into its stored coefficients
/// (`blk`, zig-zag prefix, quantised) and bitmap of non-zero discarded
/// coefficients (`nzb`).
fn block_store(
    b: &mut Bits<'_>,
    t: &Tables,
    c: &mut Comp,
    sc: &mut Scan,
    progressive: bool,
    blk: &mut [i16],
    mut nzb: Option<&mut [u32]>,
) {
    let zk = blk.len();
    let set = |blk: &mut [i16], nzb: &mut Option<&mut [u32]>, k: usize, v: i32| {
        if k < zk {
            blk[k] = v.clamp(-32768, 32767) as i16;
        } else if let Some(m) = nzb.as_deref_mut() {
            m[k / 32] |= 1 << (k % 32);
        }
    };
    let al = u32::from(sc.al);
    if sc.ss == 0 {
        if sc.ah == 0 {
            let s = u32::from(b.decode(&t.dc[usize::from(c.td)]));
            c.pred = (c.pred + b.extend(s)).clamp(-65536, 65536);
            set(blk, &mut nzb, 0, c.pred << al);
        } else if b.bit() {
            blk[0] |= 1 << al;
        }
        if progressive {
            return;
        }
    }
    let ac = &t.ac[usize::from(c.ta)];
    let ss = if sc.ss == 0 { 1 } else { usize::from(sc.ss) };
    let se = usize::from(sc.se);
    if sc.ah == 0 {
        // First AC scan (or the AC part of a sequential scan).
        if sc.eobrun > 0 {
            sc.eobrun -= 1;
            return;
        }
        let mut k = ss;
        while k <= se {
            let rs = b.decode(ac);
            let (r, s) = (u32::from(rs >> 4), u32::from(rs & 15));
            if s == 0 {
                if r < 15 {
                    if progressive {
                        sc.eobrun = (1 << r) + b.get(r) - 1;
                    }
                    break;
                }
                k += 16;
                continue;
            }
            k += r as usize;
            if k > 63 {
                break;
            }
            let v = b.extend(s) << al;
            set(blk, &mut nzb, k, v);
            k += 1;
        }
        return;
    }
    // Refinement of AC coefficients.
    let p1: i32 = 1 << al;
    let m1: i32 = -1 << al;
    let nonzero = |blk: &[i16], nzb: &Option<&mut [u32]>, k: usize| -> bool {
        if k < zk { blk[k] != 0 } else { nzb.as_deref().is_some_and(|m| m[k / 32] & (1 << (k % 32)) != 0) }
    };
    let refine = |b: &mut Bits<'_>, blk: &mut [i16], k: usize| {
        if b.bit() && k < zk {
            let v = i32::from(blk[k]);
            if v & p1 == 0 {
                blk[k] = (if v >= 0 { v + p1 } else { v + m1 }).clamp(-32768, 32767) as i16;
            }
        }
    };
    let mut k = ss;
    if sc.eobrun == 0 {
        while k <= se {
            let rs = b.decode(ac);
            let (mut r, s) = (i32::from(rs >> 4), rs & 15);
            let mut val = 0;
            if s != 0 {
                val = if b.bit() { p1 } else { m1 };
            } else if r != 15 {
                sc.eobrun = (1u32 << r) + b.get(r as u32);
                break;
            }
            while k <= se {
                if nonzero(blk, &nzb, k) {
                    refine(b, blk, k);
                } else {
                    if r == 0 {
                        if val != 0 {
                            set(blk, &mut nzb, k, val);
                        }
                        break;
                    }
                    r -= 1;
                }
                k += 1;
            }
            k += 1;
        }
    }
    if sc.eobrun > 0 {
        while k <= se {
            if nonzero(blk, &nzb, k) {
                refine(b, blk, k);
            }
            k += 1;
        }
        sc.eobrun -= 1;
    }
}

/// At a restart interval: step over the RSTn marker.
fn restart(b: &mut Bits<'_>) {
    if let Some(m) = b.marker() {
        if (0xD0..=0xD7).contains(&m) {
            b.pos += 2;
            b.clear_marker();
        }
        return;
    }
    // Not reached yet: skip to the next marker.
    while let Some(x) = b.v.get(b.pos) {
        if x == 0xFF {
            match b.v.get(b.pos + 1) {
                Some(m @ 0xD0..=0xD7) => {
                    let _ = m;
                    b.pos += 2;
                    return;
                }
                Some(0) | Some(0xFF) => b.pos += 1,
                Some(_) => {
                    // Another marker: leave it for the marker parser.
                    b.reset();
                    return;
                }
                None => return,
            }
        } else {
            b.pos += 1;
        }
    }
}

/// YCbCr (JFIF) to RGB.
#[inline]
fn ycc(y: i32, cb: i32, cr: i32) -> Rgb {
    let (cb, cr) = (cb - 128, cr - 128);
    let r = y + ((91_881 * cr + 32_768) >> 16);
    let g = y + ((-22_554 * cb - 46_802 * cr + 32_768) >> 16);
    let b = y + ((116_130 * cb + 32_768) >> 16);
    [r.clamp(0, 255) as u8, g.clamp(0, 255) as u8, b.clamp(0, 255) as u8]
}

/// The orientation tag of a TIFF structure (EXIF APP1 after "Exif\0\0"),
/// 1 when absent.
fn exif_orientation(t: View<'_>) -> u16 {
    let le = t.starts_with_at(0, b"II*\0");
    if !le && !t.starts_with_at(0, b"MM\0*") {
        return 1;
    }
    let u16_at = |o: usize| if le { t.u16_le(o) } else { t.u16_be(o) };
    let u32_at = |o: usize| if le { t.u32_le(o) } else { t.u32_be(o) };
    let Some(ifd) = u32_at(4) else { return 1 };
    let ifd = ifd as usize;
    let Some(n) = u16_at(ifd) else { return 1 };
    for i in 0..usize::from(n).min(256) {
        let e = ifd + 2 + 12 * i;
        if u16_at(e) == Some(0x0112) {
            return u16_at(e + 8).unwrap_or(1);
        }
    }
    1
}
