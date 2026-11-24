//! Decoder selection (04 §2.1): an enum over the codecs this build
//! contains, chosen from the probed [`Format`], with one `match` per call.
//! Cargo features select the codecs (`wav`, `mp3`, `flac`, `vorbis`,
//! `opus`, `speex`).

#![no_std]
#![forbid(unsafe_code)]

use orpheus_audio::{
    ByteSource, ContainerInfo, DecodeError, Decoder, Format, OggCodec, OpenError, Plan, SeekSource, SeekStep,
    Step, StreamInfo, Unsupported,
};

#[cfg(feature = "flac")]
pub use orpheus_codec_flac::FlacDecoder;
#[cfg(feature = "mp3")]
pub use orpheus_codec_mp3::Mp3Decoder;
#[cfg(feature = "opus")]
pub use orpheus_codec_opus::OpusDecoder;
#[cfg(feature = "speex")]
pub use orpheus_codec_speex::SpeexDecoder;
#[cfg(feature = "vorbis")]
pub use orpheus_codec_vorbis::VorbisDecoder;
#[cfg(feature = "wav")]
pub use orpheus_codec_wav::WavDecoder;

/// One open decoder of any built codec.
///
/// The Ogg decoders carry their page reader (a lacing table, about 300
/// bytes; Vorbis keeps two) in the value; the enum lives in the decode
/// task's state and is not moved per frame, so the size difference between
/// variants does not matter.
#[allow(clippy::large_enum_variant)]
pub enum AnyDecoder<'a> {
    #[cfg(feature = "wav")]
    Wav(WavDecoder<'a>),
    #[cfg(feature = "mp3")]
    Mp3(Mp3Decoder<'a>),
    #[cfg(feature = "flac")]
    Flac(FlacDecoder<'a>),
    #[cfg(feature = "vorbis")]
    Vorbis(VorbisDecoder<'a>),
    #[cfg(feature = "opus")]
    Opus(OpusDecoder<'a>),
    #[cfg(feature = "speex")]
    Speex(SpeexDecoder<'a>),
}

#[cfg(not(any(
    feature = "wav",
    feature = "mp3",
    feature = "flac",
    feature = "vorbis",
    feature = "opus",
    feature = "speex"
)))]
compile_error!("enable at least one codec feature");

macro_rules! each {
    ($self:expr, $d:ident => $e:expr) => {
        match $self {
            #[cfg(feature = "wav")]
            AnyDecoder::Wav($d) => $e,
            #[cfg(feature = "mp3")]
            AnyDecoder::Mp3($d) => $e,
            #[cfg(feature = "flac")]
            AnyDecoder::Flac($d) => $e,
            #[cfg(feature = "vorbis")]
            AnyDecoder::Vorbis($d) => $e,
            #[cfg(feature = "opus")]
            AnyDecoder::Opus($d) => $e,
            #[cfg(feature = "speex")]
            AnyDecoder::Speex($d) => $e,
        }
    };
}

fn unsupported() -> OpenError {
    OpenError::Unsupported(Unsupported::NotThisFormat)
}

/// Inspect the stream with the decoder for `format`.
pub fn inspect(format: Format, src: &dyn ByteSource, hint: &ContainerInfo) -> Result<Plan, OpenError> {
    match format {
        #[cfg(feature = "wav")]
        Format::Wav | Format::Aiff | Format::Au => WavDecoder::inspect(src, hint),
        #[cfg(feature = "mp3")]
        Format::Mpeg => Mp3Decoder::inspect(src, hint),
        #[cfg(feature = "flac")]
        Format::Flac => FlacDecoder::inspect(src, hint),
        #[cfg(feature = "vorbis")]
        Format::Ogg(OggCodec::Vorbis) => VorbisDecoder::inspect(src, hint),
        #[cfg(feature = "opus")]
        Format::Ogg(OggCodec::Opus) => OpusDecoder::inspect(src, hint),
        #[cfg(feature = "speex")]
        Format::Ogg(OggCodec::Speex) => SpeexDecoder::inspect(src, hint),
        _ => {
            let _ = (src, hint);
            Err(unsupported())
        }
    }
}

/// Open the decoder for `format` in `arena`.
pub fn open<'a>(
    format: Format,
    src: &mut dyn ByteSource,
    arena: &'a mut [u8],
    hint: &ContainerInfo,
) -> Result<(AnyDecoder<'a>, StreamInfo), OpenError> {
    // One function per codec (not inlined here): the firmware packs the
    // ones a gapless join opens (MP3, FLAC, WAV) together with the join's
    // other code (`hot.toml`), without the Ogg codecs' set-up.
    match format {
        #[cfg(feature = "wav")]
        Format::Wav | Format::Aiff | Format::Au => open_wav(src, arena, hint),
        #[cfg(feature = "mp3")]
        Format::Mpeg => open_mp3(src, arena, hint),
        #[cfg(feature = "flac")]
        Format::Flac => open_flac(src, arena, hint),
        #[cfg(feature = "vorbis")]
        Format::Ogg(OggCodec::Vorbis) => open_vorbis(src, arena, hint),
        #[cfg(feature = "opus")]
        Format::Ogg(OggCodec::Opus) => open_opus(src, arena, hint),
        #[cfg(feature = "speex")]
        Format::Ogg(OggCodec::Speex) => open_speex(src, arena, hint),
        _ => {
            let _ = (src, arena, hint);
            Err(unsupported())
        }
    }
}

