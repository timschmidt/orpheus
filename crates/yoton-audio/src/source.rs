//! Byte sources: the decoder's view of the stream ring (04 §2.1).

/// A stream of bytes as the decoder sees it: the unread part of the stream
/// ring.
///
/// Contract (decoders rely on it; implementations must keep it):
///
/// * `peek(n)` returns exactly `n` bytes starting at the read position, as
///   one or two slices (`a` then `b`; `b` is non-empty only when the bytes
///   wrap around the ring's end), or `None` when fewer than `n` bytes are
///   buffered. `peek(0)` is `Some((&[], &[]))`.
/// * `available()` is the number of bytes `peek` can return right now.
/// * `consume(n)` advances the read position; `n > available()` is clamped
///   (never a panic).
/// * `position()` is the absolute stream offset of the read position.
/// * `len()` is the total stream length when known.
/// * `at_end()` is true when no byte beyond the buffered ones will ever
///   arrive (end of file reached by the reader).
pub trait ByteSource {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])>;
    fn available(&self) -> usize;
    fn consume(&mut self, n: usize);
    fn position(&self) -> u64;
    fn len(&self) -> Option<u64>;
    fn at_end(&self) -> bool;

    /// No buffered bytes and nothing more coming.
    fn is_empty(&self) -> bool {
        self.at_end() && self.available() == 0
    }
}

/// A source that can be repositioned (a file behind the ring).
///
/// `seek` flushes what is buffered and moves the read position; on the
/// device the reader task then refills the ring asynchronously, so right after
/// a seek `available()` may be 0 and a decoder answers `NeedData`.
pub trait SeekSource: ByteSource {
    fn seek(&mut self, pos: u64) -> Result<(), SourceError>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceError {
    /// Beyond the end of the stream.
    OutOfRange,
    /// The source cannot seek (a pipe, a network stream).
    NotSeekable,
    /// The medium failed.
    Io,
}

/// The two halves of a peeked range, with bounds-checked accessors.
///
/// Out-of-range reads return `None` (or 0 from [`View::byte`]): parsers built
/// on a `View` cannot read beyond what they were given.
#[derive(Clone, Copy, Debug, Default)]
pub struct View<'a> {
    pub a: &'a [u8],
    pub b: &'a [u8],
}

impl<'a> View<'a> {
    #[must_use]
    pub fn new(a: &'a [u8], b: &'a [u8]) -> Self {
        Self { a, b }
    }

    /// Everything `src` holds now.
    #[must_use]
    pub fn of(src: &'a dyn ByteSource) -> Self {
        Self::peek(src, src.available()).unwrap_or_default()
    }

    /// `n` bytes of `src`, or `None` if fewer are buffered.
    #[must_use]
    pub fn peek(src: &'a dyn ByteSource, n: usize) -> Option<Self> {
        src.peek(n).map(|(a, b)| Self { a, b })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        self.a.len() + self.b.len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    #[must_use]
    #[inline]
    pub fn get(&self, i: usize) -> Option<u8> {
        match self.a.get(i) {
            Some(&v) => Some(v),
            None => self.b.get(i.wrapping_sub(self.a.len())).copied(),
        }
    }

    /// Byte `i`, or 0 beyond the end.
    #[must_use]
    #[inline]
    pub fn byte(&self, i: usize) -> u8 {
        self.get(i).unwrap_or(0)
    }

    /// The bytes from `off` to the end.
    #[must_use]
    pub fn skip(&self, off: usize) -> Self {
        if off <= self.a.len() {
            Self { a: &self.a[off..], b: self.b }
        } else {
            let k = (off - self.a.len()).min(self.b.len());
            Self { a: &self.b[k..], b: &[] }
        }
    }

    /// The first `n` bytes (fewer if the view is shorter).
    #[must_use]
    pub fn take(&self, n: usize) -> Self {
        if n <= self.a.len() {
            Self { a: &self.a[..n], b: &[] }
        } else {
            let k = (n - self.a.len()).min(self.b.len());
            Self { a: self.a, b: &self.b[..k] }
        }
    }

    /// `n` bytes at `off`, if all are present.
    #[must_use]
    pub fn range(&self, off: usize, n: usize) -> Option<Self> {
        let end = off.checked_add(n)?;
        (end <= self.len()).then(|| self.skip(off).take(n))
    }

    /// Copy `dst.len()` bytes from `off`; `false` (and `dst` untouched) if
    /// they are not all present.
    pub fn copy_to(&self, off: usize, dst: &mut [u8]) -> bool {
        let Some(r) = self.range(off, dst.len()) else { return false };
        let (d1, d2) = dst.split_at_mut(r.a.len());
        d1.copy_from_slice(r.a);
        d2.copy_from_slice(r.b);
        true
    }

    /// An array of `N` bytes from `off`.
    #[must_use]
    pub fn array<const N: usize>(&self, off: usize) -> Option<[u8; N]> {
        let mut out = [0u8; N];
        self.copy_to(off, &mut out).then_some(out)
    }

    #[must_use]
    pub fn u16_le(&self, off: usize) -> Option<u16> {
        self.array::<2>(off).map(u16::from_le_bytes)
    }
    #[must_use]
    pub fn u16_be(&self, off: usize) -> Option<u16> {
        self.array::<2>(off).map(u16::from_be_bytes)
    }
    #[must_use]
    pub fn u32_le(&self, off: usize) -> Option<u32> {
        self.array::<4>(off).map(u32::from_le_bytes)
    }
    #[must_use]
    pub fn u32_be(&self, off: usize) -> Option<u32> {
        self.array::<4>(off).map(u32::from_be_bytes)
    }
    #[must_use]
    pub fn u64_le(&self, off: usize) -> Option<u64> {
        self.array::<8>(off).map(u64::from_le_bytes)
    }
    #[must_use]
    pub fn u64_be(&self, off: usize) -> Option<u64> {
        self.array::<8>(off).map(u64::from_be_bytes)
    }

    /// Whether the bytes at `off` equal `pat`.
    #[must_use]
    pub fn starts_with_at(&self, off: usize, pat: &[u8]) -> bool {
        match self.range(off, pat.len()) {
            Some(r) => r.a == &pat[..r.a.len()] && r.b == &pat[r.a.len()..],
            None => false,
        }
    }

    /// Offset of the first occurrence of `pat` at or after `from`.
    #[must_use]
    pub fn find(&self, from: usize, pat: &[u8]) -> Option<usize> {
        let first = *pat.first()?;
        let mut i = from;
        while i + pat.len() <= self.len() {
            if self.byte(i) == first && self.starts_with_at(i, pat) {
                return Some(i);
            }
            i += 1;
        }
        None
    }
}

/// An in-memory [`SeekSource`] over a byte slice.
///
/// By default everything is available at once. [`SliceSource::ring`]
/// simulates the stream ring instead: at most `cap` bytes are buffered,
/// `peek` splits at the ring's wrap point exactly as the device ring would
/// (absolute position modulo `cap`), and new bytes only arrive when the
/// owner calls [`SliceSource::refill`] (all that fits) or
/// [`SliceSource::trickle`] (a few bytes). Decoders tested over this source
/// see the same partial and split views they get on the device.
#[derive(Debug, Clone)]
pub struct SliceSource<'d> {
    data: &'d [u8],
    pos: usize,
    filled: usize,
    cap: usize,
}

