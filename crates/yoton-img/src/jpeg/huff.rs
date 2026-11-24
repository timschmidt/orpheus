//! JPEG Huffman tables and the entropy-coded-segment bit reader.

use bytemuck::{Pod, Zeroable};

use crate::View;

/// One Huffman table: an 8-bit look-ahead table plus the canonical
/// `maxcode`/`valoff` arrays for longer codes.
#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
pub(crate) struct Huff {
    /// `len << 8 | value` for codes of at most 8 bits; 0: longer code.
    lut: [u16; 256],
    /// Largest code of each length (-1: none), index 1..=16, [17] sentinel.
    maxcode: [i32; 18],
    /// `value index - code` for each length.
    valoff: [i32; 18],
    vals: [u8; 256],
}

impl Huff {
    /// Build from the 16 counts and the values of a DHT entry. `false` if
    /// the counts are inconsistent.
    pub(crate) fn build(&mut self, counts: &[u8; 16], vals: View<'_>) -> bool {
        let total: usize = counts.iter().map(|&c| usize::from(c)).sum();
        if total > 256 || vals.len() < total {
            return false;
        }
        for i in 0..total {
            self.vals[i] = vals.byte(i);
        }
        self.lut = [0; 256];
        let mut code: i32 = 0;
        let mut k: i32 = 0;
        for l in 1..=16usize {
            let n = i32::from(counts[l - 1]);
            self.valoff[l] = k - code;
            if n > 0 {
                if l <= 8 {
                    for j in 0..n {
                        let c = (code + j) as usize;
                        let v = u16::from(self.vals[(k + j) as usize]);
                        let first = c << (8 - l);
                        let last = first + (1 << (8 - l));
                        if last > 256 {
                            return false;
                        }
                        for e in &mut self.lut[first..last] {
                            *e = ((l as u16) << 8) | v;
                        }
                    }
                }
                code += n;
                k += n;
                self.maxcode[l] = code - 1;
            } else {
                self.maxcode[l] = -1;
            }
            if code > (1 << l) {
                return false;
            }
            code <<= 1;
        }
        self.maxcode[17] = i32::MAX;
        true
    }
}

/// Bit reader state that survives between calls (the bytes already loaded
/// were consumed from the source).
#[derive(Debug, Clone, Copy, Default)]
pub(crate) struct BitState {
    acc: u32,
    n: u32,
    /// A marker was reached; zeros are fed from here on.
    pub(crate) marker: Option<u8>,
}

/// MSB-first reader over the entropy-coded data in a view: removes byte
/// stuffing, stops at markers (feeding zeros), and counts zero bytes fed
/// past the end of the view.
pub(crate) struct Bits<'v> {
    pub(crate) v: View<'v>,
    pub(crate) pos: usize,
    st: BitState,
    pub(crate) overrun: u32,
    pub(crate) corrupt: bool,
}

impl<'v> Bits<'v> {
    pub(crate) fn new(v: View<'v>, st: BitState) -> Self {
        Self { v, pos: 0, st, overrun: 0, corrupt: false }
    }

    pub(crate) fn state(&self) -> BitState {
        self.st
    }

    /// Drop buffered bits (end of scan, restart marker).
    pub(crate) fn reset(&mut self) {
        self.st.acc = 0;
        self.st.n = 0;
    }

    pub(crate) fn marker(&self) -> Option<u8> {
        self.st.marker
    }

    pub(crate) fn clear_marker(&mut self) {
        self.st.marker = None;
    }

    #[inline]
    fn fill(&mut self) {
        while self.st.n <= 24 {
            let mut b = 0u8;
            if self.st.marker.is_none() {
                match self.v.get(self.pos) {
                    None => self.overrun += 1,
                    Some(0xFF) => match self.v.get(self.pos + 1) {
                        Some(0) => {
                            self.pos += 2;
                            b = 0xFF;
                        }
                        Some(0xFF) => {
                            // Fill byte before a marker.
                            self.pos += 1;
                            continue;
                        }
                        Some(m) => self.st.marker = Some(m),
                        None => {
                            self.pos += 1;
                            self.overrun += 1;
                        }
                    },
                    Some(x) => {
                        self.pos += 1;
                        b = x;
                    }
                }
            }
            self.st.acc |= u32::from(b) << (24 - self.st.n);
            self.st.n += 8;
        }
    }

    /// `n` (≤ 16) bits.
    #[inline]
    pub(crate) fn get(&mut self, n: u32) -> u32 {
        if n == 0 {
            return 0;
        }
        if self.st.n < n {
            self.fill();
        }
        let v = self.st.acc >> (32 - n);
        self.st.acc <<= n;
        self.st.n -= n;
        v
    }

    /// More zero bits were consumed than fed: the data really ended.
    pub(crate) fn overran(&self) -> bool {
        self.overrun * 8 > self.st.n
    }

    #[inline]
    pub(crate) fn bit(&mut self) -> bool {
        self.get(1) != 0
    }

    /// `s` bits as a signed value (JPEG EXTEND).
    #[inline]
    pub(crate) fn extend(&mut self, s: u32) -> i32 {
        if s == 0 {
            return 0;
        }
        let s = s.min(16);
        let v = self.get(s) as i32;
        if v < (1 << (s - 1)) { v - (1 << s) + 1 } else { v }
    }

    #[inline]
    pub(crate) fn decode(&mut self, h: &Huff) -> u8 {
        if self.st.n < 16 {
            self.fill();
        }
        let e = h.lut[(self.st.acc >> 24) as usize];
        if e >> 8 != 0 {
            let l = u32::from(e >> 8);
            self.st.acc <<= l;
            self.st.n -= l;
            return e as u8;
        }
        let mut l = 9usize;
        let mut code = (self.st.acc >> 23) as i32;
        while l <= 16 && code > h.maxcode[l] {
            l += 1;
            code = (self.st.acc >> (32 - l.min(16))) as i32;
        }
        if l > 16 {
            self.corrupt = true;
            // Skip a byte's worth so a corrupt stream still advances.
            self.st.acc <<= 8;
            self.st.n -= 8;
            return 0;
        }
        self.st.acc <<= l;
        self.st.n -= l as u32;
        h.vals[((h.valoff[l] + code) & 255) as usize]
    }
}
