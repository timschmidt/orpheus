//! Container headers: RIFF/WAVE, AIFF/AIFF-C, AU.
//!
//! The parser walks chunks inside the peeked window only. It needs the
//! format chunk (`fmt `/`COMM`); the data chunk may lie beyond the window
//! (large `LIST`/`id3 `/picture chunks before the audio), in which case the
//! decoder walks the remaining chunks itself, consuming as it goes.

use orpheus_audio::{MAX_PEEK, OpenError, Unsupported, View};

use crate::conv::Enc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Kind {
    Wav,
    Aiff,
    Au,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Layout {
    pub kind: Kind,
    pub enc: Enc,
    pub channels: u8,
    pub rate: u32,
    /// Significant bits per sample.
    pub bits: u8,
    /// Bytes per block (one frame for plain PCM).
    pub block: u32,
    /// Frames per full block.
    pub spb: u32,
    /// Where the audio starts relative to the parse origin, and its length
    /// (`None`: until the end of the stream), if found inside the window.
    pub data: Option<(u64, Option<u64>)>,
    /// Where chunk walking continues when `data` is `None`.
    pub resume: u64,
    /// Frame count stated by the header (AIFF `COMM`, WAV `fact`).
    pub frames: Option<u64>,
}

fn bad() -> OpenError {
    OpenError::Unsupported(Unsupported::BadHeader)
}

fn need(n: usize) -> OpenError {
    if n > MAX_PEEK { bad() } else { OpenError::NeedData(n) }
}

/// Parse the header at the start of `v`. `end`: no more data will come.
pub(crate) fn parse(v: &View<'_>, end: bool) -> Result<Layout, OpenError> {
    if v.len() < 12 {
        return Err(if end { bad() } else { need(12) });
    }
    if v.starts_with_at(0, b"RIFF") && v.starts_with_at(8, b"WAVE") {
        wav(v, end)
    } else if v.starts_with_at(0, b"FORM") && (v.starts_with_at(8, b"AIFF") || v.starts_with_at(8, b"AIFC")) {
        aiff(v, end, v.starts_with_at(8, b"AIFC"))
    } else if v.starts_with_at(0, b".snd") {
        au(v, end)
    } else if v.starts_with_at(0, b"RF64") {
        Err(OpenError::Unsupported(Unsupported::Encoding))
    } else {
        Err(OpenError::Unsupported(Unsupported::NotThisFormat))
    }
}

/// Validate the common numbers and derive the block geometry.
fn finish(mut l: Layout) -> Result<Layout, OpenError> {
    if l.channels == 0 || l.channels > 8 {
        return Err(OpenError::Unsupported(Unsupported::Channels(l.channels)));
    }
    if l.rate == 0 || l.rate > 768_000 {
        return Err(OpenError::Unsupported(Unsupported::Rate(l.rate)));
    }
    let ch = u32::from(l.channels);
    match l.enc {
        Enc::Int { bytes, .. } | Enc::Float { bytes, .. } => {
            l.block = u32::from(bytes) * ch;
            l.spb = 1;
        }
        Enc::ALaw | Enc::MuLaw => {
            l.block = ch;
            l.spb = 1;
        }
        Enc::Ima => {
            if l.block < 8 * ch || l.block > 16 * 1024 {
                return Err(bad());
            }
            l.spb = 1 + (l.block - 4 * ch) / (4 * ch) * 8;
        }
        Enc::Ms => {
            if l.channels > 2 {
                return Err(OpenError::Unsupported(Unsupported::Channels(l.channels)));
            }
            if l.block < 7 * ch + 1 || l.block > 16 * 1024 {
                return Err(bad());
            }
            l.spb = (l.block - 6 * ch) * 2 / ch;
        }
        Enc::ImaQt => {
            l.block = 34 * ch;
            l.spb = 64;
        }
    }
    Ok(l)
}

fn wav(v: &View<'_>, end: bool) -> Result<Layout, OpenError> {
    let mut off = 12usize;
    let mut fmt: Option<Layout> = None;
    let mut fact: Option<u64> = None;
    loop {
        if off + 8 > v.len() {
            if let Some(l) = fmt
                && (end || off + 8 > MAX_PEEK)
            {
                return finish(Layout { resume: off as u64, frames: fact.or(l.frames), ..l });
            }
            return Err(if end { bad() } else { need(off + 8) });
        }
        let id: [u8; 4] = v.array(off).ok_or_else(bad)?;
        let size = v.u32_le(off + 4).ok_or_else(bad)?;
        let body = off + 8;
        match &id {
            b"fmt " => {
                if !(16..=4096).contains(&size) {
                    return Err(bad());
                }
                let n = size as usize;
                let Some(f) = v.range(body, n) else {
                    return Err(if end { bad() } else { need(body + n) });
                };
                fmt = Some(wav_fmt(&f)?);
            }
            b"fact" if size >= 4 => {
                if let Some(n) = v.u32_le(body) {
                    fact = Some(u64::from(n));
                }
            }
            b"data" => {
                let Some(l) = fmt else { return Err(bad()) };
                let len = if size == 0 || size == u32::MAX { None } else { Some(u64::from(size)) };
                // `fact` counts samples for compressed formats only.
                let frames = if matches!(l.enc, Enc::Ima | Enc::Ms) { fact } else { None };
                return finish(Layout { data: Some((body as u64, len)), resume: body as u64, frames, ..l });
            }
            _ => {}
        }
        off = body + size as usize + (size as usize & 1);
    }
}

fn wav_fmt(f: &View<'_>) -> Result<Layout, OpenError> {
    let mut tag = f.u16_le(0).ok_or_else(bad)?;
    let channels = f.u16_le(2).ok_or_else(bad)?;
    let rate = f.u32_le(4).ok_or_else(bad)?;
    let block = u32::from(f.u16_le(12).ok_or_else(bad)?);
    let mut bits = f.u16_le(14).ok_or_else(bad)?;
    if tag == 0xFFFE {
        // WAVE_FORMAT_EXTENSIBLE: valid bits, channel mask, sub-format GUID.
        let valid = f.u16_le(18).ok_or_else(bad)?;
        tag = f.u16_le(24).ok_or_else(bad)?;
        if valid != 0 && valid <= bits {
            bits = valid;
        }
    }
    let channels = u8::try_from(channels).map_err(|_| OpenError::Unsupported(Unsupported::Channels(255)))?;
    let ch = u32::from(channels.max(1));
    let enc = match tag {
        1 => {
            let bytes = block / ch;
            if bytes == 0 || bytes > 4 || block != bytes * ch || bits == 0 || u32::from(bits) > bytes * 8 {
                return Err(OpenError::Unsupported(Unsupported::BitDepth(bits as u8)));
            }
            Enc::Int { bytes: bytes as u8, unsigned: bytes == 1, be: false }
        }
        3 => match bits {
            32 => Enc::Float { bytes: 4, be: false },
            64 => Enc::Float { bytes: 8, be: false },
            _ => return Err(OpenError::Unsupported(Unsupported::BitDepth(bits as u8))),
        },
        6 => Enc::ALaw,
        7 => Enc::MuLaw,
        0x11 if bits == 4 => Enc::Ima,
        2 if bits == 4 => Enc::Ms,
        _ => return Err(OpenError::Unsupported(Unsupported::Encoding)),
    };
    let bits = match enc {
        Enc::Int { .. } => bits as u8,
        Enc::Float { .. } => 32,
        _ => 16,
    };
    Ok(Layout {
        kind: Kind::Wav,
        enc,
        channels,
        rate,
        bits,
        block,
        spb: 1,
        data: None,
        resume: 0,
        frames: None,
    })
}

/// 80-bit IEEE extended (AIFF sample rate) → integer Hz (truncated).
fn ext80(b: [u8; 10]) -> u32 {
    let exp = i32::from(u16::from_be_bytes([b[0], b[1]]) & 0x7FFF) - 16383;
    let mant = u64::from_be_bytes([b[2], b[3], b[4], b[5], b[6], b[7], b[8], b[9]]);
    if b[0] & 0x80 != 0 || !(0..32).contains(&exp) {
        return 0;
    }
    (mant >> (63 - exp)) as u32
}

fn aiff(v: &View<'_>, end: bool, aifc: bool) -> Result<Layout, OpenError> {
    let mut off = 12usize;
    let mut comm: Option<Layout> = None;
    let mut ssnd: Option<(u64, Option<u64>)> = None;
    loop {
        if off + 8 > v.len() {
            if let Some(l) = comm
                && (end || off + 8 > MAX_PEEK)
            {
                return finish(Layout { resume: off as u64, ..l });
            }
            return Err(if end { bad() } else { need(off + 8) });
        }
        let id: [u8; 4] = v.array(off).ok_or_else(bad)?;
        let size = v.u32_be(off + 4).ok_or_else(bad)?;
        let body = off + 8;
        match &id {
            b"COMM" => {
                let n = (size as usize).min(512);
                if n < 18 {
                    return Err(bad());
                }
                let Some(c) = v.range(body, n) else {
                    return Err(if end { bad() } else { need(body + n) });
                };
                comm = Some(aiff_comm(&c, aifc)?);
            }
            b"SSND" => {
                let Some(skip) = v.u32_be(body) else {
                    return Err(if end { bad() } else { need(body + 8) });
                };
                let start = body as u64 + 8 + u64::from(skip);
                let len = u64::from(size).checked_sub(8 + u64::from(skip));
                ssnd = Some((start, len));
            }
            _ => {}
        }
        if let (Some(l), Some(d)) = (comm, ssnd) {
            return finish(Layout { data: Some(d), resume: d.0, ..l });
        }
        // COMM after the sound data (rare): walk on to find it.
        off = body + size as usize + (size as usize & 1);
    }
}

fn aiff_comm(c: &View<'_>, aifc: bool) -> Result<Layout, OpenError> {
    let channels = c.u16_be(0).ok_or_else(bad)?;
    let frames = c.u32_be(2).ok_or_else(bad)?;
    let bits = c.u16_be(6).ok_or_else(bad)?;
    let rate = ext80(c.array(8).ok_or_else(bad)?);
    let comp: [u8; 4] = if aifc { c.array(18).ok_or_else(bad)? } else { *b"NONE" };
    let channels = u8::try_from(channels).map_err(|_| OpenError::Unsupported(Unsupported::Channels(255)))?;
    let int = |be: bool| -> Result<Enc, OpenError> {
        let bytes = bits.div_ceil(8);
        if bytes == 0 || bytes > 4 {
            return Err(OpenError::Unsupported(Unsupported::BitDepth(bits as u8)));
        }
        Ok(Enc::Int { bytes: bytes as u8, unsigned: false, be })
    };
    let enc = match &comp {
        b"NONE" | b"twos" => int(true)?,
        b"sowt" => int(false)?,
        b"raw " if bits == 8 => Enc::Int { bytes: 1, unsigned: true, be: true },
        b"fl32" | b"FL32" => Enc::Float { bytes: 4, be: true },
        b"fl64" | b"FL64" => Enc::Float { bytes: 8, be: true },
        b"alaw" | b"ALAW" => Enc::ALaw,
        b"ulaw" | b"ULAW" => Enc::MuLaw,
        b"ima4" => Enc::ImaQt,
        _ => return Err(OpenError::Unsupported(Unsupported::Encoding)),
    };
    let (bits, frames) = match enc {
        Enc::Int { .. } => (bits as u8, u64::from(frames)),
        Enc::Float { .. } => (32, u64::from(frames)),
        Enc::ImaQt => (16, u64::from(frames) * 64),
        _ => (16, u64::from(frames)),
    };
    Ok(Layout {
        kind: Kind::Aiff,
        enc,
        channels,
        rate,
        bits,
        block: 0,
        spb: 1,
        data: None,
        resume: 0,
        frames: Some(frames),
    })
}

fn au(v: &View<'_>, end: bool) -> Result<Layout, OpenError> {
    if v.len() < 24 {
        return Err(if end { bad() } else { need(24) });
    }
    let off = v.u32_be(4).ok_or_else(bad)?;
    let size = v.u32_be(8).ok_or_else(bad)?;
    let encoding = v.u32_be(12).ok_or_else(bad)?;
    let rate = v.u32_be(16).ok_or_else(bad)?;
    let channels = v.u32_be(20).ok_or_else(bad)?;
    if off < 24 {
        return Err(bad());
    }
    let (enc, bits) = match encoding {
        1 => (Enc::MuLaw, 16),
        2 => (Enc::Int { bytes: 1, unsigned: false, be: true }, 8),
        3 => (Enc::Int { bytes: 2, unsigned: false, be: true }, 16),
        4 => (Enc::Int { bytes: 3, unsigned: false, be: true }, 24),
        5 => (Enc::Int { bytes: 4, unsigned: false, be: true }, 32),
        6 => (Enc::Float { bytes: 4, be: true }, 32),
        7 => (Enc::Float { bytes: 8, be: true }, 32),
        27 => (Enc::ALaw, 16),
        _ => return Err(OpenError::Unsupported(Unsupported::Encoding)),
    };
    let channels = u8::try_from(channels).map_err(|_| OpenError::Unsupported(Unsupported::Channels(255)))?;
    let len = if size == u32::MAX { None } else { Some(u64::from(size)) };
    finish(Layout {
        kind: Kind::Au,
        enc,
        channels,
        rate,
        bits,
        block: 0,
        spb: 1,
        data: Some((u64::from(off), len)),
        resume: u64::from(off),
        frames: None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rate80() {
        // 44100 Hz
        assert_eq!(ext80([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]), 44100);
        // 8000 Hz
        assert_eq!(ext80([0x40, 0x0B, 0xFA, 0, 0, 0, 0, 0, 0, 0]), 8000);
    }
}
