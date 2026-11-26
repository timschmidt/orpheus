//! OTP-2 on the player: the Authenticator's service over the vault sector
//! (`orpheus_otp::service`, host-tested), provisioning from the developer
//! channel, the import file and typing codes on the USB keyboard.
//!
//! RAM (06 §5.2, the package's budget): while unlocked only the 32-byte
//! key is held. Changes to the vault (import, delete, move, next HOTP
//! code, PIN change, provisioning) need the decrypted payload for the
//! moment of the write: the UI lends [`SCRATCH`] bytes of its frame
//! buffer between frames (`ui::Ui::after`), the operation runs to the end
//! without yielding, the bytes are zeroed and the screen is redrawn. The
//! import file is read into the frame buffer too, by the card task, with
//! the picture viewer's slot protocol (`media::OtpJob`); the UI draws
//! nothing while those bytes are out.
//!
//! Nothing here logs a secret; the key and any provisioning request are
//! dropped (zeroised) on lock, screen-off and before a reboot into ADFU.

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, Ordering};

use atj2157_hal::nor;
use critical_section::Mutex;
use embassy_futures::block_on;
use orpheus_apps::services::{AuthEntry, AuthOutcome, Pin, VaultWork, auth_event, other};
use orpheus_otp::service::{self, Authenticator, Event as AuthEvent};
use orpheus_otp::{Entry, Storage};
use orpheus_proto::layout::dual;
use orpheus_ui::event::{Event, ServiceEvent};

use crate::media;
use crate::services::Fw;

/// Scratch the vault's changes borrow (`orpheus_otp::service::SCRATCH`).
pub const SCRATCH: usize = service::SCRATCH;
/// Where the import file is read in the lent frame buffer, and its limit
/// (32 accounts of long URIs fit easily).
const FILE_AT: usize = 4096;
const FILE_MAX: usize = 8192;

/// The device id mixed into the key derivation. The eFuse words (06 §5.2)
/// need PMU writes to be read (docs/hw/system.md §5.5), which CLAUDE.md
/// rule 3 keeps out of this package; a fixed label stands in. Changing it
/// later makes existing vaults unreadable (a new header version then).
pub const DEVICE_ID: &[u8] = b"orpheus-ym03";

/// The vault sector (`0x7DE000`, one 4 KiB sector, the same in both
/// layouts). Reads are byte-polled NOR reads; erase and program go through
/// the resident NOR driver.
pub struct NorVault;

impl Storage for NorVault {
    type Error = nor::Error;

    async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), nor::Error> {
        if off as usize + buf.len() > dual::VAULT_SIZE as usize {
            return Err(nor::Error::Refused);
        }
        nor::read(dual::VAULT + off, buf)
    }

    async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), nor::Error> {
        if off as usize + data.len() > dual::VAULT_SIZE as usize {
            return Err(nor::Error::Refused);
        }
        let mut a = dual::VAULT + off;
        let mut rest = data;
        while !rest.is_empty() {
            // Never cross a 256-byte NOR page.
            let n = rest.len().min(256 - (a as usize & 0xFF));
            nor::program_page(a, &rest[..n])?;
            a += n as u32;
            rest = &rest[n..];
        }
        Ok(())
    }

    async fn erase(&mut self) -> Result<(), nor::Error> {
        nor::erase_sector(dual::VAULT)
    }
}

static mut AUTH: MaybeUninit<Authenticator<NorVault>> = MaybeUninit::uninit();

/// Once, from `Fw::new` (UI task).
#[inline(never)]
pub fn init() -> &'static mut Authenticator<NorVault> {
    // SAFETY: called once; the UI task is the only user afterwards.
    unsafe {
        (*(&raw mut AUTH)).write(Authenticator::new(
            NorVault,
            DEVICE_ID,
            orpheus_otp::kdf::DEFAULT_ITERATIONS,
        ))
    }
}

/// Mirror for the USB task: PROVISION is accepted only while unlocked.
static UNLOCKED: AtomicBool = AtomicBool::new(false);
/// A provisioning request from the developer channel, until the user
/// answers it (or the vault locks).
static PROVISION: Mutex<RefCell<Option<Entry>>> = Mutex::new(RefCell::new(None));
static PROVISION_NEW: AtomicBool = AtomicBool::new(false);

