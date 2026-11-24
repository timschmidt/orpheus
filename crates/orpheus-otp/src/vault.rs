//! The encrypted vault in one 4 KiB NOR sector (02-architecture: NOR
//! `0x7DE000`), 06-connectivity §5.2.
//!
//! # Sector format, version 1 (little-endian)
//!
//! ```text
//! 0x000  magic "YVLT"
//! 0x004  version u8 = 1 | kdf u8 = 1 (PBKDF2-HMAC-SHA-256) | secret kind u8 (0 PIN, 1 passphrase) | 0
//! 0x008  iterations u32
//! 0x00C  generation u32        incremented on every write; input to the nonce
//! 0x010  payload length u16 | 0 u16
//! 0x014  salt [16]
//! 0x024  nonce [24]            XChaCha20-Poly1305
//! 0x03C  tag [16]
//! 0x04C  commit u32            programmed to 0 last; anything else = torn write
//! 0x050  reserved (erased)
//! 0x080  attempt bitmap [64]   bit k cleared before unlock attempt k
//! 0x0C0  success bitmap [64]   bit k cleared when attempt k succeeded
//! 0x100  ciphertext [payload length] (at most 3840 bytes)
//! ```
//!
//! The AEAD's associated data is bytes `0x000..0x03C`, so the header
//! (iterations, salt, nonce, version) is authenticated with the payload. A
//! wrong key and a modified sector are both reported as
//! [`VaultError::WrongKey`]: the tag cannot tell them apart.
//!
//! Plaintext payload: `count u8`, then per entry `len u8` (bytes that
//! follow), `kind u8` (0 TOTP, 1 HOTP), `alg u8`, `digits u8`, `period u16`,
//! `counter u64`, `secret_len u8`, `issuer_len u8`, `account_len u8`, and
//! the three byte strings.
//!
//! # Attempt counter without erasing
//!
//! NOR programming only clears bits, so every unlock attempt clears the
//! next attempt bit *before* the key is derived, and a success clears the
//! matching success bit. Consecutive failures = attempts after the last
//! success. No erase is needed per attempt; when three quarters of the
//! bitmap are used, a successful unlock rewrites the sector with the same
//! header and ciphertext and fresh bitmaps (no re-encryption).
//!
//! # Nonces
//!
//! `nonce = HMAC-SHA-256(key, "orpheus-otp/nonce" || generation || 32 bytes
//! from the entropy source)[..24]`. The generation strictly increases for a
//! given key, so a nonce never repeats even with a poor entropy source; a
//! new PIN means a new salt and a new key.
//!
//! # Two ways in
//!
//! [`Unlocked`] (OTP-1) holds the whole plaintext in its own 3.9 KiB. The
//! firmware (OTP-2) keeps only the [`Key`]: the tag is checked at unlock by
//! streaming the sector through Poly1305 ([`Vault::finish_unlock_key`]),
//! records are decrypted one at a time with the stream cipher seeked to
//! their offset ([`Vault::read_entry`]), and changes go through a
//! [`Payload`] in a buffer the caller lends for the moment of the write.
//! Both produce and accept the same sector.
//!
//! # Power cuts
//!
//! A write is erase, header, payload, commit word. A cut before the commit
//! word leaves a damaged vault ([`Status::Damaged`]); the entries must then
//! be provisioned again. Writes happen only when entries change, an HOTP
//! counter advances, the PIN changes, or (rarely) for bitmap compaction.

use chacha20::XChaCha20;
use chacha20::cipher::{KeyIvInit, StreamCipher, StreamCipherSeek};
use chacha20poly1305::aead::AeadInOut;
use chacha20poly1305::{XChaCha20Poly1305, XNonce};
use hmac::{Hmac, KeyInit, Mac};
use poly1305::Poly1305;
use poly1305::universal_hash::UniversalHash;
use sha2::Sha256;
use zeroize::Zeroize;

use crate::backoff;
use crate::entry::{Entry, EntryError, Kind, MAX_ACCOUNT, MAX_ISSUER, MAX_SECRET};
use crate::kdf::{self, Pbkdf2, SALT_LEN};
use crate::otp::{Algorithm, Code};

pub const SECTOR: usize = 4096;
pub const MAGIC: [u8; 4] = *b"YVLT";
pub const VERSION: u8 = 1;
pub const KDF_PBKDF2_SHA256: u8 = 1;
pub const MAX_ENTRIES: usize = 32;

const O_VERSION: usize = 4;
const O_KDF: usize = 5;
const O_KIND: usize = 6;
const O_ITER: usize = 8;
const O_GEN: usize = 12;
const O_LEN: usize = 16;
const O_SALT: usize = 20;
const O_NONCE: usize = 36;
const O_TAG: usize = 60;
const O_COMMIT: usize = 76;
const HDR: usize = 80;
const AAD_END: usize = O_TAG;
const O_ATTEMPTS: usize = 128;
const O_SUCCESS: usize = 192;
const BITMAP: usize = 64;
const BITS: u32 = (BITMAP * 8) as u32;
const COMPACT_AT: u32 = BITS * 3 / 4;
const O_PAYLOAD: usize = 256;
/// Largest plaintext payload.
pub const PAYLOAD_MAX: usize = SECTOR - O_PAYLOAD;
const REC_FIXED: usize = 16;

/// The vault sector. Offsets are relative to the sector start.
#[allow(async_fn_in_trait)]
pub trait Storage {
    type Error;
    async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), Self::Error>;
    /// Program bytes (NOR: only clears bits). `off` and `data.len()` are
    /// multiples of 4.
    async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), Self::Error>;
    /// Erase the whole sector to `0xFF`.
    async fn erase(&mut self) -> Result<(), Self::Error>;
}

/// Unpredictable bytes for salts and nonces (the firmware's TRNG or jitter
/// source). Nonce uniqueness does not depend on it (see the module docs);
/// salt quality does.
pub trait Entropy {
    fn fill(&mut self, buf: &mut [u8]);
}

/// How the vault is protected; shown by the UI, stored in the header.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SecretKind {
    Pin,
    Passphrase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VaultError<E> {
    Storage(E),
    /// No vault, or a damaged one: create a new one first.
    NoVault,
    /// Written by a newer firmware.
    UnsupportedVersion,
    /// Wrong PIN/passphrase, or the sector was modified.
    WrongKey,
    /// Too early: wait this many milliseconds.
    Backoff {
        wait_ms: u64,
    },
    /// The payload would exceed the sector.
    Full,
    TooManyEntries,
    BadIndex,
    /// The stored record does not decode (only possible with a bug, since
    /// the payload is authenticated).
    Corrupt,
    Entry(EntryError),
}

/// What [`Vault::load`] found.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    /// Erased sector: offer set-up.
    Absent,
    /// Torn write or foreign data: offer re-creation (entries are lost).
    Damaged,
    /// A vault; `failures` consecutive wrong attempts so far.
    Locked { kind: SecretKind, failures: u32 },
}

/// The vault sector and the attempt bookkeeping. Holds no secrets.
pub struct Vault<S: Storage> {
    storage: S,
    hdr: [u8; HDR],
    valid: bool,
    attempts: u32,
    failures: u32,
    not_before_ms: u64,
}

/// A started unlock: the attempt is recorded, the key derivation runs in
/// [`UnlockJob::step`] so the caller can yield between steps.
pub struct UnlockJob {
    kdf: Pbkdf2,
    attempt: u32,
}

impl UnlockJob {
    /// Run up to `n` iterations; `true` when the key is ready.
    pub fn step(&mut self, n: u32) -> bool {
        self.kdf.step(n)
    }

    /// Iterations still to run.
    #[must_use]
    pub fn remaining(&self) -> u32 {
        self.kdf.remaining()
    }
}

/// Decrypted contents. Key and plaintext are wiped on drop: drop it on
/// lock, screen-off and before rebooting into ADFU (RAM survives resets).
pub struct Unlocked {
    key: [u8; 32],
    buf: [u8; PAYLOAD_MAX],
    len: usize,
}

impl Drop for Unlocked {
    fn drop(&mut self) {
        self.key.zeroize();
        self.buf.zeroize();
    }
}

fn le32(b: &[u8], o: usize) -> u32 {
    u32::from_le_bytes([b[o], b[o + 1], b[o + 2], b[o + 3]])
}

