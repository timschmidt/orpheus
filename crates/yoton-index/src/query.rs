//! Reading an index: windows of rows, positions, letter jump. Nothing here
//! holds more than a 512-byte stack buffer and the caller's window.

use core::cmp::Ordering;

use yoton_apps::browse::{is_hidden_name, kind_of, letter_of};
use yoton_fs::Locator;

use crate::key::KeyPrefix;
use crate::record::{
    HEADER, HEADER_USED, Header, LETTER_RUN, RECORD, Rec, Row, Signature, cmp_full, crc32_update, encode,
    rd32,
};
use crate::{Error, FileName, MAX_NAME, NameSource, Spec, Storage};

/// Records per stack chunk (one sector: rows start sector aligned).
const CHUNK: usize = 16;

/// An open, complete index (about 80 bytes; no file handle is kept — every
/// query goes through the [`Storage`] it is given).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Index {
    file: FileName,
    h: Header,
}

/// An entry to look up with [`Index::find`] (e.g. a file just renamed).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Probe<'a> {
    pub name: &'a str,
    pub loc: Locator,
    pub is_dir: bool,
    /// FAT hidden/system attribute.
    pub hidden: bool,
    pub size: u64,
    pub mtime: u32,
}

impl Index {
    pub(crate) fn from_header(file: FileName, h: &Header) -> Index {
        Index { file, h: *h }
    }

    /// Open the index of `dir` for `spec`. `None` if there is none or it is
    /// incomplete (build interrupted), damaged, of another format version,
    /// or of another directory: build it.
    pub fn open<S: Storage>(store: &mut S, dir: u32, spec: Spec) -> Result<Option<Index>, Error<S::Error>> {
        let file = FileName::index(dir, spec);
        let Some(len) = store.len(file)? else { return Ok(None) };
        let mut b = [0u8; HEADER_USED];
        if store.read_at(file, 0, &mut b)? != HEADER_USED {
            return Ok(None);
        }
        let Some(h) = Header::decode(&b) else { return Ok(None) };
        if h.dir != dir || h.spec != spec || u64::from(len) != h.file_len() || h.dirs > h.rows {
            return Ok(None);
        }
        Ok(Some(Index { file, h }))
    }

    #[must_use]
    pub fn file(&self) -> FileName {
        self.file
    }
    /// Rows in the listing.
    #[must_use]
    pub fn count(&self) -> u32 {
        self.h.rows
    }
    /// Directory rows; they are rows `0..dirs()`.
    #[must_use]
    pub fn dirs(&self) -> u32 {
        self.h.dirs
    }
    /// File rows (`count() - dirs()`).
    #[must_use]
    pub fn files(&self) -> u32 {
        self.h.rows - self.h.dirs
    }
    /// Maximal runs of equal letters along the order.
    #[must_use]
    pub fn letter_runs(&self) -> u32 {
        self.h.letter_runs
    }
    #[must_use]
    pub fn spec(&self) -> Spec {
        self.h.spec
    }
    /// The directory signature the index was built from.
    #[must_use]
    pub fn signature(&self) -> Signature {
        self.h.sig
    }
    /// The caller's quick stamp given to the builder.
    #[must_use]
    pub fn quick(&self) -> u64 {
        self.h.quick
    }
    /// True if the directory still has the signature the index was built
    /// from (see [`crate::Verifier`]).
    #[must_use]
    pub fn is_current(&self, sig: &Signature) -> bool {
        self.h.sig == *sig
    }
    /// Bytes on the card.
    #[must_use]
    pub fn file_len(&self) -> u64 {
        self.h.file_len()
    }

    fn rec_off(i: u32) -> u32 {
        HEADER as u32 + i * RECORD as u32
    }

    fn runs_off(&self) -> u32 {
        Self::rec_off(self.h.rows)
    }

    /// Read rows `[first, first + out.len())` (clipped to the listing) into
    /// `out`; returns how many were read.
    pub fn rows<S: Storage>(
        &self,
        store: &mut S,
        first: u32,
        out: &mut [Row],
    ) -> Result<usize, Error<S::Error>> {
        let n = (self.h.rows.saturating_sub(first) as usize).min(out.len());
        let mut buf = [0u8; CHUNK * RECORD];
        let mut done = 0;
        while done < n {
            let m = (n - done).min(CHUNK);
            let b = &mut buf[..m * RECORD];
            if store.read_at(self.file, Self::rec_off(first + done as u32), b)? != b.len() {
                return Err(Error::Corrupt);
            }
            for (o, r) in out[done..done + m].iter_mut().zip(b.chunks_exact(RECORD)) {
                *o = Rec(r).row();
            }
            done += m;
        }
        Ok(n)
    }

