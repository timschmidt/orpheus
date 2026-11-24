//! The library on a `orpheus-fs` volume: [`TagSource`] over one
//! [`CardIo`] session (`orpheus_index::card`, whose [`CardIo::new_in`] with
//! [`crate::LIBRARY_DIR`] is the library's [`crate::Storage`]).
//!
//! The scan's place on the card between steps — the folder being listed
//! and the open audio file — is a plain value, [`TagState`], that the
//! caller keeps for a session (the firmware: one idle slot of the card
//! task). A builder resumed in a new session re-opens its folder with
//! `open_dir(.., after)`, so a fresh `TagState` is always correct; keeping
//! it only saves the re-open between steps.

use orpheus_fs::{BlockDevice, DirCursor, DirEntry, File, Locator};
use orpheus_index::card::{CardIo, FsError, is_dot, spin, src_entry};
use orpheus_index::{DirSource, MAX_NAME, SrcEntry};

use crate::TagSource;

/// Where a scan is on the card (about 170 bytes; no borrow).
#[derive(Clone, Default)]
pub struct TagState {
    /// The folder's first entry, and the next entry to read.
    start: Option<DirCursor>,
    cur: Option<DirCursor>,
    /// The audio file whose tags are being read.
    file: Option<File>,
    /// The entry last read is still in the caller's `DirEntry` (the
    /// builder opens the file it was just handed: no directory seek).
    last: Option<Locator>,
}

impl TagState {
    #[must_use]
    pub const fn new() -> TagState {
        TagState { start: None, cur: None, file: None, last: None }
    }
}

/// [`TagSource`] of a [`CardIo`] session; directory entries are read
/// through `e`.
pub struct CardTags<'r, 'a, 'v, 'c, D: BlockDevice> {
    io: &'r CardIo<'a, 'v, 'c, D>,
    st: &'r mut TagState,
    e: &'r mut DirEntry,
}

impl<'r, 'a, 'v, 'c, D: BlockDevice> CardTags<'r, 'a, 'v, 'c, D> {
    pub fn new(io: &'r CardIo<'a, 'v, 'c, D>, st: &'r mut TagState, e: &'r mut DirEntry) -> Self {
        CardTags { io, st, e }
    }
}

impl<D: BlockDevice> DirSource for CardTags<'_, '_, '_, '_, D> {
    type Error = FsError<D>;

    fn rewind(&mut self) -> Result<(), Self::Error> {
        self.st.cur = self.st.start;
        Ok(())
    }

    fn next_entry(&mut self, name: &mut [u8]) -> Result<Option<SrcEntry>, Self::Error> {
        self.st.last = None;
        let Some(cur) = self.st.cur.as_mut() else { return Ok(None) };
        let e = &mut *self.e;
        let r = self.io.with_vol(|vol, cx| {
            loop {
                if !spin(cx, vol.read_dir(cur, e))? {
                    return Ok(None);
                }
                if is_dot(e) {
                    continue;
                }
                let s = src_entry(e);
                let n = e.name().as_bytes();
                let k = n.len().min(name.len()).min(MAX_NAME);
                name[..k].copy_from_slice(&n[..k]);
                return Ok(Some(s));
            }
        })?;
        self.st.last = r.map(|s| s.loc);
        Ok(r)
    }
}

impl<D: BlockDevice> TagSource for CardTags<'_, '_, '_, '_, D> {
    fn open_dir(&mut self, dir: Option<Locator>, after: Option<Locator>) -> Result<Option<u32>, Self::Error> {
        self.st.start = None;
        self.st.cur = None;
        self.st.last = None;
        let e = &mut *self.e;
        let found = self.io.with_vol(|vol, cx| -> Result<Option<(DirCursor, DirCursor)>, Self::Error> {
            let start = match dir {
                None => vol.root(),
                Some(l) => {
                    match spin(cx, vol.open_locator(l, e)) {
                        Ok(()) => {}
                        Err(orpheus_fs::Error::NotFound) => return Ok(None),
                        Err(x) => return Err(x),
                    }
                    match vol.dir_cursor(e) {
                        Ok(c) => c,
                        Err(_) => return Ok(None),
                    }
                }
            };
            let mut cur = start;
            if let Some(a) = after {
                // Past the entry handed out last: seek to it and read it.
                spin(cx, vol.seek_dir(&mut cur, a.index & !(1 << 31)))?;
                let at = cur;
                if !spin(cx, vol.read_dir(&mut cur, e))? || e.locator() != a {
                    // Not there any more: continue from where it was.
                    cur = at;
                }
            }
            Ok(Some((start, cur)))
        })?;
        Ok(found.map(|(start, cur)| {
            self.st.start = Some(start);
            self.st.cur = Some(cur);
            start.dir_cluster()
        }))
    }

    fn open_file(&mut self, loc: Locator) -> Result<Option<u64>, Self::Error> {
        // Read-only handles need no close.
        self.st.file = None;
        let e = &mut *self.e;
        let f = if self.st.last == Some(loc) {
            // Just read: no directory seek.
            self.io.with_vol(|vol, _| if e.is_dir() { Ok(None) } else { vol.open_entry(e).map(Some) })?
        } else {
            self.st.last = None;
            self.io.with_vol(|vol, cx| match spin(cx, vol.open_locator(loc, e)) {
                Ok(()) if !e.is_dir() => vol.open_entry(e).map(Some),
                Ok(()) | Err(orpheus_fs::Error::NotFound) => Ok(None),
                Err(x) => Err(x),
            })?
        };
        let size = f.as_ref().map(File::size);
        self.st.file = f;
        Ok(size)
    }

    fn read_at(&mut self, offset: u64, buf: &mut [u8]) -> Result<usize, Self::Error> {
        let Some(f) = self.st.file.as_mut() else { return Ok(0) };
        f.set_position(offset);
        self.io.with_vol(|vol, cx| spin(cx, vol.read_exact_or_eof(f, buf)))
    }
}
