//! `orpheus-fs`: an allocation-free, async FAT12/16/32 and exFAT file system.
//!
//! * The block device is the [`BlockDevice`] trait (512-byte sectors,
//!   multi-sector `async fn read/write`).
//! * One [`Volume`] owns the device and a caller-sized metadata cache
//!   ([`CacheSlot`]s). Files ([`File`]) and directory cursors ([`DirCursor`])
//!   are small plain values that do not borrow the volume, so a task can keep
//!   them across `.await`s while the volume sits behind an async mutex and is
//!   locked for one operation at a time.
//! * File data moves directly between the device and the caller's buffer
//!   (multi-sector transfers over contiguous cluster runs); only partial
//!   sectors and metadata pass through the cache.
//! * Directories are iterated, never loaded; a cursor can be stored and
//!   resumed, or rebuilt from an entry index ([`Locator`]).
//! * [`Volume::extents`] maps a file to its `(LBA, sectors)` runs.
//!
//! See the README for the RAM figures, the write-ordering rules and what is
//! not supported.

#![warn(missing_docs)]
#![no_std]
#![forbid(unsafe_code)]

#[cfg(any(test, feature = "std"))]
extern crate std;

mod block;
mod cache;
mod care;
mod dir;
mod file;
pub mod format;
mod tables;
pub mod unicode;
mod volume;

#[cfg(feature = "std")]
pub mod host;

pub use block::{BlockDevice, SECTOR_SIZE, Sector, SyncPoll};
pub use cache::CacheSlot;
pub use care::CheckReport;
pub use dir::{DirCursor, DirEntry, Locator, MAX_NAME_UTF8, PathWalk, WalkCache};
pub use file::{Extent, ExtentIter, File, OpenMode, SeekFrom};
pub use volume::{DateTime, FsKind, MountOptions, Volume};

/// File-system error. `E` is the block device's error type.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Error<E> {
    /// The block device failed.
    Io(E),
    /// No FAT or exFAT file system (or supported partition) was found.
    NoFilesystem,
    /// A structure on the medium is malformed (bad chain, bad entry set...).
    Corrupt,
    /// Feature not supported (sector size other than 512, GPT, TexFAT...).
    Unsupported,
    /// Path or entry not found.
    NotFound,
    /// The target name exists already.
    AlreadyExists,
    /// A path component is not a directory.
    NotADirectory,
    /// Operation needs a file but found a directory.
    IsADirectory,
    /// Directory to remove is not empty.
    DirectoryNotEmpty,
    /// Name or path is not valid (forbidden characters, `..`, empty...).
    InvalidName,
    /// Name longer than 255 UTF-16 units, or path too deep.
    NameTooLong,
    /// The volume is mounted read-only, the file is not open for writing,
    /// or the file is marked read-only.
    ReadOnly,
    /// Write outside the configured write prefix.
    OutsidePrefix,
    /// No free cluster left.
    NoSpace,
    /// FAT file would exceed 4 GiB - 1, or a directory its maximum size.
    FileTooLarge,
    /// The fixed FAT12/16 root directory is full.
    DirectoryFull,
    /// A caller-provided buffer is too small (or the cache has no slots).
    BufferTooSmall,
}

/// Result alias used throughout.
pub type Result<T, E> = core::result::Result<T, Error<E>>;
