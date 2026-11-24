//! Playing time from the first bytes of a stream, where the format puts it
//! there: FLAC `STREAMINFO`, the MPEG Xing/Info/VBRI header (or the
//! bitrate of a constant-rate stream), WAV `fmt `/`data`, AIFF `COMM`.

use orpheus_audio::probe::{MpegHeader, Sync, find_mpeg_sync};
use orpheus_audio::{Format, View};

fn be32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_be_bytes(b.get(o..o + 4)?.try_into().ok()?))
}
fn le32(b: &[u8], o: usize) -> Option<u32> {
    Some(u32::from_le_bytes(b.get(o..o + 4)?.try_into().ok()?))
}

fn ms(frames: u64, rate: u32) -> Option<u32> {
    (rate != 0).then(|| (frames.saturating_mul(1000) / u64::from(rate)).min(u64::from(u32::MAX)) as u32)
}

/// Milliseconds of audio, from `head` (the bytes at file offset `head_at`,
/// just after any leading ID3v2 tags) and the file length; `None` when
/// they do not tell.
#[must_use]
pub fn duration_ms(format: Format, head: &[u8], head_at: u64, file_len: u64) -> Option<u32> {
    match format {
        Format::Flac => flac(head),
        Format::Mpeg => mpeg(head, file_len.saturating_sub(head_at)),
        Format::Wav => wav(head),
        Format::Aiff => aiff(head),
        _ => None,
    }
}

fn flac(h: &[u8]) -> Option<u32> {
    if h.get(..4)? != b"fLaC" || h.get(4)? & 0x7F != 0 {
        return None;
    }
    let si = h.get(8..26)?;
    let rate = (u32::from(si[10]) << 12) | (u32::from(si[11]) << 4) | (u32::from(si[12]) >> 4);
    let total = (u64::from(si[13] & 0x0F) << 32) | u64::from(be32(si, 14)?);
    if total == 0 {
        return None;
    }
    ms(total, rate)
}

fn mpeg(h: &[u8], stream_len: u64) -> Option<u32> {
    let v = View::new(h, &[]);
    let Sync::Found(at) = find_mpeg_sync(&v, true, 3) else { return None };
    let hdr = MpegHeader::parse(h.get(at..at + 4)?.try_into().ok()?)?;
    let side = match (hdr.version, hdr.channels) {
        (1, 1) => 17,
        (1, _) => 32,
        (_, 1) => 9,
        _ => 17,
    };
    let x = at + 4 + side;
    let tag = h.get(x..x + 4);
    if tag == Some(b"Xing") || tag == Some(b"Info") {
        let flags = be32(h, x + 4)?;
        if flags & 1 != 0 {
            let frames = be32(h, x + 8)?;
            return ms(u64::from(frames) * u64::from(hdr.samples), hdr.rate);
        }
    }
    let y = at + 4 + 32;
    if h.get(y..y + 4) == Some(b"VBRI") {
        let frames = be32(h, y + 14)?;
        return ms(u64::from(frames) * u64::from(hdr.samples), hdr.rate);
    }
    if hdr.bitrate == 0 {
        return None;
    }
    // Constant bitrate: bytes × 8 / kbit/s = ms (trailing tags included;
    // at most 128 + a few hundred bytes).
    let bytes = stream_len.saturating_sub(at as u64);
    Some((bytes.saturating_mul(8) / u64::from(hdr.bitrate)).min(u64::from(u32::MAX)) as u32)
}

fn wav(h: &[u8]) -> Option<u32> {
    if h.get(..4)? != b"RIFF" || h.get(8..12)? != b"WAVE" {
        return None;
    }
    let mut off = 12;
    let mut byte_rate = None;
    while off + 8 <= h.len() {
        let id = &h[off..off + 4];
        let size = le32(h, off + 4)?;
        if id == b"fmt " {
            byte_rate = le32(h, off + 16);
        } else if id == b"data" {
            let br = byte_rate?;
            return (br != 0)
                .then(|| (u64::from(size) * 1000 / u64::from(br)).min(u64::from(u32::MAX)) as u32);
        }
        off += 8 + size as usize + (size as usize & 1);
    }
    None
}