fn bit_clear(map: &[u8], k: u32) -> bool {
    map[(k / 8) as usize] & (1 << (k % 8)) == 0
}

/// Count attempts and consecutive failures from the two bitmaps.
fn count(attempts: &[u8], success: &[u8]) -> (u32, u32) {
    let a = (0..BITS).find(|&k| !bit_clear(attempts, k)).unwrap_or(BITS);
    let last_ok = (0..a).rev().find(|&k| bit_clear(success, k));
    let failures = match last_ok {
        Some(k) => a - k - 1,
        None => a,
    };
    (a, failures)
}

fn cipher(key: &[u8; 32]) -> XChaCha20Poly1305 {
    XChaCha20Poly1305::new(&(*key).into())
}

impl<S: Storage> Vault<S> {
    pub fn new(storage: S) -> Self {
        Self { storage, hdr: [0xFF; HDR], valid: false, attempts: 0, failures: 0, not_before_ms: 0 }
    }

    /// Give the storage back.
    pub fn into_storage(self) -> S {
        self.storage
    }

    /// Read the header and the attempt counter. `now_ms` is a monotonic
    /// clock; the back-off after earlier failures starts again from it, so
    /// a reboot does not shorten it.
    pub async fn load(&mut self, now_ms: u64) -> Result<Status, VaultError<S::Error>> {
        self.valid = false;
        let mut h = [0u8; HDR];
        self.storage.read(0, &mut h).await.map_err(VaultError::Storage)?;
        self.hdr = h;
        if h.iter().all(|&b| b == 0xFF) {
            return Ok(Status::Absent);
        }
        if h[..4] != MAGIC || le32(&h, O_COMMIT) != 0 {
            return Ok(Status::Damaged);
        }
        if h[O_VERSION] != VERSION || h[O_KDF] != KDF_PBKDF2_SHA256 {
            return Err(VaultError::UnsupportedVersion);
        }
        let iters = le32(&h, O_ITER);
        let len = usize::from(u16::from_le_bytes([h[O_LEN], h[O_LEN + 1]]));
        let kind = match h[O_KIND] {
            0 => SecretKind::Pin,
            1 => SecretKind::Passphrase,
            _ => return Ok(Status::Damaged),
        };
        if !(kdf::MIN_ITERATIONS..=kdf::MAX_ITERATIONS).contains(&iters) || len > PAYLOAD_MAX {
            return Ok(Status::Damaged);
        }
        let mut maps = [0u8; 2 * BITMAP];
        self.storage.read(O_ATTEMPTS as u32, &mut maps).await.map_err(VaultError::Storage)?;
        let (a, f) = count(&maps[..BITMAP], &maps[BITMAP..]);
        self.attempts = a;
        self.failures = f;
        self.not_before_ms = now_ms + u64::from(backoff::delay_s(f)) * 1000;
        self.valid = true;
        Ok(Status::Locked { kind, failures: f })
    }

    /// Consecutive failed attempts.
    #[must_use]
    pub fn failures(&self) -> u32 {
        self.failures
    }

    /// Milliseconds until the next attempt is allowed.
    #[must_use]
    pub fn wait_ms(&self, now_ms: u64) -> u64 {
        self.not_before_ms.saturating_sub(now_ms)
    }

    /// Iteration count of the loaded vault.
    #[must_use]
    pub fn iterations(&self) -> u32 {
        le32(&self.hdr, O_ITER)
    }

    /// Record an attempt (persisted before anything else) and start the key
    /// derivation. Refused while the back-off runs.
    pub async fn begin_unlock(
        &mut self,
        secret: &[u8],
        device_id: &[u8],
        now_ms: u64,
    ) -> Result<UnlockJob, VaultError<S::Error>> {
        if !self.valid {
            return Err(VaultError::NoVault);
        }
        let wait_ms = self.wait_ms(now_ms);
        if wait_ms > 0 {
            return Err(VaultError::Backoff { wait_ms });
        }
        let attempt = self.attempts;
        if attempt < BITS {
            self.clear_bit(O_ATTEMPTS, attempt).await?;
            self.attempts += 1;
        }
        // Until the result is known this attempt counts as failed.
        self.failures = self.failures.saturating_add(1);
        let mut salt = [0u8; SALT_LEN];
        salt.copy_from_slice(&self.hdr[O_SALT..O_SALT + SALT_LEN]);
        Ok(UnlockJob { kdf: Pbkdf2::new(secret, &salt, device_id, self.iterations()), attempt })
    }

    /// Finish an unlock: decrypt and check the tag. A failure starts the
    /// next back-off period at `now_ms`.
    pub async fn finish_unlock(
        &mut self,
        job: UnlockJob,
        now_ms: u64,
    ) -> Result<Unlocked, VaultError<S::Error>> {
        let UnlockJob { kdf, attempt } = job;
        let mut u = Unlocked { key: kdf.finish(), buf: [0; PAYLOAD_MAX], len: 0 };
        let len = usize::from(u16::from_le_bytes([self.hdr[O_LEN], self.hdr[O_LEN + 1]]));
        self.storage.read(O_PAYLOAD as u32, &mut u.buf[..len]).await.map_err(VaultError::Storage)?;
        let nonce = XNonce::try_from(&self.hdr[O_NONCE..O_NONCE + 24]).map_err(|_| VaultError::Corrupt)?;
        let tag =
            chacha20poly1305::Tag::try_from(&self.hdr[O_TAG..O_TAG + 16]).map_err(|_| VaultError::Corrupt)?;
        let ok = cipher(&u.key)
            .decrypt_inout_detached(&nonce, &self.hdr[..AAD_END], (&mut u.buf[..len]).into(), &tag)
            .is_ok();
        if !ok {
            self.not_before_ms = now_ms + u64::from(backoff::delay_s(self.failures)) * 1000;
            return Err(VaultError::WrongKey);
        }
        u.len = len;
        if u.len == 0 || usize::from(u.buf[0]) > MAX_ENTRIES {
            return Err(VaultError::Corrupt);
        }
        if attempt < BITS {
            self.clear_bit(O_SUCCESS, attempt).await?;
        }
        self.failures = 0;
        self.not_before_ms = 0;
        if self.attempts >= COMPACT_AT {
            self.compact().await?;
        }
        Ok(u)
    }

    /// [`begin_unlock`](Self::begin_unlock) and
    /// [`finish_unlock`](Self::finish_unlock) without yielding.
    pub async fn unlock(
        &mut self,
        secret: &[u8],
        device_id: &[u8],
        now_ms: u64,
    ) -> Result<Unlocked, VaultError<S::Error>> {
        let job = self.begin_unlock(secret, device_id, now_ms).await?;
        self.finish_unlock(job, now_ms).await
    }

    /// Create an empty vault (erasing whatever was there) and return it
    /// unlocked.
    pub async fn create(
        &mut self,
        secret: &[u8],
        kind: SecretKind,
        device_id: &[u8],
        iterations: u32,
        rng: &mut impl Entropy,
    ) -> Result<Unlocked, VaultError<S::Error>> {
        let mut u = Unlocked { key: [0; 32], buf: [0; PAYLOAD_MAX], len: 1 };
        self.rekey(&mut u, secret, kind, device_id, iterations, rng).await?;
        Ok(u)
    }

    /// Change the PIN/passphrase (new salt, new key) and rewrite.
    pub async fn change_secret(
        &mut self,
        u: &mut Unlocked,
        secret: &[u8],
        kind: SecretKind,
        device_id: &[u8],
        iterations: u32,
        rng: &mut impl Entropy,
    ) -> Result<(), VaultError<S::Error>> {
        self.rekey(u, secret, kind, device_id, iterations, rng).await
    }

    async fn rekey(
        &mut self,
        u: &mut Unlocked,
        secret: &[u8],
        kind: SecretKind,
        device_id: &[u8],
        iterations: u32,
        rng: &mut impl Entropy,
    ) -> Result<(), VaultError<S::Error>> {
        let job = Self::begin_rekey(secret, kind, device_id, iterations, rng);
        let RekeyJob { kdf, salt, kind, iterations } = job;
        u.key = kdf.finish();
        self.new_header(&salt, kind, iterations);
        self.save(u, rng).await
    }