    /// Row `i`.
    pub fn row<S: Storage>(&self, store: &mut S, i: u32) -> Result<Option<Row>, Error<S::Error>> {
        let mut r = [Row {
            loc: Locator { dir: 0, index: 0 },
            size: 0,
            mtime: 0,
            letter: None,
            kind: crate::EntryKind::Other,
            hidden: false,
        }];
        Ok((self.rows(store, i, &mut r)? == 1).then_some(r[0]))
    }

    /// Where `loc` is in the listing: a linear scan of the rows (32 bytes
    /// each, read a sector at a time). Use [`Index::find`] when the name
    /// is known.
    pub fn position_of<S: Storage>(
        &self,
        store: &mut S,
        loc: Locator,
    ) -> Result<Option<u32>, Error<S::Error>> {
        let mut buf = [0u8; CHUNK * RECORD];
        let mut i = 0u32;
        while i < self.h.rows {
            let m = ((self.h.rows - i) as usize).min(CHUNK);
            let b = &mut buf[..m * RECORD];
            if store.read_at(self.file, Self::rec_off(i), b)? != b.len() {
                return Err(Error::Corrupt);
            }
            if let Some(k) = b.chunks_exact(RECORD).position(|r| Rec(r).loc() == loc) {
                return Ok(Some(i + k as u32));
            }
            i += m as u32;
        }
        Ok(None)
    }

    /// Binary search for an entry by its sort fields: `Ok(row)` if a row
    /// with the probe's locator sits where the probe sorts, `Err(row)` with
    /// the insertion point otherwise. Comparisons use the stored key
    /// prefixes; a row's full name is read through `names` only on a prefix
    /// tie. `name` must hold [`MAX_NAME`] bytes.
    pub fn find<S, N, E>(
        &self,
        store: &mut S,
        names: &mut N,
        probe: &Probe<'_>,
        name: &mut [u8],
    ) -> Result<Result<u32, u32>, Error<E>>
    where
        S: Storage<Error = E>,
        N: NameSource<Error = E>,
    {
        let row = Row {
            loc: probe.loc,
            size: probe.size,
            mtime: probe.mtime,
            letter: letter_of(probe.name),
            kind: kind_of(probe.name, probe.is_dir),
            hidden: probe.hidden || is_hidden_name(probe.name),
        };
        let mut pr = [0u8; RECORD];
        encode(&row, &KeyPrefix::of(probe.name), &mut pr);
        let pr = Rec(&pr);
        let (mut lo, mut hi) = (0u32, self.h.rows);
        let mut rb = [0u8; RECORD];
        while lo < hi {
            let mid = lo + (hi - lo) / 2;
            if store.read_at(self.file, Self::rec_off(mid), &mut rb)? != RECORD {
                return Err(Error::Corrupt);
            }
            let r = Rec(&rb);
            let o = match crate::record::cmp_fixed(self.h.spec.sort, &r, &pr) {
                Some(o) => o,
                None => {
                    let Some(n) = names.name_of(r.loc(), name)? else { return Err(Error::Corrupt) };
                    let nm = name.get(..n.min(MAX_NAME)).ok_or(Error::BadName)?;
                    cmp_full(self.h.spec.sort, &r, nm, &pr, probe.name.as_bytes())
                }
            };
            match o {
                Ordering::Less => lo = mid + 1,
                Ordering::Greater => hi = mid,
                Ordering::Equal => return Ok(Ok(mid)),
            }
        }
        Ok(Err(lo))
    }

    fn letter_run<S: Storage>(&self, store: &mut S, j: u32) -> Result<(u32, u32), Error<S::Error>> {
        let mut b = [0u8; LETTER_RUN];
        if store.read_at(self.file, self.runs_off() + j * LETTER_RUN as u32, &mut b)? != LETTER_RUN {
            return Err(Error::Corrupt);
        }
        Ok((rd32(&b, 0), rd32(&b, 4)))
    }

    /// Letter runs `[first, first + out.len())` (clipped to
    /// [`Index::letter_runs`]) as `(first row, letter)`, `None` for a run of
    /// rows without a letter; returns how many were read. Small listings
    /// can keep the whole table in RAM and answer letter jumps from it.
    pub fn read_letter_runs<S: Storage>(
        &self,
        store: &mut S,
        first: u32,
        out: &mut [(u32, Option<char>)],
    ) -> Result<usize, Error<S::Error>> {
        let n = (self.h.letter_runs.saturating_sub(first) as usize).min(out.len());
        let mut buf = [0u8; 32 * LETTER_RUN];
        let mut done = 0;
        while done < n {
            let m = (n - done).min(32);
            let b = &mut buf[..m * LETTER_RUN];
            let off = self.runs_off() + (first + done as u32) * LETTER_RUN as u32;
            if store.read_at(self.file, off, b)? != b.len() {
                return Err(Error::Corrupt);
            }
            for (o, e) in out[done..done + m].iter_mut().zip(b.chunks_exact(LETTER_RUN)) {
                *o = (rd32(e, 0), char::from_u32(rd32(e, 4)).filter(|&c| c != '\0'));
            }
            done += m;
        }
        Ok(n)
    }

