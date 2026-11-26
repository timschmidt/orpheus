//! Audio pipeline core (docs/plan/04-audio.md §2).
//!
//! What every decoder and the playback pipeline agree on:
//!
//! * [`source`]: [`ByteSource`] — the decoder's view of the stream ring
//!   (peek across the wrap, consume, position) — and [`SeekSource`], plus
//!   [`SliceSource`], an in-memory source that can simulate the ring (wrap
//!   splits, partial fills) for host tests and fuzzing.
//! * [`arena`]: [`Arena`], the bump allocator over a caller-provided byte
//!   slice (a lease from the mode pool) from which a decoder takes all of its
//!   state and scratch; it records the high-water mark and fails cleanly when
//!   the slice is too small.
//! * [`stream`]: [`StreamInfo`], [`PcmBlock`], [`Step`], errors.
//! * [`decoder`]: the [`Decoder`] trait (inspect → open → decode/seek).
//! * [`probe`]: format detection by content (and extension as a tie-break).
//! * [`rate`]: what the DAC path does with a stream's rate and word length.
//! * [`ledger`]: [`Ledger`], underruns charged to the operation that caused
//!   them.
//! * [`pcm`]: the PCM ring's headroom and underrun accounting between the
//!   writer and the DAC's DMA.
//! * [`chore`]: when the playback loop may do other card work, and for how
//!   long (the headroom's budget, the chores' measured needs).
//! * [`synth`]: a synthesiser source beside the codecs: which files it
//!   plays, how an item starts, where its memory goes in the pool (SYN-3).
//! * [`ring`]: [`StreamRing`], the stream ring with gapless read-ahead into
//!   the next file (04 §2.2).
//! * [`bits`], [`crc`]: bit reader over a ring view, the CRCs the formats use.
//!
//! `no_std`, no `alloc`, no `unsafe`, no floating point.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod arena;
pub mod bits;
pub mod chore;
pub mod crc;
pub mod decoder;
pub mod ledger;
pub mod pcm;
pub mod probe;
pub mod rate;
pub mod ring;
pub mod source;
pub mod stream;
pub mod synth;

pub use arena::{Arena, ArenaFull};
pub use decoder::{Decoder, Plan, SeekStep};
pub use ledger::Ledger;
pub use probe::{Format, OggCodec, Probe, probe};
pub use ring::StreamRing;
pub use source::{ByteSource, SeekSource, SliceSource, SourceError, View, WindowSource};
pub use stream::{
    CodecId, ContainerInfo, DecodeError, Gapless, OpenError, PcmBlock, SampleKind, Step, StreamInfo,
    Unsupported,
};

/// The most bytes a decoder may ask the stream ring to hold at once
/// (`Step::NeedData`, `peek`). The ring must be larger than this plus one
/// read burst; the 32 KiB ring of 02 §1.3 is. A frame that would need more is
/// `Unsupported::FrameTooLarge`.
pub const MAX_PEEK: usize = 28 * 1024;
