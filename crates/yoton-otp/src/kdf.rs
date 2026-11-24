//! Key derivation: PBKDF2-HMAC-SHA-256 (RFC 8018), one 32-byte block,
//! computed in steps so the firmware can yield (and feed the watchdog)
//! during the roughly one second it takes.
//!
//! Why PBKDF2 and not a memory-hard KDF: Argon2 or scrypt would need tens of
//! KiB of RAM to be worth having, which the player's 320 KiB do not spare
//! while the UI runs, and they still could not protect a 6-digit PIN against
//! an offline attack (10^6 guesses). PBKDF2-HMAC-SHA-256 costs two SHA-256
//! compressions per iteration and no memory; the iteration count is
//! calibrated on the device ([`calibrate`]) and stored in the vault header.
//!
//! **What this does not do**: anyone with the player and a USB cable can
//! read the whole NOR (boot ROM) and the eFuse device id, so a 4–8 digit
//! PIN falls to an offline search in minutes on a desktop. Only a long
//! passphrase resists that. The PIN and its back-off stop casual access
//! through the UI, nothing more (06-connectivity §5.2).

use hmac::{Hmac, KeyInit, Mac};
use sha2::Sha256;
use zeroize::Zeroize;

/// Salt bytes stored in the vault header.
pub const SALT_LEN: usize = 16;
/// Longest device id mixed into the salt.
pub const MAX_DEVICE_ID: usize = 32;
/// Iteration count below which a vault is refused (also the floor of
/// [`calibrate`]).
pub const MIN_ITERATIONS: u32 = 1_000;
/// Iteration count to use until [`calibrate`] has run on the device.
///
/// Estimate, not a measurement on the player: `sha2` 0.11's
/// `compress256` built for thumbv7em (opt-level s) is 2310 instructions
/// per 64-byte block, 467 of them loads/stores; at about 2800 cycles per
/// compression plus ≈ 1500 cycles of HMAC state copying and padding, one
/// PBKDF2 iteration (two compressions) costs ≈ 7000 cycles: ≈ 6800
/// iterations per second at 48 MHz, ≈ 20000 at 144 MHz. 6000 is about
/// 0.9 s at 48 MHz with code resident.
pub const DEFAULT_ITERATIONS: u32 = 6_000;
/// Upper bound accepted from a header (≈ 70 s at 48 MHz: corruption guard).
pub const MAX_ITERATIONS: u32 = 1_000_000;

type HmacSha256 = Hmac<Sha256>;

/// An in-progress derivation. `step` runs a bounded number of iterations;
/// `finish` returns the key once all are done.
pub struct Pbkdf2 {
    prf: HmacSha256,
    u: [u8; 32],
    t: [u8; 32],
    left: u32,
}

impl Drop for Pbkdf2 {
    fn drop(&mut self) {
        self.u.zeroize();
        self.t.zeroize();
    }
}

impl Pbkdf2 {
    /// Start deriving from `password` with salt `salt || device_id`
    /// (the device id is truncated to [`MAX_DEVICE_ID`] bytes).
    #[must_use]
    pub fn new(password: &[u8], salt: &[u8; SALT_LEN], device_id: &[u8], iterations: u32) -> Self {
        // HMAC accepts any key length.
        let prf = <HmacSha256 as KeyInit>::new_from_slice(password).unwrap_or_else(|_| unreachable!());
        let mut m = prf.clone();
        m.update(salt);
        m.update(&device_id[..device_id.len().min(MAX_DEVICE_ID)]);
        m.update(&1u32.to_be_bytes());
        let mut u = [0u8; 32];
        u.copy_from_slice(&m.finalize().into_bytes());
        Self { prf, u, t: u, left: iterations.max(1) - 1 }
    }

    /// Run up to `n` more iterations; returns `true` when done.
    pub fn step(&mut self, n: u32) -> bool {
        let k = n.min(self.left);
        for _ in 0..k {
            let mut m = self.prf.clone();
            m.update(&self.u);
            self.u.copy_from_slice(&m.finalize().into_bytes());
            for (t, u) in self.t.iter_mut().zip(self.u.iter()) {
                *t ^= *u;
            }
        }
        self.left -= k;
        self.left == 0
    }

