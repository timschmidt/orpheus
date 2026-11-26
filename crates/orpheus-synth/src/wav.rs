//! WAV parsing for samples: RIFF/WAVE with PCM 8 (unsigned), 16, 24 and 32
//! bit or 32-bit float, mono or stereo, `WAVE_FORMAT_EXTENSIBLE` with a PCM
//! or float sub-format; loop points and the unity note from a `smpl` chunk.
//!
//! The parser reads through a [`Reader`] (any [`SampleSource`], so a file on
//! the card or a byte slice) and never trusts a size field: chunks are
//! bounded by the file length, the `data` chunk is cut to whole frames
//! within the file, at most [`MAX_CHUNKS`] chunks are visited, loop points
//! outside the data are dropped. It is fuzzed (`wav_parse`, with the
//! sample player behind it).

use crate::sample::{BLOCK, Reader, SampleSource, SourceError};

/// What a `smpl` chunk gives: the unity note and the first forward loop.
type Smpl = (Option<u8>, Option<(u32, u32)>);

/// Chunks visited before giving up.
pub const MAX_CHUNKS: usize = 64;

/// Sample encoding in the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Encoding {
    /// 8-bit unsigned.
    U8,
    /// 16-bit signed little-endian.
    S16,
    /// 24-bit signed little-endian.
    S24,
    /// 32-bit signed little-endian.
    S32,
    /// 32-bit IEEE float little-endian.
    F32,
}

impl Encoding {
    /// Bytes per sample.
    #[must_use]
    pub const fn bytes(self) -> u32 {
        match self {
            Self::U8 => 1,
            Self::S16 => 2,
            Self::S24 => 3,
            Self::S32 | Self::F32 => 4,
        }
    }
}

/// Why a file is not a usable sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WavError {
    /// The source failed.
    Source(SourceError),
    /// No `RIFF`…`WAVE` header.
    NotWave,
    /// No `fmt ` chunk, or one too short.
    NoFormat,
    /// No `data` chunk.
    NoData,
    /// A format other than PCM 8/16/24/32 or float 32, more than two
    /// channels, a block alignment that does not match, or a sample rate
    /// outside 1–192 kHz.
    Unsupported,
}

impl From<SourceError> for WavError {
    fn from(e: SourceError) -> Self {
        Self::Source(e)
    }
}

/// What the parser found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WavInfo {
    /// Encoding.
    pub encoding: Encoding,
    /// Channels, 1 or 2.
    pub channels: u8,
    /// Sample rate, Hz.
    pub rate: u32,
    /// Bytes per frame.
    pub block_align: u32,
    /// Byte offset of the first frame.
    pub data_offset: u32,
    /// Whole frames inside the file.
    pub frames: u32,
    /// Forward loop from the `smpl` chunk: first frame and the frame after
    /// the last (both within `frames`, start < end).
    pub loop_points: Option<(u32, u32)>,
    /// MIDI unity note from the `smpl` chunk (0–127).
    pub unity_note: Option<u8>,
}

fn u16le(b: &[u8]) -> u32 {
    u32::from(u16::from_le_bytes([b[0], b[1]]))
}

fn u32le(b: &[u8]) -> u32 {
    u32::from_le_bytes([b[0], b[1], b[2], b[3]])
}

/// Read exactly `out.len()` bytes at `off`, or fail as [`WavError::NotWave`]
/// (`what` decides the error for a short read).
fn exact<S: SampleSource + ?Sized>(
    r: &mut Reader<'_, S>,
    off: u32,
    out: &mut [u8],
    short: WavError,
) -> Result<(), WavError> {
    if r.read_at(off, out)? == out.len() { Ok(()) } else { Err(short) }
}

