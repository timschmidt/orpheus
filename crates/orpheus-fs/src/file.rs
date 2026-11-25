//! Files: open, read, write, seek, truncate/extend, flush, extents.

use crate::block::BlockDevice;
use crate::cache::Kind;
use crate::dir::{ATTR_ARCHIVE, ATTR_RO, DirEntry, EntryInfo, EntryLoc, Locator, NewEntry};
use crate::volume::{Link, Volume, ZERO};
use crate::{Error, Result};

const CONTIG: u8 = 1;
const WRITE: u8 = 2;
const DIRTY: u8 = 4;
const MODIFIED: u8 = 8;

/// FAT file size limit.
const FAT_MAX: u64 = 0xFFFF_FFFF;

/// How to open a file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OpenMode {
    /// Existing file, read only.
    Read,
    /// Existing file, read and write.
    ReadWrite,
    /// Open, creating it empty if missing.
    Create,
    /// Create; fail with `AlreadyExists` if it exists.
    CreateNew,
    /// Open or create, then truncate to zero length.
    Truncate,
}

/// Seek origin for [`File::seek`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SeekFrom {
    /// From the start.
    Start(u64),
    /// From the current position.
    Current(i64),
    /// From the end.
    End(i64),
}

/// An open file: a plain value (64 bytes) that does not borrow the volume.
///
/// Holds the position, the size, the location of its directory entry and a
/// one-run extent cache (`run_*`: the contiguous cluster run last used), so
/// sequential reads of a file walk the FAT once per fragment.
#[derive(Clone, Debug)]
pub struct File {
    first: u32,
    clusters: u32,
    size: u64,
    valid: u64,
    pos: u64,
    run_idx: u32,
    run_cluster: u32,
    run_len: u32,
    loc: EntryLoc,
    flags: u8,
    attr: u8,
}

impl File {
    fn from_info(info: &EntryInfo, cbits: u32, exfat: bool, write: bool) -> File {
        let mut clusters = info.size.div_ceil(1u64 << cbits).min(u64::from(u32::MAX)) as u32;
        if info.first_cluster != 0 && clusters == 0 {
            clusters = 1;
        }
        File {
            first: info.first_cluster,
            clusters: if info.first_cluster == 0 { 0 } else { clusters },
            size: info.size,
            valid: if exfat { info.valid.min(info.size) } else { info.size },
            pos: 0,
            run_idx: 0,
            run_cluster: 0,
            run_len: 0,
            loc: if exfat { info.start } else { info.main },
            flags: if exfat && (info.contig || info.first_cluster == 0) { CONTIG } else { 0 }
                | if write { WRITE } else { 0 },
            attr: info.attr,
        }
    }

    /// Size in bytes.
    #[must_use]
    pub fn size(&self) -> u64 {
        self.size
    }
    /// Current position.
    #[must_use]
    pub fn position(&self) -> u64 {
        self.pos
    }
    /// Set the position (may lie beyond the end; a write there zero-fills).
    pub fn set_position(&mut self, pos: u64) {
        self.pos = pos;
    }
    /// Seek; returns the new position, or `None` if it would be negative.
    pub fn seek(&mut self, from: SeekFrom) -> Option<u64> {
        let p = match from {
            SeekFrom::Start(p) => Some(p),
            SeekFrom::Current(d) => self.pos.checked_add_signed(d),
            SeekFrom::End(d) => self.size.checked_add_signed(d),
        }?;
        self.pos = p;
        Some(p)
    }
    /// True if the file is open for writing.
    #[must_use]
    pub fn is_writable(&self) -> bool {
        self.flags & WRITE != 0
    }
    /// True if the directory entry needs updating (see [`Volume::flush_file`]).
    #[must_use]
    pub fn is_dirty(&self) -> bool {
        self.flags & DIRTY != 0
    }
    /// First cluster (0 for an empty file).
    #[must_use]
    pub fn first_cluster(&self) -> u32 {
        self.first
    }
    /// FAT attribute bits as found when the file was opened.
    #[must_use]
    pub fn attributes(&self) -> u8 {
        self.attr
    }
    /// exFAT: bytes that are valid data (beyond it the file reads as zeros).
    /// FAT: the size.
    #[must_use]
    pub fn valid_length(&self) -> u64 {
        self.valid
    }
}