/// Developer channel PROVISION (USB task): one `otpauth://` URI.
pub fn provision(uri: &[u8]) -> bool {
    if !UNLOCKED.load(Ordering::Acquire) {
        return false;
    }
    let Some(e) = service::provision(uri) else { return false };
    critical_section::with(|cs| *PROVISION.borrow(cs).borrow_mut() = Some(e));
    PROVISION_NEW.store(true, Ordering::Release);
    true
}

fn drop_provision() {
    critical_section::with(|cs| *PROVISION.borrow(cs).borrow_mut() = None);
}

/// The user said no.
pub fn reject_provision() {
    drop_provision();
}

/// The entry waiting for the user (labels only).
pub fn provision_pending() -> Option<AuthEntry> {
    critical_section::with(|cs| PROVISION.borrow(cs).borrow().as_ref().map(AuthEntry::of))
}

/// Before a reboot into ADFU (RAM survives it): zero the key and any
/// provisioning request. The caller reboots at once; nothing uses the
/// authenticator afterwards.
pub fn wipe_secrets() {
    drop_provision();
    // SAFETY: the reboot follows immediately and the UI task (the only
    // user) does not run again; volatile writes so the zeroing is kept.
    unsafe {
        let p = (&raw mut AUTH).cast::<u8>();
        for i in 0..core::mem::size_of::<Authenticator<NorVault>>() {
            core::ptr::write_volatile(p.add(i), 0);
        }
    }
    core::sync::atomic::compiler_fence(Ordering::SeqCst);
}

/// Operations that need the scratch (run in `Ui::after`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AuthOp {
    Import,
    NextCode(u8),
    Delete(u8),
    SwapNext(u8),
    AddProvision,
    Wipe,
}

/// The import file's journey.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum OtpPhase {
    #[default]
    Idle,
    /// The card task reads the file into the lent bytes.
    Reading {
        seq: u32,
        since_ms: u64,
    },
    Read(i32),
    /// The vault took it; the card task wipes and deletes the file.
    Deleting {
        seq: u32,
        report: service::ImportReport,
    },
}

/// Seconds since 1970 of the RTC (UTC) while it is valid.
pub fn unix_now() -> Option<u64> {
    let ok = atj2157_hal::rtc_clock::running() && atj2157_hal::rtc_clock::now().year >= 2026;
    // The RTC keeps local time; less the Time zone setting it is UTC
    // (VER-13, published by the UI).
    let off = crate::ui::UTC_OFFSET_S.load(core::sync::atomic::Ordering::Relaxed);
    ok.then(|| {
        let local = u64::from(atj2157_hal::rtc_clock::epoch2000(atj2157_hal::rtc_clock::now())) + 946_684_800;
        orpheus_apps::services::utc_of_local(local, off)
    })
}

fn bytes(fb: &mut [u16]) -> &mut [u8] {
    // SAFETY: the frame buffer's u16 pixels viewed as bytes (same memory,
    // alignment 1); the borrow of `fb` is held for the slice's lifetime.
    unsafe { core::slice::from_raw_parts_mut(fb.as_mut_ptr().cast::<u8>(), fb.len() * 2) }
}

fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        // SAFETY: a valid `u8` place.
        unsafe { core::ptr::write_volatile(x, 0) };
    }
}

impl Fw {
    fn auth_event(&mut self, value: u32) {
        self.pending.push(Event::Service(ServiceEvent::Other { kind: other::AUTH, value }));
    }

    pub(crate) fn auth_out(&mut self, o: AuthOutcome) {
        self.auth_outcome = o;
        self.auth_event(auth_event::OUTCOME);
    }

    /// Something wants the UI loop soon: a key derivation, an operation
    /// waiting for the scratch, the import file.
    pub fn auth_busy(&self) -> bool {
        self.auth_op.is_some() || self.auth_pending.is_some() || !matches!(self.otp_phase, OtpPhase::Idle)
    }

    /// The import file's bytes are with the card task (no drawing).
    pub fn otp_lent(&self) -> bool {
        matches!(self.otp_phase, OtpPhase::Reading { .. })
    }

    /// The card task answered (`media::tag::OTP`).
    pub(crate) fn otp_sync(&mut self) {
        let (seq, r) = critical_section::with(|cs| media::OTP_RESULT.borrow(cs).get());
        match self.otp_phase {
            OtpPhase::Reading { seq: s, .. } if s == seq => self.otp_phase = OtpPhase::Read(r),
            OtpPhase::Deleting { seq: s, report } if s == seq => {
                self.otp_phase = OtpPhase::Idle;
                self.auth_out(AuthOutcome::Imported { report, deleted: r == 0 });
            }
            _ => {}
        }
    }

