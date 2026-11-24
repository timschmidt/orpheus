//! One interface over the three formats: samples go into the encoder's
//! block buffer ([`Encoder::push`]); a full block is encoded into the
//! caller's buffer ([`Encoder::encode`]); the header can be rewritten at
//! any time with the counts so far ([`Encoder::header`]) and has the same
//! length throughout.

use crate::adpcm::AdpcmEncoder;
use crate::flac::{self, FlacEncoder};
use crate::wav::{self, Coding, Cue};

/// Largest block in frames (FLAC's; PCM uses the same, ADPCM ≤ 1017).
pub const MAX_FRAMES: usize = flac::BLOCK;
/// Largest encoded block in bytes (a verbatim stereo FLAC frame).
pub const MAX_BLOCK_BYTES: usize = flac::max_frame_bytes(2);
/// Largest header.
pub const MAX_HEADER: usize =
    if wav::MAX_HEADER > flac::HEADER_LEN { wav::MAX_HEADER } else { flac::HEADER_LEN };

/// Recording formats (05 §7.5: the ones built).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Format {
    /// WAV, 16-bit PCM.
    #[default]
    Wav,
    /// WAV, IMA ADPCM (4 bits per sample).
    Adpcm,
    /// FLAC, lossless.
    Flac,
}

impl Format {
    /// File extension.
    #[must_use]
    pub const fn ext(self) -> &'static str {
        match self {
            Format::Wav | Format::Adpcm => "wav",
            Format::Flac => "flac",
        }
    }

    /// Developer-command word.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Format::Wav => "wav",
            Format::Adpcm => "adpcm",
            Format::Flac => "flac",
        }
    }

    #[must_use]
    pub fn parse(s: &str) -> Option<Format> {
        match s {
            "wav" => Some(Format::Wav),
            "adpcm" => Some(Format::Adpcm),
            "flac" => Some(Format::Flac),
            _ => None,
        }
    }

    /// Settings › Recording › Format index (WAV, WAV ADPCM, FLAC).
    #[must_use]
    pub const fn from_index(i: i16) -> Format {
        match i {
            1 => Format::Adpcm,
            2 => Format::Flac,
            _ => Format::Wav,
        }
    }
}

/// What to record.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Params {
    pub rate: u32,
    /// 1 or 2.
    pub channels: u8,
    pub format: Format,
    /// FLAC level 0–2.
    pub level: u8,
}

impl Params {
    /// Bytes per second of the encoded stream, for the remaining-time
    /// estimate (FLAC: assumed 60 % of PCM, 04 §5.2).
    #[must_use]
    pub const fn bytes_per_second(&self) -> u32 {
        let pcm = self.rate * self.channels as u32 * 2;
        match self.format {
            Format::Wav => pcm,
            Format::Adpcm => pcm / 4 + pcm / 64,
            Format::Flac => pcm / 10 * 6,
        }
    }
}

#[derive(Clone, Debug)]
enum Kind {
    Pcm,
    Adpcm(AdpcmEncoder),
    Flac(FlacEncoder),
}

/// Samples the block buffer must hold ([`MAX_FRAMES`] stereo frames).
pub const BUF_SAMPLES: usize = MAX_FRAMES * 2;

/// An encoder over its block buffer `B` (at least [`BUF_SAMPLES`]
/// samples: a `Vec`, a boxed slice, or on the device a slice of the card
/// task's arena; nothing large is ever built on the stack).
#[derive(Clone)]
pub struct Encoder<B> {
    p: Params,
    kind: Kind,
    buf: B,
    /// Samples in `buf`.
    fill: usize,
    /// Frames encoded (without the padding of a last ADPCM block).
    frames: u64,
    /// Encoded bytes after the header.
    bytes: u64,
}

impl<B> core::fmt::Debug for Encoder<B> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Encoder").field("p", &self.p).field("frames", &self.frames).finish_non_exhaustive()
    }
}

impl<B: AsRef<[i16]> + AsMut<[i16]>> Encoder<B> {
    #[must_use]
    pub fn new(p: Params, buf: B) -> Self {
        let p = Params { channels: p.channels.clamp(1, 2), ..p };
        let kind = match p.format {
            Format::Wav => Kind::Pcm,
            Format::Adpcm => Kind::Adpcm(AdpcmEncoder::new(p.rate, p.channels)),
            Format::Flac => Kind::Flac(FlacEncoder::new(p.rate, p.channels, p.level)),
        };
        Encoder { p, kind, buf, fill: 0, frames: 0, bytes: 0 }
    }

    #[must_use]
    pub fn params(&self) -> Params {
        self.p
    }

    /// Frames per encoded block.
    #[must_use]
    pub fn block_frames(&self) -> usize {
        match &self.kind {
            Kind::Pcm | Kind::Flac(_) => MAX_FRAMES,
            Kind::Adpcm(a) => a.frames_per_block(),
        }
    }

    /// Largest encoded block of this encoder (PCM and ADPCM: exactly a
    /// block; FLAC: a verbatim frame).
    #[must_use]
    pub fn max_block_bytes(&self) -> usize {
        match &self.kind {
            Kind::Pcm => self.block_samples() * 2,
            Kind::Adpcm(a) => a.block_align(),
            Kind::Flac(_) => flac::max_frame_bytes(self.p.channels),
        }
    }

