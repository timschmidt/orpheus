//! The authenticator as the app's service (OTP-2, 05 §7.9, 06 §5): the
//! vault, the PIN flow with its back-off, the key derivation in slices, the
//! 60-second relock, imports, provisioning and the HOTP counter, without
//! hardware. The firmware and the simulator run the same code over their
//! storage (the NOR sector, a RAM sector).
//!
//! While unlocked only the 32-byte [`Key`] is held (see [`crate::vault`]
//! "Two ways in"); every change borrows a `scratch` buffer of
//! [`PAYLOAD_MAX`] bytes for the moment of the write and leaves it wiped.
//! Nothing here logs; secrets leave only as codes.

use crate::entry::{Entry, Kind};
use crate::otp::Code;
use crate::uri;
use crate::vault::{
    Key, PAYLOAD_MAX, Pool, RekeyJob, SecretKind, Status, Storage, UnlockJob, Vault, VaultError,
};

/// Idle time after which an unlocked vault locks itself (05 §7.9).
pub const AUTO_LOCK_MS: u64 = 60_000;
/// PIN length (05 §7.9: 4–8 digits).
pub const MIN_PIN: usize = 4;
pub const MAX_PIN: usize = 8;
/// Scratch needed by the operations that change the vault.
pub const SCRATCH: usize = PAYLOAD_MAX;

/// What runs in [`Authenticator::poll`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Work {
    Unlock,
    Create,
    ChangePin,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    /// The sector has not been read (or could not be).
    Unknown,
    /// No vault: set-up.
    Absent,
    /// Torn write or foreign data: only wiping helps.
    Damaged,
    /// Written by a newer firmware.
    Unsupported,
    /// `wait_ms` until the next attempt is allowed.
    Locked {
        failures: u32,
        wait_ms: u64,
    },
    /// Deriving a key; `percent` done.
    Working {
        work: Work,
        percent: u8,
    },
    Unlocked {
        count: u8,
    },
}

/// Outcomes of [`Authenticator::poll`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    Unlocked,
    WrongPin,
    Created,
    PinChanged,
    /// Idle for [`AUTO_LOCK_MS`].
    AutoLocked,
    /// Storage failed or the vault is corrupt.
    Failed,
}

/// What an import did.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct ImportReport {
    pub added: u8,
    /// Already in the vault (same secret, issuer and account).
    pub duplicates: u8,
    /// Lines that are not valid `otpauth://` URIs.
    pub bad: u8,
    /// The vault filled up before the end of the file.
    pub full: bool,
}

impl ImportReport {
    /// The file has been dealt with and may be wiped and deleted: every
    /// line was imported or already there. A file with lines that were
    /// not understood, or that did not fit, is kept so the user can fix it.
    #[must_use]
    pub fn consumed(&self) -> bool {
        self.bad == 0 && !self.full && self.added + self.duplicates > 0
    }
}

/// A key derivation (unlock, set-up, PIN change). The caller holds it
/// while it runs ([`Job::step`]) and hands it to
/// [`Authenticator::finish`]: the firmware runs it to the end at once, so
/// its ~240 bytes live on the stack only for that moment (06 §5.2's RAM
/// budget); the simulator keeps it between frames to show the progress.
pub struct Job {
    kind: JobKind,
    total: u32,
}

enum JobKind {
    Unlock(UnlockJob),
    Create(RekeyJob),
    Change(RekeyJob),
}

impl Job {
    fn new(kind: JobKind) -> Job {
        let total = match &kind {
            JobKind::Unlock(j) => j.remaining(),
            JobKind::Create(j) | JobKind::Change(j) => j.remaining(),
        };
        Job { kind, total: total.max(1) }
    }

    /// Run up to `n` iterations; `true` when the key is ready.
    pub fn step(&mut self, n: u32) -> bool {
        match &mut self.kind {
            JobKind::Unlock(j) => j.step(n),
            JobKind::Create(j) | JobKind::Change(j) => j.step(n),
        }
    }

    #[must_use]
    pub fn work(&self) -> Work {
        match self.kind {
            JobKind::Unlock(_) => Work::Unlock,
            JobKind::Create(_) => Work::Create,
            JobKind::Change(_) => Work::ChangePin,
        }
    }

