//! DEFLATE (RFC 1951) decoding for EPUB's zip entries, streaming and in
//! bounded pieces: compressed bytes in any pieces, at most `max` bytes out
//! per call, resumable at any byte.
//!
//! RAM: the decoder itself is a few dozen bytes ([`Inflate`]); its window
//! (32 KiB, the most a DEFLATE distance reaches back) and Huffman tables
//! (2,304 B) are the caller's [`Scratch`], lent for the whole entry: the
//! decoder arena of the card task when no track is loaded (05 §7.15). The
//! output is the window itself: each call's new bytes are the `made` bytes
//! before the write position ([`Inflate::made`]), read before the next
//! call. A smaller window (any power of two from 1 KiB) decodes streams
//! whose distances stay within it and refuses the others
//! ([`Error::Window`]).

/// The largest window DEFLATE needs.
pub const WINDOW: usize = 32768;

const LIT_FAST: u32 = 9;
const DIST_FAST: u32 = 7;
const LIT_SYMS: usize = 288;
const DIST_SYMS: usize = 32;

/// A Huffman table's shape in its bytes: 16 counts, the symbols in code
/// order, then a lookup table of `1 << fast` entries (`sym << 4 | len`,
/// 0: a longer code), all `u16` little-endian.
#[derive(Clone, Copy)]
struct Shape {
    syms: usize,
    fast: u32,
}

const LIT: Shape = Shape { syms: LIT_SYMS, fast: LIT_FAST };
const DIST: Shape = Shape { syms: DIST_SYMS, fast: DIST_FAST };

impl Shape {
    const fn bytes(self) -> usize {
        2 * (16 + self.syms + (1 << self.fast))
    }
}

const LIT_BYTES: usize = LIT.bytes();
const DIST_BYTES: usize = DIST.bytes();
/// Code lengths of a dynamic block (literal/length and distance codes).
const LENS: usize = 320;

/// Bytes of the Huffman tables.
pub const TABLES: usize = LIT_BYTES + DIST_BYTES + LENS;
/// Scratch bytes for the full window.
pub const SCRATCH: usize = WINDOW + TABLES;

/// The decoder's lent memory: the window, then the tables.
pub struct Scratch<'a> {
    win: &'a mut [u8],
    lit: &'a mut [u8],
    dist: &'a mut [u8],
    lens: &'a mut [u8],
}

impl<'a> Scratch<'a> {
    /// The largest window that fits in `buf` beside the tables (a power of
    /// two, at most 32 KiB); `None` if not even 1 KiB fits.
    pub fn new(buf: &'a mut [u8]) -> Option<Scratch<'a>> {
        let room = buf.len().checked_sub(TABLES)?;
        let mut w = WINDOW;
        while w > room {
            w /= 2;
        }
        if w < 1024 {
            return None;
        }
        let (win, rest) = buf.split_at_mut(w);
        let (lit, rest) = rest.split_at_mut(LIT_BYTES);
        let (dist, rest) = rest.split_at_mut(DIST_BYTES);
        let lens = rest.get_mut(..LENS)?;
        Some(Scratch { win, lit, dist, lens })
    }

    /// The window's size.
    #[must_use]
    pub fn window_len(&self) -> usize {
        self.win.len()
    }
}

/// Why a stream cannot be decoded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Error {
    /// Not DEFLATE: a bad block type, code, length or distance.
    Corrupt,
    /// The input ended inside the stream.
    Truncated,
    /// A distance beyond the lent window (only with a window under 32 KiB).
    Window,
}

/// How a call ended.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Status {
    /// `max` bytes were made: call again.
    Full,
    /// All the input is taken (and more is to come): call with more.
    More,
    /// The end of the stream.
    Done,
}

