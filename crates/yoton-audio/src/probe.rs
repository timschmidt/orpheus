//! Format detection by content (04 §2, 07 §3.3).
//!
//! The file extension only breaks ties (raw MPEG audio after junk). Leading
//! ID3v2 tags are reported as `Probe::Skip` so the pipeline can step over them
//! (they may be megabytes of album art, far more than the stream ring) and
//! probe again; the metadata parser reads them separately.

use crate::source::{ByteSource, View};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Format {
    #[default]
    Unknown,
    /// RIFF/WAVE (and RF64, refused by the decoder for now).
    Wav,
    /// AIFF and AIFF-C.
    Aiff,
    /// Sun/NeXT `.au`/`.snd`.
    Au,
    /// Raw MPEG audio frames (layer I, II, III).
    Mpeg,
    /// Native FLAC (`fLaC`).
    Flac,
    Ogg(OggCodec),
    /// ISO base media (MP4/M4A).
    Mp4,
    Ape,
    WavPack,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OggCodec {
    Vorbis,
    Opus,
    Flac,
    Speex,
    Other,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Probe {
    Found(Format),
    /// A tag of this many bytes comes first: skip it and probe again.
    Skip(u64),
    /// Probe again with at least `n` bytes buffered (or at the end).
    NeedData(usize),
    Unknown,
}

/// How far into the stream MPEG sync is searched.
pub const PROBE_WINDOW: usize = 8 * 1024;

/// Identify the stream at the read position of `src`. `ext` is the file
/// name extension (any case), if there is one.
#[must_use]
pub fn probe(src: &dyn ByteSource, ext: Option<&[u8]>) -> Probe {
    let v = View::of(src);
    let end = src.at_end();
    if v.len() < 12 && !end {
        return Probe::NeedData(12);
    }
    if let Some(n) = id3v2_len(&v) {
        return Probe::Skip(n);
    }
    let tag = |off: usize, t: &[u8]| v.starts_with_at(off, t);
    if (tag(0, b"RIFF") || tag(0, b"RF64")) && tag(8, b"WAVE") {
        return Probe::Found(Format::Wav);
    }
    if tag(0, b"FORM") && (tag(8, b"AIFF") || tag(8, b"AIFC")) {
        return Probe::Found(Format::Aiff);
    }
    if tag(0, b".snd") {
        return Probe::Found(Format::Au);
    }
    if tag(0, b"fLaC") {
        return Probe::Found(Format::Flac);
    }
    if tag(4, b"ftyp") {
        return Probe::Found(Format::Mp4);
    }
    if tag(0, b"MAC ") {
        return Probe::Found(Format::Ape);
    }
    if tag(0, b"wvpk") {
        return Probe::Found(Format::WavPack);
    }
    if tag(0, b"OggS") {
        let nseg = usize::from(v.byte(26));
        let p = 27 + nseg;
        if v.len() < p + 8 && !end {
            return Probe::NeedData(p + 8);
        }
        let codec = if tag(p, b"\x01vorbis") {
            OggCodec::Vorbis
        } else if tag(p, b"OpusHead") {
            OggCodec::Opus
        } else if tag(p, b"\x7fFLAC") {
            OggCodec::Flac
        } else if tag(p, b"Speex   ") {
            OggCodec::Speex
        } else {
            OggCodec::Other
        };
        return Probe::Found(Format::Ogg(codec));
    }
    let mpeg_ext = ext.is_some_and(|e| {
        let mut l = [0u8; 4];
        let n = e.len().min(4);
        for (d, s) in l.iter_mut().zip(e) {
            *d = s.to_ascii_lowercase();
        }
        matches!(&l[..n], b"mp3" | b"mp2" | b"mp1" | b"mpa" | b"mpga")
    });
    match find_mpeg_sync(&v, end, if mpeg_ext { 2 } else { 3 }) {
        Sync::Found(_) => Probe::Found(Format::Mpeg),
        Sync::NeedData(n) => Probe::NeedData(n),
        Sync::None => {
            if !end && v.len() < PROBE_WINDOW {
                Probe::NeedData(PROBE_WINDOW.min(v.len() + 4096))
            } else {
                Probe::Unknown
            }
        }
    }
}

/// Length (header + body + footer) of an ID3v2 tag at the start of `v`.
#[must_use]
pub fn id3v2_len(v: &View<'_>) -> Option<u64> {
    if !v.starts_with_at(0, b"ID3") {
        return None;
    }
    let h: [u8; 10] = v.array(0)?;
    if h[3] == 0xFF || h[4] == 0xFF || h[6..10].iter().any(|b| b & 0x80 != 0) {
        return None;
    }
    let size = h[6..10].iter().fold(0u64, |a, &b| (a << 7) | u64::from(b));
    let footer = if h[3] >= 4 && h[5] & 0x10 != 0 { 10 } else { 0 };
    Some(10 + size + footer)
}

/// A decoded MPEG audio frame header (ISO 11172-3 §2.4.2.3, 13818-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MpegHeader {
    pub raw: [u8; 4],
    /// 1 = MPEG-1, 2 = MPEG-2, 25 = MPEG-2.5.
    pub version: u8,
    /// 1, 2 or 3.
    pub layer: u8,
    /// kbit/s; 0 for free format.
    pub bitrate: u32,
    pub rate: u32,
    pub padding: bool,
    pub crc: bool,
    pub channels: u8,
    /// Sample frames per frame.
    pub samples: u32,
}

const BITRATES: [[[u16; 15]; 3]; 2] = [
    [
        [0, 32, 64, 96, 128, 160, 192, 224, 256, 288, 320, 352, 384, 416, 448],
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320, 384],
        [0, 32, 40, 48, 56, 64, 80, 96, 112, 128, 160, 192, 224, 256, 320],
    ],
    [
        [0, 32, 48, 56, 64, 80, 96, 112, 128, 144, 160, 176, 192, 224, 256],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
        [0, 8, 16, 24, 32, 40, 48, 56, 64, 80, 96, 112, 128, 144, 160],
    ],
];