    #[must_use]
    pub fn percent(&self) -> u8 {
        let left = match &self.kind {
            JobKind::Unlock(j) => j.remaining(),
            JobKind::Create(j) | JobKind::Change(j) => j.remaining(),
        };
        let done = self.total.saturating_sub(left);
        u8::try_from(u64::from(done) * 100 / u64::from(self.total)).unwrap_or(100)
    }

    /// The state to show while this runs.
    #[must_use]
    pub fn state(&self) -> State {
        State::Working { work: self.work(), percent: self.percent() }
    }
}

pub struct Authenticator<S: Storage> {
    vault: Vault<S>,
    status: Option<Status>,
    unsupported: bool,
    key: Option<Key>,
    count: u8,
    pool: Pool,
    device_id: &'static [u8],
    iterations: u32,
    last_touch_ms: u64,
    /// The HOTP code last generated (entry, code): shown until locked.
    hotp: Option<(u8, Code)>,
}

type R<T, S> = Result<T, VaultError<<S as Storage>::Error>>;

impl<S: Storage> Authenticator<S> {
    /// `device_id` is mixed into the key derivation; `iterations` is used
    /// for new vaults and PIN changes.
    pub fn new(storage: S, device_id: &'static [u8], iterations: u32) -> Self {
        Authenticator {
            vault: Vault::new(storage),
            status: None,
            unsupported: false,
            key: None,
            count: 0,
            pool: Pool::new(),
            device_id,
            iterations,
            last_touch_ms: 0,
            hotp: None,
        }
    }

    /// Read the vault's header and attempt counter.
    pub async fn load(&mut self, now_ms: u64) -> R<(), S> {
        self.unsupported = false;
        match self.vault.load(now_ms).await {
            Ok(st) => {
                self.status = Some(st);
                Ok(())
            }
            Err(VaultError::UnsupportedVersion) => {
                self.status = None;
                self.unsupported = true;
                Ok(())
            }
            Err(e) => Err(e),
        }
    }

    /// The vault's state (a [`Job`] the caller runs shows as
    /// [`Job::state`] instead).
    #[must_use]
    pub fn state(&self, now_ms: u64) -> State {
        if self.key.is_some() {
            return State::Unlocked { count: self.count };
        }
        match self.status {
            None if self.unsupported => State::Unsupported,
            None => State::Unknown,
            Some(Status::Absent) => State::Absent,
            Some(Status::Damaged) => State::Damaged,
            Some(Status::Locked { .. }) => {
                State::Locked { failures: self.vault.failures(), wait_ms: self.vault.wait_ms(now_ms) }
            }
        }
    }

    #[must_use]
    pub fn is_unlocked(&self) -> bool {
        self.key.is_some()
    }

    /// Feed the entropy pool (key-press times, sensor readings…).
    pub fn mix(&mut self, data: &[u8]) {
        self.pool.mix(data);
    }

    /// The user did something in the app: restart the idle timer.
    pub fn touch(&mut self, now_ms: u64) {
        self.last_touch_ms = now_ms;
    }

    /// Forget the key (and the HOTP code).
    pub fn lock(&mut self) {
        self.key = None;
        self.hotp = None;
    }

    /// Start unlocking: the attempt is recorded on the NOR before anything
    /// else (so a reset counts as a failure). Refused during the back-off.
    pub async fn unlock(&mut self, pin: &[u8], now_ms: u64) -> R<Job, S> {
        self.mix(&now_ms.to_le_bytes());
        let job = self.vault.begin_unlock(pin, self.device_id, now_ms).await?;
        Ok(Job::new(JobKind::Unlock(job)))
    }

    /// Start creating a vault (none, or a damaged one, is there).
    pub fn create(&mut self, pin: &[u8], now_ms: u64) -> R<Job, S> {
        if self.key.is_some() || matches!(self.status, Some(Status::Locked { .. })) {
            return Err(VaultError::BadIndex);
        }
        self.mix(&now_ms.to_le_bytes());
        let job =
            Vault::<S>::begin_rekey(pin, SecretKind::Pin, self.device_id, self.iterations, &mut self.pool);
        Ok(Job::new(JobKind::Create(job)))
    }