/// Parse the WAV file behind `r`.
///
/// # Errors
///
/// [`WavError`] when the file is not a WAV sample this player can use.
pub fn parse<S: SampleSource + ?Sized>(r: &mut Reader<'_, S>) -> Result<WavInfo, WavError> {
    let len = r.len();
    let mut head = [0u8; 12];
    exact(r, 0, &mut head, WavError::NotWave)?;
    if &head[0..4] != b"RIFF" || &head[8..12] != b"WAVE" {
        return Err(WavError::NotWave);
    }
    let mut fmt: Option<(u32, u32, u32, u32, u32)> = None; // tag, channels, rate, align, bits
    let mut data: Option<(u32, u32)> = None;
    // (unity note, first loop)
    let mut smpl: Option<Smpl> = None;
    let mut off: u32 = 12;
    for _ in 0..MAX_CHUNKS {
        let Some(body) = off.checked_add(8) else { break };
        if body > len {
            break;
        }
        let mut h = [0u8; 8];
        exact(r, off, &mut h, WavError::NotWave)?;
        let size = u32le(&h[4..8]);
        let avail = len - body;
        let size_in = size.min(avail);
        match &h[0..4] {
            b"fmt " if fmt.is_none() => {
                if size_in < 16 {
                    return Err(WavError::NoFormat);
                }
                let mut f = [0u8; 40];
                let n = (size_in as usize).min(40);
                exact(r, body, &mut f[..n], WavError::NoFormat)?;
                let mut tag = u16le(&f[0..2]);
                if tag == 0xFFFE {
                    // WAVE_FORMAT_EXTENSIBLE: the sub-format GUID starts
                    // with the format tag.
                    if n < 26 {
                        return Err(WavError::Unsupported);
                    }
                    tag = u16le(&f[24..26]);
                }
                fmt = Some((tag, u16le(&f[2..4]), u32le(&f[4..8]), u16le(&f[12..14]), u16le(&f[14..16])));
            }
            b"data" if data.is_none() => data = Some((body, size_in)),
            b"smpl" if smpl.is_none() && size_in >= 36 => {
                let mut s = [0u8; 60];
                let n = (size_in as usize).min(60);
                exact(r, body, &mut s[..n], WavError::NotWave)?;
                let unity = u32le(&s[12..16]);
                let unity = if unity <= 127 { Some(unity as u8) } else { None };
                let loops = u32le(&s[28..32]);
                // The first loop record: cue id, type, start, end (inclusive).
                let lp = if loops > 0 && n >= 60 && u32le(&s[40..44]) == 0 {
                    Some((u32le(&s[44..48]), u32le(&s[48..52])))
                } else {
                    None
                };
                smpl = Some((unity, lp));
            }
            _ => {}
        }
        if size > avail {
            break;
        }
        // Chunks are padded to an even length.
        let Some(next) = body.checked_add(size).and_then(|x| x.checked_add(size & 1)) else { break };
        off = next;
    }
    let (tag, channels, rate, align, bits) = fmt.ok_or(WavError::NoFormat)?;
    let (data_offset, data_len) = data.ok_or(WavError::NoData)?;
    let encoding = match (tag, bits) {
        (1, 8) => Encoding::U8,
        (1, 16) => Encoding::S16,
        (1, 24) => Encoding::S24,
        (1, 32) => Encoding::S32,
        (3, 32) => Encoding::F32,
        _ => return Err(WavError::Unsupported),
    };
    if !(1..=2).contains(&channels)
        || align != channels * encoding.bytes()
        || !(1000..=192_000).contains(&rate)
    {
        return Err(WavError::Unsupported);
    }
    let frames = data_len / align;
    let (unity_note, lp) = smpl.unwrap_or((None, None));
    let loop_points = lp.and_then(|(s, e)| {
        let end = e.checked_add(1)?;
        (s < end && end <= frames).then_some((s, end))
    });
    Ok(WavInfo {
        encoding,
        channels: channels as u8,
        rate,
        block_align: align,
        data_offset,
        frames,
        loop_points,
        unity_note,
    })
}

/// Bytes of the conversion buffer: a multiple of every block alignment
/// (1, 2, 3, 4, 6, 8).
const CHUNK: usize = 384;
const _: () = assert!(CHUNK <= BLOCK);

/// One frame of `b` (`align` bytes) as a mono 16-bit sample (stereo is the
/// mean of the channels).
fn frame(info: &WavInfo, b: &[u8]) -> i16 {
    let one = |c: &[u8]| -> i32 {
        match info.encoding {
            Encoding::U8 => (i32::from(c[0]) - 128) << 8,
            Encoding::S16 => i32::from(i16::from_le_bytes([c[0], c[1]])),
            Encoding::S24 => i32::from_le_bytes([0, c[0], c[1], c[2]]) >> 16,
            Encoding::S32 => i32::from_le_bytes([c[0], c[1], c[2], c[3]]) >> 16,
            Encoding::F32 => {
                let x = f32::from_le_bytes([c[0], c[1], c[2], c[3]]);
                i32::from(crate::math::to_i16(crate::math::clampf(x, -1.0, 1.0)))
            }
        }
    };
    let n = info.encoding.bytes() as usize;
    if info.channels == 2 { ((one(&b[..n]) + one(&b[n..2 * n])) >> 1) as i16 } else { one(&b[..n]) as i16 }
}

/// Read frames `first..first + out.len()` (clipped to the data) as mono
/// 16-bit samples; returns how many were written.
///
/// # Errors
///
/// [`WavError::Source`] when the source fails.
pub fn read_frames<S: SampleSource + ?Sized>(
    r: &mut Reader<'_, S>,
    info: &WavInfo,
    first: u32,
    out: &mut [i16],
) -> Result<usize, WavError> {
    let align = info.block_align;
    let total = (info.frames.saturating_sub(first) as usize).min(out.len());
    let per = CHUNK / align as usize;
    let mut buf = [0u8; CHUNK];
    let mut done = 0;
    while done < total {
        let n = per.min(total - done);
        let off = info.data_offset + (first + done as u32) * align;
        let got = r.read_at(off, &mut buf[..n * align as usize])? / align as usize;
        for (o, b) in out[done..done + got].iter_mut().zip(buf.chunks_exact(align as usize)) {
            *o = frame(info, b);
        }
        done += got;
        if got < n {
            break;
        }
    }
    Ok(done)
}
