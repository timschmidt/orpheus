//! Sample conversions. Integer arithmetic only (float sources are converted
//! from their bit patterns), each bit-exact against ffmpeg's decoders and
//! `libswresample` conversions:
//!
//! * unsigned 8-bit → `(x - 128) << 8`; signed 8-bit → `x << 8`;
//! * 24-bit → `i32` left-justified (`x << 8`);
//! * float → `i32` = `x · 2³¹` rounded to nearest, ties to even, saturated
//!   (`av_clipl_int32(llrintf(x * (1U << 31)))`); NaN → 0;
//! * G.711 A-law and µ-law → `i16` per ITU-T G.711 (ffmpeg's tables).

/// How the samples of a stream are stored.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Enc {
    /// Two's complement (or offset-binary for unsigned 8-bit) integers of
    /// `bytes` bytes.
    Int {
        bytes: u8,
        unsigned: bool,
        be: bool,
    },
    Float {
        bytes: u8,
        be: bool,
    },
    ALaw,
    MuLaw,
    /// IMA ADPCM, WAV layout (`0x0011`).
    Ima,
    /// IMA ADPCM, QuickTime/AIFF-C layout (`ima4`): 34-byte packets of 64
    /// samples per channel.
    ImaQt,
    /// Microsoft ADPCM (`0x0002`).
    Ms,
}

impl Enc {
    /// Whether output samples are `i32` (else `i16`).
    pub(crate) fn wide(self) -> bool {
        matches!(self, Self::Int { bytes: 3 | 4, .. } | Self::Float { .. })
    }
}

/// Convert one sample of `bytes.len()` bytes to `i16` (narrow encodings).
#[inline]
pub(crate) fn to_i16(enc: Enc, s: &[u8]) -> i16 {
    match enc {
        Enc::Int { bytes: 1, unsigned: true, .. } => (i16::from(s[0]) - 128) << 8,
        Enc::Int { bytes: 1, .. } => i16::from(s[0] as i8) << 8,
        Enc::Int { be: true, .. } => i16::from_be_bytes([s[0], s[1]]),
        Enc::Int { .. } => i16::from_le_bytes([s[0], s[1]]),
        Enc::ALaw => alaw(s[0]),
        Enc::MuLaw => ulaw(s[0]),
        _ => 0,
    }
}

/// Convert one sample to left-justified `i32` (wide encodings).
#[inline]
pub(crate) fn to_i32(enc: Enc, s: &[u8]) -> i32 {
    match enc {
        Enc::Int { bytes: 3, be: false, .. } => i32::from_le_bytes([0, s[0], s[1], s[2]]),
        Enc::Int { bytes: 3, be: true, .. } => i32::from_be_bytes([s[0], s[1], s[2], 0]),
        Enc::Int { be: false, .. } => i32::from_le_bytes([s[0], s[1], s[2], s[3]]),
        Enc::Int { be: true, .. } => i32::from_be_bytes([s[0], s[1], s[2], s[3]]),
        Enc::Float { bytes: 4, be } => {
            let b = [s[0], s[1], s[2], s[3]];
            f32_bits_to_i32(if be { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) })
        }
        Enc::Float { be, .. } => {
            let b = [s[0], s[1], s[2], s[3], s[4], s[5], s[6], s[7]];
            f64_bits_to_i32(if be { u64::from_be_bytes(b) } else { u64::from_le_bytes(b) })
        }
        _ => 0,
    }
}

/// `round_half_even(m · 2^shift)` saturated to `i32`, with sign.
fn scale_round(neg: bool, m: u64, shift: i32) -> i32 {
    let mag: u64 = if shift >= 0 {
        let s = shift as u32;
        if s >= 63 || m > (u64::MAX >> s) { 1 << 32 } else { m << s }
    } else {
        let r = (-shift) as u32;
        if r >= 64 {
            0
        } else {
            let q = m >> r;
            let rem = m & ((1u64 << r) - 1);
            let half = 1u64 << (r - 1);
            if rem > half || (rem == half && q & 1 == 1) { q + 1 } else { q }
        }
    };
    if neg {
        if mag >= 1 << 31 { i32::MIN } else { -(mag as i32) }
    } else if mag >= (1 << 31) - 1 {
        i32::MAX
    } else {
        mag as i32
    }
}

