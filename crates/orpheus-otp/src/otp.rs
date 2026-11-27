//! HOTP (RFC 4226) and TOTP (RFC 6238).

use sha2::digest::block_api::BlockSizeUser;
use sha2::digest::{Digest, FixedOutputReset, Output, OutputSizeUser};
use zeroize::Zeroize;

/// HMAC hash of an entry.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Algorithm {
    Sha1,
    Sha256,
    Sha512,
}

impl Algorithm {
    /// Wire value in the vault record.
    #[must_use]
    pub const fn id(self) -> u8 {
        match self {
            Algorithm::Sha1 => 1,
            Algorithm::Sha256 => 2,
            Algorithm::Sha512 => 3,
        }
    }

    #[must_use]
    pub const fn from_id(id: u8) -> Option<Self> {
        match id {
            1 => Some(Algorithm::Sha1),
            2 => Some(Algorithm::Sha256),
            3 => Some(Algorithm::Sha512),
            _ => None,
        }
    }

    /// Name as used in `otpauth://` URIs.
    #[must_use]
    pub const fn name(self) -> &'static str {
        match self {
            Algorithm::Sha1 => "SHA1",
            Algorithm::Sha256 => "SHA256",
            Algorithm::Sha512 => "SHA512",
        }
    }
}

/// Digits accepted in a code. RFC 4226 requires at least 6; the plan uses
/// 6 and 8, 7 is accepted because some services issue it.
pub const MIN_DIGITS: u8 = 6;
pub const MAX_DIGITS: u8 = 8;

const POW10: [u32; 9] = [1, 10, 100, 1_000, 10_000, 100_000, 1_000_000, 10_000_000, 100_000_000];

/// Not inlined: inlined, the three HMAC states of [`hotp`] took room at
/// once (a 1.6 KB frame on the firmware's UI task, above the SHA-512
/// compression; VER-10). One at a time, the deepest is the SHA-512 one.
///
/// HMAC (RFC 2104) over one hasher, reset between the inner and the outer
/// hash, with the padded key in one block-sized buffer XORed in place.
/// BOOK-1/2 (2026-11-27): `hmac::Hmac<Sha512>` kept the inner and outer
/// states, its key block and the finalisation's copies: a 1,232 B frame
/// above SHA-512's compression, the UI task's deepest path (opening the
/// Authenticator). Checked against RFC 4226 / 6238 and against `hmac`.
#[inline(never)]
fn mac<D: Digest + FixedOutputReset + BlockSizeUser>(secret: &[u8], msg: &[u8], out: &mut [u8; 64]) -> usize {
    const IPAD: u8 = 0x36;
    const OPAD: u8 = 0x5C;
    let bs = <D as BlockSizeUser>::block_size();
    let mut k = [0u8; MAX_BLOCK];
    let Some(key) = k.get_mut(..bs) else { return 0 };
    let mut h = D::new();
    if secret.len() > bs {
        Digest::update(&mut h, secret);
        let d = h.finalize_reset();
        let n = d.len().min(bs);
        key[..n].copy_from_slice(&d[..n]);
    } else {
        key[..secret.len()].copy_from_slice(secret);
    }
    key.iter_mut().for_each(|b| *b ^= IPAD);
    Digest::update(&mut h, &*key);
    Digest::update(&mut h, msg);
    let n = <D as OutputSizeUser>::output_size();
    let Some(tag) = out.get_mut(..n).and_then(|o| <&mut Output<D>>::try_from(o).ok()) else { return 0 };
    Digest::finalize_into_reset(&mut h, tag);
    key.iter_mut().for_each(|b| *b ^= IPAD ^ OPAD);
    Digest::update(&mut h, &*key);
    Digest::update(&mut h, &*tag);
    Digest::finalize_into_reset(&mut h, tag);
    k.zeroize();
    n
}

/// The largest block of the three hashes (SHA-512's).
const MAX_BLOCK: usize = 128;

/// RFC 4226 §5.3 dynamic truncation, then modulo 10^digits.
fn truncate(h: &[u8], digits: u8) -> u32 {
    let o = usize::from(h[h.len() - 1] & 0x0F);
    let bin = u32::from_be_bytes([h[o] & 0x7F, h[o + 1], h[o + 2], h[o + 3]]);
    bin % POW10[usize::from(digits.clamp(MIN_DIGITS, MAX_DIGITS))]
}

/// HOTP value for `counter`. `digits` is clamped to 6–8.
#[must_use]
pub fn hotp(alg: Algorithm, secret: &[u8], counter: u64, digits: u8) -> u32 {
    let msg = counter.to_be_bytes();
    let mut h = [0u8; 64];
    let n = match alg {
        Algorithm::Sha1 => mac::<sha1::Sha1>(secret, &msg, &mut h),
        Algorithm::Sha256 => mac::<sha2::Sha256>(secret, &msg, &mut h),
        Algorithm::Sha512 => mac::<sha2::Sha512>(secret, &msg, &mut h),
    };
    truncate(&h[..n], digits)
}

/// RFC 6238 time step for `unix_time` (T0 = 0). `period` 0 is treated as 30.
#[must_use]
pub const fn time_step(unix_time: u64, period: u32) -> u64 {
    let p = if period == 0 { 30 } else { period as u64 };
    unix_time / p
}

