//! zlib/DEFLATE decompression (RFC 1950/1951) with a 32 KiB window taken
//! from the arena; resumable at symbol granularity, so input may arrive in
//! pieces and output is pulled into a caller buffer of any size.
//!
//! Arena: 32 KiB window + 3.6 KiB of Huffman tables
//! ([`Inflate::NEED`]).

use bytemuck::{Pod, Zeroable};

use crate::{Arena, ArenaFull, Error};

/// Where compressed bytes come from.
pub(crate) trait Input {
    /// The next byte, or `None` when none is available now.
    fn byte(&mut self) -> Option<u8>;
    /// No byte will ever come again (after the available ones).
    fn ended(&self) -> bool;
}

const WSIZE: usize = 32768;
const FAST: u32 = 9;

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Huff {
    /// `sym << 4 | len` for codes of at most `FAST` bits (indexed by the
    /// next `FAST` input bits); 0: use the slow path.
    lut: [u16; 1 << FAST],
    /// Symbols ordered by code.
    syms: [u16; 288],
    /// Codes of each length.
    count: [u16; 16],
}

#[repr(C)]
#[derive(Clone, Copy, Pod, Zeroable)]
struct Tables {
    lit: Huff,
    dist: Huff,
    lens: [u8; 320],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum St {
    Zlib,
    Block,
    Stored(u32),
    DynCounts,
    DynClen { i: u8 },
    DynLens { i: u16 },
    Codes,
    End,
}

/// Why `run` returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Status {
    /// `out` is full.
    Full,
    /// Input ran dry before `out` was full.
    NeedInput,
    /// The end of the stream; `out` may be partly filled.
    End,
}

pub(crate) struct Inflate<'a> {
    window: &'a mut [u8],
    t: &'a mut Tables,
    wpos: usize,
    bits: u64,
    nbits: u32,
    st: St,
    last: bool,
    copy_len: u32,
    copy_dist: u32,
    hlit: u16,
    hdist: u16,
    hclen: u8,
    clen: [u8; 19],
}

const LEN_BASE: [u16; 29] = [
    3, 4, 5, 6, 7, 8, 9, 10, 11, 13, 15, 17, 19, 23, 27, 31, 35, 43, 51, 59, 67, 83, 99, 115, 131, 163, 195,
    227, 258,
];
const LEN_EXTRA: [u8; 29] =
    [0, 0, 0, 0, 0, 0, 0, 0, 1, 1, 1, 1, 2, 2, 2, 2, 3, 3, 3, 3, 4, 4, 4, 4, 5, 5, 5, 5, 0];
const DIST_BASE: [u16; 30] = [
    1, 2, 3, 4, 5, 7, 9, 13, 17, 25, 33, 49, 65, 97, 129, 193, 257, 385, 513, 769, 1025, 1537, 2049, 3073,
    4097, 6145, 8193, 12289, 16385, 24577,
];
const DIST_EXTRA: [u8; 30] =
    [0, 0, 0, 0, 1, 1, 2, 2, 3, 3, 4, 4, 5, 5, 6, 6, 7, 7, 8, 8, 9, 9, 10, 10, 11, 11, 12, 12, 13, 13];
const CLEN_ORDER: [u8; 19] = [16, 17, 18, 0, 8, 7, 9, 6, 10, 5, 11, 4, 12, 3, 13, 2, 14, 1, 15];

/// Build a canonical Huffman decoder from code lengths.
fn build(h: &mut Huff, lens: &[u8]) -> Result<(), Error> {
    h.count = [0; 16];
    for &l in lens {
        h.count[usize::from(l & 15)] += 1;
    }
    h.count[0] = 0;
    let mut left: i32 = 1;
    for l in 1..16 {
        left = (left << 1) - i32::from(h.count[l]);
        if left < 0 {
            return Err(Error::Corrupt);
        }
    }
    let mut offs = [0u16; 16];
    for l in 1..15 {
        offs[l + 1] = offs[l] + h.count[l];
    }
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for (l, n) in next.iter_mut().enumerate().skip(1) {
        // RFC 1951 §3.2.2 (`count[0]` is 0).
        code = (code + h.count[l - 1]) << 1;
        *n = code;
    }
    h.lut = [0; 1 << FAST];
    for (sym, &l) in lens.iter().enumerate() {
        let l = usize::from(l & 15);
        if l == 0 {
            continue;
        }
        h.syms[usize::from(offs[l])] = sym as u16;
        offs[l] += 1;
        let code = next[l];
        next[l] += 1;
        if l as u32 <= FAST {
            let rev = (code.reverse_bits() >> (16 - l)) as usize;
            let mut k = rev;
            while k < (1 << FAST) {
                h.lut[k] = ((sym as u16) << 4) | l as u16;
                k += 1 << l;
            }
        }
    }
    Ok(())
}