    /// A fresh header for a new salt (generation 0, nothing committed).
    fn new_header(&mut self, salt: &[u8; SALT_LEN], kind: SecretKind, iterations: u32) {
        let mut h = [0xFFu8; HDR];
        h[..4].copy_from_slice(&MAGIC);
        h[O_VERSION] = VERSION;
        h[O_KDF] = KDF_PBKDF2_SHA256;
        h[O_KIND] = match kind {
            SecretKind::Pin => 0,
            SecretKind::Passphrase => 1,
        };
        h[O_KIND + 1] = 0;
        h[O_ITER..O_ITER + 4].copy_from_slice(&iterations.to_le_bytes());
        h[O_GEN..O_GEN + 4].copy_from_slice(&0u32.to_le_bytes());
        h[O_SALT..O_SALT + SALT_LEN].copy_from_slice(salt);
        self.hdr = h;
    }

    /// Encrypt and write `u` (fresh nonce, generation + 1).
    pub async fn save(
        &mut self,
        u: &mut Unlocked,
        rng: &mut impl Entropy,
    ) -> Result<(), VaultError<S::Error>> {
        let len = u.len;
        self.seal(&u.key, &mut u.buf[..len], rng).await
    }

    /// Encrypt `plain` in place, write it, decrypt it back. The nonce is
    /// derived from the key, the next generation and fresh entropy.
    async fn seal(
        &mut self,
        key: &[u8; 32],
        plain: &mut [u8],
        rng: &mut impl Entropy,
    ) -> Result<(), VaultError<S::Error>> {
        let generation = le32(&self.hdr, O_GEN).wrapping_add(1);
        let mut random = [0u8; 32];
        rng.fill(&mut random);
        let mut m = <Hmac<Sha256> as KeyInit>::new_from_slice(key).map_err(|_| VaultError::Corrupt)?;
        m.update(b"orpheus-otp/nonce");
        m.update(&generation.to_le_bytes());
        m.update(&random);
        let n = m.finalize().into_bytes();
        let mut h = self.hdr;
        h[O_GEN..O_GEN + 4].copy_from_slice(&generation.to_le_bytes());
        h[O_LEN..O_LEN + 2].copy_from_slice(&(plain.len() as u16).to_le_bytes());
        h[O_LEN + 2] = 0;
        h[O_LEN + 3] = 0;
        h[O_NONCE..O_NONCE + 24].copy_from_slice(&n[..24]);
        let nonce = XNonce::try_from(&h[O_NONCE..O_NONCE + 24]).map_err(|_| VaultError::Corrupt)?;
        let c = cipher(key);
        let tag = c
            .encrypt_inout_detached(&nonce, &h[..AAD_END], (&mut *plain).into())
            .map_err(|_| VaultError::Full)?;
        h[O_TAG..O_TAG + 16].copy_from_slice(&tag);
        h[O_COMMIT..O_COMMIT + 4].fill(0xFF);
        let r = self.write_sector(&h, &*plain).await;
        // Back to plaintext whatever happened (the tag is ours, so this
        // cannot fail).
        let _ = c.decrypt_inout_detached(&nonce, &h[..AAD_END], (&mut *plain).into(), &tag);
        r?;
        self.hdr = h;
        self.hdr[O_COMMIT..O_COMMIT + 4].fill(0);
        self.valid = true;
        self.attempts = 0;
        self.failures = 0;
        self.not_before_ms = 0;
        Ok(())
    }

    /// Erase the sector (wipe the vault).
    pub async fn wipe(&mut self) -> Result<(), VaultError<S::Error>> {
        self.valid = false;
        self.hdr = [0xFF; HDR];
        self.storage.erase().await.map_err(VaultError::Storage)
    }

    /// Advance an HOTP entry: the counter is persisted *before* the code
    /// is returned, so a code is never shown twice.
    pub async fn hotp_next(
        &mut self,
        u: &mut Unlocked,
        index: usize,
        rng: &mut impl Entropy,
    ) -> Result<Code, VaultError<S::Error>> {
        let e = u.entry(index).ok_or(VaultError::BadIndex)?;
        let Kind::Hotp { counter } = e.kind else { return Err(VaultError::BadIndex) };
        u.set_counter(index, counter.wrapping_add(1))?;
        self.save(u, rng).await?;
        Ok(e.code(0))
    }

    async fn write_sector(&mut self, h: &[u8; HDR], payload: &[u8]) -> Result<(), VaultError<S::Error>> {
        let st = &mut self.storage;
        st.erase().await.map_err(VaultError::Storage)?;
        st.program(0, &h[..O_COMMIT]).await.map_err(VaultError::Storage)?;
        let whole = payload.len() / 4 * 4;
        if whole > 0 {
            st.program(O_PAYLOAD as u32, &payload[..whole]).await.map_err(VaultError::Storage)?;
        }
        if whole < payload.len() {
            let mut tail = [0xFFu8; 4];
            tail[..payload.len() - whole].copy_from_slice(&payload[whole..]);
            st.program((O_PAYLOAD + whole) as u32, &tail).await.map_err(VaultError::Storage)?;
        }
        st.program(O_COMMIT as u32, &[0; 4]).await.map_err(VaultError::Storage)
    }

    /// Rewrite header and ciphertext unchanged with fresh bitmaps.
    async fn compact(&mut self) -> Result<(), VaultError<S::Error>> {
        let len = usize::from(u16::from_le_bytes([self.hdr[O_LEN], self.hdr[O_LEN + 1]]));
        let mut ct = [0u8; PAYLOAD_MAX];
        self.storage.read(O_PAYLOAD as u32, &mut ct[..len]).await.map_err(VaultError::Storage)?;
        let h = self.hdr;
        self.write_sector(&h, &ct[..len]).await?;
        self.attempts = 0;
        Ok(())
    }

    async fn clear_bit(&mut self, map: usize, k: u32) -> Result<(), VaultError<S::Error>> {
        let word = !(1u32 << (k % 32));
        let off = map + (k / 32) as usize * 4;
        self.storage.program(off as u32, &word.to_le_bytes()).await.map_err(VaultError::Storage)
    }
}

impl Unlocked {
    /// Number of entries.
    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.buf[0])
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Payload bytes in use (of [`PAYLOAD_MAX`]).
    #[must_use]
    pub fn used(&self) -> usize {
        self.len
    }

    /// A copy of entry `i`.
    #[must_use]
    pub fn entry(&self, i: usize) -> Option<Entry> {
        rec_entry(&self.buf, self.len, i)
    }

    /// Append an entry (in RAM; [`Vault::save`] writes it).
    pub fn push<E>(&mut self, e: &Entry) -> Result<(), VaultError<E>> {
        rec_push(&mut self.buf, &mut self.len, e)
    }

    /// Remove entry `i` (in RAM).
    pub fn remove<E>(&mut self, i: usize) -> Result<(), VaultError<E>> {
        rec_remove(&mut self.buf, &mut self.len, i)
    }

    /// Set the HOTP counter of entry `i` (in RAM).
    pub fn set_counter<E>(&mut self, i: usize, counter: u64) -> Result<(), VaultError<E>> {
        rec_set_counter(&mut self.buf, self.len, i, counter)
    }
}

// ------------------------------------------------------------ records
//
// The payload's record list, shared by `Unlocked` (OTP-1, the whole
// payload in its own buffer) and `Payload` (the firmware's: the caller's
// buffer, OTP-2).

/// Byte range of record `i` (including its length byte).
fn rec_range(buf: &[u8], len: usize, i: usize) -> Option<(usize, usize)> {
    if i >= usize::from(*buf.first()?) {
        return None;
    }
    let mut o = 1;
    for _ in 0..i {
        o += 1 + usize::from(*buf.get(o)?);
    }
    let n = 1 + usize::from(*buf.get(o)?);
    (o + n <= len).then_some((o, o + n))
}

/// Decode a record body (after its length byte).
fn rec_decode(r: &[u8]) -> Option<Entry> {
    if r.len() < REC_FIXED {
        return None;
    }
    let alg = Algorithm::from_id(r[1])?;
    let period = u32::from(u16::from_le_bytes([r[3], r[4]]));
    let counter = u64::from_le_bytes(r[5..13].try_into().ok()?);
    let kind = match r[0] {
        0 => Kind::Totp { period },
        1 => Kind::Hotp { counter },
        _ => return None,
    };
    let (sl, il, al) = (usize::from(r[13]), usize::from(r[14]), usize::from(r[15]));
    let body = r.get(16..16 + sl + il + al)?;
    let issuer = core::str::from_utf8(&body[sl..sl + il]).ok()?;
    let account = core::str::from_utf8(&body[sl + il..]).ok()?;
    Entry::new(kind, alg, r[2], &body[..sl], issuer, account).ok()
}

