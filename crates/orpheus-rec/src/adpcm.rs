//! IMA ADPCM encoder in the WAV block layout (`WAVE_FORMAT_IMA_ADPCM`,
//! 0x0011; IMA "Recommended Practices for Enhancing Digital Audio
//! Compatibility in Multimedia Systems" 3.00): 4 bits per sample.
//!
//! A block starts with one 4-byte header per channel (the first sample
//! exactly, the step index, a zero byte), then the other samples as
//! nibbles, low nibble first, in 4-byte groups of 8 samples per channel,
//! channels interleaved group by group. The quantiser chooses the nibble
//! whose reconstruction (the decoder's arithmetic, `((2·d + 1)·step) >> 3`)
//! is nearest, so the encoder's predictor is exactly the decoder's.

/// Step sizes.
pub const STEP: [i32; 89] = [
    7, 8, 9, 10, 11, 12, 13, 14, 16, 17, 19, 21, 23, 25, 28, 31, 34, 37, 41, 45, 50, 55, 60, 66, 73, 80, 88,
    97, 107, 118, 130, 143, 157, 173, 190, 209, 230, 253, 279, 307, 337, 371, 408, 449, 494, 544, 598, 658,
    724, 796, 876, 963, 1060, 1166, 1282, 1411, 1552, 1707, 1878, 2066, 2272, 2499, 2749, 3024, 3327, 3660,
    4026, 4428, 4871, 5358, 5894, 6484, 7132, 7845, 8630, 9493, 10442, 11487, 12635, 13899, 15289, 16818,
    18500, 20350, 22385, 24623, 27086, 29794, 32767,
];

const INDEX: [i32; 16] = [-1, -1, -1, -1, 2, 4, 6, 8, -1, -1, -1, -1, 2, 4, 6, 8];

/// Bytes per block per channel for a sample rate: 256 up to 12 kHz, else
/// 512 (common encoders use 1024 above 24 kHz; 512 keeps a block within
/// the encoder's 1152-frame input buffer at under 1 % more header bytes).
#[must_use]
pub const fn block_bytes_per_channel(rate: u32) -> usize {
    if rate <= 12_000 { 256 } else { 512 }
}

/// `nBlockAlign` for `channels` at `rate`.
#[must_use]
pub const fn block_align(rate: u32, channels: u8) -> usize {
    block_bytes_per_channel(rate) * channels as usize
}

/// Frames per block: the header sample plus 8 per 4-byte group.
#[must_use]
pub const fn frames_per_block(block_align: usize, channels: u8) -> usize {
    let ch = channels as usize;
    1 + (block_align - 4 * ch) / (4 * ch) * 8
}

/// One channel's predictor.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Channel {
    pub predictor: i32,
    pub index: i32,
}

impl Channel {
    /// Quantise `x`; the predictor follows the decoder.
    #[inline]
    pub fn encode(&mut self, x: i32) -> u8 {
        let step = STEP.get(self.index.clamp(0, 88) as usize).copied().unwrap_or(7).max(1);
        let diff = x - self.predictor;
        let (sign, mag) = if diff < 0 { (8u8, -diff) } else { (0u8, diff) };
        // The nearest of the eight reconstructions ((2d + 1)·step) >> 3.
        let mut d = ((mag << 2) / step).clamp(0, 7) as u8;
        let rec = |d: u8| ((2 * i32::from(d) + 1) * step) >> 3;
        if d < 7 && (rec(d + 1) - mag).abs() < (rec(d) - mag).abs() {
            d += 1;
        }
        if d > 0 && (rec(d - 1) - mag).abs() < (rec(d) - mag).abs() {
            d -= 1;
        }
        let nibble = sign | d;
        let r = rec(d);
        let p = if sign != 0 { self.predictor - r } else { self.predictor + r };
        self.predictor = p.clamp(-32768, 32767);
        self.index = (self.index + INDEX.get(usize::from(nibble & 15)).copied().unwrap_or(-1)).clamp(0, 88);
        nibble
    }
}

