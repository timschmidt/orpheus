//! The Authenticator over the real service core (`orpheus_otp::service`)
//! on a RAM sector, and a fake computer on the USB keyboard: the real
//! typist (`orpheus_hid`) types the code into a fake HID host, whose text
//! the scripts check (`expect-typed:287082↵`).
//!
//! `set:vault=absent|locked|unlocked|damaged` (locked/unlocked: the demo
//! accounts, PIN 1234), `set:otpfile=demo|bad|none` (the card's
//! `/.orpheus/otp-import.txt`), `set:provision=demo` (a developer-channel
//! request), `set:wrongpin=N` (N failed attempts).

use embassy_futures::block_on;
use orpheus_apps::services::{
    AuthEntry, AuthOutcome, AuthRequest, Code, UsbState, VaultState, auth_event, other,
};
use orpheus_hid::host::FakeHost;
use orpheus_hid::typist::{self, Transport, TypistConfig};
use orpheus_hid::{KeyboardReport, Layout};
use orpheus_otp::service::{Authenticator, Event as AuthEvent, Job, SCRATCH};
use orpheus_otp::vault::SECTOR;
use orpheus_otp::{Storage, service};
use orpheus_ui::event::{Event, ServiceEvent};

/// The demo vault's PIN.
pub const PIN: &[u8] = b"1234";
/// Iterations of the simulated vault (the minimum: scripts stay fast) and
/// those run per simulated millisecond (a full derivation in 50 ms).
const ITERATIONS: u32 = 1000;
const PER_MS: u64 = 20;

/// Demo accounts (made-up secrets; issuers in three scripts).
pub const DEMO: &[&str] = &[
    "otpauth://totp/GitHub:alice@example.org?secret=GEZDGNBVGY3TQOJQGEZDGNBVGY3TQOJQ&issuer=GitHub",
    "otpauth://totp/Proton:alice@proton.me?secret=JBSWY3DPEHPK3PXP&issuer=Proton",
    "otpauth://hotp/Bank:4711-0815?secret=MFRGGZDFMZTWQ2LK&counter=7&digits=8&issuer=Bank",
    "otpauth://totp/Mastodon:@alice@social.example?secret=KRUGKIDROVUWG2ZAMJZG653OEBTG66BAN5SXGIDP&issuer=Mastodon",
    "otpauth://totp/%D0%93%D0%BE%D1%81%D1%83%D1%81%D0%BB%D1%83%D0%B3%D0%B8:alice?secret=NBSWY3DPO5XXE3DE&period=60",
    "otpauth://totp/AWS:root@example.com?secret=ONSWG4TFOQYTEMZUGU3DOOBZ&algorithm=SHA256&digits=8",
    "otpauth://totp/%E6%97%A5%E6%9C%AC:alice?secret=MZXW6YTBOI======&issuer=%E6%97%A5%E6%9C%AC",
];

/// The vault sector in RAM (NOR semantics: program only clears bits).
pub struct RamSector(pub Vec<u8>);

impl Default for RamSector {
    fn default() -> Self {
        RamSector(vec![0xFF; SECTOR])
    }
}

impl Storage for RamSector {
    type Error = ();
    async fn read(&mut self, off: u32, buf: &mut [u8]) -> Result<(), ()> {
        let o = off as usize;
        buf.copy_from_slice(self.0.get(o..o + buf.len()).ok_or(())?);
        Ok(())
    }
    async fn program(&mut self, off: u32, data: &[u8]) -> Result<(), ()> {
        let o = off as usize;
        for (m, d) in self.0.get_mut(o..o + data.len()).ok_or(())?.iter_mut().zip(data) {
            *m &= *d;
        }
        Ok(())
    }
    async fn erase(&mut self) -> Result<(), ()> {
        self.0.fill(0xFF);
        Ok(())
    }
}

/// The fake computer on the USB keyboard.
struct Wire<'a>(&'a mut FakeHost);

impl Transport for Wire<'_> {
    type Error = ();
    async fn send(&mut self, r: &KeyboardReport) -> Result<(), ()> {
        self.0.feed(&r.bytes());
        Ok(())
    }
}

pub struct SimAuth {
    pub core: Authenticator<RamSector>,
    scratch: Vec<u8>,
    pub outcome: AuthOutcome,
    events: Vec<u32>,
    /// The card's `/.orpheus/otp-import.txt`.
    pub file: Option<Vec<u8>>,
    /// The fake computer (its layout follows the setting at each code).
    pub host: FakeHost,
    /// A provisioning request waiting for the user.
    pending: Option<orpheus_otp::Entry>,
    /// A key derivation in progress (shown with its progress).
    job: Option<Job>,
    pub now_ms: u64,
}