impl<'a> Inflate<'a> {
    /// Arena bytes needed.
    pub(crate) const NEED: usize = WSIZE + core::mem::size_of::<Tables>() + 2;

    pub(crate) fn new(arena: &mut Arena<'a>) -> Result<Self, ArenaFull> {
        let window = arena.slice::<u8>(WSIZE)?;
        let t = arena.one::<Tables>()?;
        Ok(Self {
            window,
            t,
            wpos: 0,
            bits: 0,
            nbits: 0,
            st: St::Zlib,
            last: false,
            copy_len: 0,
            copy_dist: 0,
            hlit: 0,
            hdist: 0,
            hclen: 0,
            clen: [0; 19],
        })
    }

    fn refill(&mut self, inp: &mut dyn Input) {
        while self.nbits <= 56 {
            match inp.byte() {
                Some(b) => {
                    self.bits |= u64::from(b) << self.nbits;
                    self.nbits += 8;
                }
                None => break,
            }
        }
    }

    /// Make sure `n` bits are buffered; `Ok(false)` to wait for input.
    fn want(&mut self, inp: &mut dyn Input, n: u32) -> Result<bool, Error> {
        if self.nbits < n {
            self.refill(inp);
        }
        if self.nbits >= n {
            Ok(true)
        } else if inp.ended() {
            Err(Error::Truncated)
        } else {
            Ok(false)
        }
    }

    #[inline]
    fn take(&mut self, n: u32) -> u32 {
        let v = (self.bits & ((1u64 << n) - 1)) as u32;
        self.bits >>= n;
        self.nbits -= n;
        v
    }

    /// Decode one symbol (the caller made sure 15 bits are buffered, or
    /// the input ended).
    fn decode(&mut self, dist: bool) -> Result<u16, Error> {
        let h = if dist { &self.t.dist } else { &self.t.lit };
        let e = h.lut[(self.bits & ((1 << FAST) - 1)) as usize];
        let (sym, len) = if e & 15 != 0 {
            (e >> 4, u32::from(e & 15))
        } else {
            let mut code: i32 = 0;
            let mut first: i32 = 0;
            let mut index: i32 = 0;
            let mut found = None;
            for l in 1..16u32 {
                code |= ((self.bits >> (l - 1)) & 1) as i32;
                let count = i32::from(h.count[l as usize]);
                if code - first < count {
                    found = Some((h.syms[(index + code - first) as usize], l));
                    break;
                }
                index += count;
                first += count;
                first <<= 1;
                code <<= 1;
            }
            found.ok_or(Error::Corrupt)?
        };
        if len > self.nbits {
            return Err(Error::Truncated);
        }
        self.take(len);
        Ok(sym)
    }

    #[inline]
    fn put(&mut self, b: u8, out: &mut [u8], o: &mut usize) {
        self.window[self.wpos] = b;
        self.wpos = (self.wpos + 1) & (WSIZE - 1);
        out[*o] = b;
        *o += 1;
    }

    fn fixed(&mut self) -> Result<(), Error> {
        let mut l = [0u8; 288];
        l[..144].fill(8);
        l[144..256].fill(9);
        l[256..280].fill(7);
        l[280..].fill(8);
        build(&mut self.t.lit, &l)?;
        build(&mut self.t.dist, &[5u8; 30])
    }

