//! MSB-first bit writer over a caller's buffer, and the FLAC CRCs.

/// Writes bits most-significant first. Writing past the end sets
/// [`BitWriter::overflowed`] and drops the bits (callers size the buffer
/// for the worst case, so this only guards against a mistake).
pub struct BitWriter<'a> {
    buf: &'a mut [u8],
    pos: usize,
    acc: u64,
    n: u32,
    overflow: bool,
}

impl<'a> BitWriter<'a> {
    #[must_use]
    pub fn new(buf: &'a mut [u8]) -> Self {
        BitWriter { buf, pos: 0, acc: 0, n: 0, overflow: false }
    }

    fn drain(&mut self) {
        while self.n >= 8 {
            self.n -= 8;
            let b = (self.acc >> self.n) as u8;
            match self.buf.get_mut(self.pos) {
                Some(s) => *s = b,
                None => self.overflow = true,
            }
            self.pos += 1;
        }
        self.acc &= (1u64 << self.n) - 1;
    }

    /// The low `bits` bits of `v` (`bits` ≤ 32).
    #[inline]
    pub fn put(&mut self, v: u32, bits: u32) {
        if bits == 0 {
            return;
        }
        let mask = if bits >= 32 { u32::MAX } else { (1u32 << bits) - 1 };
        self.acc = (self.acc << bits) | u64::from(v & mask);
        self.n += bits;
        if self.n >= 32 {
            self.drain();
        }
    }

    /// A signed value in two's complement, `bits` wide.
    #[inline]
    pub fn put_signed(&mut self, v: i32, bits: u32) {
        self.put(v as u32, bits);
    }

    /// `zeros` zero bits and a one.
    #[inline]
    pub fn unary(&mut self, mut zeros: u32) {
        while zeros >= 24 {
            self.put(0, 24);
            zeros -= 24;
        }
        self.put(1, zeros + 1);
    }

    /// One Rice-coded residual with parameter `k`.
    #[inline]
    pub fn rice(&mut self, v: i32, k: u32) {
        let u = zigzag(v);
        self.unary(u >> k);
        if k > 0 {
            self.put(u, k);
        }
    }

    /// Zero bits up to the next byte boundary.
    pub fn align(&mut self) {
        let pad = (8 - self.n % 8) % 8;
        self.put(0, pad);
        self.drain();
    }

    /// Whole bytes written so far (after [`Self::align`]: all of them).
    #[must_use]
    pub fn bytes(&mut self) -> usize {
        self.drain();
        self.pos
    }

    /// Bits written so far.
    #[must_use]
    pub fn bit_len(&self) -> u64 {
        self.pos as u64 * 8 + u64::from(self.n)
    }

    #[must_use]
    pub fn overflowed(&self) -> bool {
        self.overflow
    }
}

/// Signed to unsigned interleaving (0, −1, 1, −2, … → 0, 1, 2, 3, …).
#[inline]
#[must_use]
pub const fn zigzag(v: i32) -> u32 {
    ((v << 1) ^ (v >> 31)) as u32
}

/// CRC-8, polynomial `x⁸ + x² + x + 1` (FLAC frame header).
#[must_use]
pub fn crc8(data: &[u8]) -> u8 {
    let mut c = 0u8;
    for &b in data {
        c ^= b;
        for _ in 0..8 {
            c = if c & 0x80 != 0 { (c << 1) ^ 0x07 } else { c << 1 };
        }
    }
    c
}

/// CRC-16, polynomial `x¹⁶ + x¹⁵ + x² + 1` (FLAC frame footer).
#[must_use]
pub fn crc16(data: &[u8]) -> u16 {
    let mut c = 0u16;
    for &b in data {
        c ^= u16::from(b) << 8;
        for _ in 0..8 {
            c = if c & 0x8000 != 0 { (c << 1) ^ 0x8005 } else { c << 1 };
        }
    }
    c
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_msb_first_and_aligns() {
        let mut b = [0u8; 8];
        let mut w = BitWriter::new(&mut b);
        w.put(0b101, 3);
        w.put(0x1F, 5);
        w.put(0xABCD, 16);
        w.unary(3);
        w.align();
        let n = w.bytes();
        assert!(!w.overflowed());
        assert_eq!(&b[..n], &[0b1011_1111, 0xAB, 0xCD, 0b0001_0000]);
    }

    #[test]
    fn rice_and_zigzag() {
        assert_eq!([zigzag(0), zigzag(-1), zigzag(1), zigzag(-2)], [0, 1, 2, 3]);
        let mut b = [0u8; 4];
        let mut w = BitWriter::new(&mut b);
        // −3 → 5 = 0b101, k = 1: unary(2) "001", then "1".
        w.rice(-3, 1);
        w.align();
        let n = w.bytes();
        assert_eq!(&b[..n], &[0b0011_0000]);
    }

    #[test]
    fn overflow_is_flagged() {
        let mut b = [0u8; 1];
        let mut w = BitWriter::new(&mut b);
        w.put(0xFFFF, 16);
        w.align();
        let _ = w.bytes();
        assert!(w.overflowed());
    }

    #[test]
    fn crcs_match_known_values() {
        // "123456789": CRC-8/SMBUS 0xF4, CRC-16/UMTS (BUYPASS) 0xFEE8.
        assert_eq!(crc8(b"123456789"), 0xF4);
        assert_eq!(crc16(b"123456789"), 0xFEE8);
    }
}
