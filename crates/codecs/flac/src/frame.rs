//! FLAC frames: header, subframes, residual, prediction, decorrelation
//! (RFC 9639 §9). Integer arithmetic only; wrapping where corrupt input
//! could overflow, so a bad stream decodes to noise and fails its CRC
//! instead of panicking.

use orpheus_audio::View;
use orpheus_audio::bits::BitReader;
use orpheus_audio::crc::{crc8, crc16};

/// What the frame header says.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FrameHeader {
    /// Sample number of the first sample (frame number × block size for
    /// fixed-blocksize streams).
    pub first: u64,
    pub block: u32,
    /// Channel assignment code (0–7 independent, 8 L/S, 9 R/S, 10 M/S).
    pub assign: u8,
    pub channels: u8,
    pub bps: u8,
    /// Header length in bytes, CRC-8 included.
    pub len: usize,
}

/// Stream parameters a frame must agree with.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Params {
    pub rate: u32,
    pub channels: u8,
    pub bps: u8,
    pub max_block: u32,
    /// Fixed block size used to turn frame numbers into sample numbers.
    pub nominal_block: u32,
}

/// Whether `v` starts with a frame sync code.
pub(crate) fn is_sync(v: &View<'_>, at: usize) -> bool {
    v.byte(at) == 0xFF && v.byte(at + 1) & 0xFE == 0xF8 && at + 1 < v.len()
}

/// Parse and check (CRC-8, consistency with `p`) the frame header at the
/// start of `v`. `None`: not a valid header (or not all of it buffered).
pub(crate) fn header(v: &View<'_>, p: &Params) -> Option<FrameHeader> {
    let mut br = BitReader::new(v.take(16));
    if br.read(15) != 0x7FFC {
        return None;
    }
    let variable = br.bit();
    let bs_code = br.read(4);
    let sr_code = br.read(4);
    let assign = br.read(4) as u8;
    let ss_code = br.read(3);
    if br.bit() {
        return None;
    }
    let num = br.utf8_u64()?;
    let block = match bs_code {
        0 => return None,
        1 => 192,
        2..=5 => 576 << (bs_code - 2),
        6 => br.read(8) + 1,
        7 => br.read(16) + 1,
        _ => 256 << (bs_code - 8),
    };
    let rate = match sr_code {
        0 => p.rate,
        1 => 88200,
        2 => 176_400,
        3 => 192_000,
        4 => 8000,
        5 => 16000,
        6 => 22050,
        7 => 24000,
        8 => 32000,
        9 => 44100,
        10 => 48000,
        11 => 96000,
        12 => br.read(8) * 1000,
        13 => br.read(16),
        14 => br.read(16) * 10,
        _ => return None,
    };
    let bps = match ss_code {
        0 => p.bps,
        1 => 8,
        2 => 12,
        4 => 16,
        5 => 20,
        6 => 24,
        7 => 32,
        _ => return None,
    };
    let channels = match assign {
        0..=7 => assign + 1,
        8..=10 => 2,
        _ => return None,
    };
    let len = br.byte_pos() + 1;
    if br.overrun() || len > v.len() {
        return None;
    }
    let mut hb = [0u8; 16];
    if !v.copy_to(0, &mut hb[..len]) || crc8(0, &hb[..len - 1]) != hb[len - 1] {
        return None;
    }
    if rate != p.rate || bps != p.bps || channels != p.channels || block > p.max_block {
        return None;
    }
    let first = if variable { num } else { num.checked_mul(u64::from(p.nominal_block))? };
    Some(FrameHeader { first, block, assign, channels, bps, len })
}

/// Why a frame did not decode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FrameError {
    /// The frame runs past the buffered bytes.
    Short,
    /// Invalid coding or CRC-16 mismatch.
    Corrupt,
}

/// Decode the frame whose header `h` starts `v` into `planar` (channel `c`
/// at `c * stride`). Returns the frame length in bytes.
pub(crate) fn decode(
    v: &View<'_>,
    h: &FrameHeader,
    planar: &mut [i32],
    stride: usize,
) -> Result<usize, FrameError> {
    let bs = h.block as usize;
    let mut br = BitReader::new(*v);
    br.skip(h.len * 8);
    for c in 0..usize::from(h.channels) {
        // The side channel carries one extra bit.
        let side = matches!((h.assign, c), (8 | 10, 1) | (9, 0));
        let bps = u32::from(h.bps) + u32::from(side);
        let Some(out) = planar.get_mut(c * stride..c * stride + bs) else { return Err(FrameError::Corrupt) };
        let r = subframe(&mut br, out, bps);
        if br.overrun() {
            return Err(FrameError::Short);
        }
        r?;
    }
    br.align();
    let body = br.byte_pos();
    let crc = br.read(16) as u16;
    if br.overrun() {
        return Err(FrameError::Short);
    }
    let r = v.take(body);
    if crc16(crc16(0, r.a), r.b) != crc {
        return Err(FrameError::Corrupt);
    }
    decorrelate(h, planar, stride);
    Ok(body + 2)
}