impl MpegHeader {
    /// Parse 4 header bytes; `None` if they are not a valid header.
    #[must_use]
    pub fn parse(h: [u8; 4]) -> Option<Self> {
        if h[0] != 0xFF || h[1] & 0xE0 != 0xE0 {
            return None;
        }
        let version = match (h[1] >> 3) & 3 {
            0 => 25,
            2 => 2,
            3 => 1,
            _ => return None,
        };
        let layer = match (h[1] >> 1) & 3 {
            1 => 3,
            2 => 2,
            3 => 1,
            _ => return None,
        };
        let bri = usize::from(h[2] >> 4);
        let sri = usize::from((h[2] >> 2) & 3);
        if bri == 15 || sri == 3 || h[3] & 3 == 2 {
            return None;
        }
        let base = [44100u32, 48000, 32000][sri];
        let rate = match version {
            1 => base,
            2 => base / 2,
            _ => base / 4,
        };
        let bitrate = u32::from(BITRATES[usize::from(version != 1)][usize::from(layer - 1)][bri]);
        let samples = match (layer, version) {
            (1, _) => 384,
            (3, 2 | 25) => 576,
            _ => 1152,
        };
        Some(Self {
            raw: h,
            version,
            layer,
            bitrate,
            rate,
            padding: h[2] & 2 != 0,
            crc: h[1] & 1 == 0,
            channels: if h[3] >> 6 == 3 { 1 } else { 2 },
            samples,
        })
    }

    /// Frame length in bytes including padding; `free` is the stream's
    /// free-format frame length without padding (0 if unknown).
    #[must_use]
    pub fn frame_len(&self, free: u32) -> u32 {
        let pad = u32::from(self.padding);
        if self.layer == 1 {
            let slots = if self.bitrate == 0 { free / 4 } else { 12 * self.bitrate * 1000 / self.rate };
            (slots + pad) * 4
        } else {
            let body =
                if self.bitrate == 0 { free } else { self.samples / 8 * self.bitrate * 1000 / self.rate };
            body + pad
        }
    }