    fn otp_job(&mut self, buf: usize, cap: usize, delete: bool) -> Option<u32> {
        self.otp_seq = (self.otp_seq + 1) & !media::PIC_RUNNING;
        if self.otp_seq == 0 {
            self.otp_seq = 1;
        }
        let job = media::OtpJob { seq: self.otp_seq, buf, cap: cap as u32, delete };
        media::OTP_SLOT.store(job.seq, Ordering::Release);
        if media::try_request(media::Req::Otp(job)) {
            Some(job.seq)
        } else {
            media::OTP_SLOT.store(0, Ordering::Release);
            None
        }
    }

    /// Every UI loop, between event handling and drawing: the key
    /// derivation, the operations that need the frame buffer as scratch,
    /// the import file, typing results, provisioning. Returns true if the
    /// frame buffer was used (the screen must be redrawn).
    pub fn auth_step(&mut self, fb: &mut [u16], now_ms: u64, screen_on: bool) -> bool {
        let mut used = false;
        // The keyboard's settings, for the next job.
        let layout = u8::try_from(self.settings.get(orpheus_apps::settings::id::KBD_LAYOUT)).unwrap_or(0);
        crate::kbd::SET_LAYOUT.store(layout, Ordering::Relaxed);
        crate::kbd::SET_KEYPAD
            .store(self.settings.get(orpheus_apps::settings::id::KEYPAD_DIGITS) != 0, Ordering::Relaxed);
        // Screen off: lock (05 §7.9).
        if !screen_on && self.auth.is_unlocked() {
            self.auth.lock();
            self.auth_event(auth_event::STATE);
        }
        let lend = self.pic_lent.is_none() && !self.otp_lent();
        // The idle relock.
        if self.auth.poll(now_ms).is_some() {
            self.auth_event(auth_event::STATE);
        }
        // A key derivation: drawn as "Unlocking…" once, then run to the end
        // here (its state lives on the stack only for that moment).
        if lend && let Some(p) = self.auth_pending.as_mut() {
            if p.2 {
                if let Some((work, pin, _)) = self.auth_pending.take() {
                    used = true;
                    self.derive(work, &pin, &mut bytes(fb)[..SCRATCH], now_ms);
                    wipe(&mut bytes(fb)[..SCRATCH]);
                }
            } else {
                p.2 = true;
            }
        }
        let unlocked = self.auth.is_unlocked();
        UNLOCKED.store(unlocked, Ordering::Release);
        if !unlocked {
            drop_provision();
            PROVISION_NEW.store(false, Ordering::Relaxed);
        }
        if PROVISION_NEW.swap(false, Ordering::AcqRel) {
            self.auth_event(auth_event::PROVISION);
        }
        // Typing finished.
        if let Some(seen) = self.kbd_seen {
            let f = crate::kbd::FINISHED.load(Ordering::Acquire);
            if f != seen {
                self.kbd_seen = None;
                self.auth_out(match crate::kbd::STATE.load(Ordering::Relaxed) {
                    crate::kbd::state::DONE => AuthOutcome::Typed,
                    crate::kbd::state::NO_HOST => AuthOutcome::NoComputer,
                    _ => AuthOutcome::TypeFailed,
                });
            }
        }
        // The import file: a read finished, or one is stuck.
        match self.otp_phase {
            OtpPhase::Read(r) if lend => {
                self.otp_phase = OtpPhase::Idle;
                used = true;
                self.import_read(fb, r, now_ms);
            }
            OtpPhase::Reading { seq, since_ms }
                if now_ms.saturating_sub(since_ms) > 5000
                    && media::OTP_SLOT
                        .compare_exchange(seq, 0, Ordering::AcqRel, Ordering::Acquire)
                        .is_ok() =>
            {
                self.otp_phase = OtpPhase::Idle;
                self.auth_out(AuthOutcome::Failed);
            }
            _ => {}
        }
        // One operation that needs the scratch (each wipes what it used;
        // an import's file bytes are with the card task until it answers).
        if lend && let Some(op) = self.auth_op.take() {
            used |= self.auth_run(op, fb, now_ms);
        }
        used
    }