impl Default for SimAuth {
    fn default() -> Self {
        let mut a = SimAuth {
            core: Authenticator::new(RamSector::default(), b"sim", ITERATIONS),
            scratch: vec![0; SCRATCH],
            outcome: AuthOutcome::None,
            events: Vec::new(),
            file: None,
            host: FakeHost::new(Layout::Us),
            pending: None,
            job: None,
            now_ms: 0,
        };
        let _ = block_on(a.core.load(0));
        a
    }
}

fn demo_file() -> Vec<u8> {
    let mut f = b"# Exported codes\n".to_vec();
    for l in DEMO {
        f.extend_from_slice(l.as_bytes());
        f.push(b'\n');
    }
    f
}

impl SimAuth {
    fn finish(&mut self, job: Job) -> AuthEvent {
        block_on(self.core.finish(job, self.now_ms, &mut self.scratch))
    }

    /// The vault's state as the app sees it.
    pub fn state(&self) -> VaultState {
        self.job.as_ref().map_or_else(|| self.core.state(self.now_ms), Job::state)
    }

    /// `set:vault=…`.
    pub fn set_vault(&mut self, v: &str) -> anyhow::Result<()> {
        self.core = Authenticator::new(RamSector::default(), b"sim", ITERATIONS);
        let _ = block_on(self.core.load(self.now_ms));
        match v {
            "absent" => {}
            "damaged" => {
                let mut s = RamSector::default();
                s.0[..4].copy_from_slice(b"YVLT");
                self.core = Authenticator::new(s, b"sim", ITERATIONS);
                let _ = block_on(self.core.load(self.now_ms));
            }
            "locked" | "unlocked" | "empty" => {
                if let Ok(job) = self.core.create(PIN, self.now_ms) {
                    self.finish(job);
                }
                if v != "empty" {
                    let f = demo_file();
                    let _ = block_on(self.core.import(&f, &mut self.scratch, self.now_ms));
                }
                self.core.touch(self.now_ms);
                if v == "locked" {
                    self.core.lock();
                }
            }
            _ => anyhow::bail!("vault: absent, locked, unlocked, empty, damaged"),
        }
        self.events.push(auth_event::STATE);
        Ok(())
    }

    /// `set:wrongpin=N`: N failed attempts (the back-off runs from now).
    pub fn wrong_pins(&mut self, n: u32) {
        for _ in 0..n {
            let _ = block_on(self.core.load(self.now_ms));
            // Past the longest back-off.
            self.now_ms += 301_000;
            if let Ok(job) = block_on(self.core.unlock(b"0000", self.now_ms)) {
                self.finish(job);
            }
        }
        let _ = block_on(self.core.load(self.now_ms));
        self.events.push(auth_event::STATE);
    }

    pub fn set_file(&mut self, v: &str) {
        self.file = match v {
            "demo" => Some(demo_file()),
            "bad" => Some(b"otpauth://totp/A:a?secret=JBSWY3DPEHPK3PXP\nhello\n".to_vec()),
            _ => None,
        };
    }

    /// A PROVISION request arrives (accepted only while unlocked).
    pub fn set_provision(&mut self) {
        if !self.core.is_unlocked() {
            return;
        }
        self.pending =
            service::provision(b"otpauth://totp/Fastmail:alice@fastmail.com?secret=OBQXG43XN5ZGIZLMMFZQ");
        if self.pending.is_some() {
            self.events.push(auth_event::PROVISION);
        }
    }

    pub fn advance(&mut self, ms: u64) {
        self.now_ms += ms;
        let it = u32::try_from(ms.saturating_mul(PER_MS)).unwrap_or(u32::MAX);
        if let Some(mut job) = self.job.take() {
            if job.step(it) {
                let e = self.finish(job);
                self.event(e);
            } else {
                self.job = Some(job);
            }
        }
        if let Some(e) = self.core.poll(self.now_ms) {
            self.event(e);
        }
    }

    fn event(&mut self, e: AuthEvent) {
        match e {
            AuthEvent::WrongPin => self.events.push(auth_event::WRONG_PIN),
            AuthEvent::PinChanged => self.out(AuthOutcome::PinChanged),
            AuthEvent::Failed => self.out(AuthOutcome::Failed),
            AuthEvent::Unlocked | AuthEvent::Created | AuthEvent::AutoLocked => {}
        }
        self.events.push(auth_event::STATE);
    }

    fn out(&mut self, o: AuthOutcome) {
        self.outcome = o;
        self.events.push(auth_event::OUTCOME);
    }

    pub fn take_events(&mut self) -> Vec<Event> {
        self.events
            .drain(..)
            .map(|value| Event::Service(ServiceEvent::Other { kind: other::AUTH, value }))
            .collect()
    }

    /// The screen went off: lock (05 §7.9).
    pub fn screen_off(&mut self) {
        if self.core.is_unlocked() {
            self.core.lock();
            self.pending = None;
            self.events.push(auth_event::STATE);
        }
    }