    /// Whether `other` can be the next frame of the same stream (same
    /// version, layer, rate; both free format or neither).
    #[must_use]
    pub fn compatible(&self, other: &Self) -> bool {
        (self.raw[1] ^ other.raw[1]) & 0xFE == 0
            && (self.raw[2] ^ other.raw[2]) & 0x0C == 0
            && (self.bitrate == 0) == (other.bitrate == 0)
    }
}

/// Result of [`find_mpeg_sync`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Sync {
    /// Offset of the first frame of a chain of matching frames.
    Found(usize),
    NeedData(usize),
    None,
}

/// Find `chain` consecutive compatible MPEG frames in `v` (fewer if the data
/// ends after a frame). Free-format streams are not recognised here.
#[must_use]
pub fn find_mpeg_sync(v: &View<'_>, at_end: bool, chain: usize) -> Sync {
    let limit = v.len().min(PROBE_WINDOW);
    let mut i = 0;
    while i + 4 <= limit {
        if v.byte(i) == 0xFF
            && let Some(h) = v.array::<4>(i).and_then(MpegHeader::parse)
            && h.bitrate != 0
        {
            let mut off = i;
            let mut cur = h;
            let mut ok = true;
            for _ in 1..chain {
                off += cur.frame_len(0) as usize;
                if off + 4 > v.len() {
                    if at_end && off <= v.len() {
                        break;
                    }
                    if off + 4 > crate::MAX_PEEK {
                        ok = false;
                        break;
                    }
                    return Sync::NeedData(off + 4);
                }
                match v.array::<4>(off).and_then(MpegHeader::parse) {
                    Some(n) if h.compatible(&n) && n.bitrate != 0 => cur = n,
                    _ => {
                        ok = false;
                        break;
                    }
                }
            }
            if ok {
                return Sync::Found(i);
            }
        }
        i += 1;
    }
    Sync::None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SliceSource;

    #[test]
    fn magic_numbers() {
        let mut wav = [0u8; 16];
        wav[..4].copy_from_slice(b"RIFF");
        wav[8..12].copy_from_slice(b"WAVE");
        assert_eq!(probe(&SliceSource::new(&wav), None), Probe::Found(Format::Wav));
        let mut f = [0u8; 16];
        f[..4].copy_from_slice(b"fLaC");
        assert_eq!(probe(&SliceSource::new(&f), None), Probe::Found(Format::Flac));
        let id3 = *b"ID3\x04\x00\x00\x00\x00\x02\x01rest";
        assert_eq!(probe(&SliceSource::new(&id3), None), Probe::Skip(10 + 257));
        assert_eq!(probe(&SliceSource::ring(&id3, 4), None), Probe::NeedData(12));
        assert_eq!(probe(&SliceSource::new(&[0u8; 100]), None), Probe::Unknown);
    }

    #[test]
    fn mpeg_chain() {
        // MPEG-1 layer III, 128 kbit/s, 44.1 kHz, no padding: 417 bytes.
        let h = [0xFF, 0xFB, 0x90, 0x00];
        let p = MpegHeader::parse(h).unwrap();
        assert_eq!((p.version, p.layer, p.bitrate, p.rate), (1, 3, 128, 44100));
        assert_eq!(p.frame_len(0), 417);
        let mut data = std::vec![0u8; 417 * 3 + 7];
        for k in 0..3 {
            data[7 + k * 417..7 + k * 417 + 4].copy_from_slice(&h);
        }
        assert_eq!(probe(&SliceSource::new(&data), None), Probe::Found(Format::Mpeg));
        assert_eq!(find_mpeg_sync(&View::new(&data, &[]), true, 3), Sync::Found(7));
    }
}
