//! The write-behind ring between the encoder and the card (04 §5.2): only
//! the indices; the bytes are the caller's (RAM the card's DMA can read).
//!
//! The ring carries the file from its first byte (the header first), so a
//! byte's ring position is its file offset. Writes start at the oldest
//! unwritten byte, are whole 512-byte sectors (the ring's capacity is a
//! multiple of 512, so a write never starts mid-sector) and are issued
//! when [`WriteBehind::chunk`] bytes wait in one piece, or when the oldest
//! byte has waited [`WriteBehind::max_wait_ms`] (latency bound), or when
//! the file is closing (then the last partial sector goes too). A write in
//! flight keeps its bytes: the producer only ever sees [`WriteBehind::free`]
//! bytes, so it never overwrites what the card's DMA is reading.

/// Card sector.
pub const SECTOR: usize = 512;

/// One write to issue: `len` bytes from ring offset `at`, file offset
/// `file_at`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Write {
    pub at: usize,
    pub len: usize,
    pub file_at: u64,
}

/// The ring's indices.
#[derive(Clone, Copy, Debug)]
pub struct WriteBehind {
    cap: usize,
    chunk: usize,
    max_wait_ms: u32,
    /// Bytes produced (file offset of the next byte).
    head: u64,
    /// Bytes written to the card.
    tail: u64,
    /// The write in flight (from `tail`).
    flight: usize,
    /// When the oldest unwritten byte was produced.
    oldest_ms: Option<u64>,
    /// Highest fill seen.
    peak: usize,
}

impl WriteBehind {
    /// `cap` and `chunk` are rounded down to whole sectors (`chunk` ≤ `cap`).
    #[must_use]
    pub fn new(cap: usize, chunk: usize, max_wait_ms: u32) -> Self {
        let cap = cap / SECTOR * SECTOR;
        let chunk = (chunk / SECTOR * SECTOR).clamp(SECTOR, cap.max(SECTOR));
        WriteBehind { cap, chunk, max_wait_ms, head: 0, tail: 0, flight: 0, oldest_ms: None, peak: 0 }
    }

    #[must_use]
    pub fn capacity(&self) -> usize {
        self.cap
    }

    #[must_use]
    pub fn chunk(&self) -> usize {
        self.chunk
    }

    #[must_use]
    pub fn max_wait_ms(&self) -> u32 {
        self.max_wait_ms
    }

    /// Bytes waiting (including a write in flight).
    #[must_use]
    pub fn fill(&self) -> usize {
        (self.head - self.tail) as usize
    }

    /// Room for the producer.
    #[must_use]
    pub fn free(&self) -> usize {
        self.cap - self.fill()
    }

    /// Highest fill so far.
    #[must_use]
    pub fn peak(&self) -> usize {
        self.peak
    }

    /// Bytes produced (the file's length so far).
    #[must_use]
    pub fn produced(&self) -> u64 {
        self.head
    }

    /// Bytes on the card.
    #[must_use]
    pub fn written(&self) -> u64 {
        self.tail
    }

    /// Where `n` bytes go: up to two ring pieces `(at, len)` (the second
    /// after the wrap, `len` 0 if none); `None` if they do not fit.
    #[must_use]
    pub fn put_at(&self, n: usize) -> Option<[(usize, usize); 2]> {
        if n > self.free() || self.cap == 0 {
            return None;
        }
        let at = (self.head % self.cap as u64) as usize;
        let first = n.min(self.cap - at);
        Some([(at, first), (0, n - first)])
    }

    /// `n` bytes were copied where [`Self::put_at`] said.
    pub fn put(&mut self, n: usize, now_ms: u64) {
        let n = n.min(self.free());
        if n == 0 {
            return;
        }
        if self.fill() == self.flight {
            self.oldest_ms = Some(now_ms);
        }
        self.head += n as u64;
        self.peak = self.peak.max(self.fill());
    }