fn rec_entry(buf: &[u8], len: usize, i: usize) -> Option<Entry> {
    let (s, e) = rec_range(buf, len, i)?;
    rec_decode(&buf[s + 1..e])
}

fn rec_push<E>(buf: &mut [u8], len: &mut usize, e: &Entry) -> Result<(), VaultError<E>> {
    if usize::from(buf[0]) >= MAX_ENTRIES {
        return Err(VaultError::TooManyEntries);
    }
    let (secret, issuer, account) = (e.secret(), e.issuer.as_str().as_bytes(), e.account.as_str().as_bytes());
    debug_assert!(secret.len() <= MAX_SECRET && issuer.len() <= MAX_ISSUER && account.len() <= MAX_ACCOUNT);
    let body = 1 + REC_FIXED + secret.len() + issuer.len() + account.len();
    let o = *len;
    if o + body > buf.len().min(PAYLOAD_MAX) {
        return Err(VaultError::Full);
    }
    let (kind, period, counter) = match e.kind {
        Kind::Totp { period } => (0u8, period as u16, 0u64),
        Kind::Hotp { counter } => (1u8, 0u16, counter),
    };
    let b = &mut buf[o..o + body];
    b[0] = (body - 1) as u8;
    b[1] = kind;
    b[2] = e.alg.id();
    b[3] = e.digits;
    b[4..6].copy_from_slice(&period.to_le_bytes());
    b[6..14].copy_from_slice(&counter.to_le_bytes());
    b[14] = secret.len() as u8;
    b[15] = issuer.len() as u8;
    b[16] = account.len() as u8;
    let mut p = REC_FIXED + 1;
    for s in [secret, issuer, account] {
        b[p..p + s.len()].copy_from_slice(s);
        p += s.len();
    }
    *len += body;
    buf[0] += 1;
    Ok(())
}

fn rec_remove<E>(buf: &mut [u8], len: &mut usize, i: usize) -> Result<(), VaultError<E>> {
    let (s, e) = rec_range(buf, *len, i).ok_or(VaultError::BadIndex)?;
    buf.copy_within(e..*len, s);
    let n = e - s;
    *len -= n;
    buf[*len..*len + n].zeroize();
    buf[0] -= 1;
    Ok(())
}

fn rec_set_counter<E>(buf: &mut [u8], len: usize, i: usize, counter: u64) -> Result<(), VaultError<E>> {
    let (s, _) = rec_range(buf, len, i).ok_or(VaultError::BadIndex)?;
    if buf[s + 1] != 1 {
        return Err(VaultError::BadIndex);
    }
    buf[s + 6..s + 14].copy_from_slice(&counter.to_le_bytes());
    Ok(())
}

/// Exchange records `i` and `i + 1` (Move up/down in the app).
fn rec_swap_next<E>(buf: &mut [u8], len: usize, i: usize) -> Result<(), VaultError<E>> {
    let (a, m) = rec_range(buf, len, i).ok_or(VaultError::BadIndex)?;
    let (_, b) = rec_range(buf, len, i + 1).ok_or(VaultError::BadIndex)?;
    // Rotate [a, b) left by the first record's length.
    buf[a..b].rotate_left(m - a);
    Ok(())
}

// ------------------------------------------------- key-only access (OTP-2)
//
// The firmware cannot spare the 3.9 KiB of an `Unlocked` while the app is
// open (06 §5.2, RAM budget of OTP-2). It keeps only the [`Key`], checks
// the tag at unlock by streaming the sector through Poly1305, and decrypts
// single records on demand with the stream cipher positioned at their
// offset. Changes decrypt the whole payload into a buffer the caller lends
// for the duration ([`Payload`], wiped when dropped). The sector format is
// the same; this is only another way to read and write it.

/// Size of one record, the largest there can be.
pub const RECORD_MAX: usize = 1 + REC_FIXED + MAX_SECRET + MAX_ISSUER + MAX_ACCOUNT;

/// The vault key. Wiped on drop: drop it on lock, screen-off and before a
/// reboot into ADFU.
pub struct Key([u8; 32]);

impl Drop for Key {
    fn drop(&mut self) {
        self.0.zeroize();
    }
}

/// A started create or change of PIN: a new salt, the derivation in steps
/// ([`RekeyJob::step`]) so the caller can yield.
pub struct RekeyJob {
    kdf: Pbkdf2,
    salt: [u8; SALT_LEN],
    kind: SecretKind,
    iterations: u32,
}

impl RekeyJob {
    /// Run up to `n` iterations; `true` when the key is ready.
    pub fn step(&mut self, n: u32) -> bool {
        self.kdf.step(n)
    }

    /// Iterations still to run, of [`RekeyJob::iterations`].
    #[must_use]
    pub fn remaining(&self) -> u32 {
        self.kdf.remaining()
    }

    #[must_use]
    pub fn iterations(&self) -> u32 {
        self.iterations
    }
}

/// The decrypted payload in a buffer the caller lent; the buffer is wiped
/// when this is dropped.
pub struct Payload<'b> {
    buf: &'b mut [u8],
    len: usize,
}

impl Drop for Payload<'_> {
    fn drop(&mut self) {
        self.buf.zeroize();
    }
}

impl<'b> Payload<'b> {
    /// An empty payload in `buf` (at least one byte; at most
    /// [`PAYLOAD_MAX`] of it are used).
    pub fn empty<E>(buf: &'b mut [u8]) -> Result<Self, VaultError<E>> {
        let first = buf.first_mut().ok_or(VaultError::Full)?;
        *first = 0;
        Ok(Payload { buf, len: 1 })
    }

    #[must_use]
    pub fn len(&self) -> usize {
        usize::from(self.buf[0])
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Payload bytes in use.
    #[must_use]
    pub fn used(&self) -> usize {
        self.len
    }

    #[must_use]
    pub fn entry(&self, i: usize) -> Option<Entry> {
        rec_entry(self.buf, self.len, i)
    }

    pub fn push<E>(&mut self, e: &Entry) -> Result<(), VaultError<E>> {
        rec_push(self.buf, &mut self.len, e)
    }

    pub fn remove<E>(&mut self, i: usize) -> Result<(), VaultError<E>> {
        rec_remove(self.buf, &mut self.len, i)
    }

    pub fn set_counter<E>(&mut self, i: usize, counter: u64) -> Result<(), VaultError<E>> {
        rec_set_counter(self.buf, self.len, i, counter)
    }

    /// Exchange entries `i` and `i + 1`.
    pub fn swap_next<E>(&mut self, i: usize) -> Result<(), VaultError<E>> {
        rec_swap_next(self.buf, self.len, i)
    }

    /// An entry with the same secret, issuer and account is already there
    /// (imports skip it).
    #[must_use]
    pub fn contains(&self, e: &Entry) -> bool {
        (0..self.len())
            .filter_map(|i| self.entry(i))
            .any(|x| x.secret() == e.secret() && x.issuer == e.issuer && x.account == e.account)
    }
}

fn stream(key: &[u8; 32], nonce: &[u8]) -> XChaCha20 {
    let mut n = [0u8; 24];
    n.copy_from_slice(&nonce[..24]);
    <XChaCha20 as KeyIvInit>::new(&(*key).into(), &n.into())
}

impl<S: Storage> Vault<S> {
    /// Start a create or a change of PIN: draws a new salt.
    pub fn begin_rekey(
        secret: &[u8],
        kind: SecretKind,
        device_id: &[u8],
        iterations: u32,
        rng: &mut impl Entropy,
    ) -> RekeyJob {
        let iterations = iterations.clamp(kdf::MIN_ITERATIONS, kdf::MAX_ITERATIONS);
        let mut salt = [0u8; SALT_LEN];
        rng.fill(&mut salt);
        RekeyJob { kdf: Pbkdf2::new(secret, &salt, device_id, iterations), salt, kind, iterations }
    }

