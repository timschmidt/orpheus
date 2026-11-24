//! Opus packet framing (RFC 6716 §3), after libopus `src/opus.c`
//! (`opus_packet_parse_impl`, BSD-3-Clause).

/// Coding mode from the TOC byte.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Silk,
    Hybrid,
    Celt,
}

/// Audio bandwidth (RFC 6716 §2): NB, MB, WB, SWB, FB.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Bandwidth {
    Narrow,
    Medium,
    Wide,
    SuperWide,
    Full,
}

/// The TOC byte, decoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Toc {
    pub mode: Mode,
    pub bandwidth: Bandwidth,
    /// Samples per frame at 48 kHz.
    pub frame_size: usize,
    pub stereo: bool,
}

impl Toc {
    #[must_use]
    pub fn parse(toc: u8) -> Self {
        let (mode, bandwidth, frame_size) = if toc & 0x80 != 0 {
            let bw = match (toc >> 5) & 3 {
                0 => Bandwidth::Narrow,
                1 => Bandwidth::Wide,
                2 => Bandwidth::SuperWide,
                _ => Bandwidth::Full,
            };
            (Mode::Celt, bw, (48000usize << ((toc >> 3) & 3)) / 400)
        } else if toc & 0x60 == 0x60 {
            let bw = if toc & 0x10 != 0 { Bandwidth::Full } else { Bandwidth::SuperWide };
            (Mode::Hybrid, bw, if toc & 0x08 != 0 { 960 } else { 480 })
        } else {
            let bw = match (toc >> 5) & 3 {
                0 => Bandwidth::Narrow,
                1 => Bandwidth::Medium,
                _ => Bandwidth::Wide,
            };
            let fs = match (toc >> 3) & 3 {
                3 => 2880,
                k => 480usize << k,
            };
            (Mode::Silk, bw, fs)
        };
        Self { mode, bandwidth, frame_size, stereo: toc & 4 != 0 }
    }
}

/// Up to 48 frames (120 ms of 2.5 ms frames).
pub const MAX_FRAMES: usize = 48;

/// A parsed packet: the frames are `data[offset[i]..offset[i] + size[i]]`.
#[derive(Debug, Clone, Copy)]
pub struct Frames {
    pub toc: Toc,
    pub count: usize,
    pub offset: [u16; MAX_FRAMES],
    pub size: [u16; MAX_FRAMES],
}

fn parse_size(data: &[u8]) -> Option<(usize, usize)> {
    let b0 = usize::from(*data.first()?);
    if b0 < 252 {
        Some((b0, 1))
    } else {
        let b1 = usize::from(*data.get(1)?);
        Some((4 * b1 + b0, 2))
    }
}

/// Split a packet (not self-delimited) into its frames; `None` if it is
/// malformed (RFC 6716 §3.4 requirements R1–R7).
#[must_use]
pub fn parse(packet: &[u8]) -> Option<Frames> {
    let toc_byte = *packet.first()?;
    let toc = Toc::parse(toc_byte);
    let mut pos = 1usize;
    let mut len = packet.len() - 1;
    let mut size = [0u16; MAX_FRAMES];
    let count;
    let mut last_size = len;
    match toc_byte & 3 {
        0 => count = 1,
        1 => {
            count = 2;
            if len & 1 != 0 {
                return None;
            }
            last_size = len / 2;
            size[0] = last_size as u16;
        }
        2 => {
            count = 2;
            let (s, b) = parse_size(&packet[pos..])?;
            len -= b;
            if s > len {
                return None;
            }
            pos += b;
            size[0] = s as u16;
            last_size = len - s;
        }
        _ => {
            if len < 1 {
                return None;
            }
            let ch = packet[pos];
            pos += 1;
            len -= 1;
            count = usize::from(ch & 0x3F);
            if count == 0 || toc.frame_size * count > 5760 {
                return None;
            }
            if ch & 0x40 != 0 {
                loop {
                    let p = usize::from(*packet.get(pos)?);
                    if len == 0 {
                        return None;
                    }
                    pos += 1;
                    len -= 1;
                    let tmp = if p == 255 { 254 } else { p };
                    len = len.checked_sub(tmp)?;
                    if p != 255 {
                        break;
                    }
                }
            }
            if ch & 0x80 != 0 {
                last_size = len;
                for s in size.iter_mut().take(count - 1) {
                    let (n, b) = parse_size(packet.get(pos..)?)?;
                    len = len.checked_sub(b)?;
                    if n > len {
                        return None;
                    }
                    pos += b;
                    *s = n as u16;
                    last_size = last_size.checked_sub(b + n)?;
                }
            } else {
                last_size = len / count;
                if last_size * count != len {
                    return None;
                }
                for s in size.iter_mut().take(count - 1) {
                    *s = last_size as u16;
                }
            }
        }
    }
    if last_size > 1275 {
        return None;
    }
    size[count - 1] = last_size as u16;
    let mut offset = [0u16; MAX_FRAMES];
    for i in 0..count {
        offset[i] = pos as u16;
        pos += usize::from(size[i]);
    }
    if pos > packet.len() {
        return None;
    }
    Some(Frames { toc, count, offset, size })
}

/// Samples (at 48 kHz) in a packet, from its first bytes; `None` if
/// malformed.
#[must_use]
pub fn samples(packet: &[u8]) -> Option<usize> {
    let toc = Toc::parse(*packet.first()?);
    let count = match packet[0] & 3 {
        0 => 1,
        1 | 2 => 2,
        _ => usize::from(*packet.get(1)? & 0x3F),
    };
    let n = toc.frame_size * count;
    (count > 0 && n <= 5760).then_some(n)
}
