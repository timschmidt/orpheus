//! Synchronous byte sources for the streaming parser: whatever can read a
//! few bytes at an offset (a file through a sector cache, a slice).
//!
//! The parser itself only answers [`crate::smf::Pull::Need`]; the
//! `*_from` drivers of [`crate::player::MidiPlayer`] and
//! [`crate::player::summary`] read through a `MidiSource`. An `async`
//! caller (the player's card task) answers the `Need`s itself.

use crate::smf::MidiError;

/// Random-access reads.
pub trait MidiSource {
    /// File length, bytes.
    fn len(&self) -> u32;

    /// Whether the file is empty.
    fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read up to `buf.len()` bytes at `offset`; returns how many (fewer
    /// only at the end of the file).
    fn read_at(&mut self, offset: u32, buf: &mut [u8]) -> Result<usize, MidiError>;
}

/// A file in memory.
#[derive(Clone, Copy, Debug)]
pub struct SliceSource<'a> {
    data: &'a [u8],
    reads: u32,
}

impl<'a> SliceSource<'a> {
    /// The source over `data` (files over 4 GiB are cut).
    #[must_use]
    pub fn new(data: &'a [u8]) -> Self {
        Self { data: &data[..data.len().min(u32::MAX as usize)], reads: 0 }
    }

    /// Reads served so far.
    #[must_use]
    pub fn reads(&self) -> u32 {
        self.reads
    }
}

impl MidiSource for SliceSource<'_> {
    fn len(&self) -> u32 {
        self.data.len() as u32
    }

    fn read_at(&mut self, offset: u32, buf: &mut [u8]) -> Result<usize, MidiError> {
        self.reads += 1;
        let at = (offset as usize).min(self.data.len());
        let n = buf.len().min(self.data.len() - at);
        buf[..n].copy_from_slice(&self.data[at..at + n]);
        Ok(n)
    }
}
