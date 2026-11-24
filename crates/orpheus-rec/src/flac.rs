//! FLAC encoder (RFC 9639), 16-bit, one or two channels, a fixed block
//! size of [`BLOCK`] frames, fixed predictors and Rice coding: lossless at
//! a fraction of the CPU of LPC.
//!
//! Levels (the encoder's own, close in spirit to `flac -0…-2`):
//!
//! | Level | Stereo decorrelation | Rice partition order |
//! |---|---|---|
//! | 0 | none (left, right) | ≤ 3 |
//! | 1 | best of L/R, L/S, S/R, M/S by estimate | ≤ 4 |
//! | 2 | as 1 | ≤ 6 |
//!
//! Each subframe is the cheapest of CONSTANT, FIXED order 0–4 (order by
//! the sum of absolute residuals, Rice parameters per partition by
//! estimate, the total counted exactly) and VERBATIM, so a frame is never
//! larger than [`max_frame_bytes`].
//!
//! The stream header ([`HEADER_LEN`] bytes: `fLaC` and STREAMINFO) is
//! written first with the counts so far and rewritten in place as the file
//! grows; the MD5 field stays zero ("not computed", allowed by the RFC).

use crate::bits::{BitWriter, crc8, crc16, zigzag};

/// Frames per FLAC block.
pub const BLOCK: usize = 1152;
/// `fLaC` + STREAMINFO block.
pub const HEADER_LEN: usize = 42;
/// Highest level.
pub const MAX_LEVEL: u8 = 2;

const MAX_PORDER: [u32; 3] = [3, 4, 6];
const MAX_K: u32 = 14;
const BPS: u32 = 16;

/// Largest encoded frame for `channels` (verbatim subframes, the side
/// channel one bit wider, the longest header and the footer).
#[must_use]
pub const fn max_frame_bytes(channels: u8) -> usize {
    let ch = channels as usize;
    16 + ch * (1 + (BLOCK * 17).div_ceil(8)) + 2
}

/// Where a subframe's samples come from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Chan {
    Ch(usize),
    Mid,
    Side,
}

impl Chan {
    const fn bps(self) -> u32 {
        match self {
            Chan::Side => BPS + 1,
            _ => BPS,
        }
    }
}

#[inline]
fn sample(pcm: &[i16], ch: usize, c: Chan, i: usize) -> i32 {
    let at = |k: usize| i32::from(pcm.get(i * ch + k).copied().unwrap_or(0));
    match c {
        Chan::Ch(k) => at(k),
        Chan::Mid => (at(0) + at(1)) >> 1,
        Chan::Side => at(0) - at(1),
    }
}

/// Fixed-predictor residual of order `o` from the sample `x` and the four
/// before it (`h[0]` = x[i−1] …).
#[inline]
fn fixed(o: u32, x: i32, h: &[i32; 4]) -> i32 {
    match o {
        0 => x,
        1 => x - h[0],
        2 => x - 2 * h[0] + h[1],
        3 => x - 3 * h[0] + 3 * h[1] - h[2],
        _ => x - 4 * h[0] + 6 * h[1] - 4 * h[2] + h[3],
    }
}

/// Call `f(i, residual)` for `i` in `order..n`, in order: one pass, no
/// buffer.
#[inline]
fn residuals(pcm: &[i16], ch: usize, c: Chan, order: u32, n: usize, mut f: impl FnMut(usize, i32)) {
    let mut h = [0i32; 4];
    for i in 0..n {
        let x = sample(pcm, ch, c, i);
        if i >= order as usize {
            f(i, fixed(order, x, &h));
        }
        h = [x, h[0], h[1], h[2]];
    }
}

/// How one subframe will be coded.
#[derive(Clone, Copy, Debug)]
enum Plan {
    Constant(i32),
    Verbatim,
    Fixed { order: u32, porder: u32, ks: [u8; 64] },
}

/// A chosen plan and its exact size in bits.
#[derive(Clone, Copy, Debug)]
struct Choice {
    plan: Plan,
    bits: u64,
}

/// Best Rice parameter for `count` values summing to `sum` (estimate).
fn best_k(sum: u64, count: u64) -> (u32, u64) {
    let mut best = (0, u64::MAX);
    for k in 0..=MAX_K {
        let bits = count * (u64::from(k) + 1) + (sum >> k);
        if bits < best.1 {
            best = (k, bits);
        }
    }
    best
}

