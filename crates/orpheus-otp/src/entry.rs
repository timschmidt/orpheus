//! One authenticator entry: parameters, secret and labels.

use zeroize::Zeroize;

use crate::otp::{self, Algorithm, Code};

/// Longest secret (SHA-512's block-sized key; RFC 6238's SHA-512 test seed
/// is 64 bytes).
pub const MAX_SECRET: usize = 64;
/// Longest issuer, in UTF-8 bytes (longer ones are cut at a character
/// boundary).
pub const MAX_ISSUER: usize = 32;
/// Longest account name, in UTF-8 bytes (cut like the issuer).
pub const MAX_ACCOUNT: usize = 64;
/// Longest TOTP period accepted, seconds.
pub const MAX_PERIOD: u32 = 3600;

/// TOTP with its period, or HOTP with its next counter value.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    Totp { period: u32 },
    Hotp { counter: u64 },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryError {
    EmptySecret,
    SecretTooLong,
    BadDigits,
    BadPeriod,
}

/// UTF-8 text of at most `N` bytes, stored inline.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct Text<const N: usize> {
    bytes: [u8; N],
    len: u8,
}

impl<const N: usize> core::fmt::Debug for Text<N> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        core::fmt::Debug::fmt(self.as_str(), f)
    }
}

impl<const N: usize> Default for Text<N> {
    fn default() -> Self {
        Self { bytes: [0; N], len: 0 }
    }
}

impl<const N: usize> Text<N> {
    /// Copy `s`, cut at the last character boundary that fits.
    #[must_use]
    pub fn new(s: &str) -> Self {
        let mut end = s.len().min(N).min(255);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        let mut t = Self::default();
        t.bytes[..end].copy_from_slice(&s.as_bytes()[..end]);
        t.len = end as u8;
        t
    }

    #[must_use]
    pub fn as_str(&self) -> &str {
        // Constructed only from `&str` cut at a boundary, or validated bytes.
        core::str::from_utf8(&self.bytes[..usize::from(self.len)]).unwrap_or("")
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len == 0
    }
}

/// An authenticator entry. The secret is wiped when the value is dropped.
#[derive(Clone, PartialEq, Eq)]
pub struct Entry {
    pub kind: Kind,
    pub alg: Algorithm,
    pub digits: u8,
    secret: [u8; MAX_SECRET],
    secret_len: u8,
    pub issuer: Text<MAX_ISSUER>,
    pub account: Text<MAX_ACCOUNT>,
}

impl core::fmt::Debug for Entry {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Entry")
            .field("kind", &self.kind)
            .field("alg", &self.alg)
            .field("digits", &self.digits)
            .field("secret_len", &self.secret_len)
            .field("issuer", &self.issuer)
            .field("account", &self.account)
            .finish()
    }
}

impl Drop for Entry {
    fn drop(&mut self) {
        self.secret.zeroize();
    }
}

impl Entry {
    pub fn new(
        kind: Kind,
        alg: Algorithm,
        digits: u8,
        secret: &[u8],
        issuer: &str,
        account: &str,
    ) -> Result<Self, EntryError> {
        if secret.is_empty() {
            return Err(EntryError::EmptySecret);
        }
        if secret.len() > MAX_SECRET {
            return Err(EntryError::SecretTooLong);
        }
        if !(otp::MIN_DIGITS..=otp::MAX_DIGITS).contains(&digits) {
            return Err(EntryError::BadDigits);
        }
        if let Kind::Totp { period } = kind
            && (period == 0 || period > MAX_PERIOD)
        {
            return Err(EntryError::BadPeriod);
        }
        let mut s = [0u8; MAX_SECRET];
        s[..secret.len()].copy_from_slice(secret);
        Ok(Self {
            kind,
            alg,
            digits,
            secret: s,
            secret_len: secret.len() as u8,
            issuer: Text::new(issuer),
            account: Text::new(account),
        })
    }

    #[must_use]
    pub fn secret(&self) -> &[u8] {
        &self.secret[..usize::from(self.secret_len)]
    }

    /// The code to show at `unix_time` (TOTP) or for the stored counter
    /// (HOTP; the caller advances the counter through the vault).
    #[must_use]
    pub fn code(&self, unix_time: u64) -> Code {
        let v = match self.kind {
            Kind::Totp { period } => otp::totp(self.alg, self.secret(), unix_time, period, self.digits),
            Kind::Hotp { counter } => otp::hotp(self.alg, self.secret(), counter, self.digits),
        };
        Code::new(v, self.digits)
    }
}