    #[must_use]
    pub fn remaining(&self) -> u32 {
        self.left
    }

    /// The derived key; runs any remaining iterations first.
    #[must_use]
    pub fn finish(mut self) -> [u8; 32] {
        while !self.step(u32::MAX) {}
        self.t
    }
}

/// One-shot derivation.
#[must_use]
pub fn derive(password: &[u8], salt: &[u8; SALT_LEN], device_id: &[u8], iterations: u32) -> [u8; 32] {
    Pbkdf2::new(password, salt, device_id, iterations).finish()
}

/// Iteration count for a target duration, from a measurement: `probe`
/// iterations took `elapsed_us`. Rounded down to a multiple of 1000 and
/// clamped to [`MIN_ITERATIONS`]..=[`MAX_ITERATIONS`].
#[must_use]
pub fn calibrate(probe: u32, elapsed_us: u64, target_ms: u32) -> u32 {
    let per_s = u64::from(probe) * 1_000_000 / elapsed_us.max(1);
    let n = per_s * u64::from(target_ms) / 1000;
    let n = (n / 1000 * 1000).clamp(u64::from(MIN_ITERATIONS), u64::from(MAX_ITERATIONS));
    n as u32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(s: &str) -> std::vec::Vec<u8> {
        (0..s.len()).step_by(2).map(|i| u8::from_str_radix(&s[i..i + 2], 16).unwrap()).collect()
    }

    #[test]
    fn known_vectors() {
        // PBKDF2-HMAC-SHA256("password", "salt", c, 32): the widely used
        // vectors (also what Python's hashlib.pbkdf2_hmac returns), through
        // the reference crate to pin it down.
        let cases = [
            (1, "120fb6cffcf8b32c43e7225256c4f837a86548c92ccc35480805987cb70be17b"),
            (2, "ae4d0c95af6b46d32d0adff928f06dd02a303f8ef3c251dfd6e2d85a95474c43"),
            (4096, "c5e478d59288c841aa530db6845c4c8d962893a001ce4e11a4963873aa98134a"),
        ];
        for (c, want) in cases {
            let mut out = [0u8; 32];
            pbkdf2::pbkdf2_hmac::<Sha256>(b"password", b"salt", c, &mut out);
            assert_eq!(out.to_vec(), hex(want), "reference crate, c={c}");
        }
        // Our split salt (16 bytes + device id) against the reference crate.
        let salt = *b"0123456789abcdef";
        let dev = b"\x01\x02\x03\x04\x05\x06\x07\x08";
        let mut full = std::vec::Vec::from(&salt[..]);
        full.extend_from_slice(dev);
        for c in [1, 2, 3, 1000] {
            let mut want = [0u8; 32];
            pbkdf2::pbkdf2_hmac::<Sha256>(b"123456", &full, c, &mut want);
            assert_eq!(derive(b"123456", &salt, dev, c), want, "c={c}");
        }
    }

    #[test]
    fn stepping_equals_one_shot() {
        let salt = [7u8; SALT_LEN];
        let mut p = Pbkdf2::new(b"pin", &salt, b"dev", 2500);
        let mut steps = 0;
        while !p.step(300) {
            steps += 1;
        }
        assert_eq!(steps, 8);
        assert_eq!(p.remaining(), 0);
        assert_eq!(p.finish(), derive(b"pin", &salt, b"dev", 2500));
    }

    #[test]
    fn calibration() {
        // 1000 iterations in 160 ms -> 6250/s -> 6000 for one second.
        assert_eq!(calibrate(1000, 160_000, 1000), 6000);
        assert_eq!(calibrate(1000, 10_000_000, 1000), MIN_ITERATIONS);
        assert_eq!(calibrate(1000, 1, 1000), MAX_ITERATIONS);
    }
}