/// Partition of frame `i` at partition order `p` (partitions of `n >> p`).
#[inline]
const fn part_of(i: usize, n: usize, p: u32) -> usize {
    i / (n >> p)
}

/// Plan one subframe of `n` frames from channel `c`.
fn plan(pcm: &[i16], ch: usize, n: usize, c: Chan, max_porder: u32) -> Choice {
    let bps = c.bps();
    let verbatim = Choice { plan: Plan::Verbatim, bits: 8 + n as u64 * u64::from(bps) };
    let first = sample(pcm, ch, c, 0);
    // Order by the sum of absolute residuals over the frames every order
    // predicts (from 4 on); constant if every sample is the first.
    let mut sums = [0u64; 5];
    let mut constant = true;
    let mut h = [0i32; 4];
    for i in 0..n {
        let x = sample(pcm, ch, c, i);
        constant &= x == first;
        if i >= 4 {
            for (o, s) in sums.iter_mut().enumerate() {
                *s += u64::from(fixed(o as u32, x, &h).unsigned_abs());
            }
        }
        h = [x, h[0], h[1], h[2]];
    }
    if constant {
        return Choice { plan: Plan::Constant(first), bits: 8 + u64::from(bps) };
    }
    let order = (0..5u32).filter(|&o| (o as usize) < n).min_by_key(|&o| sums[o as usize]).unwrap_or(0);
    // Partition order: n divisible by 2^p and partitions longer than the
    // order.
    let mut pmax = max_porder;
    while pmax > 0 && (n % (1 << pmax) != 0 || (n >> pmax) <= order as usize) {
        pmax -= 1;
    }
    if n <= order as usize {
        return verbatim;
    }
    // Zigzag sums per partition at `pmax`.
    let mut level = [0u64; 64];
    residuals(pcm, ch, c, order, n, |i, r| {
        if let Some(s) = level.get_mut(part_of(i, n, pmax)) {
            *s += u64::from(zigzag(r));
        }
    });
    // The best partition order by estimate, merging pairs going down.
    let mut best: Option<(u32, u64, [u8; 64])> = None;
    for p in (0..=pmax).rev() {
        let parts = 1usize << p;
        let len = n >> p;
        let mut ks = [0u8; 64];
        let mut bits = 0u64;
        for (q, k_out) in ks.iter_mut().enumerate().take(parts) {
            let count = if q == 0 { len - order as usize } else { len } as u64;
            let (k, b) = best_k(level[q], count);
            *k_out = k as u8;
            bits += 4 + b;
        }
        if best.is_none_or(|(_, b, _)| bits < b) {
            best = Some((p, bits, ks));
        }
        if p > 0 {
            for q in 0..parts / 2 {
                level[q] = level[2 * q] + level[2 * q + 1];
            }
        }
    }
    let Some((porder, _, ks)) = best else { return verbatim };
    // Exact size of the residual with those parameters.
    let mut bits = 8 + u64::from(order) * u64::from(bps) + 2 + 4 + 4 * (1u64 << porder);
    residuals(pcm, ch, c, order, n, |i, r| {
        let k = ks.get(part_of(i, n, porder)).copied().unwrap_or(0);
        bits += u64::from(zigzag(r) >> k) + 1 + u64::from(k);
    });
    if bits < verbatim.bits { Choice { plan: Plan::Fixed { order, porder, ks }, bits } } else { verbatim }
}

fn write_subframe(w: &mut BitWriter<'_>, pcm: &[i16], ch: usize, n: usize, c: Chan, plan: &Plan) {
    let bps = c.bps();
    match *plan {
        Plan::Constant(v) => {
            w.put(0, 8);
            w.put_signed(v, bps);
        }
        Plan::Verbatim => {
            w.put(0b0000_0010, 8);
            for i in 0..n {
                w.put_signed(sample(pcm, ch, c, i), bps);
            }
        }
        Plan::Fixed { order, porder, ks } => {
            w.put((0b001000 | order) << 1, 8);
            for i in 0..order as usize {
                w.put_signed(sample(pcm, ch, c, i), bps);
            }
            w.put(0, 2);
            w.put(porder, 4);
            let len = n >> porder;
            let mut k = 0u32;
            residuals(pcm, ch, c, order, n, |i, r| {
                // A partition starts: its parameter first.
                if i == order as usize || i % len == 0 {
                    k = u32::from(ks.get(i / len).copied().unwrap_or(0));
                    w.put(k, 4);
                }
                w.rice(r, k);
            });
        }
    }
}

