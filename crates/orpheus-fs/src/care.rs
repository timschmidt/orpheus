//! Card care (Settings › Storage, 05 §7.11): a read-only check of the
//! whole tree and removing a directory tree (the caches under the write
//! prefix).

use crate::block::BlockDevice;
use crate::dir::{DirCursor, DirEntry};
use crate::volume::Volume;
use crate::{Error, Result};

/// What [`Volume::check`] found.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct CheckReport {
    /// Folders read (the root not counted).
    pub dirs: u32,
    /// Files whose cluster chain was followed.
    pub files: u32,
    /// Folders that could not be read to their end and files whose chain
    /// is broken (a free, out-of-range or looping link before the file's
    /// size is reached).
    pub problems: u32,
    /// Folders deeper than the check's cursor stack (counted, not read).
    pub too_deep: u32,
    /// The check was stopped before its end (`stop` said so).
    pub stopped: bool,
}

/// The first `n` bytes of `b` as a path.
fn as_str<E>(b: &[u8], n: usize) -> Result<&str, E> {
    core::str::from_utf8(b.get(..n).unwrap_or_default()).map_err(|_| Error::InvalidName)
}

impl<D: BlockDevice> Volume<'_, D> {
    /// Read every folder and follow every file's clusters, writing nothing.
    /// `stack` holds a cursor per folder level (its length is the depth
    /// read); `e` is the entry buffer. `stop` is asked before each entry
    /// and ends the check early when it says so. Device errors end it
    /// (`Err`); damage is counted in [`CheckReport::problems`].
    pub async fn check(
        &mut self,
        stack: &mut [DirCursor],
        e: &mut DirEntry,
        stop: &mut impl FnMut(&CheckReport) -> bool,
    ) -> Result<CheckReport, D::Error> {
        let mut r = CheckReport::default();
        let Some(first) = stack.first_mut() else { return Err(Error::BufferTooSmall) };
        *first = self.root();
        let mut depth = 1usize;
        while depth > 0 {
            if stop(&r) {
                r.stopped = true;
                break;
            }
            let Some(cur) = stack.get_mut(depth - 1) else { break };
            match self.read_dir(cur, e).await {
                Ok(true) if e.is_dir() => {
                    r.dirs += 1;
                    match (stack.get_mut(depth), self.dir_cursor(e)) {
                        (Some(next), Ok(c)) => {
                            *next = c;
                            depth += 1;
                        }
                        (None, _) => r.too_deep += 1,
                        (_, Err(_)) => r.problems += 1,
                    }
                }
                Ok(true) => {
                    r.files += 1;
                    match self.chain_ok(e).await {
                        Ok(true) => {}
                        Ok(false) => r.problems += 1,
                        Err(x) => return Err(x),
                    }
                }
                Ok(false) => depth -= 1,
                Err(Error::Io(x)) => return Err(Error::Io(x)),
                Err(_) => {
                    r.problems += 1;
                    depth -= 1;
                }
            }
        }
        Ok(r)
    }

    /// The file's clusters reach its size through valid links.
    async fn chain_ok(&mut self, e: &DirEntry) -> Result<bool, D::Error> {
        let Ok(f) = self.open_entry(e) else { return Ok(false) };
        let mut it = self.extents(&f);
        let mut bytes = 0u64;
        loop {
            match self.next_extent(&mut it).await {
                Ok(Some(x)) => bytes += u64::from(x.sectors) * 512,
                Ok(None) => return Ok(bytes >= f.size()),
                Err(Error::Io(x)) => return Err(Error::Io(x)),
                Err(_) => return Ok(false),
            }
        }
    }

    /// Remove `path` and everything below it (inside the write prefix, as
    /// every removal). `buf` holds the path being worked on (its length
    /// bounds the depth); `e` is the entry buffer. A missing `path` is
    /// nothing to do. Returns the files removed.
    pub async fn remove_tree(
        &mut self,
        path: &str,
        buf: &mut [u8],
        e: &mut DirEntry,
    ) -> Result<u32, D::Error> {
        let top = path.trim_end_matches('/').len();
        let Some(b) = buf.get_mut(..top) else { return Err(Error::NameTooLong) };
        b.copy_from_slice(&path.as_bytes()[..top]);
        let mut len = top;
        let mut files = 0u32;
        loop {
            let here = as_str(buf, len)?;
            let first = match self.open_dir(here).await {
                Ok(mut cur) => self.read_dir(&mut cur, e).await?,
                Err(Error::NotFound) if len == top => return Ok(files),
                Err(Error::NotADirectory) => {
                    // A file (only `path` itself can be one here).
                    self.remove(here).await?;
                    return Ok(files + 1);
                }
                Err(x) => return Err(x),
            };
            if first {
                let name = e.name().as_bytes();
                let end = len + 1 + name.len();
                let Some(slot) = buf.get_mut(len..end) else { return Err(Error::NameTooLong) };
                slot[0] = b'/';
                slot[1..].copy_from_slice(name);
                if e.is_dir() {
                    len = end;
                } else {
                    self.remove(as_str(buf, end)?).await?;
                    files += 1;
                }
                continue;
            }
            // Empty: remove it and go back up.
            self.remove(here).await?;
            if len == top {
                return Ok(files);
            }
            len = buf[..len].iter().rposition(|&c| c == b'/').unwrap_or(0);
        }
    }
}