type Opened<'a> = Result<(AnyDecoder<'a>, StreamInfo), OpenError>;

#[cfg(feature = "wav")]
#[inline(never)]
fn open_wav<'a>(src: &mut dyn ByteSource, arena: &'a mut [u8], hint: &ContainerInfo) -> Opened<'a> {
    WavDecoder::open(src, arena, hint).map(|(d, i)| (AnyDecoder::Wav(d), i))
}

#[cfg(feature = "mp3")]
#[inline(never)]
fn open_mp3<'a>(src: &mut dyn ByteSource, arena: &'a mut [u8], hint: &ContainerInfo) -> Opened<'a> {
    Mp3Decoder::open(src, arena, hint).map(|(d, i)| (AnyDecoder::Mp3(d), i))
}

#[cfg(feature = "flac")]
#[inline(never)]
fn open_flac<'a>(src: &mut dyn ByteSource, arena: &'a mut [u8], hint: &ContainerInfo) -> Opened<'a> {
    FlacDecoder::open(src, arena, hint).map(|(d, i)| (AnyDecoder::Flac(d), i))
}

#[cfg(feature = "vorbis")]
#[inline(never)]
fn open_vorbis<'a>(src: &mut dyn ByteSource, arena: &'a mut [u8], hint: &ContainerInfo) -> Opened<'a> {
    VorbisDecoder::open(src, arena, hint).map(|(d, i)| (AnyDecoder::Vorbis(d), i))
}

#[cfg(feature = "opus")]
#[inline(never)]
fn open_opus<'a>(src: &mut dyn ByteSource, arena: &'a mut [u8], hint: &ContainerInfo) -> Opened<'a> {
    OpusDecoder::open(src, arena, hint).map(|(d, i)| (AnyDecoder::Opus(d), i))
}

#[cfg(feature = "speex")]
#[inline(never)]
fn open_speex<'a>(src: &mut dyn ByteSource, arena: &'a mut [u8], hint: &ContainerInfo) -> Opened<'a> {
    SpeexDecoder::open(src, arena, hint).map(|(d, i)| (AnyDecoder::Speex(d), i))
}

impl AnyDecoder<'_> {
    pub fn decode(&mut self, src: &mut dyn ByteSource) -> Result<Step<'_>, DecodeError> {
        each!(self, d => d.decode(src))
    }

    pub fn seek(&mut self, src: &mut dyn SeekSource, frame: u64) -> Result<SeekStep, DecodeError> {
        each!(self, d => d.seek(src, frame))
    }

    #[must_use]
    pub fn arena_used(&self) -> usize {
        each!(self, d => d.arena_used())
    }
}