    /// Index of the letter run that holds row `i` (`i < count()`).
    fn run_of<S: Storage>(&self, store: &mut S, i: u32) -> Result<u32, Error<S::Error>> {
        let (mut lo, mut hi) = (0u32, self.h.letter_runs);
        while hi - lo > 1 {
            let mid = lo + (hi - lo) / 2;
            if self.letter_run(store, mid)?.0 <= i {
                lo = mid;
            } else {
                hi = mid;
            }
        }
        Ok(lo)
    }

    /// Letter of row `i` (from the letter-run table).
    pub fn letter<S: Storage>(&self, store: &mut S, i: u32) -> Result<Option<char>, Error<S::Error>> {
        if i >= self.h.rows {
            return Ok(None);
        }
        let j = self.run_of(store, i)?;
        Ok(char::from_u32(self.letter_run(store, j)?.1).filter(|&c| c != '\0'))
    }

    /// Letter jump with the semantics of `ListSource::seek_letter`: the
    /// first row after `from` (`forward`), or the first row of the group
    /// before it, whose letter differs from `from`'s; rows without a letter
    /// are passed over. Binary search plus a walk over letter runs (read
    /// 256 bytes at a time), never over rows.
    pub fn seek_letter<S: Storage>(
        &self,
        store: &mut S,
        from: u32,
        forward: bool,
    ) -> Result<Option<(u32, char)>, Error<S::Error>> {
        if from >= self.h.rows {
            return Ok(None);
        }
        let r = self.run_of(store, from)?;
        let cur = self.letter_run(store, r)?.1;
        if cur == 0 {
            return Ok(None);
        }
        let mut buf = [0u8; 32 * LETTER_RUN];
        let total = self.h.letter_runs;
        if forward {
            let mut j = r + 1;
            while j < total {
                let m = ((total - j) as usize).min(32);
                let b = &mut buf[..m * LETTER_RUN];
                if store.read_at(self.file, self.runs_off() + j * LETTER_RUN as u32, b)? != b.len() {
                    return Err(Error::Corrupt);
                }
                for e in b.chunks_exact(LETTER_RUN) {
                    let l = rd32(e, 4);
                    if l != 0 && l != cur {
                        return Ok(char::from_u32(l).map(|c| (rd32(e, 0), c)));
                    }
                }
                j += m as u32;
            }
        } else {
            let mut j = r;
            while j > 0 {
                let m = (j as usize).min(32);
                let start = j - m as u32;
                let b = &mut buf[..m * LETTER_RUN];
                if store.read_at(self.file, self.runs_off() + start * LETTER_RUN as u32, b)? != b.len() {
                    return Err(Error::Corrupt);
                }
                for e in b.chunks_exact(LETTER_RUN).rev() {
                    let l = rd32(e, 4);
                    if l != 0 && l != cur {
                        return Ok(char::from_u32(l).map(|c| (rd32(e, 0), c)));
                    }
                }
                j = start;
            }
        }
        Ok(None)
    }

    /// Check the CRCs of the rows and letter runs (reads the whole file
    /// through `buf`; any size ≥ 8). `open` does not do this: the header is
    /// written last, after a sync.
    pub fn verify<S: Storage>(&self, store: &mut S, buf: &mut [u8]) -> Result<bool, Error<S::Error>> {
        let chunk = buf.len() / LETTER_RUN * LETTER_RUN;
        if chunk == 0 {
            return Ok(false);
        }
        let mut crc_of = |store: &mut S, mut off: u32, len: u32| -> Result<u32, Error<S::Error>> {
            let mut crc = 0;
            let end = off + len;
            while off < end {
                let n = ((end - off) as usize).min(chunk);
                if store.read_at(self.file, off, &mut buf[..n])? != n {
                    return Err(Error::Corrupt);
                }
                crc = crc32_update(crc, &buf[..n]);
                off += n as u32;
            }
            Ok(crc)
        };
        let rec = crc_of(store, HEADER as u32, self.h.rows * RECORD as u32)?;
        let runs = crc_of(store, self.runs_off(), self.h.letter_runs * LETTER_RUN as u32)?;
        Ok(rec == self.h.rec_crc && runs == self.h.run_crc)
    }
}