/// The encoder: the predictors carry their step index across blocks.
#[derive(Clone, Copy, Debug)]
pub struct AdpcmEncoder {
    channels: u8,
    block_align: usize,
    ch: [Channel; 2],
}

impl AdpcmEncoder {
    #[must_use]
    pub fn new(rate: u32, channels: u8) -> Self {
        let channels = channels.clamp(1, 2);
        AdpcmEncoder { channels, block_align: block_align(rate, channels), ch: [Channel::default(); 2] }
    }

    #[must_use]
    pub fn block_align(&self) -> usize {
        self.block_align
    }

    #[must_use]
    pub fn frames_per_block(&self) -> usize {
        frames_per_block(self.block_align, self.channels)
    }

    /// Encode one block from `pcm` (interleaved, [`Self::frames_per_block`]
    /// frames; a shorter last block is padded with its last frame) into
    /// `out` (at least [`Self::block_align`] bytes). Returns the bytes
    /// written (always a whole block).
    pub fn encode(&mut self, pcm: &[i16], out: &mut [u8]) -> usize {
        self.encode_recon(pcm, out, &mut [])
    }

    /// [`Self::encode`], also writing what a decoder will output for the
    /// block into `recon` (interleaved, as far as it reaches): tests use it
    /// to check decoders bit for bit.
    pub fn encode_recon(&mut self, pcm: &[i16], out: &mut [u8], recon: &mut [i16]) -> usize {
        let ch = usize::from(self.channels);
        let fpb = self.frames_per_block();
        let have = pcm.len() / ch;
        if have == 0 || out.len() < self.block_align {
            return 0;
        }
        let at = |f: usize, c: usize| -> i32 {
            let f = f.min(have - 1);
            i32::from(pcm.get(f * ch + c).copied().unwrap_or(0))
        };
        let mut keep = |f: usize, c: usize, v: i32| {
            if let Some(r) = recon.get_mut(f * ch + c) {
                *r = v as i16;
            }
        };
        for (c, (pr, hdr)) in self.ch.iter_mut().zip(out.chunks_exact_mut(4)).take(ch).enumerate() {
            let s = at(0, c);
            pr.predictor = s;
            keep(0, c, s);
            let b = (s as i16).to_le_bytes();
            hdr.copy_from_slice(&[b[0], b[1], pr.index as u8, 0]);
        }
        let groups = (fpb - 1) / 8;
        let mut o = 4 * ch;
        for g in 0..groups {
            for (c, pr) in self.ch.iter_mut().take(ch).enumerate() {
                for b in 0..4 {
                    let f = 1 + g * 8 + b * 2;
                    let lo = pr.encode(at(f, c));
                    keep(f, c, pr.predictor);
                    let hi = pr.encode(at(f + 1, c));
                    keep(f + 1, c, pr.predictor);
                    if let Some(x) = out.get_mut(o) {
                        *x = lo | (hi << 4);
                    }
                    o += 1;
                }
            }
        }
        o
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn block_geometry() {
        assert_eq!(block_align(8_000, 1), 256);
        assert_eq!(frames_per_block(256, 1), 505);
        assert_eq!(block_align(44_100, 2), 1024);
        assert_eq!(frames_per_block(1024, 2), 1017);
        assert_eq!(frames_per_block(512, 1), 1017);
    }

    #[test]
    fn follows_a_slow_ramp_closely() {
        let mut c = Channel::default();
        let mut worst = 0;
        let mut p = 0;
        for i in 0..2000 {
            // A triangle, ±2000, 7 per sample.
            let t = (i * 7) % 8000;
            let x = if t < 4000 { t - 2000 } else { 6000 - t };
            c.encode(x);
            if i > 50 {
                worst = worst.max((c.predictor - x).abs());
            }
            p = c.predictor;
        }
        assert!(worst < 200, "{worst} {p}");
    }
}