fn decorrelate(h: &FrameHeader, planar: &mut [i32], stride: usize) {
    let bs = h.block as usize;
    if !(8..=10).contains(&h.assign) {
        return;
    }
    let (a, b) = planar.split_at_mut(stride);
    let (l, r) = (&mut a[..bs], &mut b[..bs]);
    match h.assign {
        8 => {
            // left/side: right = left - side
            for (x, s) in l.iter().zip(r.iter_mut()) {
                *s = x.wrapping_sub(*s);
            }
        }
        9 => {
            // side/right: left = side + right
            for (s, x) in l.iter_mut().zip(r.iter()) {
                *s = s.wrapping_add(*x);
            }
        }
        _ => {
            // mid/side
            for (m, s) in l.iter_mut().zip(r.iter_mut()) {
                let mid = (*m << 1) | (*s & 1);
                let side = *s;
                *m = mid.wrapping_add(side) >> 1;
                *s = mid.wrapping_sub(side) >> 1;
            }
        }
    }
}

fn subframe(br: &mut BitReader<'_>, out: &mut [i32], bps: u32) -> Result<(), FrameError> {
    if br.bit() {
        return Err(FrameError::Corrupt);
    }
    let kind = br.read(6);
    let wasted = if br.bit() { br.unary() + 1 } else { 0 };
    if wasted >= bps {
        return Err(FrameError::Corrupt);
    }
    let bps = bps - wasted;
    match kind {
        0 => {
            let v = br.read_signed(bps);
            out.fill(v);
        }
        1 => {
            for x in out.iter_mut() {
                *x = br.read_signed(bps);
            }
        }
        8..=12 => {
            let order = (kind - 8) as usize;
            warmup(br, out, order, bps)?;
            residual(br, out, order)?;
            fixed(out, order);
        }
        32..=63 => {
            let order = (kind - 31) as usize;
            warmup(br, out, order, bps)?;
            let precision = br.read(4) + 1;
            if precision == 16 {
                return Err(FrameError::Corrupt);
            }
            let shift = br.read_signed(5);
            if shift < 0 {
                return Err(FrameError::Corrupt);
            }
            let mut coef = [0i32; 32];
            for c in coef.iter_mut().take(order) {
                *c = br.read_signed(precision);
            }
            residual(br, out, order)?;
            lpc(out, &coef[..order], shift as u32);
        }
        _ => return Err(FrameError::Corrupt),
    }
    if wasted > 0 {
        for x in out.iter_mut() {
            *x = x.wrapping_shl(wasted);
        }
    }
    Ok(())
}

fn warmup(br: &mut BitReader<'_>, out: &mut [i32], order: usize, bps: u32) -> Result<(), FrameError> {
    if order > out.len() {
        return Err(FrameError::Corrupt);
    }
    for x in out.iter_mut().take(order) {
        *x = br.read_signed(bps);
    }
    Ok(())
}

/// Rice-coded residual into `out[order..]`.
fn residual(br: &mut BitReader<'_>, out: &mut [i32], order: usize) -> Result<(), FrameError> {
    let method = br.read(2);
    let (pbits, escape) = match method {
        0 => (4, 15),
        1 => (5, 31),
        _ => return Err(FrameError::Corrupt),
    };
    let porder = br.read(4);
    let bs = out.len();
    let parts = 1usize << porder;
    if !bs.is_multiple_of(parts) {
        return Err(FrameError::Corrupt);
    }
    let plen = bs >> porder;
    if plen < order {
        return Err(FrameError::Corrupt);
    }
    let mut i = order;
    for p in 0..parts {
        let end = (p + 1) * plen;
        let k = br.read(pbits);
        if k == escape {
            let n = br.read(5);
            for x in &mut out[i..end] {
                *x = br.read_signed(n);
            }
        } else {
            for x in &mut out[i..end] {
                *x = br.rice(k);
            }
            if br.exhausted() {
                return Err(FrameError::Short);
            }
        }
        i = end;
    }
    Ok(())
}

fn fixed(x: &mut [i32], order: usize) {
    let n = x.len();
    match order {
        1 => {
            for i in 1..n {
                x[i] = x[i].wrapping_add(x[i - 1]);
            }
        }
        2 => {
            for i in 2..n {
                let p = x[i - 1].wrapping_mul(2).wrapping_sub(x[i - 2]);
                x[i] = x[i].wrapping_add(p);
            }
        }
        3 => {
            for i in 3..n {
                let p = x[i - 1].wrapping_sub(x[i - 2]).wrapping_mul(3).wrapping_add(x[i - 3]);
                x[i] = x[i].wrapping_add(p);
            }
        }
        4 => {
            for i in 4..n {
                let p = x[i - 1]
                    .wrapping_add(x[i - 3])
                    .wrapping_mul(4)
                    .wrapping_sub(x[i - 2].wrapping_mul(6))
                    .wrapping_sub(x[i - 4]);
                x[i] = x[i].wrapping_add(p);
            }
        }
        _ => {}
    }
}

/// Linear prediction with 64-bit accumulation (exact for every valid
/// stream; LLVM lowers the product-sum to `smlal` on the M4).
fn lpc(x: &mut [i32], coef: &[i32], shift: u32) {
    let order = coef.len();
    for i in order..x.len() {
        let mut sum = 0i64;
        for (j, &c) in coef.iter().enumerate() {
            sum += i64::from(c) * i64::from(x[i - 1 - j]);
        }
        x[i] = x[i].wrapping_add((sum >> shift) as i32);
    }
}