/// UTF-8-style coding of a frame number (up to 36 bits).
fn put_utf8(w: &mut BitWriter<'_>, v: u64) {
    if v < 0x80 {
        w.put(v as u32, 8);
        return;
    }
    let bytes = match v {
        0..0x800 => 2,
        0x800..0x1_0000 => 3,
        0x1_0000..0x20_0000 => 4,
        0x20_0000..0x400_0000 => 5,
        0x400_0000..0x8000_0000 => 6,
        _ => 7,
    };
    let lead_bits = 7 - bytes;
    let mut shift = 6 * (bytes - 1);
    let lead_mark = (0xFF00u32 >> bytes) & 0xFF;
    w.put(lead_mark | ((v >> shift) as u32 & ((1 << lead_bits) - 1)), 8);
    while shift > 0 {
        shift -= 6;
        w.put(0x80 | ((v >> shift) as u32 & 0x3F), 8);
    }
}

const fn rate_code(rate: u32) -> u32 {
    match rate {
        88_200 => 1,
        176_400 => 2,
        192_000 => 3,
        8_000 => 4,
        16_000 => 5,
        22_050 => 6,
        24_000 => 7,
        32_000 => 8,
        44_100 => 9,
        48_000 => 10,
        96_000 => 11,
        _ => 0,
    }
}

/// The FLAC encoder's state (no buffers: the block is the caller's).
#[derive(Clone, Debug)]
pub struct FlacEncoder {
    rate: u32,
    channels: u8,
    level: u8,
    frame: u64,
    total: u64,
    min_frame: u32,
    max_frame: u32,
}

impl FlacEncoder {
    /// `channels` 1 or 2; `level` 0–2 (higher is clamped).
    #[must_use]
    pub fn new(rate: u32, channels: u8, level: u8) -> Self {
        FlacEncoder {
            rate,
            channels: channels.clamp(1, 2),
            level: level.min(MAX_LEVEL),
            frame: 0,
            total: 0,
            min_frame: 0,
            max_frame: 0,
        }
    }