    /// Decompress into `out`; returns the bytes written and why it stopped.
    pub(crate) fn run(&mut self, inp: &mut dyn Input, out: &mut [u8]) -> Result<(usize, Status), Error> {
        let mut o = 0usize;
        loop {
            // A pending match copy first.
            while self.copy_len > 0 {
                if o == out.len() {
                    return Ok((o, Status::Full));
                }
                let b = self.window[(self.wpos.wrapping_sub(self.copy_dist as usize)) & (WSIZE - 1)];
                self.put(b, out, &mut o);
                self.copy_len -= 1;
            }
            match self.st {
                St::End => return Ok((o, Status::End)),
                St::Zlib => {
                    if !self.want(inp, 16)? {
                        return Ok((o, Status::NeedInput));
                    }
                    let cmf = self.take(8);
                    let flg = self.take(8);
                    if cmf & 15 != 8 || (cmf >> 4) > 7 || (cmf * 256 + flg) % 31 != 0 || flg & 0x20 != 0 {
                        return Err(Error::Corrupt);
                    }
                    self.st = St::Block;
                }
                St::Block => {
                    if self.last {
                        self.st = St::End;
                        continue;
                    }
                    if !self.want(inp, 3)? {
                        return Ok((o, Status::NeedInput));
                    }
                    self.last = self.take(1) == 1;
                    match self.take(2) {
                        0 => {
                            let drop = self.nbits % 8;
                            self.take(drop);
                            self.st = St::Stored(u32::MAX);
                        }
                        1 => {
                            self.fixed()?;
                            self.st = St::Codes;
                        }
                        2 => self.st = St::DynCounts,
                        _ => return Err(Error::Corrupt),
                    }
                }
                St::Stored(left) => {
                    let mut left = left;
                    if left == u32::MAX {
                        if !self.want(inp, 32)? {
                            return Ok((o, Status::NeedInput));
                        }
                        let len = self.take(16);
                        let nlen = self.take(16);
                        if len != !nlen & 0xFFFF {
                            return Err(Error::Corrupt);
                        }
                        left = len;
                    }
                    while left > 0 {
                        if o == out.len() {
                            self.st = St::Stored(left);
                            return Ok((o, Status::Full));
                        }
                        let b = if self.nbits >= 8 {
                            self.take(8) as u8
                        } else {
                            match inp.byte() {
                                Some(b) => b,
                                None => {
                                    self.st = St::Stored(left);
                                    if inp.ended() {
                                        return Err(Error::Truncated);
                                    }
                                    return Ok((o, Status::NeedInput));
                                }
                            }
                        };
                        self.put(b, out, &mut o);
                        left -= 1;
                    }
                    self.st = St::Block;
                }
                St::DynCounts => {
                    if !self.want(inp, 14)? {
                        return Ok((o, Status::NeedInput));
                    }
                    self.hlit = self.take(5) as u16 + 257;
                    self.hdist = self.take(5) as u16 + 1;
                    self.hclen = self.take(4) as u8 + 4;
                    if self.hlit > 286 || self.hdist > 30 {
                        return Err(Error::Corrupt);
                    }
                    self.clen = [0; 19];
                    self.st = St::DynClen { i: 0 };
                }
                St::DynClen { mut i } => {
                    while i < self.hclen {
                        if !self.want(inp, 3)? {
                            self.st = St::DynClen { i };
                            return Ok((o, Status::NeedInput));
                        }
                        self.clen[usize::from(CLEN_ORDER[usize::from(i)])] = self.take(3) as u8;
                        i += 1;
                    }
                    let clen = self.clen;
                    build(&mut self.t.dist, &clen)?;
                    self.st = St::DynLens { i: 0 };
                }
                St::DynLens { mut i } => {
                    let total = self.hlit + self.hdist;
                    while i < total {
                        if !self.want(inp, 14).or_else(|e| if self.nbits > 0 { Ok(true) } else { Err(e) })? {
                            self.st = St::DynLens { i };
                            return Ok((o, Status::NeedInput));
                        }
                        let sym = self.decode(true)?;
                        let (val, rep) = match sym {
                            0..=15 => (sym as u8, 1),
                            16 => {
                                if i == 0 {
                                    return Err(Error::Corrupt);
                                }
                                (self.t.lens[usize::from(i - 1)], 3 + self.take_checked(2)?)
                            }
                            17 => (0, 3 + self.take_checked(3)?),
                            _ => (0, 11 + self.take_checked(7)?),
                        };
                        if u32::from(i) + rep > u32::from(total) {
                            return Err(Error::Corrupt);
                        }
                        for _ in 0..rep {
                            self.t.lens[usize::from(i)] = val;
                            i += 1;
                        }
                    }
                    let (hl, hd) = (usize::from(self.hlit), usize::from(self.hdist));
                    if self.t.lens[256] == 0 {
                        return Err(Error::Corrupt);
                    }
                    let lens = self.t.lens;
                    build(&mut self.t.lit, &lens[..hl])?;
                    build(&mut self.t.dist, &lens[hl..hl + hd])?;
                    self.st = St::Codes;
                }
                St::Codes => loop {
                    if o == out.len() {
                        return Ok((o, Status::Full));
                    }
                    if self.nbits < 48 {
                        self.refill(inp);
                        if self.nbits < 48 && !inp.ended() {
                            return Ok((o, Status::NeedInput));
                        }
                        if self.nbits == 0 {
                            return Err(Error::Truncated);
                        }
                    }
                    let sym = self.decode(false)?;
                    if sym < 256 {
                        self.put(sym as u8, out, &mut o);
                        continue;
                    }
                    if sym == 256 {
                        self.st = St::Block;
                        break;
                    }
                    let li = usize::from(sym - 257);
                    if li >= 29 {
                        return Err(Error::Corrupt);
                    }
                    let len = u32::from(LEN_BASE[li]) + self.take_checked(u32::from(LEN_EXTRA[li]))?;
                    let ds = usize::from(self.decode(true)?);
                    if ds >= 30 {
                        return Err(Error::Corrupt);
                    }
                    let dist = u32::from(DIST_BASE[ds]) + self.take_checked(u32::from(DIST_EXTRA[ds]))?;
                    self.copy_len = len;
                    self.copy_dist = dist;
                    while self.copy_len > 0 && o < out.len() {
                        let b = self.window[(self.wpos.wrapping_sub(dist as usize)) & (WSIZE - 1)];
                        self.put(b, out, &mut o);
                        self.copy_len -= 1;
                    }
                },
            }
        }
    }