    /// Create an empty vault (erasing whatever was there) with a finished
    /// [`RekeyJob`]; returns its key.
    pub async fn finish_create(
        &mut self,
        job: RekeyJob,
        rng: &mut impl Entropy,
    ) -> Result<Key, VaultError<S::Error>> {
        let RekeyJob { kdf, salt, kind, iterations } = job;
        let key = Key(kdf.finish());
        self.new_header(&salt, kind, iterations);
        let mut empty = [0u8; 1];
        self.seal(&key.0, &mut empty, rng).await?;
        Ok(key)
    }

    /// Change the PIN: re-encrypt `p` (the payload opened with the old
    /// key) under a new salt and key; returns the new key.
    pub async fn finish_rekey(
        &mut self,
        job: RekeyJob,
        p: &mut Payload<'_>,
        rng: &mut impl Entropy,
    ) -> Result<Key, VaultError<S::Error>> {
        let RekeyJob { kdf, salt, kind, iterations } = job;
        let key = Key(kdf.finish());
        let old = self.hdr;
        self.new_header(&salt, kind, iterations);
        let r = self.seal(&key.0, &mut p.buf[..p.len], rng).await;
        if r.is_err() {
            self.hdr = old;
        }
        r.map(|()| key)
    }

    /// [`finish_unlock`](Self::finish_unlock) without the plaintext: the
    /// tag is checked by streaming the ciphertext through `chunk` (any
    /// size of 16 bytes or more). A bitmap that needs compacting is left
    /// for [`compact_with`](Self::compact_with) ([`needs_compaction`](Self::needs_compaction)).
    pub async fn finish_unlock_key(
        &mut self,
        job: UnlockJob,
        now_ms: u64,
        chunk: &mut [u8],
    ) -> Result<Key, VaultError<S::Error>> {
        let UnlockJob { kdf, attempt } = job;
        let key = Key(kdf.finish());
        if !self.verify_stream(&key, chunk).await? {
            self.not_before_ms = now_ms + u64::from(backoff::delay_s(self.failures)) * 1000;
            return Err(VaultError::WrongKey);
        }
        if self.count(&key).await? > MAX_ENTRIES {
            return Err(VaultError::Corrupt);
        }
        if attempt < BITS {
            self.clear_bit(O_SUCCESS, attempt).await?;
        }
        self.failures = 0;
        self.not_before_ms = 0;
        Ok(key)
    }

    /// Three quarters of the attempt bitmap are used: compact at the next
    /// chance ([`compact_with`](Self::compact_with)).
    #[must_use]
    pub fn needs_compaction(&self) -> bool {
        self.valid && self.attempts >= COMPACT_AT
    }

    /// Rewrite header and ciphertext with fresh bitmaps, through `buf`
    /// (at least the payload length; wiped after).
    pub async fn compact_with(&mut self, buf: &mut [u8]) -> Result<(), VaultError<S::Error>> {
        let len = self.payload_len();
        let ct = buf.get_mut(..len).ok_or(VaultError::Full)?;
        self.storage.read(O_PAYLOAD as u32, ct).await.map_err(VaultError::Storage)?;
        let h = self.hdr;
        let r = self.write_sector(&h, ct).await;
        ct.zeroize();
        r?;
        self.attempts = 0;
        Ok(())
    }

    fn payload_len(&self) -> usize {
        usize::from(u16::from_le_bytes([self.hdr[O_LEN], self.hdr[O_LEN + 1]])).min(PAYLOAD_MAX)
    }

    /// Check the tag over header and ciphertext without decrypting.
    async fn verify_stream(&mut self, key: &Key, chunk: &mut [u8]) -> Result<bool, VaultError<S::Error>> {
        let step = chunk.len() / 16 * 16;
        if step == 0 {
            return Err(VaultError::Full);
        }
        let len = self.payload_len();
        let mut c = stream(&key.0, &self.hdr[O_NONCE..O_NONCE + 24]);
        let mut mac_key = [0u8; 32];
        c.try_apply_keystream(&mut mac_key).map_err(|_| VaultError::Corrupt)?;
        let mut mac = <Poly1305 as poly1305::universal_hash::KeyInit>::new(&mac_key.into());
        mac_key.zeroize();
        mac.update_padded(&self.hdr[..AAD_END]);
        let mut off = 0;
        while off < len {
            let n = step.min(len - off);
            self.storage
                .read((O_PAYLOAD + off) as u32, &mut chunk[..n])
                .await
                .map_err(VaultError::Storage)?;
            mac.update_padded(&chunk[..n]);
            off += n;
        }
        let mut lens = [0u8; 16];
        lens[..8].copy_from_slice(&(AAD_END as u64).to_le_bytes());
        lens[8..].copy_from_slice(&(len as u64).to_le_bytes());
        mac.update_padded(&lens);
        let mut tag = [0u8; 16];
        tag.copy_from_slice(&self.hdr[O_TAG..O_TAG + 16]);
        Ok(mac.verify(&tag.into()).is_ok())
    }

    /// Decrypt payload bytes `off..off + out.len()` from the sector.
    async fn read_plain(
        &mut self,
        key: &Key,
        off: usize,
        out: &mut [u8],
    ) -> Result<(), VaultError<S::Error>> {
        if off + out.len() > self.payload_len() {
            return Err(VaultError::Corrupt);
        }
        self.storage.read((O_PAYLOAD + off) as u32, out).await.map_err(VaultError::Storage)?;
        let mut c = stream(&key.0, &self.hdr[O_NONCE..O_NONCE + 24]);
        c.try_seek(64 + off as u64).map_err(|_| VaultError::Corrupt)?;
        c.try_apply_keystream(out).map_err(|_| VaultError::Corrupt)?;
        Ok(())
    }

    /// Number of entries (unlocked with `key`).
    pub async fn count(&mut self, key: &Key) -> Result<usize, VaultError<S::Error>> {
        let mut b = [0u8; 1];
        self.read_plain(key, 0, &mut b).await?;
        Ok(usize::from(b[0]))
    }

    /// Entry `i`, decrypted from the sector (the record only).
    pub async fn read_entry(&mut self, key: &Key, i: usize) -> Result<Entry, VaultError<S::Error>> {
        if i >= self.count(key).await? {
            return Err(VaultError::BadIndex);
        }
        let len = self.payload_len();
        let mut o = 1usize;
        let mut b = [0u8; 1];
        for _ in 0..i {
            self.read_plain(key, o, &mut b).await?;
            o += 1 + usize::from(b[0]);
        }
        self.read_plain(key, o, &mut b).await?;
        let n = 1 + usize::from(b[0]);
        if o + n > len || n > RECORD_MAX {
            return Err(VaultError::Corrupt);
        }
        let mut rec = [0u8; RECORD_MAX];
        self.read_plain(key, o, &mut rec[..n]).await?;
        let e = rec_decode(&rec[1..n]);
        rec.zeroize();
        e.ok_or(VaultError::Corrupt)
    }

    /// Decrypt the whole payload into `buf` (wiped when the returned
    /// [`Payload`] is dropped). `buf` must hold [`PAYLOAD_MAX`] bytes so
    /// entries can be added.
    pub async fn open<'b>(
        &mut self,
        key: &Key,
        buf: &'b mut [u8],
    ) -> Result<Payload<'b>, VaultError<S::Error>> {
        let len = self.payload_len();
        if buf.len() < PAYLOAD_MAX {
            return Err(VaultError::Full);
        }
        let p = Payload { buf, len };
        self.storage.read(O_PAYLOAD as u32, &mut p.buf[..len]).await.map_err(VaultError::Storage)?;
        let nonce = XNonce::try_from(&self.hdr[O_NONCE..O_NONCE + 24]).map_err(|_| VaultError::Corrupt)?;
        let tag =
            chacha20poly1305::Tag::try_from(&self.hdr[O_TAG..O_TAG + 16]).map_err(|_| VaultError::Corrupt)?;
        cipher(&key.0)
            .decrypt_inout_detached(&nonce, &self.hdr[..AAD_END], (&mut p.buf[..len]).into(), &tag)
            .map_err(|_| VaultError::WrongKey)?;
        if len == 0 || usize::from(p.buf[0]) > MAX_ENTRIES {
            return Err(VaultError::Corrupt);
        }
        Ok(p)
    }

    /// Encrypt and write `p` (fresh nonce, generation + 1); `p` is
    /// plaintext again afterwards.
    pub async fn save_payload(
        &mut self,
        key: &Key,
        p: &mut Payload<'_>,
        rng: &mut impl Entropy,
    ) -> Result<(), VaultError<S::Error>> {
        let len = p.len;
        self.seal(&key.0, &mut p.buf[..len], rng).await
    }

    /// Advance HOTP entry `index` in `p` and write; the code is returned
    /// only after the counter is stored.
    pub async fn hotp_next_payload(
        &mut self,
        key: &Key,
        p: &mut Payload<'_>,
        index: usize,
        rng: &mut impl Entropy,
    ) -> Result<Code, VaultError<S::Error>> {
        let e = p.entry(index).ok_or(VaultError::BadIndex)?;
        let Kind::Hotp { counter } = e.kind else { return Err(VaultError::BadIndex) };
        p.set_counter(index, counter.wrapping_add(1))?;
        self.save_payload(key, p, rng).await?;
        Ok(e.code(0))
    }
}