    /// The next write to issue, if one is due (none while one is in
    /// flight). `closing`: everything left, the last sector partial.
    pub fn next_write(&mut self, now_ms: u64, closing: bool) -> Option<Write> {
        if self.flight != 0 || self.cap == 0 {
            return None;
        }
        let fill = self.fill();
        if fill == 0 {
            return None;
        }
        let at = (self.tail % self.cap as u64) as usize;
        let run = fill.min(self.cap - at);
        let whole = run / SECTOR * SECTOR;
        let waited = self.oldest_ms.is_some_and(|t| now_ms.saturating_sub(t) >= u64::from(self.max_wait_ms));
        // A run that reaches the ring's end and more waits after the wrap:
        // the run is all that can go in one piece now.
        let to_end = at + run == self.cap && fill > run;
        let len = if closing {
            run
        } else if whole >= self.chunk {
            whole / self.chunk * self.chunk
        } else if (waited || to_end) && whole > 0 {
            whole
        } else {
            return None;
        };
        self.flight = len;
        Some(Write { at, len, file_at: self.tail })
    }

    /// The write in flight finished (`ok`) or failed (its bytes stay and it
    /// is issued again).
    pub fn done(&mut self, ok: bool, now_ms: u64) {
        if ok {
            self.tail += self.flight as u64;
            self.oldest_ms = (self.fill() > 0).then_some(now_ms);
        }
        self.flight = 0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_whole_aligned_chunks() {
        let mut r = WriteBehind::new(8 * 1024, 2048, 500);
        assert!(r.put_at(100).is_some());
        r.put(100, 0);
        assert_eq!(r.next_write(10, false), None, "not a sector yet, not old");
        r.put(5000, 20);
        let w = r.next_write(30, false).unwrap();
        assert_eq!((w.at, w.len, w.file_at), (0, 4096, 0));
        assert_eq!(r.next_write(30, false), None, "one in flight");
        r.done(true, 40);
        assert_eq!(r.fill(), 1004);
        // Latency bound: 1004 bytes wait; after 500 ms one sector goes.
        assert_eq!(r.next_write(100, false), None);
        let w = r.next_write(600, false).unwrap();
        assert_eq!((w.at, w.len, w.file_at), (4096, 512, 4096));
        r.done(true, 600);
        let w = r.next_write(600, true).unwrap();
        assert_eq!((w.len, w.file_at), (492, 4608));
        r.done(true, 600);
        assert_eq!(r.fill(), 0);
    }

    #[test]
    fn wraps_and_never_overlaps_the_write_in_flight() {
        let mut r = WriteBehind::new(4096, 1024, 100);
        r.put(3584, 0);
        let w = r.next_write(0, false).unwrap();
        assert_eq!(w.len, 3072);
        // In flight: only 512 bytes are free.
        assert_eq!(r.free(), 512);
        assert!(r.put_at(513).is_none());
        r.done(true, 1);
        let p = r.put_at(2048).unwrap();
        assert_eq!(p, [(3584, 512), (0, 1536)]);
        r.put(2048, 2);
        let w = r.next_write(2, false).unwrap();
        assert_eq!((w.at, w.len), (3072, 1024), "up to the wrap");
        r.done(true, 3);
        let w = r.next_write(3, false).unwrap();
        assert_eq!((w.at, w.len, w.file_at), (0, 1024, 4096));
    }

    #[test]
    fn the_piece_before_the_wrap_goes_at_once() {
        let mut r = WriteBehind::new(5 * 1024, 2048, 10_000);
        r.put(4096, 0);
        let w = r.next_write(0, false).unwrap();
        assert_eq!(w.len, 4096);
        r.done(true, 0);
        // 1 KiB to the end, more after the wrap: no waiting for a chunk.
        r.put(2048, 1);
        let w = r.next_write(1, false).unwrap();
        assert_eq!((w.at, w.len), (4096, 1024));
    }

    #[test]
    fn a_failed_write_is_issued_again() {
        let mut r = WriteBehind::new(4096, 1024, 100);
        r.put(2048, 0);
        let w = r.next_write(0, false).unwrap();
        r.done(false, 1);
        assert_eq!(r.next_write(1, false), Some(w));
    }
}
