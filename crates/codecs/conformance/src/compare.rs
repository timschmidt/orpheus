//! Comparisons against reference decodes.

use md5::{Digest, Md5};

#[derive(Debug, Clone, Copy, Default)]
pub struct Diff {
    pub len_ours: usize,
    pub len_ref: usize,
    pub first_mismatch: Option<usize>,
    pub max_abs: i64,
    /// RMS of the difference over the common length, in dB relative to full
    /// scale (`f64::NEG_INFINITY` when identical).
    pub rms_db: f64,
}

impl Diff {
    #[must_use]
    pub fn exact(&self) -> bool {
        self.len_ours == self.len_ref && self.first_mismatch.is_none()
    }
}

/// Compare two sample streams at the same scale; `full` is full scale
/// (32768 for 16-bit, 2³¹ for 32-bit).
#[must_use]
pub fn diff(ours: &[i32], reference: &[i32], full: f64) -> Diff {
    let n = ours.len().min(reference.len());
    let mut d = Diff { len_ours: ours.len(), len_ref: reference.len(), ..Diff::default() };
    let mut sum = 0f64;
    for (i, (&a, &b)) in ours[..n].iter().zip(&reference[..n]).enumerate() {
        let e = i64::from(a) - i64::from(b);
        if e != 0 && d.first_mismatch.is_none() {
            d.first_mismatch = Some(i);
        }
        d.max_abs = d.max_abs.max(e.abs());
        let ef = e as f64;
        sum += ef * ef;
    }
    let rms = if n == 0 { 0.0 } else { (sum / n as f64).sqrt() };
    d.rms_db = if rms == 0.0 { f64::NEG_INFINITY } else { 20.0 * (rms / full).log10() };
    d
}

/// Raw little-endian reference samples (`s16` or `s32`) widened to `i32`.
#[must_use]
pub fn load_raw(bytes: &[u8], wide: bool) -> Vec<i32> {
    if wide {
        bytes.as_chunks::<4>().0.iter().map(|c| i32::from_le_bytes(*c)).collect()
    } else {
        bytes.as_chunks::<2>().0.iter().map(|c| i32::from(i16::from_le_bytes(*c))).collect()
    }
}

/// The FLAC STREAMINFO MD5 over our output: each sample shifted back to
/// `bits` bits and written little-endian in `ceil(bits / 8)` bytes.
#[must_use]
pub fn flac_md5(pcm: &[i32], wide: bool, bits: u8) -> [u8; 16] {
    let shift = if wide { 32 - u32::from(bits) } else { 16 - u32::from(bits) };
    let bytes = usize::from(bits).div_ceil(8);
    let mut h = Md5::new();
    let mut buf = Vec::with_capacity(pcm.len() * bytes);
    for &s in pcm {
        let v = s >> shift;
        buf.extend_from_slice(&v.to_le_bytes()[..bytes]);
    }
    h.update(&buf);
    h.finalize().into()
}

/// STREAMINFO MD5 of a native FLAC file, if present and non-zero.
#[must_use]
pub fn flac_streaminfo_md5(file: &[u8]) -> Option<[u8; 16]> {
    if file.get(..4)? != b"fLaC" {
        return None;
    }
    let m: [u8; 16] = file.get(26..42)?.try_into().ok()?;
    (m != [0; 16]).then_some(m)
}

/// FLAC STREAMINFO maximum block size.
#[must_use]
pub fn flac_max_block(file: &[u8]) -> Option<u16> {
    if file.get(..4)? != b"fLaC" {
        return None;
    }
    Some(u16::from_be_bytes([*file.get(10)?, *file.get(11)?]))
}