/// One contiguous run of a file on the device.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Extent {
    /// Absolute LBA on the block device (partition offset included).
    pub lba: u32,
    /// Sectors in the run (the last run is cut to the file size).
    pub sectors: u32,
    /// File offset of the run's first byte.
    pub offset: u64,
}

/// Iterator state for [`Volume::next_extent`].
#[derive(Clone, Copy, Debug)]
pub struct ExtentIter {
    first: u32,
    clusters: u32,
    size: u64,
    contig: bool,
    next_cluster: u32,
    next_idx: u32,
}

impl<D: BlockDevice> Volume<'_, D> {
    /// Open a file by path.
    pub async fn open(&mut self, path: &str, mode: OpenMode) -> Result<File, D::Error> {
        let write = mode != OpenMode::Read;
        if write {
            self.check_prefix(path)?;
            if self.read_only {
                return Err(Error::ReadOnly);
            }
        }
        let w = self.walk(path).await?;
        if w.name.is_empty() {
            return Err(Error::IsADirectory);
        }
        let cbits = self.g.cbits();
        let exfat = self.g.is_exfat();
        match (w.info, mode) {
            (Some(_), OpenMode::CreateNew) => Err(Error::AlreadyExists),
            (Some(info), _) => {
                if info.is_dir() {
                    return Err(Error::IsADirectory);
                }
                if write && info.attr & ATTR_RO != 0 {
                    return Err(Error::ReadOnly);
                }
                let mut f = File::from_info(&info, cbits, exfat, write);
                if mode == OpenMode::Truncate && f.size != 0 {
                    self.set_len(&mut f, 0).await?;
                }
                Ok(f)
            }
            (None, OpenMode::Read | OpenMode::ReadWrite) => Err(Error::NotFound),
            (None, _) => {
                self.begin_write().await?;
                let mut parent = w.parent;
                let now = self.now;
                let ne = NewEntry {
                    attr: ATTR_ARCHIVE,
                    first: 0,
                    size: 0,
                    valid: 0,
                    contig: false,
                    created: now,
                    modified: now,
                };
                let info = self.create_entry(&mut parent, w.name, ne).await?;
                Ok(File::from_info(&info, cbits, exfat, true))
            }
        }
    }

    /// Open a file by path, read-only, and say where its entry is: a
    /// [`Locator`] that [`Volume::open_at`] reopens it from later without
    /// the path walk.
    pub async fn open_located(&mut self, path: &str) -> Result<(File, Locator), D::Error> {
        let w = self.walk(path).await?;
        if w.name.is_empty() {
            return Err(Error::IsADirectory);
        }
        let info = w.info.ok_or(Error::NotFound)?;
        if info.is_dir() {
            return Err(Error::IsADirectory);
        }
        let loc =
            Locator { dir: w.parent.first, index: info.index | if w.parent.contig { 1 << 31 } else { 0 } };
        Ok((File::from_info(&info, self.g.cbits(), self.g.is_exfat(), false), loc))
    }

    /// Open, read-only, the file whose entry `loc` points at, if it is
    /// still there and still named `name` (compared as a path walk does);
    /// `NotFound` otherwise (removed, renamed, its slot taken by another
    /// file, another volume). The entry is read again, so a file rewritten
    /// in place opens with its new clusters and size. Costs the seek to the
    /// entry (one step per directory cluster) and its sectors, where a path
    /// walk searches every directory on the way (a large root folder is
    /// tens to hundreds of sector reads).
    pub async fn open_at(&mut self, loc: Locator, name: &str) -> Result<File, D::Error> {
        let info = self.find_at(loc, name).await?;
        if info.is_dir() {
            return Err(Error::IsADirectory);
        }
        Ok(File::from_info(&info, self.g.cbits(), self.g.is_exfat(), false))
    }

    /// Open a file found by `read_dir`/`stat`, read-only.
    pub fn open_entry(&self, e: &DirEntry) -> Result<File, D::Error> {
        if e.is_dir() {
            return Err(Error::IsADirectory);
        }
        Ok(File::from_info(&e.info, self.g.cbits(), self.g.is_exfat(), false))
    }

    /// Cluster holding file cluster index `idx`; updates the run cache.
    async fn cluster_at(&mut self, f: &mut File, idx: u32) -> Result<Option<u32>, D::Error> {
        if f.first == 0 || idx >= f.clusters {
            return Ok(None);
        }
        if f.flags & CONTIG != 0 {
            let c = f.first.checked_add(idx).ok_or(Error::Corrupt)?;
            return if self.g.valid_cluster(c) { Ok(Some(c)) } else { Err(Error::Corrupt) };
        }
        if f.run_len > 0 && idx >= f.run_idx && idx - f.run_idx < f.run_len {
            return Ok(Some(f.run_cluster + (idx - f.run_idx)));
        }
        let (mut i, mut c) =
            if f.run_len > 0 && idx >= f.run_idx { (f.run_idx, f.run_cluster) } else { (0, f.first) };
        let mut steps = 0u32;
        loop {
            let mut len = 1u32;
            let mut next = None;
            while i + len < f.clusters {
                steps += 1;
                if steps > self.g.cluster_count + 1 {
                    return Err(Error::Corrupt);
                }
                match self.fat_get(c + len - 1).await? {
                    Link::Next(n) if n == c + len => len += 1,
                    Link::Next(n) => {
                        next = Some(n);
                        break;
                    }
                    Link::End => break,
                    Link::Free | Link::Bad => return Err(Error::Corrupt),
                }
            }
            if idx - i < len {
                let r = c + (idx - i);
                if !self.g.valid_cluster(r) {
                    return Err(Error::Corrupt);
                }
                f.run_idx = i;
                f.run_cluster = c;
                f.run_len = len;
                return Ok(Some(r));
            }
            match next {
                Some(n) => {
                    i += len;
                    c = n;
                }
                None => return Err(Error::Corrupt),
            }
        }
    }

    /// Clusters from `idx` to the end of its contiguous run (after `cluster_at`).
    fn run_left(f: &File, idx: u32) -> u32 {
        if f.flags & CONTIG != 0 {
            f.clusters - idx
        } else if f.run_len > 0 && idx >= f.run_idx && idx - f.run_idx < f.run_len {
            f.run_idx + f.run_len - idx
        } else {
            1
        }
    }

    /// Grow the chain until it has more than `upto` clusters.
    async fn extend(&mut self, f: &mut File, upto: u32) -> Result<(), D::Error> {
        let exfat = self.g.is_exfat();
        while f.clusters <= upto {
            let idx = f.clusters;
            let last = if idx == 0 || f.first == 0 {
                None
            } else {
                Some(self.cluster_at(f, idx - 1).await?.ok_or(Error::Corrupt)?)
            };
            let contig = f.flags & CONTIG != 0;
            let chain = !(exfat && (contig || last.is_none()));
            let near = last.map_or(self.next_free, |l| l + 1);
            let new = self.alloc_cluster(near, chain).await?;
            match last {
                None => {
                    f.first = new;
                    if exfat {
                        f.flags |= CONTIG;
                    }
                }
                Some(l) => {
                    if exfat && contig {
                        if new != l + 1 {
                            for k in 0..idx - 1 {
                                self.fat_set(f.first + k, f.first + k + 1).await?;
                            }
                            self.fat_set(l, new).await?;
                            self.fat_set(new, self.g.eoc()).await?;
                            f.flags &= !CONTIG;
                            f.run_idx = 0;
                            f.run_cluster = f.first;
                            f.run_len = idx;
                        }
                    } else {
                        self.fat_set(l, new).await?;
                    }
                }
            }
            if f.run_len > 0 && f.run_idx + f.run_len == idx && f.run_cluster + f.run_len == new {
                f.run_len += 1;
            } else {
                f.run_idx = idx;
                f.run_cluster = new;
                f.run_len = 1;
            }
            f.clusters += 1;
            f.flags |= DIRTY;
        }
        Ok(())
    }

    /// Read at the current position; returns bytes read (0 at the end).
    pub async fn read(&mut self, f: &mut File, buf: &mut [u8]) -> Result<usize, D::Error> {
        if f.pos >= f.size {
            return Ok(0);
        }
        let n = (buf.len() as u64).min(f.size - f.pos) as usize;
        let cbits = self.g.cbits();
        let cmask = (1u64 << cbits) - 1;
        let spc = u64::from(self.g.spc());
        let mut done = 0usize;
        while done < n {
            let pos = f.pos;
            if pos >= f.valid {
                buf[done..n].fill(0);
                f.pos += (n - done) as u64;
                break;
            }
            let lim = ((n - done) as u64).min(f.valid - pos);
            let idx = u32::try_from(pos >> cbits).map_err(|_| Error::Corrupt)?;
            let c = self.cluster_at(f, idx).await?.ok_or(Error::Corrupt)?;
            let in_cl = pos & cmask;
            let sec = (in_cl >> 9) as u32;
            let byte = (in_cl & 511) as usize;
            let lba = self.g.cluster_lba(c) + sec;
            let adv = if byte == 0 && lim >= 512 {
                let avail = u64::from(Self::run_left(f, idx)) * spc - u64::from(sec);
                let k = (lim / 512).min(avail).min(u64::from(u32::MAX >> 9)) as u32;
                let len = k as usize * 512;
                self.cache.flush_range(&mut self.dev, lba, k).await?;
                let (chunks, _) = buf[done..done + len].as_chunks_mut::<512>();
                self.dev.read(lba, chunks).await.map_err(Error::Io)?;
                len
            } else {
                let m = (512 - byte).min(lim as usize);
                let i = self.cache.load(&mut self.dev, lba, Kind::Data, false).await?;
                buf[done..done + m].copy_from_slice(&self.cache.data(i)[byte..byte + m]);
                m
            };
            f.pos += adv as u64;
            done += adv;
        }
        Ok(n)
    }

    /// Read until `buf` is full or the end of the file.
    pub async fn read_exact_or_eof(&mut self, f: &mut File, buf: &mut [u8]) -> Result<usize, D::Error> {
        let mut got = 0;
        while got < buf.len() {
            let n = self.read(f, &mut buf[got..]).await?;
            if n == 0 {
                break;
            }
            got += n;
        }
        Ok(got)
    }

    /// Write `data` (or zeros if `None`) of `len` bytes at the position.
    async fn write_data(&mut self, f: &mut File, data: Option<&[u8]>, len: u64) -> Result<(), D::Error> {
        let cbits = self.g.cbits();
        let cmask = (1u64 << cbits) - 1;
        let spc = u64::from(self.g.spc());
        let mut done = 0u64;
        while done < len {
            let pos = f.pos;
            let idx = u32::try_from(pos >> cbits).map_err(|_| Error::FileTooLarge)?;
            if idx >= f.clusters || f.first == 0 {
                let last_needed =
                    u32::try_from((pos + (len - done) - 1) >> cbits).map_err(|_| Error::FileTooLarge)?;
                let upto = last_needed.min(idx.saturating_add(255));
                self.extend(f, upto).await?;
            }
            let c = self.cluster_at(f, idx).await?.ok_or(Error::Corrupt)?;
            let in_cl = pos & cmask;
            let sec = (in_cl >> 9) as u32;
            let byte = (in_cl & 511) as usize;
            let lba = self.g.cluster_lba(c) + sec;
            let rem = len - done;
            let adv: u64 = if byte == 0 && rem >= 512 {
                let avail = u64::from(Self::run_left(f, idx)) * spc - u64::from(sec);
                let k = (rem / 512).min(avail).min(u64::from(u32::MAX >> 9)) as u32;
                match data {
                    Some(d) => {
                        let s = done as usize;
                        let (chunks, _) = d[s..s + k as usize * 512].as_chunks::<512>();
                        self.dev.write(lba, chunks).await.map_err(Error::Io)?;
                    }
                    None => {
                        for j in 0..k {
                            self.dev.write(lba + j, core::slice::from_ref(&ZERO)).await.map_err(Error::Io)?;
                        }
                    }
                }
                self.cache.invalidate_range(lba, k);
                u64::from(k) * 512
            } else {
                let m = (512 - byte).min(rem as usize);
                let fresh = pos - byte as u64 >= f.valid;
                let i = self.cache.load(&mut self.dev, lba, Kind::Data, fresh).await?;
                let s = self.cache.data_mut(i);
                match data {
                    Some(d) => {
                        let o = done as usize;
                        s[byte..byte + m].copy_from_slice(&d[o..o + m]);
                    }
                    None => s[byte..byte + m].fill(0),
                }
                m as u64
            };
            f.pos += adv;
            done += adv;
            if f.pos > f.size {
                f.size = f.pos;
            }
            if f.pos > f.valid {
                f.valid = f.pos;
            }
            f.flags |= DIRTY | MODIFIED;
        }
        Ok(())
    }

    /// Write at the current position (zero-filling any gap after the end).
    /// Returns `buf.len()`.
    pub async fn write(&mut self, f: &mut File, buf: &[u8]) -> Result<usize, D::Error> {
        if f.flags & WRITE == 0 {
            return Err(Error::ReadOnly);
        }
        if buf.is_empty() {
            return Ok(0);
        }
        let end = f.pos.checked_add(buf.len() as u64).ok_or(Error::FileTooLarge)?;
        if !self.g.is_exfat() && end > FAT_MAX {
            return Err(Error::FileTooLarge);
        }
        self.begin_write().await?;
        if f.pos > f.valid {
            let target = f.pos;
            f.pos = f.valid;
            self.write_data(f, None, target - f.valid).await?;
        }
        self.write_data(f, Some(buf), buf.len() as u64).await?;
        Ok(buf.len())
    }

    /// Replace the whole content of `f` with `data` inside the clusters it
    /// has, when they are as many as `data` needs (at least one): nothing
    /// is freed or allocated, no data sector is read (the old content is
    /// not kept), the size (exFAT: and the valid-data length) becomes
    /// `data.len()`. The entry changes in `f` only: [`Volume::write_entry`]
    /// or [`Volume::close`] after it. `Ok(false)`, with nothing changed,
    /// when the clusters do not fit `data` (truncate and write instead).
    ///
    /// Where truncating and writing frees the old clusters and allocates
    /// new ones (on a large, full card a search of the allocation table:
    /// a games save was 452 bitmap reads on the owner's card, 2026-11-25),
    /// this costs the data sectors and the entry. A power cut leaves the
    /// old entry (size and time) with some or all of the new data sectors
    /// (data reach the card before the entry): no cluster is ever in
    /// question, the volume needs no repair; the file reads as the new
    /// content cut or padded to the old size.
    pub async fn rewrite(&mut self, f: &mut File, data: &[u8]) -> Result<bool, D::Error> {
        if f.flags & WRITE == 0 {
            return Err(Error::ReadOnly);
        }
        let need = (data.len() as u64).div_ceil(u64::from(self.g.cluster_bytes()));
        if data.is_empty() || f.first == 0 || u64::from(f.clusters) != need {
            return Ok(false);
        }
        self.begin_write().await?;
        f.pos = 0;
        // Nothing of the old content is valid any more: every sector the
        // write touches is fresh (filled, not read).
        f.valid = 0;
        let r = self.write_data(f, Some(data), data.len() as u64).await;
        f.size = f.pos;
        f.valid = f.pos;
        f.flags |= DIRTY | MODIFIED;
        r.map(|()| true)
    }

    /// Truncate or extend. exFAT extends by allocation only (the new range
    /// reads as zeros through the valid-data length); FAT zero-fills.
    pub async fn set_len(&mut self, f: &mut File, len: u64) -> Result<(), D::Error> {
        if f.flags & WRITE == 0 {
            return Err(Error::ReadOnly);
        }
        let exfat = self.g.is_exfat();
        if !exfat && len > FAT_MAX {
            return Err(Error::FileTooLarge);
        }
        self.begin_write().await?;
        let cbits = self.g.cbits();
        if len > f.size {
            if exfat {
                let need = u32::try_from(len.div_ceil(1 << cbits)).map_err(|_| Error::FileTooLarge)?;
                if need > 0 {
                    self.extend(f, need - 1).await?;
                }
                f.size = len;
                f.flags |= DIRTY | MODIFIED;
            } else {
                let save = f.pos;
                f.pos = f.size;
                let gap = len - f.size;
                let r = self.write_data(f, None, gap).await;
                f.pos = save;
                r?;
            }
            return Ok(());
        }
        if len == f.size {
            return Ok(());
        }
        let keep = len.div_ceil(1 << cbits) as u32;
        let old = f.clusters;
        let contig = f.flags & CONTIG != 0;
        f.size = len;
        f.valid = f.valid.min(len);
        f.flags |= DIRTY | MODIFIED;
        if keep >= old || f.first == 0 {
            return Ok(());
        }
        let (last, tail) = if keep == 0 {
            (0, f.first)
        } else {
            let last = self.cluster_at(f, keep - 1).await?.ok_or(Error::Corrupt)?;
            let tail = if contig {
                last + 1
            } else {
                match self.fat_get(last).await? {
                    Link::Next(n) => n,
                    _ => return Err(Error::Corrupt),
                }
            };
            (last, tail)
        };
        if keep == 0 {
            f.first = 0;
        }
        f.clusters = keep;
        f.run_len = 0;
        // Entry first (on disk), then release: a cut leaks, never cross-links.
        self.update_entry(f).await?;
        self.cache.flush(&mut self.dev).await?;
        if keep > 0 && !contig {
            self.fat_set(last, self.g.eoc()).await?;
        }
        if contig {
            self.free_chain(tail, Some(old - keep)).await?;
        } else {
            self.free_chain(tail, None).await?;
        }
        Ok(())
    }

    /// exFAT: declare the whole allocated size valid without writing it
    /// (valid-data length := size). For raw-LBA scratch files filled through
    /// [`Volume::extents`]: whatever the clusters hold becomes file content,
    /// so only use it on files whose every sector the caller writes. FAT:
    /// no effect (FAT has no valid-data length).
    pub async fn mark_valid(&mut self, f: &mut File) -> Result<(), D::Error> {
        if f.flags & WRITE == 0 {
            return Err(Error::ReadOnly);
        }
        if self.g.is_exfat() && f.valid != f.size {
            self.begin_write().await?;
            f.valid = f.size;
            f.flags |= DIRTY | MODIFIED;
        }
        Ok(())
    }

    /// Write the file's directory entry (size, first cluster, flags, time).
    async fn update_entry(&mut self, f: &mut File) -> Result<(), D::Error> {
        if f.flags & DIRTY == 0 {
            return Ok(());
        }
        let (d, t) = self.now.to_fat();
        let modified = f.flags & MODIFIED != 0;
        let first = f.first;
        let size = f.size;
        if self.g.is_exfat() {
            let valid = f.valid;
            let contig = f.flags & CONTIG != 0;
            let ts = (u32::from(d) << 16) | u32::from(t);
            let tenms = self.now.tenms();
            let sl = self.loc_next(f.loc).await?;
            self.modify_entry(sl, |s| {
                s[1] = (s[1] & !3) | u8::from(first != 0) | if contig && first != 0 { 2 } else { 0 };
                s[8..16].copy_from_slice(&valid.to_le_bytes());
                s[20..24].copy_from_slice(&first.to_le_bytes());
                s[24..32].copy_from_slice(&size.to_le_bytes());
            })
            .await?;
            self.modify_entry(f.loc, |e| {
                if modified {
                    e[4] |= ATTR_ARCHIVE;
                    e[12..16].copy_from_slice(&ts.to_le_bytes());
                    e[16..20].copy_from_slice(&ts.to_le_bytes());
                    e[21] = tenms;
                }
            })
            .await?;
            self.exfat_fix_checksum(f.loc).await?;
        } else {
            let size32 = size.min(FAT_MAX) as u32;
            self.modify_entry(f.loc, |e| {
                e[20..22].copy_from_slice(&((first >> 16) as u16).to_le_bytes());
                e[26..28].copy_from_slice(&(first as u16).to_le_bytes());
                e[28..32].copy_from_slice(&size32.to_le_bytes());
                if modified {
                    e[11] |= ATTR_ARCHIVE;
                    e[18..20].copy_from_slice(&d.to_le_bytes());
                    e[22..24].copy_from_slice(&t.to_le_bytes());
                    e[24..26].copy_from_slice(&d.to_le_bytes());
                }
            })
            .await?;
        }
        f.flags &= !(DIRTY | MODIFIED);
        Ok(())
    }

    /// `fsync`: write the file's entry, then flush the whole volume.
    pub async fn flush_file(&mut self, f: &mut File) -> Result<(), D::Error> {
        if f.flags & WRITE != 0 {
            self.update_entry(f).await?;
            self.flush().await?;
        }
        Ok(())
    }

    /// The file's directory entry written into the sector cache (size,
    /// first cluster, time), without flushing the volume: with
    /// [`Volume::flush_step`] after it, [`Volume::close`] in steps.
    pub async fn write_entry(&mut self, f: &mut File) -> Result<(), D::Error> {
        if f.flags & WRITE != 0 {
            self.update_entry(f).await?;
        }
        Ok(())
    }

    /// Flush and drop a file handle.
    pub async fn close(&mut self, mut f: File) -> Result<(), D::Error> {
        self.flush_file(&mut f).await
    }

    // ---------------------------------------------------------- extents

    /// Start mapping a file to device runs.
    pub fn extents(&self, f: &File) -> ExtentIter {
        ExtentIter {
            first: f.first,
            clusters: f.clusters,
            size: f.size,
            contig: f.flags & CONTIG != 0,
            next_cluster: f.first,
            next_idx: 0,
        }
    }

    /// Next contiguous run, or `None` after the last.
    pub async fn next_extent(&mut self, it: &mut ExtentIter) -> Result<Option<Extent>, D::Error> {
        if it.first == 0 || it.next_idx >= it.clusters {
            return Ok(None);
        }
        let spc_shift = self.g.spc_shift;
        let total_sectors = it.size.div_ceil(512);
        let i = it.next_idx;
        let start_sector = u64::from(i) << spc_shift;
        if start_sector >= total_sectors {
            it.next_idx = it.clusters;
            return Ok(None);
        }
        let c = it.next_cluster;
        if !self.g.valid_cluster(c) {
            return Err(Error::Corrupt);
        }
        let mut len = 1u32;
        if it.contig {
            len = it.clusters - i;
            if !self.g.valid_cluster(c + len - 1) {
                return Err(Error::Corrupt);
            }
        } else {
            let mut steps = 0u32;
            while i + len < it.clusters {
                steps += 1;
                if steps > self.g.cluster_count {
                    return Err(Error::Corrupt);
                }
                match self.fat_get(c + len - 1).await? {
                    Link::Next(n) if n == c + len => len += 1,
                    Link::Next(n) => {
                        it.next_cluster = n;
                        break;
                    }
                    _ => return Err(Error::Corrupt),
                }
            }
        }
        it.next_idx = i + len;
        let sectors = (u64::from(len) << spc_shift).min(total_sectors - start_sector);
        Ok(Some(Extent {
            lba: self.g.cluster_lba(c),
            sectors: u32::try_from(sectors).map_err(|_| Error::FileTooLarge)?,
            offset: u64::from(i) << self.g.cbits(),
        }))
    }

    /// True if the file occupies one contiguous run (or none).
    pub async fn is_contiguous(&mut self, f: &File) -> Result<bool, D::Error> {
        if f.flags & CONTIG != 0 {
            return Ok(true);
        }
        let mut it = self.extents(f);
        let mut n = 0;
        while self.next_extent(&mut it).await?.is_some() {
            n += 1;
            if n > 1 {
                return Ok(false);
            }
        }
        Ok(true)
    }
}

