//! LSB-first bit reader over an Ogg packet (its byte ranges in the ring
//! view). Reads past the end return zeros and set `eop` (Vorbis I §2.1.4:
//! end-of-packet is a normal condition during audio decode).

use yoton_audio::View;
use yoton_ogg::Packet;

pub(crate) struct Bits<'a> {
    v: View<'a>,
    ranges: &'a [(u32, u32)],
    ri: usize,
    off: usize,
    cache: u64,
    nbits: u32,
    pub eop: bool,
}

impl<'a> Bits<'a> {
    pub(crate) fn new(v: View<'a>, p: &'a Packet) -> Self {
        Self { v, ranges: &p.ranges[..p.n], ri: 0, off: 0, cache: 0, nbits: 0, eop: false }
    }

    #[inline]
    fn refill(&mut self) {
        while self.nbits <= 56 {
            let Some(&(at, len)) = self.ranges.get(self.ri) else { return };
            if self.off >= len as usize {
                self.ri += 1;
                self.off = 0;
                continue;
            }
            let b = self.v.byte(at as usize + self.off);
            self.off += 1;
            self.cache |= u64::from(b) << self.nbits;
            self.nbits += 8;
        }
    }

    /// `n` (0..=32) bits.
    #[inline]
    pub(crate) fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.nbits < n {
            self.refill();
            if self.nbits < n {
                self.eop = true;
                self.cache = 0;
                self.nbits = 0;
                return 0;
            }
        }
        let v = (self.cache & ((1u64 << n) - 1)) as u32;
        self.cache >>= n;
        self.nbits -= n;
        v
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    /// Vorbis `float32_unpack`: 21-bit mantissa, sign, 10-bit exponent.
    pub(crate) fn float32(&mut self) -> f32 {
        let x = self.read(32);
        let mant = (x & 0x1F_FFFF) as f32;
        let exp = ((x >> 21) & 0x3FF) as i32;
        let m = if x & 0x8000_0000 != 0 { -mant } else { mant };
        crate::math::ldexp(m, exp - 788)
    }
}

/// Bits needed to hold `x` (Vorbis `ilog`).
#[must_use]
pub(crate) fn ilog(x: u32) -> u32 {
    32 - x.leading_zeros()
}