/// 80-bit IEEE extended (big endian) → integer (sample rates).
fn ext80(b: &[u8]) -> Option<u32> {
    let exp = i32::from(u16::from_be_bytes([b[0], b[1]]) & 0x7FFF) - 16383;
    let mant = u64::from_be_bytes(b.get(2..10)?.try_into().ok()?);
    if !(0..32).contains(&exp) {
        return None;
    }
    Some((mant >> (63 - exp)) as u32)
}

fn aiff(h: &[u8]) -> Option<u32> {
    if h.get(..4)? != b"FORM" || !matches!(h.get(8..12)?, b"AIFF" | b"AIFC") {
        return None;
    }
    let mut off = 12;
    while off + 8 <= h.len() {
        let id = &h[off..off + 4];
        let size = be32(h, off + 4)?;
        if id == b"COMM" {
            let c = h.get(off + 8..off + 8 + 18)?;
            let frames = be32(c, 2)?;
            return ms(u64::from(frames), ext80(&c[8..18])?);
        }
        off += 8 + size as usize + (size as usize & 1);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    #[test]
    fn flac_streaminfo() {
        let mut h = Vec::from(*b"fLaC");
        h.extend_from_slice(&[0x80, 0, 0, 34]);
        let mut si = [0u8; 34];
        // 44100 Hz, 2 ch, 16 bit, 441000 samples = 10 s.
        let rate = 44100u32;
        si[10] = (rate >> 12) as u8;
        si[11] = (rate >> 4) as u8;
        si[12] = ((rate & 0xF) << 4) as u8 | (1 << 1);
        si[13] = 0xF0;
        si[14..18].copy_from_slice(&441_000u32.to_be_bytes());
        h.extend_from_slice(&si);
        assert_eq!(duration_ms(Format::Flac, &h, 0, 1 << 20), Some(10_000));
    }

    #[test]
    fn mpeg_cbr_and_xing() {
        // MPEG-1 layer III 128 kbit/s 44.1 kHz stereo: 417-byte frames.
        let hdr = [0xFF, 0xFB, 0x90, 0x00];
        let mut s = Vec::new();
        for _ in 0..4 {
            s.extend_from_slice(&hdr);
            s.resize(s.len() + 413, 0);
        }
        // 1,600,000 bytes at 128 kbit/s = 100 s.
        assert_eq!(duration_ms(Format::Mpeg, &s, 0, 1_600_000), Some(100_000));
        let mut x = s.clone();
        x[4 + 32..4 + 36].copy_from_slice(b"Xing");
        x[4 + 36..4 + 40].copy_from_slice(&1u32.to_be_bytes());
        x[4 + 40..4 + 44].copy_from_slice(&3828u32.to_be_bytes());
        // 3828 frames × 1152 / 44100 = 99.99 s.
        assert_eq!(duration_ms(Format::Mpeg, &x, 0, 1_600_000), Some(99_996));
    }

    #[test]
    fn wav_and_aiff() {
        let mut w = Vec::from(*b"RIFF\0\0\0\0WAVEfmt ");
        w.extend_from_slice(&16u32.to_le_bytes());
        w.extend_from_slice(&[1, 0, 2, 0]);
        w.extend_from_slice(&44100u32.to_le_bytes());
        w.extend_from_slice(&176_400u32.to_le_bytes());
        w.extend_from_slice(&[4, 0, 16, 0]);
        w.extend_from_slice(b"data");
        w.extend_from_slice(&(176_400u32 * 3).to_le_bytes());
        assert_eq!(duration_ms(Format::Wav, &w, 0, 1 << 22), Some(3000));
        let mut a = Vec::from(*b"FORM\0\0\0\0AIFFCOMM");
        a.extend_from_slice(&18u32.to_be_bytes());
        a.extend_from_slice(&[0, 2]);
        a.extend_from_slice(&88_200u32.to_be_bytes());
        a.extend_from_slice(&[0, 16]);
        // 44100 as 80-bit extended.
        a.extend_from_slice(&[0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
        assert_eq!(duration_ms(Format::Aiff, &a, 0, 1 << 22), Some(2000));
    }
}