    #[inline(never)]
    fn import_read(&mut self, fb: &mut [u16], r: i32, now_ms: u64) {
        let Ok(n) = usize::try_from(r) else {
            self.auth_out(match r {
                media::OTP_NONE => AuthOutcome::NoImportFile,
                media::OTP_TOO_LARGE => AuthOutcome::ImportTooLarge,
                _ => AuthOutcome::Failed,
            });
            return;
        };
        let b = bytes(fb);
        let (scratch, rest) = b.split_at_mut(FILE_AT);
        let text = &mut rest[..FILE_MAX];
        let r = block_on(self.auth.import(&text[..n.min(FILE_MAX)], &mut scratch[..SCRATCH], now_ms));
        // The file's text holds secrets: gone before anything else.
        wipe(text);
        wipe(scratch);
        match r {
            Ok(report) => {
                if report.added > 0 {
                    self.auth_event(auth_event::ENTRIES);
                }
                if report.consumed() {
                    match self.otp_job(0, 0, true) {
                        Some(seq) => self.otp_phase = OtpPhase::Deleting { seq, report },
                        None => self.auth_out(AuthOutcome::Imported { report, deleted: false }),
                    }
                } else {
                    self.auth_out(AuthOutcome::Imported { report, deleted: false });
                }
            }
            Err(_) => self.auth_out(AuthOutcome::Failed),
        }
    }

    /// Run `op` with the frame buffer as scratch; true if it was used.
    #[inline(never)]
    fn auth_run(&mut self, op: AuthOp, fb: &mut [u16], now_ms: u64) -> bool {
        if op == AuthOp::Import {
            if !crate::state::CARD_READY.load(Ordering::Relaxed) {
                self.auth_out(AuthOutcome::NoImportFile);
                return false;
            }
            let buf = bytes(fb)[FILE_AT..].as_mut_ptr() as usize;
            match self.otp_job(buf, FILE_MAX, false) {
                Some(seq) => self.otp_phase = OtpPhase::Reading { seq, since_ms: now_ms },
                None => self.auth_out(AuthOutcome::Failed),
            }
            return false;
        }
        let scratch = &mut bytes(fb)[..SCRATCH];
        let r = match op {
            AuthOp::Import => return false,
            AuthOp::NextCode(i) => block_on(self.auth.next_code(usize::from(i), scratch, now_ms)).map(|_| ()),
            AuthOp::Delete(i) => block_on(self.auth.delete(usize::from(i), scratch, now_ms)),
            AuthOp::SwapNext(i) => block_on(self.auth.swap_next(usize::from(i), scratch, now_ms)),
            AuthOp::AddProvision => {
                let e = critical_section::with(|cs| PROVISION.borrow(cs).borrow_mut().take());
                let Some(e) = e else { return false };
                match block_on(self.auth.add(&e, scratch, now_ms)) {
                    Ok(true) => {
                        self.auth_out(AuthOutcome::Added);
                        Ok(())
                    }
                    Ok(false) => {
                        self.auth_out(AuthOutcome::AlreadyThere);
                        return true;
                    }
                    Err(e) => Err(e),
                }
            }
            AuthOp::Wipe => {
                drop_provision();
                let r = block_on(self.auth.wipe());
                self.auth_event(auth_event::STATE);
                r
            }
        };
        wipe(&mut bytes(fb)[..SCRATCH]);
        match r {
            Ok(()) => self.auth_event(auth_event::ENTRIES),
            Err(_) => self.auth_out(AuthOutcome::Failed),
        }
        true
    }

    /// Run a key derivation to the end (about a second at 48 MHz, a third
    /// at 144 MHz: the clock is raised for it; the governor lowers it again).
    #[inline(never)]
    fn derive(&mut self, work: VaultWork, pin: &Pin, scratch: &mut [u8], now_ms: u64) {
        use atj2157_hal::clock::{Level, current, set_level};
        if matches!(current(), Level::Low | Level::Base | Level::Mid) {
            set_level(Level::High);
        }
        // Started and finished in frames of their own: inlined here, the
        // start's futures and temporaries stayed in this frame (4.2 KB) under
        // the finish's sealing and hashing (VER-10).
        let ev = match self.derive_start(work, pin, now_ms) {
            Some(job) => self.derive_finish(job, now_ms, scratch),
            // Refused (back-off): as a wrong PIN, the pad shows the wait.
            None if work == VaultWork::Unlock => AuthEvent::WrongPin,
            None => AuthEvent::Failed,
        };
        match ev {
            AuthEvent::WrongPin => self.auth_event(auth_event::WRONG_PIN),
            AuthEvent::PinChanged => self.auth_out(AuthOutcome::PinChanged),
            AuthEvent::Failed => self.auth_out(AuthOutcome::Failed),
            _ => {}
        }
        self.auth_event(auth_event::STATE);
    }