/// IEEE single bits → `i32` (value · 2³¹, ties to even, saturating).
#[must_use]
pub fn f32_bits_to_i32(bits: u32) -> i32 {
    let neg = bits >> 31 != 0;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let frac = u64::from(bits & 0x7F_FFFF);
    match exp {
        0xFF if frac != 0 => 0,
        0xFF => {
            if neg {
                i32::MIN
            } else {
                i32::MAX
            }
        }
        // Subnormals: frac · 2^(-149) · 2^31.
        0 => scale_round(neg, frac, -149 + 31),
        _ => scale_round(neg, frac | 1 << 23, exp - 127 - 23 + 31),
    }
}

/// IEEE double bits → `i32` (value · 2³¹, ties to even, saturating).
#[must_use]
pub fn f64_bits_to_i32(bits: u64) -> i32 {
    let neg = bits >> 63 != 0;
    let exp = ((bits >> 52) & 0x7FF) as i32;
    let frac = bits & ((1 << 52) - 1);
    match exp {
        0x7FF if frac != 0 => 0,
        0x7FF => {
            if neg {
                i32::MIN
            } else {
                i32::MAX
            }
        }
        0 => scale_round(neg, frac, -1074 + 31),
        _ => scale_round(neg, frac | 1 << 52, exp - 1023 - 52 + 31),
    }
}

/// G.711 A-law → linear.
#[must_use]
pub fn alaw(v: u8) -> i16 {
    let a = v ^ 0x55;
    let mut t = i16::from(a & 0x0F);
    let seg = (a & 0x70) >> 4;
    if seg != 0 {
        t = (t * 2 + 1 + 32) << (seg + 2);
    } else {
        t = (t * 2 + 1) << 3;
    }
    if a & 0x80 != 0 { t } else { -t }
}

/// G.711 µ-law → linear.
#[must_use]
pub fn ulaw(v: u8) -> i16 {
    let u = !v;
    let t = ((i16::from(u & 0x0F) << 3) + 0x84) << ((u & 0x70) >> 4);
    if u & 0x80 != 0 { 0x84 - t } else { t - 0x84 }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reference(x: f32) -> i32 {
        // What libswresample computes (round half to even, saturate).
        let v = f64::from(x) * 2147483648.0;
        let r = v.round_ties_even();
        r.clamp(-2147483648.0, 2147483647.0) as i32
    }

    #[test]
    fn float_conversion_matches_rint() {
        let samples = [
            0.0f32,
            -0.0,
            1.0,
            -1.0,
            0.5,
            -0.5,
            1.5,
            -2.0,
            1e-10,
            -1e-10,
            0.123_456_7,
            -0.999_999_9,
            4.656_613e-10,
            6.984_919_5e-10,
            2.328_306_4e-10,
            f32::MIN_POSITIVE,
            1e-45,
            3.0e38,
        ];
        for &x in &samples {
            assert_eq!(f32_bits_to_i32(x.to_bits()), reference(x), "{x}");
            assert_eq!(f64_bits_to_i32(f64::from(x).to_bits()), reference(x), "{x} as f64");
        }
        let mut s = 0x1234_5678u32;
        for _ in 0..100_000 {
            s ^= s << 13;
            s ^= s >> 17;
            s ^= s << 5;
            let x = f32::from_bits(s);
            if x.is_nan() {
                continue;
            }
            assert_eq!(f32_bits_to_i32(s), reference(x), "{x}");
        }
        assert_eq!(f32_bits_to_i32(f32::NAN.to_bits()), 0);
        assert_eq!(f32_bits_to_i32(f32::NEG_INFINITY.to_bits()), i32::MIN);
    }

    #[test]
    fn g711() {
        assert_eq!(ulaw(0xFF), 0);
        assert_eq!(ulaw(0x00), -32124);
        assert_eq!(ulaw(0x80), 32124);
        assert_eq!(alaw(0xD5), 8);
        assert_eq!(alaw(0x55), -8);
        assert_eq!(alaw(0xAA), 32256);
    }
}
