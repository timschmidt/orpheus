//! Authenticator core (06-connectivity §5): HOTP/TOTP, `otpauth://` URIs,
//! the encrypted vault, wrong-PIN back-off and RTC drift correction.
//! Hardware-free: the vault reaches the NOR sector through
//! [`vault::Storage`] and gets randomness through [`vault::Entropy`].
//!
//! Honest limit (06 §5.2): the boot ROM lets anyone with the player and a
//! USB cable read the NOR and the eFuse device id, so a short PIN does not
//! survive an offline brute-force attack; only a long passphrase does.
//! There is no secure element.

#![no_std]
#![forbid(unsafe_code)]

#[cfg(test)]
extern crate std;

pub mod backoff;
pub mod base32;
pub mod drift;
pub mod entry;
pub mod kdf;
pub mod otp;
pub mod service;
pub mod uri;
pub mod vault;

pub use entry::{Entry, Kind, Text};
pub use otp::{Algorithm, Code};
pub use vault::{
    Entropy, Key, Payload, Pool, RekeyJob, SecretKind, Status, Storage, UnlockJob, Unlocked, Vault,
    VaultError,
};