    /// Start changing the PIN of the unlocked vault.
    pub fn change_pin(&mut self, pin: &[u8], now_ms: u64) -> R<Job, S> {
        if self.key.is_none() {
            return Err(VaultError::NoVault);
        }
        self.mix(&now_ms.to_le_bytes());
        let job =
            Vault::<S>::begin_rekey(pin, SecretKind::Pin, self.device_id, self.iterations, &mut self.pool);
        Ok(Job::new(JobKind::Change(job)))
    }

    /// Lock after [`AUTO_LOCK_MS`] without [`touch`](Self::touch).
    pub fn poll(&mut self, now_ms: u64) -> Option<Event> {
        if self.key.is_some() && now_ms.saturating_sub(self.last_touch_ms) >= AUTO_LOCK_MS {
            self.lock();
            return Some(Event::AutoLocked);
        }
        None
    }

    /// Run what is left of `job` and finish it. `scratch` ([`SCRATCH`]
    /// bytes) is used only by a PIN change and a bitmap compaction, and is
    /// wiped after.
    pub async fn finish(&mut self, job: Job, now_ms: u64, scratch: &mut [u8]) -> Event {
        match job.work() {
            Work::Unlock => self.finish_unlock(job, now_ms, scratch).await,
            Work::Create => self.finish_create(job, now_ms).await,
            Work::ChangePin => self.finish_change(job, now_ms, scratch).await,
        }
    }

    /// [`finish`](Self::finish) for a job whose [`Job::work`] is
    /// [`Work::Unlock`] (another kind: [`Event::Failed`]). The three kinds
    /// are separate futures so a caller that polls one at a time (the
    /// firmware, each in a frame of its own) never holds the others' state
    /// on its stack.
    pub async fn finish_unlock(&mut self, mut job: Job, now_ms: u64, scratch: &mut [u8]) -> Event {
        while !job.step(u32::MAX) {}
        let JobKind::Unlock(j) = job.kind else { return Event::Failed };
        let mut chunk = [0u8; 256];
        match self.vault.finish_unlock_key(j, now_ms, &mut chunk).await {
            Ok(k) => {
                let Ok(n) = self.vault.count(&k).await else { return Event::Failed };
                self.count = u8::try_from(n).unwrap_or(0);
                self.key = Some(k);
                self.touch(now_ms);
                if self.vault.needs_compaction() {
                    let _ = self.vault.compact_with(scratch).await;
                }
                Event::Unlocked
            }
            Err(VaultError::WrongKey) => Event::WrongPin,
            Err(_) => Event::Failed,
        }
    }