    #[inline(never)]
    fn derive_start(&mut self, work: VaultWork, pin: &Pin, now_ms: u64) -> Option<service::Job> {
        match work {
            VaultWork::Unlock => block_on(self.auth.unlock(pin.as_bytes(), now_ms)),
            VaultWork::Create => self.auth.create(pin.as_bytes(), now_ms),
            VaultWork::ChangePin => self.auth.change_pin(pin.as_bytes(), now_ms),
        }
        .ok()
    }

    /// Each kind of job is finished in a frame of its own: one future for
    /// all three held the unlock's, the set-up's and the PIN change's
    /// states side by side (a 4,008 B frame under the sealing and hashing,
    /// the UI task's deepest path; STACK, 2026-11-25).
    fn derive_finish(&mut self, job: service::Job, now_ms: u64, scratch: &mut [u8]) -> AuthEvent {
        match job.work() {
            service::Work::Unlock => self.finish_unlock(job, now_ms, scratch),
            service::Work::Create => self.finish_create(job, now_ms),
            service::Work::ChangePin => self.finish_change(job, now_ms, scratch),
        }
    }

    #[inline(never)]
    fn finish_unlock(&mut self, job: service::Job, now_ms: u64, scratch: &mut [u8]) -> AuthEvent {
        block_on(self.auth.finish_unlock(job, now_ms, scratch))
    }

    #[inline(never)]
    fn finish_create(&mut self, job: service::Job, now_ms: u64) -> AuthEvent {
        block_on(self.auth.finish_create(job, now_ms))
    }

    #[inline(never)]
    fn finish_change(&mut self, job: service::Job, now_ms: u64, scratch: &mut [u8]) -> AuthEvent {
        block_on(self.auth.finish_change(job, now_ms, scratch))
    }

    /// Entry `i`'s code at `t`: the entry is read in a frame of its own
    /// and hashed in this one. Through `Authenticator::code`, the read's
    /// future (a 640 B `block_on` frame) stayed under the HMAC-SHA-512,
    /// the UI task's deepest path with Salvo built in (STACK, 2026-11-25).
    #[inline(never)]
    pub(crate) fn auth_code_now(&mut self, i: usize, t: Option<u64>) -> Option<orpheus_otp::Code> {
        let e = self.auth_read_entry(i)?;
        self.auth.code_of(i, &e, t)
    }

    #[inline(never)]
    fn auth_read_entry(&mut self, i: usize) -> Option<Entry> {
        block_on(self.auth.entry(i)).ok()
    }

    /// Type the current code of entry `i` on the USB keyboard.
    pub(crate) fn auth_type(&mut self, i: u8) {
        let t = unix_now();
        let Some(code) = self.auth_code_now(usize::from(i), t) else {
            self.auth_out(AuthOutcome::NoCode);
            return;
        };
        if !crate::state::snapshot().cable {
            self.auth_out(AuthOutcome::NoComputer);
            return;
        }
        let mut text = [0u8; 9];
        let c = code.as_bytes();
        text[..c.len()].copy_from_slice(c);
        let mut n = c.len();
        if self.settings.get(orpheus_apps::settings::id::ENTER_AFTER_CODE) != 0 {
            text[n] = b'\n';
            n += 1;
        }
        let r = crate::kbd::queue(core::str::from_utf8(&text[..n]).unwrap_or(""));
        wipe(&mut text);
        match r {
            Ok(()) => {
                self.kbd_seen = Some(crate::kbd::FINISHED.load(Ordering::Acquire));
                self.auth_out(AuthOutcome::Typing);
            }
            Err(_) => self.auth_out(AuthOutcome::TypeFailed),
        }
    }

    /// Read the vault's header at boot.
    pub fn auth_boot(&mut self, now_ms: u64) {
        let _ = block_on(self.auth.load(now_ms));
        self.auth.mix(&atj2157_hal::time::now_us().to_le_bytes());
        self.auth.mix(&crate::state::snapshot().millivolts.to_le_bytes());
        if let Some(t) = unix_now() {
            self.auth.mix(&t.to_le_bytes());
        }
    }
}