    /// Frames encoded so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.total
    }

    /// `fLaC` and STREAMINFO with the counts so far.
    pub fn header(&self, out: &mut [u8; HEADER_LEN]) {
        out.fill(0);
        out[..4].copy_from_slice(b"fLaC");
        // Last metadata block, type 0 (STREAMINFO), length 34.
        out[4] = 0x80;
        out[5..8].copy_from_slice(&[0, 0, 34]);
        let mut w = BitWriter::new(&mut out[8..]);
        w.put(BLOCK as u32, 16);
        w.put(BLOCK as u32, 16);
        w.put(self.min_frame, 24);
        w.put(self.max_frame, 24);
        w.put(self.rate, 20);
        w.put(u32::from(self.channels) - 1, 3);
        w.put(BPS - 1, 5);
        w.put((self.total >> 32) as u32, 4);
        w.put(self.total as u32, 32);
        // MD5: zero (not computed).
        w.align();
    }

    /// Encode one block of `pcm.len() / channels` frames (at most
    /// [`BLOCK`]; only the last block may be shorter) into `out`, which
    /// holds at least [`max_frame_bytes`]. Returns the frame's length.
    pub fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> usize {
        let ch = usize::from(self.channels);
        let n = (pcm.len() / ch).min(BLOCK);
        if n == 0 {
            return 0;
        }
        let pmax = MAX_PORDER[usize::from(self.level)];
        // Channel assignment and the subframes' plans.
        let (assign, subs): (u32, [(Chan, Choice); 2]) = if ch == 1 {
            let p = plan(pcm, ch, n, Chan::Ch(0), pmax);
            (0, [(Chan::Ch(0), p), (Chan::Ch(0), p)])
        } else if self.level == 0 {
            let l = plan(pcm, ch, n, Chan::Ch(0), pmax);
            let r = plan(pcm, ch, n, Chan::Ch(1), pmax);
            (1, [(Chan::Ch(0), l), (Chan::Ch(1), r)])
        } else {
            let l = (Chan::Ch(0), plan(pcm, ch, n, Chan::Ch(0), pmax));
            let r = (Chan::Ch(1), plan(pcm, ch, n, Chan::Ch(1), pmax));
            let m = (Chan::Mid, plan(pcm, ch, n, Chan::Mid, pmax));
            let s = (Chan::Side, plan(pcm, ch, n, Chan::Side, pmax));
            let options = [(1, [l, r]), (8, [l, s]), (9, [s, r]), (10, [m, s])];
            let cost = |o: &(u32, [(Chan, Choice); 2])| o.1[0].1.bits + o.1[1].1.bits;
            options.iter().min_by_key(|o| cost(o)).copied().unwrap_or(options[0])
        };
        let bs_code = match n {
            192 => 1,
            576 => 2,
            1152 => 3,
            2304 => 4,
            4608 => 5,
            _ if n <= 256 => 6,
            _ => 7,
        };
        // Header (byte-aligned throughout), then its CRC-8.
        let hl = {
            let mut w = BitWriter::new(out);
            w.put(0xFFF8, 16);
            w.put(bs_code, 4);
            w.put(rate_code(self.rate), 4);
            w.put(assign, 4);
            w.put(0b100, 3);
            w.put(0, 1);
            put_utf8(&mut w, self.frame);
            match bs_code {
                6 => w.put(n as u32 - 1, 8),
                7 => w.put(n as u32 - 1, 16),
                _ => {}
            }
            w.bytes()
        };
        let crc = crc8(out.get(..hl).unwrap_or(&[]));
        let mut w = BitWriter::new(out.get_mut(hl..).unwrap_or(&mut []));
        w.put(u32::from(crc), 8);
        for (c, p) in subs.iter().take(ch) {
            write_subframe(&mut w, pcm, ch, n, *c, &p.plan);
        }
        w.align();
        let body = w.bytes();
        let end = hl + body;
        let crc = crc16(out.get(..end).unwrap_or(&[]));
        let len = end + 2;
        if let Some(f) = out.get_mut(end..len) {
            f.copy_from_slice(&crc.to_be_bytes());
        }
        self.frame += 1;
        self.total += n as u64;
        let l = len as u32;
        self.min_frame = if self.min_frame == 0 { l } else { self.min_frame.min(l) };
        self.max_frame = self.max_frame.max(l);
        len
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::{FakeAdc, Signal};
    use std::vec::Vec;

    #[test]
    fn utf8_numbers() {
        for (v, want) in [
            (0u64, &[0x00u8][..]),
            (0x7F, &[0x7F]),
            (0x80, &[0xC2, 0x80]),
            (0x7FF, &[0xDF, 0xBF]),
            (0x800, &[0xE0, 0xA0, 0x80]),
            (0x1_0000, &[0xF0, 0x90, 0x80, 0x80]),
        ] {
            let mut b = [0u8; 8];
            let mut w = BitWriter::new(&mut b);
            put_utf8(&mut w, v);
            let n = w.bytes();
            assert_eq!(&b[..n], want, "{v:#x}");
        }
    }

    #[test]
    fn frames_stay_within_the_bound() {
        for ch in [1u8, 2] {
            let mut adc = FakeAdc::new(44_100, ch, Signal::Noise(32_000), 7);
            let mut pcm = std::vec![0i16; BLOCK * usize::from(ch)];
            adc.fill(&mut pcm);
            for level in 0..=MAX_LEVEL {
                let mut e = FlacEncoder::new(44_100, ch, level);
                let mut out = std::vec![0u8; max_frame_bytes(ch)];
                let n = e.encode(&pcm, &mut out);
                assert!(n <= max_frame_bytes(ch));
            }
        }
    }

    #[test]
    fn silence_is_constant() {
        let pcm = [0i16; BLOCK * 2];
        let mut e = FlacEncoder::new(48_000, 2, 1);
        let mut out: Vec<u8> = std::vec![0; max_frame_bytes(2)];
        let n = e.encode(&pcm, &mut out);
        assert!(n < 20, "{n}");
    }
}