    /// [`finish`](Self::finish) for [`Work::Create`] (another kind:
    /// [`Event::Failed`]).
    ///
    /// The derivation runs to its end when this is called; the future
    /// holds only its result (BOOK-1/2: the job moved into the future was
    /// copied on the firmware's stack under the sealing).
    pub fn finish_create(&mut self, job: Job, now_ms: u64) -> impl Future<Output = Event> + '_ {
        let j = match job.kind {
            JobKind::Create(j) => Some(j.finish()),
            _ => None,
        };
        self.create_with(j, now_ms)
    }

    async fn create_with(&mut self, j: Option<crate::vault::Rekeyed>, now_ms: u64) -> Event {
        let Some(j) = j else { return Event::Failed };
        match self.vault.finish_create(j, &mut self.pool).await {
            Ok(k) => {
                self.key = Some(k);
                self.count = 0;
                self.status = Some(Status::Locked { kind: SecretKind::Pin, failures: 0 });
                self.touch(now_ms);
                Event::Created
            }
            Err(_) => {
                self.status = Some(Status::Damaged);
                Event::Failed
            }
        }
    }

    /// [`finish`](Self::finish) for [`Work::ChangePin`] (another kind:
    /// [`Event::Failed`]).
    ///
    /// The derivation runs to its end when this is called (as
    /// [`finish_create`](Self::finish_create)).
    pub fn finish_change<'a>(
        &'a mut self,
        job: Job,
        now_ms: u64,
        scratch: &'a mut [u8],
    ) -> impl Future<Output = Event> + 'a {
        let j = match job.kind {
            JobKind::Change(j) => Some(j.finish()),
            _ => None,
        };
        self.change_with(j, now_ms, scratch)
    }

    async fn change_with(
        &mut self,
        j: Option<crate::vault::Rekeyed>,
        now_ms: u64,
        scratch: &mut [u8],
    ) -> Event {
        let Some(j) = j else { return Event::Failed };
        let Some(old) = self.key.take() else { return Event::Failed };
        let r = match self.vault.open(&old, scratch).await {
            Ok(mut p) => self.vault.finish_rekey(j, &mut p, &mut self.pool).await,
            Err(e) => Err(e),
        };
        match r {
            Ok(k) => {
                self.key = Some(k);
                self.touch(now_ms);
                Event::PinChanged
            }
            Err(_) => {
                // The old key still opens the vault unless the
                // write was torn.
                self.key = Some(old);
                Event::Failed
            }
        }
    }

    /// Entries in the unlocked vault.
    #[must_use]
    pub fn count(&self) -> usize {
        if self.key.is_some() { usize::from(self.count) } else { 0 }
    }

    /// Entry `i` (decrypted from storage; the caller drops it soon: the
    /// secret is wiped on drop).
    pub async fn entry(&mut self, i: usize) -> R<Entry, S> {
        let k = self.key.as_ref().ok_or(VaultError::NoVault)?;
        self.vault.read_entry(k, i).await
    }

    /// The code to show for entry `i`: TOTP at `unix_time` (none without a
    /// valid clock), HOTP the one last generated with
    /// [`next_code`](Self::next_code) (none before).
    pub async fn code(&mut self, i: usize, unix_time: Option<u64>) -> R<Option<Code>, S> {
        let e = self.entry(i).await?;
        Ok(self.code_of(i, &e, unix_time))
    }

    /// [`code`](Self::code) for entry `i` already read with
    /// [`entry`](Self::entry): the firmware reads the entry in a frame of
    /// its own and hashes outside it, so the read's state is not on the
    /// stack under the HMAC (STACK, 2026-11-25).
    #[must_use]
    pub fn code_of(&self, i: usize, e: &Entry, unix_time: Option<u64>) -> Option<Code> {
        match e.kind {
            Kind::Totp { .. } => unix_time.map(|t| e.code(t)),
            Kind::Hotp { .. } => self.hotp.filter(|(k, _)| usize::from(*k) == i).map(|(_, c)| c),
        }
    }

    /// The HOTP code shown for entry `i`, if one was generated.
    #[must_use]
    pub fn hotp_shown(&self, i: usize) -> Option<Code> {
        self.hotp.filter(|(k, _)| usize::from(*k) == i).map(|(_, c)| c)
    }

    /// Advance HOTP entry `i`: the counter is written before the code is
    /// returned, so a code is never shown twice.
    pub async fn next_code(&mut self, i: usize, scratch: &mut [u8], now_ms: u64) -> R<Code, S> {
        self.mix(&now_ms.to_le_bytes());
        let k = self.key.as_ref().ok_or(VaultError::NoVault)?;
        let mut p = self.vault.open(k, scratch).await?;
        let c = self.vault.hotp_next_payload(k, &mut p, i, &mut self.pool).await?;
        self.hotp = Some((u8::try_from(i).unwrap_or(0), c));
        Ok(c)
    }

    /// Delete entry `i`.
    pub async fn delete(&mut self, i: usize, scratch: &mut [u8], now_ms: u64) -> R<(), S> {
        self.mix(&now_ms.to_le_bytes());
        let k = self.key.as_ref().ok_or(VaultError::NoVault)?;
        let mut p = self.vault.open(k, scratch).await?;
        p.remove(i)?;
        self.vault.save_payload(k, &mut p, &mut self.pool).await?;
        self.count = u8::try_from(p.len()).unwrap_or(0);
        self.hotp = match self.hotp {
            Some((h, _)) if usize::from(h) == i => None,
            Some((h, c)) if usize::from(h) > i => Some((h - 1, c)),
            other => other,
        };
        Ok(())
    }

    /// Exchange entries `i` and `i + 1` (Move up / Move down).
    pub async fn swap_next(&mut self, i: usize, scratch: &mut [u8], now_ms: u64) -> R<(), S> {
        self.mix(&now_ms.to_le_bytes());
        let k = self.key.as_ref().ok_or(VaultError::NoVault)?;
        let mut p = self.vault.open(k, scratch).await?;
        p.swap_next(i)?;
        self.vault.save_payload(k, &mut p, &mut self.pool).await?;
        self.hotp = match self.hotp {
            Some((h, c)) if usize::from(h) == i => Some((h + 1, c)),
            Some((h, c)) if usize::from(h) == i + 1 => Some((h - 1, c)),
            other => other,
        };
        Ok(())
    }

    /// Import `otpauth://` URIs, one per line (blank lines and lines
    /// starting with `#` are skipped). One write for the whole file.
    pub async fn import(&mut self, text: &[u8], scratch: &mut [u8], now_ms: u64) -> R<ImportReport, S> {
        self.mix(&now_ms.to_le_bytes());
        let k = self.key.as_ref().ok_or(VaultError::NoVault)?;
        let mut p = self.vault.open(k, scratch).await?;
        let mut rep = ImportReport::default();
        for line in text.split(|&b| b == b'\n') {
            let line = line.trim_ascii();
            if line.is_empty() || line.starts_with(b"#") {
                continue;
            }
            let parsed = core::str::from_utf8(line).ok().and_then(|s| uri::parse(s).ok());
            let Some(e) = parsed else {
                rep.bad = rep.bad.saturating_add(1);
                continue;
            };
            if p.contains(&e) {
                rep.duplicates = rep.duplicates.saturating_add(1);
                continue;
            }
            match p.push::<S::Error>(&e) {
                Ok(()) => rep.added += 1,
                Err(_) => {
                    rep.full = true;
                    break;
                }
            }
        }
        if rep.added > 0 {
            self.vault.save_payload(k, &mut p, &mut self.pool).await?;
            self.count = u8::try_from(p.len()).unwrap_or(0);
        }
        Ok(rep)
    }

    /// Add one entry (a provisioning request the user confirmed, 06
    /// §2.5). `Ok(false)` when an identical one is already there.
    pub async fn add(&mut self, e: &Entry, scratch: &mut [u8], now_ms: u64) -> R<bool, S> {
        self.mix(&now_ms.to_le_bytes());
        let k = self.key.as_ref().ok_or(VaultError::NoVault)?;
        let mut p = self.vault.open(k, scratch).await?;
        if p.contains(e) {
            return Ok(false);
        }
        p.push(e)?;
        self.vault.save_payload(k, &mut p, &mut self.pool).await?;
        self.count = u8::try_from(p.len()).unwrap_or(0);
        Ok(true)
    }

    /// Erase the vault sector: every entry is gone, set-up starts again.
    pub async fn wipe(&mut self) -> R<(), S> {
        self.lock();
        self.count = 0;
        self.vault.wipe().await?;
        self.status = Some(Status::Absent);
        Ok(())
    }

    /// Ensure `scratch` is long enough for the operations above.
    #[must_use]
    pub fn scratch_ok(scratch: &[u8]) -> bool {
        scratch.len() >= SCRATCH
    }

    /// Give the storage back (tests).
    pub fn into_storage(self) -> S {
        self.vault.into_storage()
    }
}