    fn take_checked(&mut self, n: u32) -> Result<u32, Error> {
        if n > self.nbits {
            return Err(Error::Truncated);
        }
        Ok(self.take(n))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Bytes<'d> {
        d: &'d [u8],
        pos: usize,
        lim: usize,
    }

    impl Input for Bytes<'_> {
        fn byte(&mut self) -> Option<u8> {
            if self.pos < self.lim.min(self.d.len()) {
                self.pos += 1;
                Some(self.d[self.pos - 1])
            } else {
                None
            }
        }
        fn ended(&self) -> bool {
            self.lim >= self.d.len()
        }
    }

    fn zlib_store(data: &[u8]) -> std::vec::Vec<u8> {
        let mut v = std::vec![0x78, 0x01];
        let chunks: std::vec::Vec<&[u8]> = data.chunks(1000).collect();
        for (i, c) in chunks.iter().enumerate() {
            v.push(u8::from(i + 1 == chunks.len()));
            let l = c.len() as u16;
            v.extend_from_slice(&l.to_le_bytes());
            v.extend_from_slice(&(!l).to_le_bytes());
            v.extend_from_slice(c);
        }
        v.extend_from_slice(&[0, 0, 0, 0]);
        v
    }

    #[test]
    fn stored_in_pieces() {
        let data: std::vec::Vec<u8> = (0..5000u32).map(|i| (i * 7 % 251) as u8).collect();
        let z = zlib_store(&data);
        let mut mem = std::vec![0u8; Inflate::NEED + 64];
        let mut arena = Arena::new(&mut mem);
        let mut inf = Inflate::new(&mut arena).unwrap();
        let mut inp = Bytes { d: &z, pos: 0, lim: 0 };
        let mut out = std::vec::Vec::new();
        let mut buf = [0u8; 333];
        for _ in 0..10000 {
            let (n, st) = inf.run(&mut inp, &mut buf).unwrap();
            out.extend_from_slice(&buf[..n]);
            match st {
                Status::End => break,
                Status::NeedInput => inp.lim += 7,
                Status::Full => {}
            }
        }
        assert_eq!(out, data);
    }

    #[test]
    fn huffman_tables() {
        // Fixed and dynamic blocks are covered by the PNG tests (real zlib
        // output); here: over-subscribed lengths are rejected, a single code
        // is accepted.
        let mut h = Huff::zeroed();
        assert!(build(&mut h, &[1, 1, 1]).is_err());
        assert!(build(&mut h, &[0, 1]).is_ok());
        assert_eq!(h.lut[0], (1 << 4) | 1);
    }
}