    fn block_samples(&self) -> usize {
        (self.block_frames() * usize::from(self.p.channels)).min(self.buf.as_ref().len())
    }

    fn coding(&self) -> Coding {
        match &self.kind {
            Kind::Adpcm(a) => Coding::Ima { block_align: a.block_align() as u16 },
            _ => Coding::Pcm16,
        }
    }

    #[must_use]
    pub fn header_len(&self) -> usize {
        match self.kind {
            Kind::Flac(_) => flac::HEADER_LEN,
            _ => wav::header_len(self.coding()),
        }
    }

    /// Frames encoded so far.
    #[must_use]
    pub fn frames(&self) -> u64 {
        self.frames
    }

    /// Bytes encoded so far after the header.
    #[must_use]
    pub fn bytes(&self) -> u64 {
        self.bytes
    }

    /// Frames waiting in the block buffer.
    #[must_use]
    pub fn buffered(&self) -> usize {
        self.fill / usize::from(self.p.channels)
    }

    /// The header with the counts so far and `trailer` bytes of closing
    /// chunks after the audio (0 while recording). `out` holds
    /// [`MAX_HEADER`]; returns [`Self::header_len`].
    pub fn header(&self, out: &mut [u8], trailer: u32) -> usize {
        match &self.kind {
            Kind::Flac(f) => {
                if let Some(h) = out.first_chunk_mut::<{ flac::HEADER_LEN }>() {
                    f.header(h);
                }
                flac::HEADER_LEN
            }
            _ => wav::header(
                out,
                self.coding(),
                self.p.rate,
                self.p.channels,
                u32::try_from(self.bytes).unwrap_or(u32::MAX),
                u32::try_from(self.frames).unwrap_or(u32::MAX),
                trailer,
            ),
        }
    }

    /// The header as it should read while only `on_card` bytes of the
    /// file (header included) are on the card: a WAV header then claims
    /// only the whole blocks there, so a file cut by a power loss reads
    /// cleanly. FLAC: the encoder's counts (decoders stop at a cut frame).
    pub fn header_on_card(&self, out: &mut [u8], on_card: u64) -> usize {
        let hl = self.header_len() as u64;
        let data = on_card.saturating_sub(hl).min(self.bytes);
        let ch = u64::from(self.p.channels);
        let (bytes, frames) = match &self.kind {
            Kind::Flac(_) => return self.header(out, 0),
            Kind::Pcm => {
                let b = data / (2 * ch) * (2 * ch);
                (b, b / (2 * ch))
            }
            Kind::Adpcm(a) => {
                let blocks = data / a.block_align() as u64;
                (blocks * a.block_align() as u64, (blocks * a.frames_per_block() as u64).min(self.frames))
            }
        };
        wav::header(
            out,
            self.coding(),
            self.p.rate,
            self.p.channels,
            u32::try_from(bytes).unwrap_or(u32::MAX),
            u32::try_from(frames).unwrap_or(u32::MAX),
            0,
        )
    }

    /// Take samples (interleaved) into the block buffer until it is full;
    /// returns how many were taken.
    pub fn push(&mut self, pcm: &[i16]) -> usize {
        let room = self.block_samples() - self.fill;
        let n = room.min(pcm.len());
        if let Some(d) = self.buf.as_mut().get_mut(self.fill..self.fill + n) {
            d.copy_from_slice(&pcm[..n]);
        }
        self.fill += n;
        n
    }

    /// A whole block is waiting.
    #[must_use]
    pub fn ready(&self) -> bool {
        self.fill >= self.block_samples()
    }

    /// Encode the waiting block, full or (at the end) partial, into `out`
    /// ([`MAX_BLOCK_BYTES`]); returns the bytes (0 if nothing waits).
    pub fn encode(&mut self, out: &mut [u8]) -> usize {
        if self.fill == 0 {
            return 0;
        }
        let pcm = self.buf.as_ref().get(..self.fill).unwrap_or(&[]);
        let frames = self.fill / usize::from(self.p.channels);
        let n = match &mut self.kind {
            Kind::Pcm => {
                let mut o = 0;
                for (s, d) in pcm.iter().zip(out.chunks_exact_mut(2)) {
                    d.copy_from_slice(&s.to_le_bytes());
                    o += 2;
                }
                o
            }
            Kind::Adpcm(a) => a.encode(pcm, out),
            Kind::Flac(f) => f.encode(pcm, out),
        };
        self.fill = 0;
        self.frames += frames as u64;
        self.bytes += n as u64;
        n
    }

    /// Drop the waiting block unencoded (the write-behind ring has no room
    /// for it); returns its frames. The stream stays valid: FLAC frame
    /// numbers stay contiguous, an ADPCM block starts from its own first
    /// sample.
    pub fn discard(&mut self) -> usize {
        let frames = self.fill / usize::from(self.p.channels);
        self.fill = 0;
        frames
    }

    /// Bytes of the closing chunks for `cues` (WAV only; FLAC has none).
    #[must_use]
    pub fn trailer_len(&self, cues: &[Cue]) -> usize {
        match self.kind {
            Kind::Flac(_) => 0,
            _ => wav::trailer_len(cues),
        }
    }

    /// Write the closing chunks; returns [`Self::trailer_len`].
    pub fn trailer(&self, out: &mut [u8], cues: &[Cue]) -> usize {
        match self.kind {
            Kind::Flac(_) => 0,
            _ => wav::trailer(out, cues),
        }
    }
}