/// Compile-time checks that also run for the device target.
const _: () = assert!(core::mem::size_of::<File>() <= 64);
const _: () = assert!(core::mem::size_of::<crate::DirCursor>() <= 48);

#[cfg(test)]
mod tests {
    use super::*;

    use crate::{BlockDevice, CacheSlot, DirCursor, DirEntry, MountOptions, Sector, Volume};
    use core::mem::{size_of, size_of_val};

    struct NullDev;
    impl BlockDevice for NullDev {
        type Error = ();
        async fn read(&mut self, _: u32, _: &mut [Sector]) -> core::result::Result<(), ()> {
            Ok(())
        }
        async fn write(&mut self, _: u32, _: &[Sector]) -> core::result::Result<(), ()> {
            Ok(())
        }
        fn sector_count(&self) -> u32 {
            0
        }
    }

    /// RAM figures quoted in the README (host build; pointers are 8 bytes
    /// here and 4 on the device, so the device figures are at most these).
    #[test]
    fn ram_figures() {
        assert!(size_of::<File>() <= 64, "File {}", size_of::<File>());
        assert!(size_of::<DirCursor>() <= 48, "DirCursor {}", size_of::<DirCursor>());
        let vol = size_of::<Volume<'_, NullDev>>();
        assert!(vol <= 256, "Volume {vol}");
        let mut slots = [CacheSlot::EMPTY, CacheSlot::EMPTY];
        let m = Volume::mount(NullDev, &mut slots, MountOptions::default());
        let mount_fut = size_of_val(&m);
        drop(m);
        std::println!(
            "File {} B, DirCursor {} B, DirEntry {} B, CacheSlot {} B, Volume (w/o device) {} B",
            size_of::<File>(),
            size_of::<DirCursor>(),
            size_of::<DirEntry>(),
            size_of::<CacheSlot>(),
            vol
        );
        std::println!("future sizes: mount {mount_fut} B");
    }
}
