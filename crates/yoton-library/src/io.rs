//! Buffered sequential streams over [`Storage`] files whose buffers live in
//! the lent scratch: the state is a plain value, the buffer is passed to
//! every call. Writes end on multiples of the buffer length in the file
//! (whole sectors for 512-byte buffers); suspending flushes writers and
//! rewinds readers to their next unread byte.

use yoton_index::{FileName, Storage, crc32_update};

/// An append-only output stream (a section of the library file or a
/// temporary list).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Out {
    pub file: FileName,
    /// File offset where this stream started.
    pub base: u32,
    /// File offset of the first byte not yet written to the file.
    pub pos: u32,
    pub fill: u16,
    pub crc: u32,
}

impl Out {
    pub fn new(file: FileName, base: u32) -> Out {
        Out { file, base, pos: base, fill: 0, crc: 0 }
    }

    /// Bytes put so far.
    pub fn written(&self) -> u32 {
        self.pos + u32::from(self.fill) - self.base
    }

    pub fn put<S: Storage>(
        &mut self,
        store: &mut S,
        buf: &mut [u8],
        mut data: &[u8],
    ) -> Result<(), S::Error> {
        self.crc = crc32_update(self.crc, data);
        let blen = buf.len();
        while !data.is_empty() {
            let cap = blen - self.pos as usize % blen;
            let fill = usize::from(self.fill);
            let n = (cap - fill).min(data.len());
            buf[fill..fill + n].copy_from_slice(&data[..n]);
            self.fill += n as u16;
            data = &data[n..];
            if usize::from(self.fill) == cap {
                self.flush(store, buf)?;
            }
        }
        Ok(())
    }

    pub fn flush<S: Storage>(&mut self, store: &mut S, buf: &[u8]) -> Result<(), S::Error> {
        if self.fill > 0 {
            store.write_at(self.file, self.pos, &buf[..usize::from(self.fill)])?;
            self.pos += u32::from(self.fill);
            self.fill = 0;
        }
        Ok(())
    }
}

/// A sequential input stream over `[pos, end)` of a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct In {
    pub file: FileName,
    /// File offset of `buf[0]`.
    pub pos: u32,
    pub end: u32,
    pub lo: u16,
    pub hi: u16,
}

/// The stream ended inside a record, or the file is shorter than recorded.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Short;

impl In {
    pub fn new(file: FileName, start: u32, end: u32) -> In {
        In { file, pos: start, end, lo: 0, hi: 0 }
    }

    /// File offset of the next unread byte.
    pub fn at(&self) -> u32 {
        self.pos + u32::from(self.lo)
    }

    pub fn remaining(&self) -> u32 {
        self.end - self.at()
    }

    pub fn is_empty(&self) -> bool {
        self.at() >= self.end
    }

    /// Forget the buffer (it is about to be lent out).
    pub fn rewind(&mut self) {
        self.pos += u32::from(self.lo);
        self.lo = 0;
        self.hi = 0;
    }

    /// Make `n` bytes available at `buf[lo..]`. `Ok(Err(Short))` if the
    /// stream has fewer left.
    pub fn need<S: Storage>(
        &mut self,
        store: &mut S,
        buf: &mut [u8],
        n: usize,
    ) -> Result<Result<(), Short>, S::Error> {
        let avail = usize::from(self.hi - self.lo);
        if avail >= n {
            return Ok(Ok(()));
        }
        if n > buf.len() || (self.remaining() as usize) < n {
            return Ok(Err(Short));
        }
        buf.copy_within(usize::from(self.lo)..usize::from(self.hi), 0);
        self.pos += u32::from(self.lo);
        self.lo = 0;
        self.hi = avail as u16;
        let from = self.pos + avail as u32;
        let mut k = (buf.len() - avail).min((self.end - from) as usize);
        // End on a sector boundary when reading more than a sector.
        let cut = (from as usize + k) % 512;
        if k > 512 && cut != 0 && (from + k as u32) < self.end && avail + k - cut >= n {
            k -= cut;
        }
        let got = store.read_at(self.file, from, &mut buf[avail..avail + k])?;
        self.hi += got as u16;
        if got < k {
            self.end = from + got as u32;
        }
        Ok(if usize::from(self.hi) >= n { Ok(()) } else { Err(Short) })
    }

    /// The next `n` bytes (after a successful [`In::need`]).
    pub fn take<'b>(&mut self, buf: &'b [u8], n: usize) -> &'b [u8] {
        let lo = usize::from(self.lo);
        self.lo += n as u16;
        &buf[lo..lo + n]
    }

    /// Skip forward to file offset `to` (≥ [`In::at`]).
    pub fn skip_to(&mut self, to: u32) {
        let at = self.at();
        if to <= at {
            return;
        }
        let d = to - at;
        if d <= u32::from(self.hi - self.lo) {
            self.lo += d as u16;
        } else {
            self.pos = to;
            self.lo = 0;
            self.hi = 0;
        }
    }
}