/// What a call did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Run {
    /// Input bytes taken (all of them unless `Full` or `Done`).
    pub used: usize,
    /// Bytes made (in the window, before the write position).
    pub made: usize,
    pub status: Status,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum St {
    Header,
    StoredLen,
    Stored,
    DynHeader,
    DynClen,
    DynLens,
    Codes,
    Done,
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

fn rd(t: &[u8], i: usize) -> u16 {
    match t.get(2 * i..2 * i + 2) {
        Some(b) => u16::from_le_bytes([b[0], b[1]]),
        None => 0,
    }
}

fn wr(t: &mut [u8], i: usize, v: u16) {
    if let Some(b) = t.get_mut(2 * i..2 * i + 2) {
        b.copy_from_slice(&v.to_le_bytes());
    }
}

/// A canonical Huffman decoder from code lengths (0–15 each). An
/// over-subscribed set is corrupt; an incomplete one is taken (a code it
/// lacks is corrupt when met).
fn build(t: &mut [u8], sh: Shape, lens: &[u8]) -> Result<(), Error> {
    let mut count = [0u16; 16];
    for &l in lens {
        count[usize::from(l & 15)] += 1;
    }
    count[0] = 0;
    let mut left: i32 = 1;
    for &c in &count[1..] {
        left = (left << 1) - i32::from(c);
        if left < 0 {
            return Err(Error::Corrupt);
        }
    }
    let mut offs = [0u16; 16];
    for l in 1..15 {
        offs[l + 1] = offs[l] + count[l];
    }
    let mut next = [0u16; 16];
    let mut code = 0u16;
    for l in 1..16 {
        code = (code + count[l - 1]) << 1;
        next[l] = code;
    }
    for (l, &c) in count.iter().enumerate() {
        wr(t, l, c);
    }
    let lut = 16 + sh.syms;
    for k in 0..(1usize << sh.fast) {
        wr(t, lut + k, 0);
    }
    for (sym, &l) in lens.iter().enumerate() {
        let l = usize::from(l & 15);
        if l == 0 {
            continue;
        }
        wr(t, 16 + usize::from(offs[l]), sym as u16);
        offs[l] += 1;
        let code = next[l];
        next[l] += 1;
        if l as u32 <= sh.fast {
            let mut k = usize::from(code.reverse_bits() >> (16 - l));
            while k < (1 << sh.fast) {
                wr(t, lut + k, ((sym as u16) << 4) | l as u16);
                k += 1 << l;
            }
        }
    }
    Ok(())
}

/// The symbol at the front of `bits` and its code's length.
fn decode(t: &[u8], sh: Shape, bits: u64, nbits: u32) -> Result<(u16, u32), Error> {
    let e = rd(t, 16 + sh.syms + (bits & ((1 << sh.fast) - 1)) as usize);
    let (sym, len) = if e != 0 {
        (e >> 4, u32::from(e & 15))
    } else {
        let (mut code, mut first, mut index) = (0i32, 0i32, 0i32);
        let mut found = None;
        for l in 1..16u32 {
            code |= ((bits >> (l - 1)) & 1) as i32;
            let count = i32::from(rd(t, l as usize));
            if code - first < count {
                found = Some((rd(t, 16 + (index + code - first) as usize), l));
                break;
            }
            index += count;
            first = (first + count) << 1;
            code <<= 1;
        }
        found.ok_or(Error::Corrupt)?
    };
    if len > nbits {
        return Err(Error::Truncated);
    }
    Ok((sym, len))
}

/// The decoder's state (no buffers: see [`Scratch`]).
#[derive(Clone, Copy, Debug)]
pub struct Inflate {
    st: St,
    bits: u64,
    nbits: u32,
    last: bool,
    copy_len: u32,
    copy_dist: u32,
    hlit: u16,
    hdist: u16,
    hclen: u16,
    i: u16,
    stored: u32,
    /// Write position (wrapping; the window index is masked).
    wpos: u32,
    /// Bytes made in all (saturating): a distance may not reach before.
    total: u32,
}

impl Default for Inflate {
    fn default() -> Self {
        Inflate::new()
    }
}

/// The input of one call and how much of it is taken.
struct In<'i> {
    b: &'i [u8],
    used: usize,
    end: bool,
}

impl Inflate {
    #[must_use]
    pub const fn new() -> Inflate {
        Inflate {
            st: St::Header,
            bits: 0,
            nbits: 0,
            last: false,
            copy_len: 0,
            copy_dist: 0,
            hlit: 0,
            hdist: 0,
            hclen: 0,
            i: 0,
            stored: 0,
            wpos: 0,
            total: 0,
        }
    }