/// Seconds left in the current period (1..=period).
#[must_use]
pub const fn seconds_left(unix_time: u64, period: u32) -> u32 {
    let p = if period == 0 { 30 } else { period as u64 };
    (p - unix_time % p) as u32
}

/// TOTP value at `unix_time`.
#[must_use]
pub fn totp(alg: Algorithm, secret: &[u8], unix_time: u64, period: u32, digits: u8) -> u32 {
    hotp(alg, secret, time_step(unix_time, period), digits)
}

/// A code as ASCII digits with leading zeros, ready for display or typing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Code {
    digits: [u8; MAX_DIGITS as usize],
    len: u8,
}

impl Code {
    #[must_use]
    pub fn new(value: u32, digits: u8) -> Self {
        let len = digits.clamp(MIN_DIGITS, MAX_DIGITS);
        let mut d = [b'0'; MAX_DIGITS as usize];
        let mut v = value;
        for i in (0..usize::from(len)).rev() {
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
        }
        Self { digits: d, len }
    }

    #[must_use]
    pub fn as_bytes(&self) -> &[u8] {
        &self.digits[..usize::from(self.len)]
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Only ASCII digits are ever stored.
        core::str::from_utf8(self.as_bytes()).unwrap_or("")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hand-written HMAC equals the `hmac` crate's for keys shorter
    /// than, equal to and longer than each hash's block (RFC 4226/6238
    /// cover only short keys) and for messages of several lengths.
    #[test]
    fn mac_matches_the_hmac_crate() {
        use hmac::{Hmac, KeyInit, Mac};
        fn want<M: Mac + KeyInit>(k: &[u8], m: &[u8]) -> ([u8; 64], usize) {
            let mut h = <M as KeyInit>::new_from_slice(k).unwrap();
            h.update(m);
            let t = h.finalize().into_bytes();
            let mut o = [0u8; 64];
            o[..t.len()].copy_from_slice(&t);
            (o, t.len())
        }
        let data: [u8; 300] = core::array::from_fn(|i| ((i as u32).wrapping_mul(151) >> 3) as u8);
        for klen in [0, 1, 20, 63, 64, 65, 127, 128, 129, 300] {
            for mlen in [0, 8, 64, 129] {
                let (k, m) = (&data[..klen], &data[300 - mlen..]);
                let mut out = [0u8; 64];
                let n = mac::<sha1::Sha1>(k, m, &mut out);
                let (w, wn) = want::<Hmac<sha1::Sha1>>(k, m);
                assert_eq!(&out[..n], &w[..wn], "sha1 {klen}/{mlen}");
                let n = mac::<sha2::Sha256>(k, m, &mut out);
                let (w, wn) = want::<Hmac<sha2::Sha256>>(k, m);
                assert_eq!(&out[..n], &w[..wn], "sha256 {klen}/{mlen}");
                let n = mac::<sha2::Sha512>(k, m, &mut out);
                let (w, wn) = want::<Hmac<sha2::Sha512>>(k, m);
                assert_eq!(&out[..n], &w[..wn], "sha512 {klen}/{mlen}");
            }
        }
    }

    const S20: &[u8] = b"12345678901234567890";
    const S32: &[u8] = b"12345678901234567890123456789012";
    const S64: &[u8] = b"1234567890123456789012345678901234567890123456789012345678901234";

    #[test]
    fn rfc4226_appendix_d() {
        let want = [755224, 287082, 359152, 969429, 338314, 254676, 287922, 162583, 399871, 520489];
        for (c, w) in want.iter().enumerate() {
            assert_eq!(hotp(Algorithm::Sha1, S20, c as u64, 6), *w, "counter {c}");
        }
    }

    #[test]
    fn rfc6238_appendix_b() {
        let rows: [(u64, u32, u32, u32); 6] = [
            (59, 94287082, 46119246, 90693936),
            (1111111109, 7081804, 68084774, 25091201),
            (1111111111, 14050471, 67062674, 99943326),
            (1234567890, 89005924, 91819424, 93441116),
            (2000000000, 69279037, 90698825, 38618901),
            (20000000000, 65353130, 77737706, 47863826),
        ];
        for (t, s1, s256, s512) in rows {
            assert_eq!(totp(Algorithm::Sha1, S20, t, 30, 8), s1, "SHA1 t={t}");
            assert_eq!(totp(Algorithm::Sha256, S32, t, 30, 8), s256, "SHA256 t={t}");
            assert_eq!(totp(Algorithm::Sha512, S64, t, 30, 8), s512, "SHA512 t={t}");
        }
    }

    #[test]
    fn period_60_and_formatting() {
        // A 60 s period uses step t/60: t = 119 is step 1, the RFC 4226 value
        // for counter 1.
        assert_eq!(totp(Algorithm::Sha1, S20, 119, 60, 6), 287082);
        assert_eq!(seconds_left(119, 60), 1);
        assert_eq!(seconds_left(120, 60), 60);
        assert_eq!(Code::new(7081804, 8).as_str(), "07081804");
        assert_eq!(Code::new(42, 6).as_str(), "000042");
        assert_eq!(Code::new(1, 3).as_str(), "000001");
    }

    #[test]
    fn algorithm_ids_round_trip() {
        for a in [Algorithm::Sha1, Algorithm::Sha256, Algorithm::Sha512] {
            assert_eq!(Algorithm::from_id(a.id()), Some(a));
        }
        assert_eq!(Algorithm::from_id(0), None);
    }
}
