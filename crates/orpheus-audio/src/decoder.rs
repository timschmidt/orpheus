//! The decoder interface (04 §2.1).
//!
//! Life of a stream:
//!
//! 1. [`Decoder::inspect`] looks at the head of the stream without consuming
//!    anything and returns a [`Plan`]: the [`StreamInfo`] and the arena bytes
//!    this stream needs. It answers `OpenError::NeedData(n)` until enough of
//!    the header is buffered (never more than [`crate::MAX_PEEK`]). The
//!    pipeline leases `plan.arena` bytes from the mode pool (the rest stays
//!    lendable) — or refuses the file when the lease cannot be had.
//! 2. [`Decoder::open`] re-reads the same header, takes its buffers from the
//!    arena and consumes the header bytes. It does not need more data than
//!    `inspect` did.
//! 3. [`Decoder::decode`] is called once per frame from the decode task:
//!    `Pcm` (push it), `NeedData(n)` (wait for `n` bytes or the end of the
//!    file, then call again), `EndOfStream`, or `Err(Corrupt)` (a bad frame was
//!    skipped; count it). Decoders are restartable at frame boundaries: a call
//!    that returns `NeedData` has consumed nothing it will need again.
//! 4. [`Decoder::seek`] repositions to a sample frame (in the trimmed,
//!    playable time line) and is called again while it answers
//!    `SeekStep::NeedData`. After `Done(f)` the next PCM is frame `f`.
//!
//! The decoder applies gapless trims itself: the first sample handed out is
//! the first audible one and the last is the last audible one.
//!
//! Progress guarantee (checked by the conformance harness with an iteration
//! budget): every call returns PCM, consumes at least one byte, asks for more
//! bytes than are buffered, or ends the stream.

use crate::source::{ByteSource, SeekSource};
use crate::stream::{ContainerInfo, DecodeError, OpenError, Step, StreamInfo};

/// Result of [`Decoder::inspect`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Plan {
    pub info: StreamInfo,
    /// Arena bytes `open` will take for this stream (≥ `ARENA_MIN`).
    pub arena: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SeekStep {
    /// Positioned: the next PCM block starts at this frame.
    Done(u64),
    /// Call `seek` again (with the same target) once `available() >= n` or
    /// the source is at its end.
    NeedData(usize),
}

/// A codec. `'a` is the lifetime of the arena lease.
pub trait Decoder<'a>: Sized {
    /// The smallest arena any stream of this codec opens with. Streams may
    /// need more (`Plan::arena`).
    const ARENA_MIN: usize;

    /// Parse the stream header without consuming anything.
    fn inspect(src: &dyn ByteSource, hint: &ContainerInfo) -> Result<Plan, OpenError>;

    /// Open the stream: take buffers from `arena`, consume the header.
    fn open(
        src: &mut dyn ByteSource,
        arena: &'a mut [u8],
        hint: &ContainerInfo,
    ) -> Result<(Self, StreamInfo), OpenError>;

    /// Decode the next frame (or the next part of a large decoded block).
    fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError>;

    /// Reposition to sample frame `frame` of the playable time line.
    fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError>;

    /// Arena bytes in use (the high-water mark: decoders allocate only at
    /// open).
    fn arena_used(&self) -> usize;
}