/// A provisioning request (developer channel PROVISION, 06 §2.5): the
/// entry to show the user for confirmation, if `uri` is a valid
/// `otpauth://` URI. The caller keeps it until the answer and accepts
/// requests only while the vault is unlocked.
#[must_use]
pub fn provision(uri: &[u8]) -> Option<Entry> {
    core::str::from_utf8(uri).ok().and_then(|s| uri::parse(s).ok())
}

/// A payload buffer must not outlive the borrow: helper for callers that
/// want to be sure the scratch is wiped even when an operation was not
/// started (the operations above wipe it themselves).
pub fn wipe(scratch: &mut [u8]) {
    use zeroize::Zeroize;
    scratch.zeroize();
}

#[cfg(test)]
mod tests {
    use super::*;
    use embassy_futures::block_on;
    use std::vec::Vec;

    #[derive(Default)]
    struct Ram {
        mem: Vec<u8>,
        erases: u32,
    }
    impl Storage for &mut Ram {
        type Error = ();
        async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), ()> {
            if self.mem.is_empty() {
                self.mem = std::vec![0xFF; crate::vault::SECTOR];
            }
            let o = off as usize;
            buf.copy_from_slice(&self.mem[o..o + buf.len()]);
            Ok(())
        }
        async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
            for (m, d) in self.mem[off as usize..].iter_mut().zip(data) {
                *m &= *d;
            }
            Ok(())
        }
        async fn erase(&mut self) -> Result<(), ()> {
            self.erases += 1;
            self.mem = std::vec![0xFF; crate::vault::SECTOR];
            Ok(())
        }
    }

    const IT: u32 = 1000;
    const GH: &[u8] =
        b"otpauth://totp/GitHub:me@example.org?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=GitHub";
    const BANK: &[u8] = b"otpauth://hotp/Bank:x?secret=MFRGGZDFMZTWQ2LK&counter=0&digits=8";

    fn run(a: &mut Authenticator<&mut Ram>, job: Job, now: u64, scratch: &mut [u8]) -> Event {
        block_on(a.finish(job, now, scratch))
    }

    #[test]
    fn set_up_lock_unlock_and_back_off() {
        let mut ram = Ram::default();
        let mut s = std::vec![0u8; SCRATCH];
        let mut a = Authenticator::new(&mut ram, b"dev", IT);
        assert_eq!(a.state(0), State::Unknown);
        block_on(a.load(0)).unwrap();
        assert_eq!(a.state(0), State::Absent);
        let mut job = a.create(b"1234", 0).unwrap();
        assert_eq!(job.state(), State::Working { work: Work::Create, percent: 0 });
        assert!(!job.step(300));
        assert_eq!(job.state(), State::Working { work: Work::Create, percent: 30 });
        assert_eq!(run(&mut a, job, 0, &mut s), Event::Created);
        assert_eq!(a.state(0), State::Unlocked { count: 0 });
        a.lock();
        assert_eq!(a.state(0), State::Locked { failures: 0, wait_ms: 0 });
        // Wrong PIN: 1 s back-off, refused while it runs.
        let job = block_on(a.unlock(b"9999", 0)).unwrap();
        assert_eq!(job.work(), Work::Unlock);
        assert_eq!(run(&mut a, job, 0, &mut s), Event::WrongPin);
        assert_eq!(a.state(500), State::Locked { failures: 1, wait_ms: 500 });
        assert_eq!(block_on(a.unlock(b"1234", 500)).err(), Some(VaultError::Backoff { wait_ms: 500 }));
        let job = block_on(a.unlock(b"1234", 1000)).unwrap();
        assert_eq!(run(&mut a, job, 1000, &mut s), Event::Unlocked);
        assert_eq!(a.state(1000), State::Unlocked { count: 0 });
        // Idle relock after 60 s without touch.
        a.touch(5_000);
        assert!(a.poll(64_999).is_none());
        assert_eq!(a.poll(65_000), Some(Event::AutoLocked));
        assert!(!a.is_unlocked());
        // The failure count survives a reload (reboot).
        let job = block_on(a.unlock(b"0000", 70_000)).unwrap();
        assert_eq!(run(&mut a, job, 70_000, &mut s), Event::WrongPin);
        let ram = a.into_storage();
        let mut b = Authenticator::new(ram, b"dev", IT);
        block_on(b.load(100)).unwrap();
        assert_eq!(b.state(100), State::Locked { failures: 1, wait_ms: 1000 });
    }

    fn unlocked(ram: &mut Ram) -> Authenticator<&mut Ram> {
        let mut s = std::vec![0u8; SCRATCH];
        let mut a = Authenticator::new(ram, b"dev", IT);
        block_on(a.load(0)).unwrap();
        let job = a.create(b"2580", 0).unwrap();
        assert_eq!(run(&mut a, job, 0, &mut s), Event::Created);
        a
    }

    #[test]
    fn import_provision_codes_and_edits() {
        let mut ram = Ram::default();
        let mut a = unlocked(&mut ram);
        let mut s = std::vec![0u8; SCRATCH];
        let mut file = Vec::new();
        file.extend_from_slice(b"# my codes\r\n");
        file.extend_from_slice(GH);
        file.extend_from_slice(b"\r\n\nnot a uri\n");
        file.extend_from_slice(BANK);
        file.extend_from_slice(b"\n");
        file.extend_from_slice(GH);
        let rep = block_on(a.import(&file, &mut s, 1)).unwrap();
        assert_eq!(rep, ImportReport { added: 2, duplicates: 1, bad: 1, full: false });
        assert!(!rep.consumed(), "a bad line keeps the file");
        assert!(s.iter().all(|&b| b == 0), "scratch wiped");
        assert_eq!(a.state(1), State::Unlocked { count: 2 });
        // TOTP: RFC 6238's SHA-1 seed at t = 59 (6 digits of 94287082).
        assert_eq!(block_on(a.code(0, Some(59))).unwrap().unwrap().as_str(), "287082");
        assert_eq!(block_on(a.code(0, None)).unwrap(), None, "no clock, no TOTP code");
        // HOTP: nothing until Next code; then counter 0's code, then 1's.
        assert_eq!(block_on(a.code(1, Some(59))).unwrap(), None);
        let c0 = block_on(a.next_code(1, &mut s, 2)).unwrap();
        let c1 = block_on(a.next_code(1, &mut s, 3)).unwrap();
        assert_ne!(c0, c1);
        assert_eq!(block_on(a.code(1, None)).unwrap(), Some(c1));
        assert_eq!(a.entry_kind_counter(1), Kind::Hotp { counter: 2 });
        // Provisioning: parsed, then added once (a duplicate is not).
        let e = provision(b"otpauth://totp/Mail:me?secret=JBSWY3DPEHPK3PXP").unwrap();
        assert_eq!(e.issuer.as_str(), "Mail");
        assert!(block_on(a.add(&e, &mut s, 4)).unwrap());
        assert!(!block_on(a.add(&e, &mut s, 5)).unwrap());
        assert!(provision(b"https://example.org").is_none());
        assert_eq!(a.count(), 3);
        // Move the bank up (its shown code follows it), delete GitHub.
        block_on(a.swap_next(0, &mut s, 6)).unwrap();
        assert_eq!(block_on(a.entry(0)).unwrap().issuer.as_str(), "Bank");
        assert_eq!(a.hotp_shown(0), Some(c1));
        block_on(a.delete(1, &mut s, 7)).unwrap();
        assert_eq!(a.count(), 2);
        assert_eq!(block_on(a.entry(1)).unwrap().issuer.as_str(), "Mail");
        // Locked: nothing readable, nothing added.
        a.lock();
        assert_eq!(block_on(a.entry(0)).err(), Some(VaultError::NoVault));
        assert_eq!(block_on(a.add(&e, &mut s, 8)).err(), Some(VaultError::NoVault));
        assert_eq!(a.hotp_shown(0), None);
    }

    /// Set-up, an import, a PIN change and an unlock, finished through
    /// [`Authenticator::finish`] or (`per_kind`) the finishes the firmware
    /// polls in frames of their own (STACK, 2026-11-25); the sector after.
    fn sector_after(per_kind: bool) -> (Vec<u8>, Option<Code>) {
        let mut ram = Ram::default();
        let mut s = std::vec![0u8; SCRATCH];
        let mut a = Authenticator::new(&mut ram, b"dev", IT);
        block_on(a.load(0)).unwrap();
        a.mix(b"keys");
        let job = a.create(b"2580", 0).unwrap();
        let ev = if per_kind { block_on(a.finish_create(job, 0)) } else { run(&mut a, job, 0, &mut s) };
        assert_eq!(ev, Event::Created);
        assert!(block_on(a.import(GH, &mut s, 1)).unwrap().consumed());
        let job = a.change_pin(b"13579", 2).unwrap();
        let ev =
            if per_kind { block_on(a.finish_change(job, 2, &mut s)) } else { run(&mut a, job, 2, &mut s) };
        assert_eq!(ev, Event::PinChanged);
        a.lock();
        let job = block_on(a.unlock(b"13579", 3)).unwrap();
        let ev =
            if per_kind { block_on(a.finish_unlock(job, 3, &mut s)) } else { run(&mut a, job, 3, &mut s) };
        assert_eq!(ev, Event::Unlocked);
        let code = if per_kind {
            let e = block_on(a.entry(0)).unwrap();
            a.code_of(0, &e, Some(59))
        } else {
            block_on(a.code(0, Some(59))).unwrap()
        };
        drop(a);
        (ram.mem, code)
    }

    /// The per-kind finishes and `code_of` write and show what `finish`
    /// and `code` do, bit for bit (the same keys, salts and nonces).
    #[test]
    fn per_kind_finish_equals_finish() {
        let (sector, code) = sector_after(false);
        assert_eq!(sector_after(true), (sector, code));
        assert_eq!(code.unwrap().as_str(), "287082");
    }

    /// A job handed to another kind's finish is refused: nothing is
    /// written, the vault's state and key stay as they were.
    #[test]
    fn per_kind_finish_refuses_other_kinds() {
        let mut ram = Ram::default();
        let mut s = std::vec![0u8; SCRATCH];
        let mut a = Authenticator::new(&mut ram, b"dev", IT);
        block_on(a.load(0)).unwrap();
        let job = a.create(b"2580", 0).unwrap();
        assert_eq!(block_on(a.finish_unlock(job, 0, &mut s)), Event::Failed);
        let job = a.create(b"2580", 0).unwrap();
        assert_eq!(block_on(a.finish_change(job, 0, &mut s)), Event::Failed);
        assert_eq!(a.state(0), State::Absent);
        let job = a.create(b"2580", 0).unwrap();
        assert_eq!(block_on(a.finish_create(job, 0)), Event::Created);
        let job = a.change_pin(b"13579", 1).unwrap();
        assert_eq!(block_on(a.finish_create(job, 1)), Event::Failed);
        let job = a.change_pin(b"13579", 1).unwrap();
        assert_eq!(block_on(a.finish_unlock(job, 1, &mut s)), Event::Failed);
        assert!(a.is_unlocked(), "the key is kept");
        a.lock();
        let job = block_on(a.unlock(b"2580", 2)).unwrap();
        assert_eq!(block_on(a.finish_change(job, 2, &mut s)), Event::Failed);
        assert!(!a.is_unlocked());
        // The PIN is still the first one.
        let job = block_on(a.unlock(b"2580", 3)).unwrap();
        assert_eq!(block_on(a.finish_unlock(job, 3, &mut s)), Event::Unlocked);
    }

    impl Authenticator<&mut Ram> {
        fn entry_kind_counter(&mut self, i: usize) -> Kind {
            block_on(self.entry(i)).unwrap().kind
        }
    }

    #[test]
    fn change_pin_wipe_and_full_imports() {
        let mut ram = Ram::default();
        let mut a = unlocked(&mut ram);
        let mut s = std::vec![0u8; SCRATCH];
        assert!(block_on(a.import(GH, &mut s, 1)).unwrap().consumed());
        let job = a.change_pin(b"13579", 2).unwrap();
        assert_eq!(run(&mut a, job, 2, &mut s), Event::PinChanged);
        a.lock();
        let job = block_on(a.unlock(b"2580", 3)).unwrap();
        assert_eq!(run(&mut a, job, 3, &mut s), Event::WrongPin);
        let job = block_on(a.unlock(b"13579", 2000)).unwrap();
        assert_eq!(run(&mut a, job, 2000, &mut s), Event::Unlocked);
        assert_eq!(a.count(), 1);
        // 32 entries at most: the 33rd line does not fit.
        let mut file = Vec::new();
        for i in 0..40 {
            file.extend_from_slice(
                std::format!("otpauth://totp/S{i}:a?secret=JBSWY3DPEHPK3PXP\n").as_bytes(),
            );
        }
        let rep = block_on(a.import(&file, &mut s, 3000)).unwrap();
        assert_eq!((rep.added, rep.full), (31, true));
        assert!(!rep.consumed());
        // Creating over a vault is refused; wiping starts over.
        assert!(a.create(b"1111", 0).is_err());
        block_on(a.wipe()).unwrap();
        assert_eq!(a.state(0), State::Absent);
        assert!(a.create(b"1111", 0).is_ok());
    }
}