    /// Bytes made in all.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.total
    }

    /// The last call's `made` bytes, in the window (two pieces when they
    /// wrap around its end).
    #[must_use]
    pub fn made<'s>(&self, s: &'s Scratch<'_>, made: usize) -> (&'s [u8], &'s [u8]) {
        let w = s.win.len();
        let made = made.min(w);
        let end = self.wpos as usize & (w - 1);
        let start = end.wrapping_sub(made) & (w - 1);
        if made == 0 {
            (&[], &[])
        } else if start < end {
            (&s.win[start..end], &[])
        } else {
            (&s.win[start..], &s.win[..end])
        }
    }

    fn refill(&mut self, inp: &mut In<'_>) {
        while self.nbits <= 56 {
            let Some(&b) = inp.b.get(inp.used) else { break };
            self.bits |= u64::from(b) << self.nbits;
            self.nbits += 8;
            inp.used += 1;
        }
    }

    /// `n` bits buffered (`Ok(false)`: wait for more input).
    fn want(&mut self, inp: &mut In<'_>, n: u32) -> Result<bool, Error> {
        if self.nbits < n {
            self.refill(inp);
        }
        if self.nbits >= n {
            Ok(true)
        } else if inp.end {
            Err(Error::Truncated)
        } else {
            Ok(false)
        }
    }

    fn take(&mut self, n: u32) -> u32 {
        let v = (self.bits & ((1u64 << n) - 1)) as u32;
        self.bits >>= n;
        self.nbits -= n;
        v
    }

    fn take_checked(&mut self, n: u32) -> Result<u32, Error> {
        if n > self.nbits {
            return Err(Error::Truncated);
        }
        Ok(self.take(n))
    }

    fn put(&mut self, win: &mut [u8], b: u8) {
        let mask = win.len() - 1;
        win[self.wpos as usize & mask] = b;
        self.wpos = self.wpos.wrapping_add(1);
        self.total = self.total.saturating_add(1);
    }

    fn after_block(&mut self) {
        self.st = if self.last { St::Done } else { St::Header };
    }

    /// Decode from `input` (`end`: nothing follows it) into the window,
    /// at most `max` bytes (and never more than the window holds).
    pub fn run(&mut self, s: &mut Scratch<'_>, input: &[u8], end: bool, max: usize) -> Result<Run, Error> {
        let mut inp = In { b: input, used: 0, end };
        let mut made = 0usize;
        let r = self.run_in(s, &mut inp, max.min(s.win.len()), &mut made);
        r.map(|status| Run { used: inp.used, made, status })
    }

    fn run_in(
        &mut self,
        s: &mut Scratch<'_>,
        inp: &mut In<'_>,
        max: usize,
        made: &mut usize,
    ) -> Result<Status, Error> {
        let mask = s.win.len() - 1;
        loop {
            // A match being copied first.
            while self.copy_len > 0 {
                if *made == max {
                    return Ok(Status::Full);
                }
                let b = s.win[(self.wpos.wrapping_sub(self.copy_dist)) as usize & mask];
                self.put(s.win, b);
                *made += 1;
                self.copy_len -= 1;
            }
            match self.st {
                St::Done => return Ok(Status::Done),
                St::Header => {
                    if !self.want(inp, 3)? {
                        return Ok(Status::More);
                    }
                    self.last = self.take(1) == 1;
                    match self.take(2) {
                        0 => {
                            let drop = self.nbits % 8;
                            self.take(drop);
                            self.st = St::StoredLen;
                        }
                        1 => {
                            let mut l = [0u8; 288];
                            l[..144].fill(8);
                            l[144..256].fill(9);
                            l[256..280].fill(7);
                            l[280..].fill(8);
                            build(s.lit, LIT, &l)?;
                            build(s.dist, DIST, &[5u8; 30])?;
                            self.st = St::Codes;
                        }
                        2 => self.st = St::DynHeader,
                        _ => return Err(Error::Corrupt),
                    }
                }
                St::StoredLen => {
                    if !self.want(inp, 32)? {
                        return Ok(Status::More);
                    }
                    let len = self.take(16);
                    let nlen = self.take(16);
                    if len != !nlen & 0xFFFF {
                        return Err(Error::Corrupt);
                    }
                    self.stored = len;
                    self.st = St::Stored;
                }
                St::Stored => {
                    while self.stored > 0 {
                        if *made == max {
                            return Ok(Status::Full);
                        }
                        let b = if self.nbits >= 8 {
                            self.take(8) as u8
                        } else if let Some(&b) = inp.b.get(inp.used) {
                            inp.used += 1;
                            b
                        } else if inp.end {
                            return Err(Error::Truncated);
                        } else {
                            return Ok(Status::More);
                        };
                        self.put(s.win, b);
                        *made += 1;
                        self.stored -= 1;
                    }
                    self.after_block();
                }
                St::DynHeader => {
                    if !self.want(inp, 14)? {
                        return Ok(Status::More);
                    }
                    self.hlit = self.take(5) as u16 + 257;
                    self.hdist = self.take(5) as u16 + 1;
                    self.hclen = self.take(4) as u16 + 4;
                    if self.hlit > 286 || self.hdist > 30 {
                        return Err(Error::Corrupt);
                    }
                    s.lens[..19].fill(0);
                    self.i = 0;
                    self.st = St::DynClen;
                }
                St::DynClen => {
                    while self.i < self.hclen {
                        if !self.want(inp, 3)? {
                            return Ok(Status::More);
                        }
                        let v = self.take(3) as u8;
                        s.lens[usize::from(CLEN_ORDER[usize::from(self.i)])] = v;
                        self.i += 1;
                    }
                    // The code-length code, in the distance table's place.
                    build(s.dist, DIST, &s.lens[..19])?;
                    self.i = 0;
                    self.st = St::DynLens;
                }
                St::DynLens => {
                    let total = self.hlit + self.hdist;
                    while self.i < total {
                        if self.nbits < 14 {
                            self.refill(inp);
                            if self.nbits < 14 && !inp.end {
                                return Ok(Status::More);
                            }
                        }
                        let (sym, len) = decode(s.dist, DIST, self.bits, self.nbits)?;
                        self.take(len);
                        let (val, rep) = match sym {
                            0..=15 => (sym as u8, 1),
                            16 => {
                                let Some(&prev) = s.lens.get(usize::from(self.i).wrapping_sub(1)) else {
                                    return Err(Error::Corrupt);
                                };
                                (prev, 3 + self.take_checked(2)?)
                            }
                            17 => (0, 3 + self.take_checked(3)?),
                            18 => (0, 11 + self.take_checked(7)?),
                            _ => return Err(Error::Corrupt),
                        };
                        if u32::from(self.i) + rep > u32::from(total) {
                            return Err(Error::Corrupt);
                        }
                        for _ in 0..rep {
                            s.lens[usize::from(self.i)] = val;
                            self.i += 1;
                        }
                    }
                    let (hl, hd) = (usize::from(self.hlit), usize::from(self.hdist));
                    if s.lens[256] == 0 {
                        return Err(Error::Corrupt);
                    }
                    build(s.lit, LIT, &s.lens[..hl])?;
                    build(s.dist, DIST, &s.lens[hl..hl + hd])?;
                    self.st = St::Codes;
                }
                St::Codes => loop {
                    if *made == max {
                        return Ok(Status::Full);
                    }
                    if self.nbits < 48 {
                        self.refill(inp);
                        if self.nbits < 48 && !inp.end {
                            return Ok(Status::More);
                        }
                    }
                    let (sym, len) = decode(s.lit, LIT, self.bits, self.nbits)?;
                    self.take(len);
                    if sym < 256 {
                        self.put(s.win, sym as u8);
                        *made += 1;
                        continue;
                    }
                    if sym == 256 {
                        self.after_block();
                        break;
                    }
                    let li = usize::from(sym - 257);
                    if li >= 29 {
                        return Err(Error::Corrupt);
                    }
                    let len = u32::from(LEN_BASE[li]) + self.take_checked(u32::from(LEN_EXTRA[li]))?;
                    let (ds, dl) = decode(s.dist, DIST, self.bits, self.nbits)?;
                    self.take(dl);
                    let ds = usize::from(ds);
                    if ds >= 30 {
                        return Err(Error::Corrupt);
                    }
                    let dist = u32::from(DIST_BASE[ds]) + self.take_checked(u32::from(DIST_EXTRA[ds]))?;
                    if dist > self.total {
                        return Err(Error::Corrupt);
                    }
                    if dist as usize > s.win.len() {
                        return Err(Error::Window);
                    }
                    self.copy_len = len;
                    self.copy_dist = dist;
                    while self.copy_len > 0 && *made < max {
                        let b = s.win[(self.wpos.wrapping_sub(dist)) as usize & mask];
                        self.put(s.win, b);
                        *made += 1;
                        self.copy_len -= 1;
                    }
                },
            }
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    extern crate std;
    use super::*;
    use std::vec::Vec;

    /// Decode `z` fed in `piece`-byte pieces, at most `max` out per call.
    pub fn inflate(z: &[u8], piece: usize, max: usize, scratch: usize) -> Result<Vec<u8>, Error> {
        let mut mem = std::vec![0u8; scratch];
        let mut s = Scratch::new(&mut mem).unwrap();
        let mut d = Inflate::new();
        let mut out = Vec::new();
        let mut at = 0;
        for _ in 0..10_000_000 {
            let end = (at + piece).min(z.len());
            let r = d.run(&mut s, &z[at..end], end == z.len(), max)?;
            let (a, b) = d.made(&s, r.made);
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            at += r.used;
            match r.status {
                Status::Done => return Ok(out),
                Status::More => assert_eq!(at, end, "all taken"),
                Status::Full => assert_eq!(r.made, max.min(s.window_len())),
            }
        }
        panic!("stalls");
    }

    /// Bits written LSB first (RFC 1951 order), for hand-made streams.
    #[derive(Default)]
    struct Bits {
        out: Vec<u8>,
        acc: u32,
        n: u32,
    }

    impl Bits {
        fn put(&mut self, v: u32, n: u32) {
            for k in 0..n {
                self.acc |= ((v >> k) & 1) << self.n;
                self.n += 1;
                if self.n == 8 {
                    self.out.push(self.acc as u8);
                    self.acc = 0;
                    self.n = 0;
                }
            }
        }
        /// A Huffman code (most significant bit first).
        fn code(&mut self, code: u32, len: u32) {
            for k in (0..len).rev() {
                self.put((code >> k) & 1, 1);
            }
        }
        /// A literal/length symbol of the fixed code.
        fn fixed(&mut self, sym: u32) {
            match sym {
                0..=143 => self.code(0x30 + sym, 8),
                144..=255 => self.code(0x190 + sym - 144, 9),
                256..=279 => self.code(sym - 256, 7),
                _ => self.code(0xC0 + sym - 280, 8),
            }
        }
        fn done(mut self) -> Vec<u8> {
            if self.n > 0 {
                self.out.push(self.acc as u8);
            }
            self.out
        }
    }

    #[test]
    fn known_vectors() {
        let full = SCRATCH;
        // An empty fixed block.
        assert_eq!(inflate(&[0x03, 0x00], 1, 100, full), Ok(Vec::new()));
        // A stored block.
        assert_eq!(
            inflate(&[0x01, 0x05, 0x00, 0xFA, 0xFF, b'h', b'e', b'l', b'l', b'o'], 3, 2, full).unwrap(),
            b"hello"
        );
        // A fixed block: "a", then length 3 distance 1, the end.
        let mut b = Bits::default();
        b.put(1, 1);
        b.put(1, 2);
        b.fixed(u32::from(b'a'));
        b.fixed(257);
        b.code(0, 5);
        b.fixed(256);
        assert_eq!(inflate(&b.done(), 1, 1, full).unwrap(), b"aaaa");
        // Block type 3.
        assert_eq!(inflate(&[0x07], 1, 10, full), Err(Error::Corrupt));
        // A distance before the start.
        let mut b = Bits::default();
        b.put(1, 1);
        b.put(1, 2);
        b.fixed(u32::from(b'a'));
        b.fixed(257);
        b.code(1, 5);
        b.fixed(256);
        assert_eq!(inflate(&b.done(), 4, 10, full), Err(Error::Corrupt));
        // Stored length check, truncation.
        assert_eq!(inflate(&[0x01, 0x05, 0x00, 0xFB, 0xFF], 9, 9, full), Err(Error::Corrupt));
        assert_eq!(inflate(&[0x01, 0x05, 0x00, 0xFA, 0xFF, b'h'], 9, 9, full), Err(Error::Truncated));
        assert_eq!(inflate(&[], 1, 1, full), Err(Error::Truncated));
        // A dynamic header whose code-length code is over-subscribed.
        let mut b = Bits::default();
        b.put(1, 1);
        b.put(2, 2);
        b.put(0, 5);
        b.put(0, 5);
        b.put(15, 4);
        for _ in 0..19 {
            b.put(1, 3);
        }
        assert_eq!(inflate(&b.done(), 2, 10, full), Err(Error::Corrupt));
    }

    #[test]
    fn small_scratch() {
        assert!(Scratch::new(&mut [0u8; TABLES + 1023]).is_none());
        let mut m = std::vec![0u8; TABLES + 5000];
        assert_eq!(Scratch::new(&mut m).unwrap().window_len(), 4096);
    }

    /// Deterministic bytes: xorshift.
    pub fn noise(n: usize, seed: u32) -> Vec<u8> {
        let mut x = seed | 1;
        (0..n)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as u8
            })
            .collect()
    }

    pub fn deflate(data: &[u8], level: u32) -> Vec<u8> {
        use std::io::Write;
        let mut e = flate2::write::DeflateEncoder::new(Vec::new(), flate2::Compression::new(level));
        e.write_all(data).unwrap();
        e.finish().unwrap()
    }

    fn samples() -> Vec<Vec<u8>> {
        let text = "It was the best of times, it was the worst of times, it was the age of wisdom, \
                    it was the age of foolishness — Это было лучшее из времён. 春はあけぼの。\n";
        let mut words = Vec::new();
        for (i, w) in text.split(' ').cycle().take(30_000).enumerate() {
            words.extend_from_slice(w.as_bytes());
            words.push(if i % 17 == 0 { b'\n' } else { b' ' });
        }
        let mut mixed = noise(40_000, 7);
        mixed.extend_from_slice(&noise(40_000, 7)[..20_000]);
        mixed.extend_from_slice(&words[..50_000]);
        std::vec![Vec::new(), b"a".to_vec(), noise(70_000, 3), std::vec![0u8; 100_000], words, mixed]
    }

    /// Every sample at every level, in odd pieces and output limits: the
    /// same bytes as flate2 (miniz_oxide) compressed.
    #[test]
    fn against_flate2() {
        for (k, data) in samples().iter().enumerate() {
            for level in [0, 1, 6, 9] {
                let z = deflate(data, level);
                for (piece, max) in
                    [(usize::MAX / 2, usize::MAX / 2), (1, 1), (7, 333), (512, 4096), (13, 65_536)]
                {
                    let out =
                        inflate(&z, piece, max, SCRATCH).unwrap_or_else(|e| panic!("{k} {level}: {e:?}"));
                    assert!(out == *data, "sample {k} level {level} piece {piece} max {max}");
                }
            }
        }
    }

    /// BOOK-2, 2026-11-25: a call allowed more output than the window
    /// holds, so its first bytes were overwritten before the caller read
    /// them (a stored block over 32 KiB came out wrong).
    #[test]
    fn regress_inflate_max_over_window() {
        let data = noise(70_000, 11);
        let z = deflate(&data, 0);
        assert_eq!(inflate(&z, usize::MAX / 2, 1 << 20, SCRATCH).unwrap(), data);
    }

    /// A 1 KiB window decodes near matches and refuses far ones.
    #[test]
    fn window_limits() {
        let near = b"abcabcabd".repeat(5000);
        assert_eq!(inflate(&deflate(&near, 9), 100, 1000, TABLES + 1024).unwrap(), near);
        let mut far = noise(3000, 9);
        far.extend_from_slice(&far.clone());
        assert_eq!(inflate(&deflate(&far, 9), 100, 1000, TABLES + 1024), Err(Error::Window));
        assert_eq!(inflate(&deflate(&far, 9), 100, 1000, SCRATCH).unwrap(), far);
    }
}
