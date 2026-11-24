//! The stream ring with gapless read-ahead (04 §2, §2.2).
//!
//! One byte ring holds the current stream (the file the decoder reads) and,
//! once the reader has read that file to its end, the head of the next one:
//!
//! ```text
//!   ┌────────── current ──────────┐┌──── next ────┐
//!   … rd ··········· wr = end of file│0 ······ nwr │ free …
//! ```
//!
//! The decoder sees only the current stream ([`ByteSource`] and
//! [`SeekSource`] are implemented for it, positions are its file offsets);
//! the next stream's bytes follow without a gap. When the decoder has
//! finished, [`StreamRing::hand_over`] makes the next stream the current one
//! (its offset 0 lies right after the current stream's last byte): the new
//! decoder opens on bytes that are already there, and the old stream's
//! unread bytes (trailing tags) are dropped.
//!
//! The reader asks [`StreamRing::space`] where to read next: into the
//! current stream until its end, then into the next one. A seek in the
//! current stream drops what was read of the next one (it is read again,
//! from its start, once the current one is at its end again).
//!
//! The ring's length must be a power of two. Host-tested here; the firmware
//! wraps it with its card reads (`fw/firmware/src/audio.rs`). No panic
//! paths (each would put this file's path into the resident image).

use crate::source::{ByteSource, SeekSource, SourceError};

/// One stream's part of the ring (offsets are the stream's file offsets).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Part {
    rd: u64,
    wr: u64,
    len: Option<u64>,
    /// The reader reached the end of the file.
    eof: bool,
    /// The read position moved: the reader repositions the file to `wr`.
    reseek: bool,
}

impl Part {
    const fn new(len: Option<u64>) -> Self {
        Self { rd: 0, wr: 0, len, eof: false, reseek: false }
    }
}

/// Which stream the reader is to read into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Target {
    Current,
    Next,
}

/// Where the reader reads next (see [`StreamRing::space`]).
#[derive(Debug)]
pub struct Space<'r> {
    pub target: Target,
    /// Set the file's position here before reading (after a seek).
    pub seek: Option<u64>,
    /// Free, contiguous bytes of the ring for the read.
    pub buf: &'r mut [u8],
}

pub struct StreamRing<'a> {
    buf: &'a mut [u8],
    mask: u64,
    /// Ring counter of the current stream's offset 0 (the counters only
    /// grow; a byte lives at `counter & mask`).
    base: u64,
    cur: Part,
    next: Option<Part>,
}

impl<'a> StreamRing<'a> {
    /// A ring over `buf`; only its largest power-of-two prefix is used.
    /// Starts with an empty current stream at its end (nothing to read).
    #[must_use]
    pub fn new(buf: &'a mut [u8]) -> Self {
        // The top bit of the length (no panic paths: this runs in the
        // firmware's audio task).
        let ones = usize::MAX.checked_shr(buf.len().leading_zeros()).unwrap_or(0);
        let cap = ones - (ones >> 1);
        let buf = buf.get_mut(..cap).unwrap_or_default();
        let mut cur = Part::new(Some(0));
        cur.eof = true;
        Self { mask: (cap as u64).wrapping_sub(1), buf, base: 0, cur, next: None }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.buf.len()
    }

    /// A new current stream of `len` bytes from offset 0; the ring is
    /// emptied and a next stream forgotten.
    pub fn reset(&mut self, len: Option<u64>) {
        self.base = 0;
        self.cur = Part::new(len);
        self.next = None;
    }

    /// The reader has read the current stream to its end.
    #[must_use]
    pub fn eof(&self) -> bool {
        self.cur.eof
    }

    /// The current stream's read position moved and the reader has not
    /// read from the new position yet.
    #[must_use]
    pub fn reseek_pending(&self) -> bool {
        self.cur.reseek
    }

    /// Move the current stream's read position to `pos` (skipping a tag
    /// larger than what is buffered): the ring is emptied, the reader reads
    /// from `pos` next.
    pub fn skip_to(&mut self, pos: u64) {
        self.cur.rd = pos;
        self.cur.wr = pos;
        self.cur.eof = self.cur.len.is_some_and(|l| pos >= l);
        self.cur.reseek = true;
        self.drop_next_data();
    }

    /// The bytes of the next stream are no longer where they were (the
    /// current stream's end moved): read them again from its start.
    fn drop_next_data(&mut self) {
        if let Some(n) = &mut self.next {
            *n = Part { reseek: true, ..Part::new(n.len) };
        }
    }

    // ------------------------------------------------------- next stream

    /// Read ahead into a next stream of `len` bytes (from its offset 0)
    /// after the current one; replaces any next stream.
    pub fn begin_next(&mut self, len: Option<u64>) {
        self.next = Some(Part::new(len));
    }

    /// Forget the next stream (its bytes are dropped).
    pub fn cancel_next(&mut self) {
        self.next = None;
    }

    #[must_use]
    pub fn has_next(&self) -> bool {
        self.next.is_some()
    }

