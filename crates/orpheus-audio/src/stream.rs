//! What a decoder reports and returns (04 §2.1).

use crate::probe::Format;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecId {
    #[default]
    Unknown,
    /// Integer PCM (WAV, AIFF, AU).
    Pcm,
    /// IEEE float PCM.
    FloatPcm,
    ALaw,
    MuLaw,
    /// IMA ADPCM as in WAV (`0x0011`).
    ImaAdpcm,
    /// IMA ADPCM as in AIFF-C/QuickTime (`ima4`).
    ImaQt,
    MsAdpcm,
    Mp1,
    Mp2,
    Mp3,
    Flac,
    Vorbis,
    Opus,
    Speex,
    Aac,
    Alac,
    WavPack,
    Ape,
}

/// Width of the samples a decoder hands out.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SampleKind {
    /// Interleaved `i16`.
    #[default]
    I16,
    /// Interleaved `i32`, left-justified: a 24-bit sample `s` is `s << 8`.
    I32,
}

/// Encoder delay and padding, in sample frames, as stored by the encoder
/// (LAME/Xing tag, iTunSMPB, container granule positions).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Gapless {
    pub delay: u32,
    pub padding: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct StreamInfo {
    /// Sample rate in Hz.
    pub rate: u32,
    /// Channels in the output (interleaved).
    pub channels: u8,
    /// Significant bits per sample in the source (8–32); the output width
    /// is `sample`.
    pub bits: u8,
    pub sample: SampleKind,
    /// Playable sample frames (after the gapless trims), when known.
    pub frames: Option<u64>,
    /// Frames the decoder drops at the start (encoder + decoder delay).
    /// Already applied: the first frame handed out is the first audible one.
    pub delay: u32,
    /// Frames the decoder drops at the end. Already applied.
    pub padding: u32,
    /// Average bit rate in bits per second (0 if unknown).
    pub bitrate: u32,
    pub codec: CodecId,
}

/// Decoded samples, interleaved, `channels` per frame. Mutable so DSP stages
/// can work in place before the block is pushed to the PCM ring.
#[derive(Debug)]
pub enum PcmBlock<'a> {
    I16(&'a mut [i16]),
    I32(&'a mut [i32]),
}

impl PcmBlock<'_> {
    /// Samples (all channels).
    #[must_use]
    pub fn len(&self) -> usize {
        match self {
            Self::I16(s) => s.len(),
            Self::I32(s) => s.len(),
        }
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// One answer of [`crate::Decoder::decode`].
#[derive(Debug)]
pub enum Step<'a> {
    /// A block of samples (never empty).
    Pcm(PcmBlock<'a>),
    /// Call again once `available() >= n` or the source is at its end.
    /// `n` is larger than what is buffered now and at most
    /// [`crate::MAX_PEEK`].
    NeedData(usize),
    /// The stream is finished; further calls return `EndOfStream` again.
    EndOfStream,
}

/// What the container layer knows when the decoder is opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ContainerInfo {
    /// Format found by [`crate::probe()`].
    pub format: Format,
    /// Gapless information found in tags outside the codec stream (iTunSMPB
    /// in an ID3v2 comment), used when the stream carries none itself.
    pub gapless: Option<Gapless>,
    /// Length of the codec stream when known (file length minus trailing
    /// tags), counted from the decoder's start position.
    pub stream_len: Option<u64>,
}

/// Why a stream cannot be played. Reported to the user (04 §1 item 4).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Unsupported {
    /// The bytes are not this decoder's format.
    NotThisFormat,
    /// A variant of the format this decoder does not implement.
    Encoding,
    Channels(u8),
    Rate(u32),
    BitDepth(u8),
    /// A frame or block larger than the decoder's limits.
    FrameTooLarge,
    /// The arena lease is too small for this stream.
    ArenaTooSmall {
        needed: usize,
        capacity: usize,
    },
    /// Header fields are inconsistent.
    BadHeader,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenError {
    /// Inspect/open again when `available() >= n` (or at end). Nothing was
    /// consumed.
    NeedData(usize),
    Unsupported(Unsupported),
}

impl From<crate::ArenaFull> for OpenError {
    fn from(e: crate::ArenaFull) -> Self {
        Self::Unsupported(Unsupported::ArenaTooSmall { needed: e.needed, capacity: e.capacity })
    }
}

impl From<Unsupported> for OpenError {
    fn from(u: Unsupported) -> Self {
        Self::Unsupported(u)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeError {
    /// A corrupt frame was skipped (at least one byte was consumed). The
    /// caller counts these; ten in a row end the track (04 §2.1).
    Corrupt,
    /// The stream changed into something this decoder cannot handle (a
    /// block larger than announced, a new channel count).
    Unsupported(Unsupported),
    /// The source failed to seek.
    Source(crate::SourceError),
}

impl From<crate::SourceError> for DecodeError {
    fn from(e: crate::SourceError) -> Self {
        Self::Source(e)
    }
}
