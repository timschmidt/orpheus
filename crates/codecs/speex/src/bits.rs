//! The Speex bit reader (libspeex `bits.c`, decoder side): MSB first, with
//! libspeex's overflow semantics (reading past the end sets a sticky flag;
//! every later read gives 0 and `remaining` gives −1).

#[derive(Debug, Clone, Copy, Default)]
pub struct Bits {
    /// Bit position.
    pos: usize,
    /// Bits in the packet.
    len: usize,
    overflow: bool,
}

impl Bits {
    /// Start reading a packet of `bytes` bytes.
    #[must_use]
    pub fn new(bytes: usize) -> Self {
        Self { pos: 0, len: bytes.saturating_mul(8), overflow: false }
    }

    /// Bits left (−1 after an overflow).
    #[must_use]
    pub fn remaining(&self) -> i32 {
        if self.overflow { -1 } else { (self.len - self.pos).min(i32::MAX as usize) as i32 }
    }

    fn bit(buf: &[u8], pos: usize) -> u32 {
        u32::from(buf.get(pos >> 3).copied().unwrap_or(0) >> (7 - (pos & 7)) & 1)
    }

    /// `speex_bits_unpack_unsigned`.
    pub fn unpack(&mut self, buf: &[u8], n: u32) -> u32 {
        if self.pos + n as usize > self.len {
            self.overflow = true;
        }
        if self.overflow {
            return 0;
        }
        let mut d = 0;
        for _ in 0..n {
            d = d << 1 | Self::bit(buf, self.pos);
            self.pos += 1;
        }
        d
    }

    /// `speex_bits_peek_unsigned`.
    pub fn peek_n(&mut self, buf: &[u8], n: u32) -> u32 {
        if self.pos + n as usize > self.len {
            self.overflow = true;
        }
        if self.overflow {
            return 0;
        }
        let mut d = 0;
        for i in 0..n as usize {
            d = d << 1 | Self::bit(buf, self.pos + i);
        }
        d
    }

    /// `speex_bits_peek` (one bit).
    pub fn peek(&mut self, buf: &[u8]) -> u32 {
        self.peek_n(buf, 1)
    }

    /// `speex_bits_advance`; a negative `n` steps back (libspeex does that
    /// for the unused wideband sub-modes).
    pub fn advance(&mut self, n: i32) {
        let to = self.pos as i64 + i64::from(n);
        if self.overflow || to > self.len as i64 {
            self.overflow = true;
            return;
        }
        self.pos = to.max(0) as usize;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_msb_first_and_overflows() {
        let buf = [0b1011_0011u8, 0xF0];
        let mut b = Bits::new(2);
        assert_eq!(b.unpack(&buf, 3), 0b101);
        assert_eq!(b.peek_n(&buf, 5), 0b10011);
        assert_eq!(b.remaining(), 13);
        b.advance(5);
        assert_eq!(b.unpack(&buf, 4), 0xF);
        b.advance(-4);
        assert_eq!(b.peek(&buf), 1);
        assert_eq!(b.unpack(&buf, 9), 0);
        assert_eq!(b.remaining(), -1);
        assert_eq!(b.unpack(&buf, 1), 0);
    }
}
