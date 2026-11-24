//! RIFF/WAVE headers for 16-bit PCM and IMA ADPCM, rewritten in place as
//! the file grows, and the closing `cue `/`LIST adtl` chunks that carry
//! the recording's marks and dropouts.
//!
//! Layout: `RIFF` · `WAVE` · `fmt ` · (`fact`, ADPCM only) · `data` ·
//! audio · (`cue ` · `LIST adtl` with one `labl` per point, at close).
//! The header has a fixed length per format ([`header_len`]), so the audio
//! starts at the same offset whatever the counts say.

/// What a cue point marks.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CueKind {
    /// The user's mark (Next while recording).
    Mark,
    /// Audio dropped here: the card could not keep up.
    Dropout,
}

impl CueKind {
    const fn label(self) -> &'static [u8] {
        match self {
            CueKind::Mark => b"mark",
            CueKind::Dropout => b"dropout",
        }
    }
}

/// A point in the recording (frame number) and what it is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cue {
    pub frame: u32,
    pub kind: CueKind,
}

/// The audio encoding inside the WAV file.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Coding {
    Pcm16,
    /// IMA ADPCM with this block alignment.
    Ima {
        block_align: u16,
    },
}

/// Header bytes for a coding (PCM 44, IMA ADPCM 60).
#[must_use]
pub const fn header_len(c: Coding) -> usize {
    match c {
        Coding::Pcm16 => 44,
        Coding::Ima { .. } => 60,
    }
}

/// Largest header ([`header_len`]).
pub const MAX_HEADER: usize = 60;

fn put32(out: &mut [u8], at: usize, v: u32) {
    if let Some(s) = out.get_mut(at..at + 4) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

fn put16(out: &mut [u8], at: usize, v: u16) {
    if let Some(s) = out.get_mut(at..at + 2) {
        s.copy_from_slice(&v.to_le_bytes());
    }
}

fn tag(out: &mut [u8], at: usize, t: &[u8; 4]) {
    if let Some(s) = out.get_mut(at..at + 4) {
        s.copy_from_slice(t);
    }
}

/// Write the header for `data_bytes` of audio (`frames` for the ADPCM
/// `fact` count) and `trailer_bytes` after it (the cue chunks; 0 while
/// recording). Returns its length.
pub fn header(
    out: &mut [u8],
    c: Coding,
    rate: u32,
    channels: u8,
    data_bytes: u32,
    frames: u32,
    trailer_bytes: u32,
) -> usize {
    let len = header_len(c);
    let ch = u16::from(channels);
    let pad = data_bytes & 1;
    let riff = (len as u32 - 8).saturating_add(data_bytes).saturating_add(pad).saturating_add(trailer_bytes);
    tag(out, 0, b"RIFF");
    put32(out, 4, riff);
    tag(out, 8, b"WAVE");
    tag(out, 12, b"fmt ");
    match c {
        Coding::Pcm16 => {
            put32(out, 16, 16);
            put16(out, 20, 1);
            put16(out, 22, ch);
            put32(out, 24, rate);
            put32(out, 28, rate * u32::from(ch) * 2);
            put16(out, 32, ch * 2);
            put16(out, 34, 16);
            tag(out, 36, b"data");
            put32(out, 40, data_bytes);
        }
        Coding::Ima { block_align } => {
            let spb = crate::adpcm::frames_per_block(usize::from(block_align), channels) as u32;
            put32(out, 16, 20);
            put16(out, 20, 0x0011);
            put16(out, 22, ch);
            put32(out, 24, rate);
            put32(out, 28, rate * u32::from(block_align) / spb.max(1));
            put16(out, 32, block_align);
            put16(out, 34, 4);
            put16(out, 36, 2);
            put16(out, 38, spb as u16);
            tag(out, 40, b"fact");
            put32(out, 44, 4);
            put32(out, 48, frames);
            tag(out, 52, b"data");
            put32(out, 56, data_bytes);
        }
    }
    len
}

/// Bytes of the closing chunks for `n` cue points (0 for none).
#[must_use]
pub const fn trailer_len(cues: &[Cue]) -> usize {
    if cues.is_empty() {
        return 0;
    }
    let n = cues.len();
    // cue: 8 + 4 + 24·n; LIST adtl: 8 + 4 + Σ(8 + 4 + label + NUL, even).
    let mut labl = 0;
    let mut i = 0;
    while i < n {
        let l = 4 + cues[i].kind.label().len() + 1;
        labl += 8 + l + (l & 1);
        i += 1;
    }
    12 + 24 * n + 12 + labl
}

/// Write the closing chunks (the data chunk's pad byte, if any, is the
/// caller's) into `out`; returns the length (= [`trailer_len`]).
pub fn trailer(out: &mut [u8], cues: &[Cue]) -> usize {
    if cues.is_empty() {
        return 0;
    }
    let n = cues.len() as u32;
    tag(out, 0, b"cue ");
    put32(out, 4, 4 + 24 * n);
    put32(out, 8, n);
    let mut o = 12;
    for (i, c) in cues.iter().enumerate() {
        put32(out, o, i as u32 + 1);
        put32(out, o + 4, c.frame);
        tag(out, o + 8, b"data");
        put32(out, o + 12, 0);
        put32(out, o + 16, 0);
        put32(out, o + 20, c.frame);
        o += 24;
    }
    let list_at = o;
    tag(out, o, b"LIST");
    tag(out, o + 8, b"adtl");
    o += 12;
    for (i, c) in cues.iter().enumerate() {
        let label = c.kind.label();
        let l = 4 + label.len() + 1;
        tag(out, o, b"labl");
        put32(out, o + 4, l as u32);
        put32(out, o + 8, i as u32 + 1);
        if let Some(s) = out.get_mut(o + 12..o + 12 + label.len()) {
            s.copy_from_slice(label);
        }
        for b in out.get_mut(o + 12 + label.len()..o + 8 + l + (l & 1)).into_iter().flatten() {
            *b = 0;
        }
        o += 8 + l + (l & 1);
    }
    put32(out, list_at + 4, (o - list_at - 8) as u32);
    o
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcm_header_fields() {
        let mut h = [0u8; MAX_HEADER];
        let n = header(&mut h, Coding::Pcm16, 44_100, 2, 1000, 250, 0);
        assert_eq!(n, 44);
        assert_eq!(&h[..4], b"RIFF");
        assert_eq!(u32::from_le_bytes(h[4..8].try_into().unwrap()), 36 + 1000);
        assert_eq!(u32::from_le_bytes(h[28..32].try_into().unwrap()), 176_400);
        assert_eq!(u32::from_le_bytes(h[40..44].try_into().unwrap()), 1000);
    }

    #[test]
    fn trailer_length_matches() {
        let cues = [Cue { frame: 10, kind: CueKind::Mark }, Cue { frame: 99, kind: CueKind::Dropout }];
        let mut b = [0u8; 256];
        let n = trailer(&mut b, &cues);
        assert_eq!(n, trailer_len(&cues));
        assert_eq!(&b[..4], b"cue ");
        assert_eq!(&b[60..64], b"LIST");
        assert_eq!(&b[68..72], b"adtl");
    }
}
