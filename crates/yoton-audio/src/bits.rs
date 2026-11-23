//! MSB-first bit reader over a [`View`] (the two halves of a ring peek).
//!
//! Reads past the end return zero bits and are detected afterwards with
//! [`BitReader::overrun`]: a decoder parses a whole frame optimistically and
//! then checks whether the frame was really all there (`NeedData`) or the
//! data was corrupt. Loops that could run on zero bits (unary codes) stop
//! once the reader is well past the end, so corrupt input cannot spin.

use crate::source::View;

#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    a: &'a [u8],
    b: &'a [u8],
    len: usize,
    /// Next byte to load (may run past `len`; zeros are loaded there).
    pos: usize,
    /// Left-aligned bits; bits below the valid ones are zero.
    cache: u64,
    nbits: u32,
}

impl<'a> BitReader<'a> {
    #[must_use]
    pub fn new(v: View<'a>) -> Self {
        Self { a: v.a, b: v.b, len: v.len(), pos: 0, cache: 0, nbits: 0 }
    }

    #[inline]
    fn refill(&mut self) {
        while self.nbits <= 56 {
            let byte = if let Some(&x) = self.a.get(self.pos) {
                x
            } else {
                self.b.get(self.pos.wrapping_sub(self.a.len())).copied().unwrap_or(0)
            };
            self.cache |= u64::from(byte) << (56 - self.nbits);
            self.nbits += 8;
            self.pos += 1;
        }
    }

    /// Bits consumed so far.
    #[must_use]
    pub fn bit_pos(&self) -> usize {
        self.pos * 8 - self.nbits as usize
    }

    /// Whole bytes consumed (rounded up).
    #[must_use]
    pub fn byte_pos(&self) -> usize {
        self.bit_pos().div_ceil(8)
    }

    /// Whether more bits were consumed than the view holds.
    #[must_use]
    pub fn overrun(&self) -> bool {
        self.bit_pos() > self.len * 8
    }

    /// Far enough past the end that a loop should give up.
    #[must_use]
    #[inline]
    pub fn exhausted(&self) -> bool {
        self.pos > self.len + 8
    }

    /// Read `n` (0..=32) bits.
    #[inline]
    pub fn read(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.nbits < n {
            self.refill();
        }
        let v = (self.cache >> (64 - n)) as u32;
        self.cache <<= n;
        self.nbits -= n;
        v
    }

    /// Read `n` (1..=32) bits as a two's complement number.
    #[inline]
    pub fn read_signed(&mut self, n: u32) -> i32 {
        if n == 0 {
            return 0;
        }
        let v = self.read(n);
        ((v << (32 - n)) as i32) >> (32 - n)
    }

    #[inline]
    pub fn bit(&mut self) -> bool {
        self.read(1) != 0
    }

    pub fn skip(&mut self, mut n: usize) {
        while n > 0 {
            let k = n.min(32);
            self.read(k as u32);
            n -= k;
        }
    }

    /// Skip to the next byte boundary.
    pub fn align(&mut self) {
        let r = self.bit_pos() % 8;
        if r != 0 {
            self.read(8 - r as u32);
        }
    }

    /// Count zero bits up to and including the next one bit; returns the
    /// count of zeros. Gives up (returning what it has) past the end.
    #[inline]
    pub fn unary(&mut self) -> u32 {
        let mut count = 0u32;
        loop {
            let lz = self.cache.leading_zeros();
            if lz < self.nbits {
                self.cache <<= lz + 1;
                self.nbits -= lz + 1;
                return count + lz;
            }
            count = count.saturating_add(self.nbits);
            self.cache = 0;
            self.nbits = 0;
            if self.exhausted() {
                return count;
            }
            self.refill();
        }
    }

    /// One Rice-coded signed value with parameter `k`.
    #[inline]
    pub fn rice(&mut self, k: u32) -> i32 {
        let q = self.unary();
        let u = (q << k) | self.read(k);
        ((u >> 1) as i32) ^ -((u & 1) as i32)
    }

    /// FLAC's UTF-8-like coded number (up to 36 bits). `None` if invalid.
    pub fn utf8_u64(&mut self) -> Option<u64> {
        let first = self.read(8);
        let (mut v, extra) = match first {
            0x00..=0x7F => return Some(u64::from(first)),
            0xC0..=0xDF => (first & 0x1F, 1),
            0xE0..=0xEF => (first & 0x0F, 2),
            0xF0..=0xF7 => (first & 0x07, 3),
            0xF8..=0xFB => (first & 0x03, 4),
            0xFC..=0xFD => (first & 0x01, 5),
            0xFE => (0, 6),
            _ => return None,
        };
        let mut v64 = u64::from(v);
        for _ in 0..extra {
            let b = self.read(8);
            if b & 0xC0 != 0x80 {
                return None;
            }
            v = b & 0x3F;
            v64 = (v64 << 6) | u64::from(v);
        }
        Some(v64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_across_split() {
        let a = [0b1011_0000u8, 0x12];
        let b = [0x34u8, 0x80];
        let mut r = BitReader::new(View::new(&a, &b));
        assert_eq!(r.read(4), 0b1011);
        assert_eq!(r.read(4), 0);
        assert_eq!(r.read(16), 0x1234);
        assert_eq!(r.unary(), 0);
        assert!(!r.overrun());
        assert_eq!(r.read(7), 0);
        assert!(!r.overrun());
        r.read(1);
        assert!(r.overrun());
        let mut z = BitReader::new(View::new(&[0u8; 4], &[]));
        let c = z.unary();
        assert!(c >= 32 && z.overrun());
    }

    #[test]
    fn rice_and_signed() {
        // rice k=2: value 3 -> zigzag 6 -> q=1, r=2 -> "01" "10"
        let mut r = BitReader::new(View::new(&[0b0110_1111, 0xFF], &[]));
        assert_eq!(r.rice(2), 3);
        assert_eq!(r.read_signed(4), -1);
    }
}