impl<'d> SliceSource<'d> {
    /// All of `data`, available at once, never split.
    #[must_use]
    pub fn new(data: &'d [u8]) -> Self {
        Self { data, pos: 0, filled: data.len(), cap: 0 }
    }

    /// A simulated ring of `cap` bytes over `data`, initially full.
    #[must_use]
    pub fn ring(data: &'d [u8], cap: usize) -> Self {
        let mut s = Self { data, pos: 0, filled: 0, cap: cap.max(1) };
        s.refill();
        s
    }

    /// Top the simulated ring up to its capacity (or the end of the data).
    pub fn refill(&mut self) {
        self.filled = if self.cap == 0 { self.data.len() } else { self.data.len().min(self.pos + self.cap) };
    }

    /// Add at most `n` bytes to the simulated ring.
    pub fn trickle(&mut self, n: usize) {
        let limit = if self.cap == 0 { self.data.len() } else { self.data.len().min(self.pos + self.cap) };
        self.filled = limit.min(self.filled.saturating_add(n)).max(self.filled);
    }

    /// The whole underlying data.
    #[must_use]
    pub fn data(&self) -> &'d [u8] {
        self.data
    }
}

impl ByteSource for SliceSource<'_> {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])> {
        if n > self.filled - self.pos {
            return None;
        }
        let all = &self.data[self.pos..self.pos + n];
        if self.cap == 0 {
            return Some((all, &[]));
        }
        let phys = self.pos % self.cap;
        let first = n.min(self.cap - phys);
        Some(all.split_at(first))
    }

    fn available(&self) -> usize {
        self.filled - self.pos
    }

    fn consume(&mut self, n: usize) {
        self.pos += n.min(self.filled - self.pos);
    }

    fn position(&self) -> u64 {
        self.pos as u64
    }

    fn len(&self) -> Option<u64> {
        Some(self.data.len() as u64)
    }

    fn at_end(&self) -> bool {
        self.filled == self.data.len()
    }
}

impl SeekSource for SliceSource<'_> {
    fn seek(&mut self, pos: u64) -> Result<(), SourceError> {
        let pos = usize::try_from(pos).map_err(|_| SourceError::OutOfRange)?;
        if pos > self.data.len() {
            return Err(SourceError::OutOfRange);
        }
        self.pos = pos;
        // The device ring is flushed by a seek; the unbounded source simply
        // stays fully available.
        self.filled = if self.cap == 0 { self.data.len() } else { pos };
        Ok(())
    }
}

/// Consume up to `n` bytes; returns how many were consumed.
pub fn skip(src: &mut dyn ByteSource, n: u64) -> u64 {
    let k = usize::try_from(n).unwrap_or(usize::MAX).min(src.available());
    src.consume(k);
    k as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_splits_at_wrap() {
        let data: std::vec::Vec<u8> = (0..100u8).collect();
        let mut s = SliceSource::ring(&data, 16);
        assert_eq!(s.available(), 16);
        s.consume(10);
        assert_eq!(s.available(), 6);
        s.refill();
        assert_eq!(s.available(), 16);
        let (a, b) = s.peek(10).unwrap();
        assert_eq!(a, &data[10..16]);
        assert_eq!(b, &data[16..20]);
        let v = View::peek(&s, 10).unwrap();
        assert_eq!(v.u32_le(4), Some(u32::from_le_bytes([14, 15, 16, 17])));
        assert!(v.starts_with_at(5, &[15, 16, 17]));
        assert_eq!(v.find(0, &[16, 17]), Some(6));
        assert_eq!(v.skip(7).a, &[17, 18, 19]);
        assert!(s.peek(17).is_none());
        s.seek(95).unwrap();
        assert_eq!(s.available(), 0);
        s.trickle(3);
        assert_eq!(s.available(), 3);
        s.refill();
        assert_eq!(s.available(), 5);
        assert!(s.at_end());
    }
}