    /// Bytes of the next stream in the ring.
    #[must_use]
    pub fn next_buffered(&self) -> usize {
        self.next.map_or(0, |n| (n.wr - n.rd) as usize)
    }

    /// Make the next stream the current one: its read position is its
    /// offset 0, what it has buffered stays; the old stream's unread bytes
    /// are dropped. False (nothing changes) without a next stream.
    pub fn hand_over(&mut self) -> bool {
        let Some(n) = self.next.take() else { return false };
        self.base += self.cur.wr;
        self.cur = n;
        true
    }

    // ------------------------------------------------------------ reader

    fn used(&self) -> u64 {
        (self.cur.wr - self.cur.rd) + self.next.map_or(0, |n| n.wr - n.rd)
    }

    /// Where the reader reads next: free space of at least `min` bytes (a
    /// read at the ring's end may be shorter: the contiguous part), at most
    /// `max`; into the current stream until its end, then into the next
    /// one. `None`: nothing to read, or no room. Clears the target's
    /// pending reseek (returned in [`Space::seek`]).
    pub fn space(&mut self, min: usize, max: usize) -> Option<Space<'_>> {
        if self.cur.eof {
            // Nothing more is read from the current file: no reposition.
            self.cur.reseek = false;
        }
        let (target, counter) = if !self.cur.eof {
            (Target::Current, self.base + self.cur.wr)
        } else {
            let n = self.next.filter(|n| !n.eof)?;
            (Target::Next, self.base + self.cur.wr + n.wr)
        };
        let cap = self.buf.len() as u64;
        let free = cap - self.used();
        if free < min.max(1) as u64 {
            return None;
        }
        let start = (counter & self.mask) as usize;
        let n = (free as usize).min(self.buf.len() - start).min(max.max(1));
        let part = match target {
            Target::Current => &mut self.cur,
            Target::Next => self.next.as_mut()?,
        };
        let seek = core::mem::take(&mut part.reseek).then_some(part.wr);
        Some(Space { target, seek, buf: self.buf.get_mut(start..start + n)? })
    }

    /// `n` bytes were read into the space [`StreamRing::space`] gave for
    /// `t`.
    pub fn wrote(&mut self, t: Target, n: usize) {
        let room = self.buf.len() as u64 - self.used();
        let n = (n as u64).min(room);
        match t {
            Target::Current => self.cur.wr += n,
            Target::Next => {
                if let Some(p) = &mut self.next {
                    p.wr += n;
                }
            }
        }
    }

    /// The reader reached the end of `t`'s file (or cannot read it).
    pub fn end(&mut self, t: Target) {
        match t {
            Target::Current => self.cur.eof = true,
            Target::Next => {
                if let Some(p) = &mut self.next {
                    p.eof = true;
                }
            }
        }
    }
}

impl ByteSource for StreamRing<'_> {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])> {
        if self.available() < n {
            return None;
        }
        let start = ((self.base + self.cur.rd) & self.mask) as usize;
        let first = n.min(self.buf.len() - start);
        Some((self.buf.get(start..start + first)?, self.buf.get(..n - first)?))
    }
    fn available(&self) -> usize {
        (self.cur.wr - self.cur.rd) as usize
    }
    fn consume(&mut self, n: usize) {
        self.cur.rd += n.min(self.available()) as u64;
    }
    fn position(&self) -> u64 {
        self.cur.rd
    }
    fn len(&self) -> Option<u64> {
        self.cur.len
    }
    fn at_end(&self) -> bool {
        self.cur.eof
    }
}