/// Entropy for salts and nonces from events the firmware sees (key-press
/// times in microseconds, battery readings, the clock): each is hashed into
/// a 32-byte pool; output is HMAC-SHA-256 of the pool over a counter, and
/// the pool is ratcheted after each request. The player has no known
/// hardware random source; nonce uniqueness does not depend on this
/// (module docs), salt quality does.
pub struct Pool {
    state: [u8; 32],
    counter: u64,
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.state.zeroize();
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::new()
    }
}

impl Pool {
    #[must_use]
    pub const fn new() -> Self {
        Pool { state: [0; 32], counter: 0 }
    }

    /// Hash `data` into the pool.
    pub fn mix(&mut self, data: &[u8]) {
        use sha2::Digest;
        let mut h = Sha256::new();
        h.update(self.state);
        h.update(data);
        self.state.copy_from_slice(&h.finalize());
    }
}

impl Entropy for Pool {
    fn fill(&mut self, buf: &mut [u8]) {
        for chunk in buf.chunks_mut(32) {
            self.counter += 1;
            let Ok(mut m) = <Hmac<Sha256> as KeyInit>::new_from_slice(&self.state) else { return };
            m.update(b"orpheus-otp/pool");
            m.update(&self.counter.to_le_bytes());
            let out = m.finalize().into_bytes();
            chunk.copy_from_slice(&out[..chunk.len()]);
        }
        let c = self.counter.to_le_bytes();
        self.mix(&c);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::block_on;
    use std::vec::Vec;

    /// NOR model: erase sets 0xFF, program ANDs, alignment checked; can
    /// simulate a power cut after `cut_after` programs.
    struct Nor {
        mem: Vec<u8>,
        erases: u32,
        programs: u32,
        cut_after: Option<u32>,
    }
    #[derive(Debug, PartialEq, Eq, Clone, Copy)]
    struct Cut;
    impl Nor {
        fn new() -> Self {
            Self { mem: std::vec![0xFF; SECTOR], erases: 0, programs: 0, cut_after: None }
        }
    }
    impl Storage for &mut Nor {
        type Error = Cut;
        async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), Cut> {
            let o = off as usize;
            buf.copy_from_slice(&self.mem[o..o + buf.len()]);
            Ok(())
        }
        async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), Cut> {
            assert_eq!(off % 4, 0);
            assert_eq!(data.len() % 4, 0);
            if self.cut_after == Some(self.programs) {
                return Err(Cut);
            }
            self.programs += 1;
            for (m, d) in self.mem[off as usize..].iter_mut().zip(data) {
                *m &= *d;
            }
            Ok(())
        }
        async fn erase(&mut self) -> Result<(), Cut> {
            self.erases += 1;
            self.mem.fill(0xFF);
            Ok(())
        }
    }

    struct Rng(u64);
    impl Entropy for Rng {
        fn fill(&mut self, buf: &mut [u8]) {
            for b in buf {
                self.0 = self.0.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
                *b = (self.0 >> 33) as u8;
            }
        }
    }

    const DEV: &[u8] = b"\x11\x22\x33\x44\x55\x66\x77\x88";
    const IT: u32 = 1000;

    fn totp(issuer: &str, secret: &[u8]) -> Entry {
        Entry::new(Kind::Totp { period: 30 }, Algorithm::Sha1, 6, secret, issuer, "me@example.org").unwrap()
    }

    #[test]
    fn round_trip() {
        block_on(async {
            let mut nor = Nor::new();
            let mut rng = Rng(1);
            let mut v = Vault::new(&mut nor);
            assert_eq!(v.load(0).await.unwrap(), Status::Absent);
            let mut u = v.create(b"123456", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap();
            assert!(u.is_empty());
            u.push::<Cut>(&totp("GitHub", b"12345678901234567890")).unwrap();
            let h = Entry::new(Kind::Hotp { counter: 5 }, Algorithm::Sha512, 8, &[0xAB; 64], "Bank €", "x")
                .unwrap();
            u.push::<Cut>(&h).unwrap();
            v.save(&mut u, &mut rng).await.unwrap();
            drop(u);

            let mut v = Vault::new(&mut nor);
            assert_eq!(v.load(0).await.unwrap(), Status::Locked { kind: SecretKind::Pin, failures: 0 });
            let mut u = v.unlock(b"123456", DEV, 0).await.unwrap();
            assert_eq!(u.len(), 2);
            let e0 = u.entry(0).unwrap();
            assert_eq!(e0, totp("GitHub", b"12345678901234567890"));
            assert_eq!(e0.code(59).as_str(), "287082");
            assert_eq!(u.entry(1).unwrap(), h);

            // HOTP advances and persists before the code is returned.
            let want = crate::otp::hotp(Algorithm::Sha512, &[0xAB; 64], 5, 8);
            let c = v.hotp_next(&mut u, 1, &mut rng).await.unwrap();
            assert_eq!(c, Code::new(want, 8));
            assert_eq!(u.entry(1).unwrap().kind, Kind::Hotp { counter: 6 });
            u.remove::<Cut>(0).unwrap();
            v.save(&mut u, &mut rng).await.unwrap();
            drop(u);
            let mut v = Vault::new(&mut nor);
            v.load(0).await.unwrap();
            let u = v.unlock(b"123456", DEV, 0).await.unwrap();
            assert_eq!(u.len(), 1);
            assert_eq!(u.entry(0).unwrap().kind, Kind::Hotp { counter: 6 });
            assert!(u.entry(1).is_none());
        });
    }

    #[test]
    fn plaintext_never_on_flash() {
        block_on(async {
            let mut nor = Nor::new();
            let mut rng = Rng(2);
            let mut v = Vault::new(&mut nor);
            let mut u = v.create(b"pw", SecretKind::Passphrase, DEV, IT, &mut rng).await.unwrap();
            u.push::<Cut>(&totp("VisibleIssuer", b"SECRETSECRETSECRET")).unwrap();
            v.save(&mut u, &mut rng).await.unwrap();
            drop(u);
            let hay = &nor.mem;
            for needle in [&b"VisibleIssuer"[..], b"SECRETSECRET", b"me@example"] {
                assert!(!hay.windows(needle.len()).any(|w| w == needle));
            }
        });
    }

    #[test]
    fn wrong_key_and_device_rejected() {
        block_on(async {
            let mut nor = Nor::new();
            let mut rng = Rng(3);
            let mut v = Vault::new(&mut nor);
            drop(v.create(b"123456", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap());
            v.load(0).await.unwrap();
            assert_eq!(v.unlock(b"654321", DEV, 0).await.err(), Some(VaultError::WrongKey));
            assert_eq!(v.failures(), 1);
            // Same PIN, other device id: different key.
            assert_eq!(v.unlock(b"123456", b"other", 5000).await.err(), Some(VaultError::WrongKey));
            assert_eq!(v.failures(), 2);
            assert!(v.unlock(b"123456", DEV, 7000).await.is_ok());
            assert_eq!(v.failures(), 0);
        });
    }

    #[test]
    fn tamper_detected() {
        // Flip one bit anywhere in the authenticated header or the
        // ciphertext: the right PIN no longer opens the vault.
        let mut base = Nor::new();
        let mut rng = Rng(4);
        block_on(async {
            let mut v = Vault::new(&mut base);
            let mut u = v.create(b"1111", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap();
            u.push::<Cut>(&totp("A", b"12345678901234567890")).unwrap();
            v.save(&mut u, &mut rng).await.unwrap();
        });
        let len = usize::from(u16::from_le_bytes([base.mem[O_LEN], base.mem[O_LEN + 1]]));
        let spots = [O_KIND, O_GEN, O_SALT + 3, O_NONCE + 23, O_TAG, O_PAYLOAD, O_PAYLOAD + len - 1];
        for &at in &spots {
            let mut nor = Nor { mem: base.mem.clone(), erases: 0, programs: 0, cut_after: None };
            nor.mem[at] ^= 0x01;
            block_on(async {
                let mut v = Vault::new(&mut nor);
                match v.load(0).await {
                    Ok(Status::Locked { .. }) => {
                        assert_eq!(
                            v.unlock(b"1111", DEV, 0).await.err(),
                            Some(VaultError::WrongKey),
                            "at {at:#x}"
                        );
                    }
                    Ok(Status::Damaged) => {}
                    other => panic!("at {at:#x}: {other:?}"),
                }
            });
        }
        // Another valid iteration count is authenticated too.
        let mut nor = Nor { mem: base.mem.clone(), erases: 0, programs: 0, cut_after: None };
        nor.mem[O_ITER..O_ITER + 4].copy_from_slice(&2000u32.to_le_bytes());
        block_on(async {
            let mut v = Vault::new(&mut nor);
            v.load(0).await.unwrap();
            assert_eq!(v.iterations(), 2000);
            assert_eq!(v.unlock(b"1111", DEV, 0).await.err(), Some(VaultError::WrongKey));
        });
    }

    #[test]
    fn backoff_is_persisted_and_enforced() {
        block_on(async {
            let mut nor = Nor::new();
            let mut rng = Rng(5);
            let mut v = Vault::new(&mut nor);
            drop(v.create(b"2468", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap());
            v.load(0).await.unwrap();
            let mut now = 0u64;
            for f in 1..=4u32 {
                assert_eq!(v.unlock(b"0000", DEV, now).await.err(), Some(VaultError::WrongKey));
                assert_eq!(v.failures(), f);
                let wait = u64::from(backoff::delay_s(f)) * 1000;
                assert_eq!(v.wait_ms(now), wait);
                assert_eq!(
                    v.unlock(b"2468", DEV, now + wait - 1).await.err(),
                    Some(VaultError::Backoff { wait_ms: 1 })
                );
                now += wait;
            }
            let erases = nor.erases;
            // A reboot keeps the count and restarts the delay from boot.
            let mut v = Vault::new(&mut nor);
            assert_eq!(v.load(100).await.unwrap(), Status::Locked { kind: SecretKind::Pin, failures: 4 });
            assert_eq!(v.wait_ms(100), 8000);
            // A reset during the key derivation counts as a failure.
            let job = v.begin_unlock(b"2468", DEV, 8100).await.unwrap();
            drop(job);
            let mut v = Vault::new(&mut nor);
            assert_eq!(v.load(0).await.unwrap(), Status::Locked { kind: SecretKind::Pin, failures: 5 });
            assert!(v.unlock(b"2468", DEV, 16_000).await.is_ok());
            let mut v = Vault::new(&mut nor);
            assert_eq!(v.load(0).await.unwrap(), Status::Locked { kind: SecretKind::Pin, failures: 0 });
            // No erase was needed for any of this.
            assert_eq!(nor.erases, erases);
        });
    }

    #[test]
    fn bitmap_compaction() {
        block_on(async {
            let mut nor = Nor::new();
            let mut rng = Rng(6);
            let mut v = Vault::new(&mut nor);
            let mut u = v.create(b"9", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap();
            u.push::<Cut>(&totp("K", b"abcdefghij")).unwrap();
            v.save(&mut u, &mut rng).await.unwrap();
            drop(u);
            v.load(0).await.unwrap();
            for _ in 0..COMPACT_AT {
                let u = v.unlock(b"9", DEV, 0).await.unwrap();
                assert_eq!(u.entry(0).unwrap().issuer.as_str(), "K");
            }
            let erases = nor.erases;
            let mut v = Vault::new(&mut nor);
            v.load(0).await.unwrap();
            assert_eq!(v.attempts, 0, "compacted after {COMPACT_AT} attempts (erases {erases})");
            assert!(v.unlock(b"9", DEV, 0).await.is_ok());
        });
    }

    #[test]
    fn capacity_limits() {
        block_on(async {
            let mut nor = Nor::new();
            let mut rng = Rng(7);
            let mut v = Vault::new(&mut nor);
            let mut u = v.create(b"1", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap();
            // 32 typical entries (20-byte secret, short labels) fit.
            for i in 0..MAX_ENTRIES {
                let issuer = std::format!("Service {i}");
                u.push::<Cut>(&totp(&issuer, b"12345678901234567890")).unwrap();
            }
            assert_eq!(u.push::<Cut>(&totp("one more", b"x")), Err(VaultError::TooManyEntries));
            v.save(&mut u, &mut rng).await.unwrap();
            // Largest entries: 176 bytes each; the payload fills before 32.
            let mut u = v.create(b"1", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap();
            let big = Entry::new(
                Kind::Totp { period: 30 },
                Algorithm::Sha512,
                8,
                &[1; MAX_SECRET],
                &"i".repeat(MAX_ISSUER),
                &"a".repeat(MAX_ACCOUNT),
            )
            .unwrap();
            let mut n = 0;
            while u.push::<Cut>(&big).is_ok() {
                n += 1;
            }
            assert_eq!(n, (PAYLOAD_MAX - 1) / (1 + 16 + MAX_SECRET + MAX_ISSUER + MAX_ACCOUNT));
            v.save(&mut u, &mut rng).await.unwrap();
            drop(u);
            v.load(0).await.unwrap();
            let u = v.unlock(b"1", DEV, 0).await.unwrap();
            assert_eq!(u.len(), n);
            assert_eq!(u.entry(n - 1).unwrap(), big);
        });
    }

    #[test]
    fn torn_write_is_damaged_not_misread() {
        for cut in 0..3 {
            let mut nor = Nor::new();
            let mut rng = Rng(8);
            block_on(async {
                let mut v = Vault::new(&mut nor);
                drop(v.create(b"1", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap());
            });
            // The unlock below programs two bitmap words first.
            nor.cut_after = Some(nor.programs + 2 + cut);
            block_on(async {
                let mut v = Vault::new(&mut nor);
                v.load(0).await.unwrap();
                let mut u = v.unlock(b"1", DEV, 0).await.unwrap();
                u.push::<Cut>(&totp("x", b"y")).unwrap();
                assert_eq!(v.save(&mut u, &mut rng).await, Err(VaultError::Storage(Cut)));
                // The RAM copy is plaintext again after the failed write.
                assert_eq!(u.entry(0).unwrap().issuer.as_str(), "x");
            });
            nor.cut_after = None;
            block_on(async {
                let mut v = Vault::new(&mut nor);
                // Erased-then-cut reads as absent, later cuts as damaged;
                // never as a vault.
                let st = v.load(0).await.unwrap();
                assert_eq!(st, if cut == 0 { Status::Absent } else { Status::Damaged }, "cut {cut}");
                assert_eq!(v.unlock(b"1", DEV, 0).await.err(), Some(VaultError::NoVault));
            });
        }
    }

    #[test]
    fn nonces_differ_between_writes() {
        block_on(async {
            let mut nor = Nor::new();
            let mut rng = Rng(9);
            let mut v = Vault::new(&mut nor);
            let mut u = v.create(b"1", SecretKind::Pin, DEV, IT, &mut rng).await.unwrap();
            let mut seen = std::collections::HashSet::new();
            for _ in 0..20 {
                v.save(&mut u, &mut rng).await.unwrap();
                assert!(seen.insert(v.hdr[O_NONCE..O_NONCE + 24].to_vec()));
            }
            assert_eq!(le32(&v.hdr, O_GEN), 21);
        });
    }

    // ------------------------------------------- key-only access (OTP-2)

    fn hotp(issuer: &str, counter: u64) -> Entry {
        Entry::new(Kind::Hotp { counter }, Algorithm::Sha256, 8, b"hotp-secret-0123", issuer, "acct").unwrap()
    }

    /// A vault with three entries written through `Unlocked` (OTP-1's path).
    fn three(nor: &mut Nor, rng: &mut Rng) {
        block_on(async {
            let mut v = Vault::new(nor);
            let mut u = v.create(b"2468", SecretKind::Pin, DEV, IT, rng).await.unwrap();
            u.push::<Cut>(&totp("GitHub", b"12345678901234567890")).unwrap();
            u.push::<Cut>(&hotp("Bank", 7)).unwrap();
            u.push::<Cut>(&totp("Mail €", &[0x5A; 64])).unwrap();
            v.save(&mut u, rng).await.unwrap();
        });
    }

    async fn key_unlock(v: &mut Vault<&mut Nor>, pin: &[u8], chunk: usize) -> Result<Key, VaultError<Cut>> {
        v.load(0).await.unwrap();
        let mut job = v.begin_unlock(pin, DEV, 0).await?;
        while !job.step(100) {}
        let mut c = std::vec![0u8; chunk];
        v.finish_unlock_key(job, 0, &mut c).await
    }

    #[test]
    fn key_path_reads_what_unlocked_wrote() {
        let mut nor = Nor::new();
        let mut rng = Rng(11);
        three(&mut nor, &mut rng);
        let reference: Vec<Entry> = block_on(async {
            let mut v = Vault::new(&mut nor);
            v.load(0).await.unwrap();
            let u = v.unlock(b"2468", DEV, 0).await.unwrap();
            (0..u.len()).map(|i| u.entry(i).unwrap()).collect()
        });
        for chunk in [16, 17, 100, 256, 4096] {
            block_on(async {
                let mut v = Vault::new(&mut nor);
                let k = key_unlock(&mut v, b"2468", chunk).await.unwrap();
                assert_eq!(v.count(&k).await.unwrap(), 3);
                for (i, want) in reference.iter().enumerate() {
                    assert_eq!(&v.read_entry(&k, i).await.unwrap(), want, "entry {i}, chunk {chunk}");
                }
                assert_eq!(v.read_entry(&k, 3).await.err(), Some(VaultError::BadIndex));
                assert_eq!(v.failures(), 0);
            });
        }
    }

    #[test]
    fn key_path_rejects_wrong_pin_and_tampering() {
        let mut nor = Nor::new();
        let mut rng = Rng(12);
        three(&mut nor, &mut rng);
        let clean = nor.mem.clone();
        block_on(async {
            let mut v = Vault::new(&mut nor);
            assert_eq!(key_unlock(&mut v, b"1357", 64).await.err(), Some(VaultError::WrongKey));
            assert_eq!(v.failures(), 1);
            assert_eq!(v.wait_ms(0), 1000, "back-off as on the OTP-1 path");
        });
        // One ciphertext bit, the tag.
        for (off, bit) in [(O_PAYLOAD + 40, 0x01u8), (O_PAYLOAD + 2, 0x80), (O_TAG + 3, 0x10)] {
            let mut n2 = Nor { mem: clean.clone(), erases: 0, programs: 0, cut_after: None };
            n2.mem[off] ^= bit;
            block_on(async {
                let mut v = Vault::new(&mut n2);
                assert_eq!(key_unlock(&mut v, b"2468", 64).await.err(), Some(VaultError::WrongKey), "{off}");
            });
        }
    }

    #[test]
    fn payload_edits_round_trip_and_wipe() {
        let mut nor = Nor::new();
        let mut rng = Rng(13);
        three(&mut nor, &mut rng);
        let mut buf = std::vec![0u8; PAYLOAD_MAX];
        block_on(async {
            let mut v = Vault::new(&mut nor);
            let k = key_unlock(&mut v, b"2468", 256).await.unwrap();
            {
                let mut p = v.open(&k, &mut buf).await.unwrap();
                assert_eq!(p.len(), 3);
                assert!(p.contains(&totp("GitHub", b"12345678901234567890")));
                assert!(!p.contains(&totp("GitHub", b"other")));
                p.remove::<Cut>(0).unwrap();
                p.swap_next::<Cut>(0).unwrap();
                p.push::<Cut>(&totp("New", b"abcdefghij")).unwrap();
                v.save_payload(&k, &mut p, &mut rng).await.unwrap();
                // HOTP: the counter is stored before the code is returned.
                let c = v.hotp_next_payload(&k, &mut p, 1, &mut rng).await.unwrap();
                assert_eq!(c, hotp("Bank", 7).code(0));
                assert_eq!(p.entry(1).unwrap().kind, Kind::Hotp { counter: 8 });
                assert_eq!(
                    v.hotp_next_payload(&k, &mut p, 0, &mut rng).await.err(),
                    Some(VaultError::BadIndex)
                );
                assert_eq!(p.swap_next::<Cut>(2).err(), Some(VaultError::BadIndex));
            }
            assert!(buf.iter().all(|&b| b == 0), "the lent buffer is wiped");
            // The OTP-1 path reads the result.
            let u = v.unlock(b"2468", DEV, 0).await.unwrap();
            let names: Vec<_> = (0..u.len())
                .map(|i| std::string::String::from(u.entry(i).unwrap().issuer.as_str()))
                .collect();
            assert_eq!(names, ["Mail €", "Bank", "New"]);
            assert_eq!(u.entry(1).unwrap().kind, Kind::Hotp { counter: 8 });
            // A buffer too small for additions is refused.
            let mut small = [0u8; 64];
            assert_eq!(v.open(&k, &mut small).await.err(), Some(VaultError::Full));
        });
        assert!(!nor.mem.windows(5).any(|w| w == b"abcde"), "no plaintext on flash");
    }

    #[test]
    fn stepwise_create_and_change_pin() {
        let mut nor = Nor::new();
        let mut rng = Rng(14);
        let mut buf = std::vec![0u8; PAYLOAD_MAX];
        block_on(async {
            let mut v = Vault::new(&mut nor);
            let mut job = Vault::<&mut Nor>::begin_rekey(b"1111", SecretKind::Pin, DEV, IT, &mut rng);
            let mut steps = 0u32;
            while !job.step(64) {
                steps += 1;
            }
            assert!(steps + 1 >= IT / 64);
            let k = v.finish_create(job, &mut rng).await.unwrap();
            assert_eq!(v.count(&k).await.unwrap(), 0);
            let mut p = v.open(&k, &mut buf).await.unwrap();
            p.push::<Cut>(&totp("A", b"aaaaaaaaaa")).unwrap();
            v.save_payload(&k, &mut p, &mut rng).await.unwrap();
            let mut job = Vault::<&mut Nor>::begin_rekey(b"2222", SecretKind::Pin, DEV, IT, &mut rng);
            while !job.step(500) {}
            let k2 = v.finish_rekey(job, &mut p, &mut rng).await.unwrap();
            drop(p);
            assert_eq!(v.read_entry(&k2, 0).await.unwrap().issuer.as_str(), "A");
            assert_eq!(key_unlock(&mut v, b"1111", 64).await.err(), Some(VaultError::WrongKey));
        });
        block_on(async {
            let mut v = Vault::new(&mut nor);
            v.load(10_000).await.unwrap();
            let u = v.unlock(b"2222", DEV, 11_000).await.unwrap();
            assert_eq!(u.entry(0).unwrap().issuer.as_str(), "A");
        });
    }

    #[test]
    fn compaction_through_a_lent_buffer() {
        let mut nor = Nor::new();
        let mut rng = Rng(15);
        three(&mut nor, &mut rng);
        let mut buf = std::vec![0u8; PAYLOAD_MAX];
        block_on(async {
            let mut v = Vault::new(&mut nor);
            for _ in 0..COMPACT_AT {
                drop(key_unlock(&mut v, b"2468", 256).await.unwrap());
            }
            assert!(v.needs_compaction());
            v.compact_with(&mut buf).await.unwrap();
            assert!(!v.needs_compaction());
            assert!(buf.iter().all(|&b| b == 0));
            let k = key_unlock(&mut v, b"2468", 256).await.unwrap();
            assert_eq!(v.read_entry(&k, 2).await.unwrap().issuer.as_str(), "Mail €");
        });
    }

    #[test]
    fn pool_output_depends_on_what_was_mixed() {
        let mut a = Pool::new();
        let mut b = Pool::new();
        a.mix(&1u64.to_le_bytes());
        b.mix(&2u64.to_le_bytes());
        let (mut x, mut y, mut z) = ([0u8; 40], [0u8; 40], [0u8; 40]);
        a.fill(&mut x);
        b.fill(&mut y);
        a.fill(&mut z);
        assert_ne!(x, y);
        assert_ne!(x, z, "each request differs");
        assert!(x.iter().any(|&v| v != 0));
    }
}