    pub fn entry(&mut self, i: usize) -> Option<AuthEntry> {
        block_on(self.core.entry(i)).ok().map(|e| AuthEntry::of(&e))
    }

    pub fn code(&mut self, i: usize, unix: Option<u64>) -> Option<Code> {
        block_on(self.core.code(i, unix)).ok().flatten()
    }

    pub fn provision_pending(&self) -> Option<AuthEntry> {
        self.pending.as_ref().map(AuthEntry::of)
    }

    /// Requests: `unix` is the clock, `usb` whether a computer is there,
    /// `cfg` the keyboard settings and `enter` whether Enter follows.
    pub fn request(
        &mut self,
        r: AuthRequest,
        unix: Option<u64>,
        usb: UsbState,
        cfg: TypistConfig,
        enter: bool,
    ) {
        let now = self.now_ms;
        match r {
            AuthRequest::Touch => self.core.touch(now),
            AuthRequest::Unlock(pin) => match block_on(self.core.unlock(pin.as_bytes(), now)) {
                Ok(job) => {
                    self.job = Some(job);
                    self.events.push(auth_event::STATE);
                }
                Err(_) => self.events.push(auth_event::WRONG_PIN),
            },
            AuthRequest::Create(pin) => {
                if let Ok(job) = self.core.create(pin.as_bytes(), now) {
                    self.job = Some(job);
                    self.events.push(auth_event::STATE);
                }
            }
            AuthRequest::ChangePin(pin) => {
                if let Ok(job) = self.core.change_pin(pin.as_bytes(), now) {
                    self.job = Some(job);
                    self.events.push(auth_event::STATE);
                }
            }
            AuthRequest::Lock => {
                self.core.lock();
                self.pending = None;
                self.events.push(auth_event::STATE);
            }
            AuthRequest::Import => match self.file.take() {
                None => self.out(AuthOutcome::NoImportFile),
                Some(f) => match block_on(self.core.import(&f, &mut self.scratch, now)) {
                    Ok(report) => {
                        let deleted = report.consumed();
                        if !deleted {
                            self.file = Some(f);
                        }
                        self.events.push(auth_event::ENTRIES);
                        self.out(AuthOutcome::Imported { report, deleted });
                    }
                    Err(_) => {
                        self.file = Some(f);
                        self.out(AuthOutcome::Failed);
                    }
                },
            },
            AuthRequest::NextCode(i) => {
                match block_on(self.core.next_code(usize::from(i), &mut self.scratch, now)) {
                    Ok(_) => self.events.push(auth_event::ENTRIES),
                    Err(_) => self.out(AuthOutcome::Failed),
                }
            }
            AuthRequest::Delete(i) => {
                match block_on(self.core.delete(usize::from(i), &mut self.scratch, now)) {
                    Ok(()) => self.events.push(auth_event::ENTRIES),
                    Err(_) => self.out(AuthOutcome::Failed),
                }
            }
            AuthRequest::SwapNext(i) => {
                match block_on(self.core.swap_next(usize::from(i), &mut self.scratch, now)) {
                    Ok(()) => self.events.push(auth_event::ENTRIES),
                    Err(_) => self.out(AuthOutcome::Failed),
                }
            }
            AuthRequest::Type(i) => {
                if usb != UsbState::Host {
                    self.out(AuthOutcome::NoComputer);
                    return;
                }
                let Some(code) = self.code(usize::from(i), unix) else {
                    self.out(AuthOutcome::NoCode);
                    return;
                };
                let mut text = String::from(code.as_str());
                if enter {
                    text.push('\n');
                }
                self.host = FakeHost::new(cfg.layout);
                let r = block_on(typist::type_text(
                    &mut Wire(&mut self.host),
                    cfg,
                    &text,
                    |_| core::future::ready(()),
                    || false,
                ));
                self.out(if r.is_ok() { AuthOutcome::Typed } else { AuthOutcome::TypeFailed });
            }
            AuthRequest::Provision(yes) => {
                let Some(e) = self.pending.take() else { return };
                if !yes {
                    return;
                }
                match block_on(self.core.add(&e, &mut self.scratch, now)) {
                    Ok(true) => {
                        self.events.push(auth_event::ENTRIES);
                        self.out(AuthOutcome::Added);
                    }
                    Ok(false) => self.out(AuthOutcome::AlreadyThere),
                    Err(_) => self.out(AuthOutcome::Failed),
                }
            }
            AuthRequest::Wipe => {
                let _ = block_on(self.core.wipe());
                self.events.push(auth_event::STATE);
            }
        }
    }

    /// What the fake computer received.
    #[must_use]
    pub fn typed(&self) -> &str {
        self.host.text()
    }
}