/// A seek empties the ring; the reader refills it from the new position.
impl SeekSource for StreamRing<'_> {
    fn seek(&mut self, pos: u64) -> Result<(), SourceError> {
        let len = self.cur.len.ok_or(SourceError::NotSeekable)?;
        if pos > len {
            return Err(SourceError::OutOfRange);
        }
        self.skip_to(pos);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// Files behind the ring, read the way the firmware reads them.
    struct Files<'d> {
        cur: &'d [u8],
        next: Option<&'d [u8]>,
        pos: [usize; 2],
        chunk: usize,
    }

    impl Files<'_> {
        /// One `fill`: read until the ring is full or nothing is left.
        fn fill(&mut self, r: &mut StreamRing<'_>) {
            while let Some(sp) = r.space(1, self.chunk) {
                let (i, data) = match sp.target {
                    Target::Current => (0, self.cur),
                    Target::Next => (1, self.next.unwrap()),
                };
                if let Some(p) = sp.seek {
                    self.pos[i] = p as usize;
                }
                let n = sp.buf.len().min(data.len() - self.pos[i].min(data.len()));
                sp.buf[..n].copy_from_slice(&data[self.pos[i]..self.pos[i] + n]);
                let t = sp.target;
                self.pos[i] += n;
                if n == 0 { r.end(t) } else { r.wrote(t, n) }
            }
        }

        fn hand_over(&mut self, r: &mut StreamRing<'_>) {
            assert!(r.hand_over());
            self.cur = self.next.take().unwrap();
            self.pos = [self.pos[1], 0];
        }
    }

    fn data(n: usize, salt: u8) -> Vec<u8> {
        (0..n).map(|i| (i as u8).wrapping_mul(31).wrapping_add(salt ^ (i >> 8) as u8)).collect()
    }

    /// Consume everything of the current stream in `step`-byte reads.
    fn drain(r: &mut StreamRing<'_>, f: &mut Files<'_>, step: usize, out: &mut Vec<u8>) {
        loop {
            f.fill(r);
            let n = r.available().min(step);
            if n == 0 && r.at_end() {
                return;
            }
            let (a, b) = r.peek(n).unwrap();
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            r.consume(n);
        }
    }

    #[test]
    fn two_streams_back_to_back() {
        for (la, lb, step, chunk) in
            [(5000, 7000, 100, 512), (4096, 4096, 1000, 4096), (1, 3, 7, 3), (0, 9000, 33, 700)]
        {
            let (a, b) = (data(la, 1), data(lb, 2));
            let mut buf = std::vec![0u8; 1024];
            let mut r = StreamRing::new(&mut buf);
            r.reset(Some(la as u64));
            let mut f = Files { cur: &a, next: None, pos: [0, 0], chunk };
            let mut out = Vec::new();
            // Read A until its reader is at the end, then prefetch B.
            while !r.eof() {
                f.fill(&mut r);
                let n = r.available().min(step);
                let (x, y) = r.peek(n).unwrap();
                out.extend_from_slice(x);
                out.extend_from_slice(y);
                r.consume(n);
            }
            r.begin_next(Some(lb as u64));
            f.next = Some(&b);
            drain(&mut r, &mut f, step, &mut out);
            assert_eq!(out, a, "A alone, no byte of B");
            assert!(r.next_buffered() > 0 || lb == 0);
            f.hand_over(&mut r);
            assert_eq!(r.position(), 0);
            assert_eq!(r.len(), Some(lb as u64));
            let mut out = Vec::new();
            drain(&mut r, &mut f, step, &mut out);
            assert_eq!(out, b);
        }
    }

    #[test]
    fn hand_over_drops_unread_tail() {
        let (a, b) = (data(3000, 3), data(2000, 4));
        let mut buf = std::vec![0u8; 4096];
        let mut r = StreamRing::new(&mut buf[..3000]);
        assert_eq!(r.capacity(), 2048);
        r.reset(Some(3000));
        let mut f = Files { cur: &a, next: Some(&b), pos: [0, 0], chunk: 4096 };
        r.begin_next(Some(2000));
        f.fill(&mut r);
        assert_eq!(r.available(), 2048);
        assert!(!r.eof());
        while r.position() < 2900 {
            let n = r.available().min(2900 - r.position() as usize);
            r.consume(n);
            f.fill(&mut r);
        }
        assert!(r.eof());
        // 100 bytes of A left (a trailing tag) when the decoder ends.
        assert_eq!(r.available(), 100);
        assert_eq!(r.next_buffered(), 1948);
        f.hand_over(&mut r);
        let mut out = Vec::new();
        drain(&mut r, &mut f, 64, &mut out);
        assert_eq!(out, b);
    }

    #[test]
    fn seek_in_current_rereads_next() {
        let (a, b) = (data(6000, 5), data(5000, 6));
        let mut buf = std::vec![0u8; 2048];
        let mut r = StreamRing::new(&mut buf);
        r.reset(Some(6000));
        let mut f = Files { cur: &a, next: Some(&b), pos: [0, 0], chunk: 300 };
        r.begin_next(Some(5000));
        let mut sink = Vec::new();
        while r.next_buffered() == 0 {
            f.fill(&mut r);
            let n = r.available().min(500);
            r.consume(n);
        }
        // Seek back in A: B's bytes are gone and read again later.
        r.seek(100).unwrap();
        assert_eq!(r.next_buffered(), 0);
        assert!(r.has_next());
        drain(&mut r, &mut f, 77, &mut sink);
        assert_eq!(sink, a[100..]);
        f.hand_over(&mut r);
        let mut out = Vec::new();
        drain(&mut r, &mut f, 77, &mut out);
        assert_eq!(out, b);
    }

    #[test]
    fn skip_and_cancel() {
        let a = data(10_000, 7);
        let mut buf = std::vec![0u8; 1024];
        let mut r = StreamRing::new(&mut buf);
        r.reset(Some(10_000));
        let mut f = Files { cur: &a, next: None, pos: [0, 0], chunk: 512 };
        f.fill(&mut r);
        r.skip_to(9_000);
        assert!(r.reseek_pending() && r.available() == 0);
        let mut out = Vec::new();
        drain(&mut r, &mut f, 100, &mut out);
        assert_eq!(out, a[9_000..]);
        r.begin_next(Some(5));
        r.cancel_next();
        assert!(!r.hand_over());
        r.skip_to(20_000);
        assert!(r.eof());
        assert!(r.space(1, 100).is_none());
    }
}
