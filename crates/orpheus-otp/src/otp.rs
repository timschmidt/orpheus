//! HOTP (RFC 4226) and TOTP (RFC 6238).

use hmac::{Hmac, KeyInit, Mac};

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
#[inline(never)]
fn mac<M: Mac + KeyInit>(secret: &[u8], msg: &[u8], out: &mut [u8; 64]) -> usize {
    // `new_from_slice` accepts keys of any length for HMAC.
    let Ok(mut m) = <M as KeyInit>::new_from_slice(secret) else { return 0 };
    m.update(msg);
    let tag = m.finalize().into_bytes();
    let n = tag.len().min(64);
    out[..n].copy_from_slice(&tag[..n]);
    n
}

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
        Algorithm::Sha1 => mac::<Hmac<sha1::Sha1>>(secret, &msg, &mut h),
        Algorithm::Sha256 => mac::<Hmac<sha2::Sha256>>(secret, &msg, &mut h),
        Algorithm::Sha512 => mac::<Hmac<sha2::Sha512>>(secret, &msg, &mut h),
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
