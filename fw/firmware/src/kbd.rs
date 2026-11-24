//! USB-5: the HID boot keyboard (06 §2.3 "Keyboard" personality, §5.4).
//!
//! The keyboard personality is the developer channel plus a HID boot
//! keyboard interface with one interrupt IN endpoint (8-byte reports,
//! polled every 4 ms at full speed, 1 ms at high speed). Report
//! descriptor, layouts and the press/release pacing are `orpheus_hid`'s
//! (host-tested, shared with the Bluetooth presenter, BT-4); this module
//! is the job queue between the UI / developer channel and the USB task.
//!
//! One job at a time: up to [`TEXT_MAX`] characters with the typist's
//! configuration. The text is wiped once typed. Typing stops on suspend,
//! bus reset or a personality change, and every stop releases all keys;
//! after a resume one more all-released report is sent.
//!
//! U-USB-11 (interrupt FIFO type 3 with the Actions DMA) is answered by the
//! device test in docs/STATUS.md: whether the host enumerates the keyboard
//! and receives the reports.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU16, AtomicU32, Ordering};

use critical_section::Mutex;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Timer, with_timeout};
use embassy_usb::class::hid::{HidProtocolMode, HidWriter, ReportId, RequestHandler};
use embassy_usb::control::OutResponse;
use orpheus_hid::typist::{self, Transport, TypeError, TypistConfig};
use orpheus_hid::{KeyboardReport, Layout};

macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

/// Longest text of one job (a code with Enter needs 9; `kbd type` tests).
pub const TEXT_MAX: usize = 64;
/// How long a keyboard personality waits for the computer to configure it
/// before giving up (a charger, or a host that refused the interface).
pub const CONFIGURE_TIMEOUT: Duration = Duration::from_secs(8);

/// The last job's outcome ([`STATE`]).
pub mod state {
    pub const IDLE: u8 = 0;
    pub const QUEUED: u8 = 1;
    pub const TYPING: u8 = 2;
    pub const DONE: u8 = 3;
    pub const FAILED: u8 = 4;
    pub const CANCELLED: u8 = 5;
    /// No computer configured the keyboard in time.
    pub const NO_HOST: u8 = 6;
    pub const UNTYPABLE: u8 = 7;
}

struct Job {
    text: [u8; TEXT_MAX],
    len: u8,
    cfg: TypistConfig,
    queued: bool,
}

static JOB: Mutex<RefCell<Job>> = Mutex::new(RefCell::new(Job {
    text: [0; TEXT_MAX],
    len: 0,
    cfg: TypistConfig { layout: Layout::Us, keypad_digits: false, delay_ms: typist::DEFAULT_DELAY_MS },
    queued: false,
}));
/// Wakes the pump: a job, or a resume (release all).
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

pub static STATE: AtomicU8 = AtomicU8::new(state::IDLE);
/// Bumped when a job ends (the UI turns it into an outcome).
pub static FINISHED: AtomicU32 = AtomicU32::new(0);
pub static JOBS: AtomicU32 = AtomicU32::new(0);
pub static TYPED: AtomicU32 = AtomicU32::new(0);
/// The keyboard personality is configured by a computer.
pub static READY: AtomicBool = AtomicBool::new(false);
pub static ACTIVE: AtomicBool = AtomicBool::new(false);
pub static SUSPENDED: AtomicBool = AtomicBool::new(false);
/// Stop the job now (reset, personality change).
static CANCEL: AtomicBool = AtomicBool::new(false);
/// Send an all-released report when idle (after a resume).
static RELEASE: AtomicBool = AtomicBool::new(false);
/// LED report from the computer (bit 0 Num Lock).
pub static LEDS: AtomicU8 = AtomicU8::new(0);
pub static BOOT_PROTOCOL: AtomicBool = AtomicBool::new(false);
/// Typing settings: the UI mirrors Settings › USB here; the developer
/// channel may override layout and keypad (0xFF = follow the setting).
pub static SET_LAYOUT: AtomicU8 = AtomicU8::new(0);
pub static SET_KEYPAD: AtomicBool = AtomicBool::new(false);
static OVERRIDE_LAYOUT: AtomicU8 = AtomicU8::new(0xFF);
static OVERRIDE_KEYPAD: AtomicU8 = AtomicU8::new(0xFF);
pub static DELAY_MS: AtomicU16 = AtomicU16::new(typist::DEFAULT_DELAY_MS);

/// The configuration a new job uses.
#[must_use]
pub fn config() -> TypistConfig {
    let l = match OVERRIDE_LAYOUT.load(Ordering::Relaxed) {
        0xFF => SET_LAYOUT.load(Ordering::Relaxed),
        v => v,
    };
    let keypad = match OVERRIDE_KEYPAD.load(Ordering::Relaxed) {
        0xFF => SET_KEYPAD.load(Ordering::Relaxed),
        v => v != 0,
    };
    TypistConfig {
        layout: Layout::from_index(l),
        keypad_digits: keypad,
        delay_ms: DELAY_MS.load(Ordering::Relaxed),
    }
}

/// Why a job was not queued.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum QueueError {
    Busy,
    TooLong,
    Untypable(char),
    /// The card reader is attached: switching would pull the card from
    /// under the computer.
    CardReader,
}

/// Queue `text` for typing; switches to the keyboard personality if
/// needed (the job waits for the computer to configure it).
pub fn queue(text: &str) -> Result<(), QueueError> {
    let cfg = config();
    if text.len() > TEXT_MAX {
        return Err(QueueError::TooLong);
    }
    if let Some(c) = orpheus_hid::layout::untypable(cfg.layout, text, cfg.keypad_digits) {
        return Err(QueueError::Untypable(c));
    }
    if crate::usbdev::CURRENT.load(Ordering::Relaxed) == crate::usbdev::CURRENT_CARD_READER {
        return Err(QueueError::CardReader);
    }
    let ok = critical_section::with(|cs| {
        let mut j = JOB.borrow(cs).borrow_mut();
        if j.queued || STATE.load(Ordering::Relaxed) == state::TYPING {
            return false;
        }
        j.text[..text.len()].copy_from_slice(text.as_bytes());
        j.text[text.len()..].fill(0);
        j.len = text.len() as u8;
        j.cfg = cfg;
        j.queued = true;
        true
    });
    if !ok {
        return Err(QueueError::Busy);
    }
    STATE.store(state::QUEUED, Ordering::Relaxed);
    CANCEL.store(false, Ordering::Relaxed);
    JOBS.fetch_add(1, Ordering::Relaxed);
    if !ACTIVE.load(Ordering::Relaxed) {
        crate::usbdev::MODE.signal(crate::usbdev::Personality::Keyboard);
    }
    WAKE.signal(());
    Ok(())
}

/// Take the queued job into `out` (the static copy is wiped).
fn take(out: &mut [u8; TEXT_MAX]) -> Option<(usize, TypistConfig)> {
    critical_section::with(|cs| {
        let mut j = JOB.borrow(cs).borrow_mut();
        if !j.queued {
            return None;
        }
        j.queued = false;
        let n = usize::from(j.len);
        out.copy_from_slice(&j.text);
        wipe(&mut j.text);
        j.len = 0;
        Some((n, j.cfg))
    })
}

/// Zero `b` in a way the optimiser keeps.
fn wipe(b: &mut [u8]) {
    for x in b.iter_mut() {
        // SAFETY: a valid, aligned `u8` place.
        unsafe { core::ptr::write_volatile(x, 0) };
    }
}

fn finish(st: u8) {
    STATE.store(st, Ordering::Relaxed);
    FINISHED.fetch_add(1, Ordering::Release);
}

/// A queued job could not start (no computer): it ends here.
fn drop_job(st: u8) {
    let mut tmp = [0u8; TEXT_MAX];
    if take(&mut tmp).is_some() {
        finish(st);
    }
    wipe(&mut tmp);
}

/// HID class requests: the LED report, boot/report protocol, idle rate.
pub struct Requests;

impl RequestHandler for Requests {
    fn set_report(&mut self, id: ReportId, data: &[u8]) -> OutResponse {
        if let (ReportId::Out(_), Some(&v)) = (id, data.first()) {
            LEDS.store(v, Ordering::Relaxed);
        }
        OutResponse::Accepted
    }
    fn get_protocol(&self) -> HidProtocolMode {
        if BOOT_PROTOCOL.load(Ordering::Relaxed) { HidProtocolMode::Boot } else { HidProtocolMode::Report }
    }
    fn set_protocol(&mut self, p: HidProtocolMode) -> OutResponse {
        // The report descriptor is the boot layout: both protocols send
        // the same 8 bytes (PC firmware set-up screens ask for boot).
        BOOT_PROTOCOL.store(p == HidProtocolMode::Boot, Ordering::Relaxed);
        OutResponse::Accepted
    }
    fn get_report(&mut self, _id: ReportId, buf: &mut [u8]) -> Option<usize> {
        let r = KeyboardReport::RELEASED.bytes();
        let n = r.len().min(buf.len());
        buf[..n].copy_from_slice(&r[..n]);
        Some(n)
    }
}

/// Bus events of the keyboard personality.
pub struct Bus;

impl embassy_usb::Handler for Bus {
    fn reset(&mut self) {
        READY.store(false, Ordering::Relaxed);
        CANCEL.store(true, Ordering::Relaxed);
    }
    fn suspended(&mut self, suspended: bool) {
        SUSPENDED.store(suspended, Ordering::Relaxed);
        if !suspended {
            RELEASE.store(true, Ordering::Relaxed);
            WAKE.signal(());
        }
    }
    fn configured(&mut self, configured: bool) {
        READY.store(configured, Ordering::Relaxed);
    }
}

struct Usb<'a, 'd>(&'a mut HidWriter<'d, atj2157_hal::usb::Driver, 8>);

impl Transport for Usb<'_, '_> {
    type Error = ();
    async fn send(&mut self, r: &KeyboardReport) -> Result<(), ()> {
        let b = r.bytes();
        self.0.write(&b).await.map_err(|_| ())
    }
}

/// The keyboard side of the keyboard personality: wait for the computer,
/// then type each job. Returns when no computer configured the keyboard
/// in time (the caller goes back to the developer personality).
pub async fn pump(w: &mut HidWriter<'_, atj2157_hal::usb::Driver, 8>) {
    ACTIVE.store(true, Ordering::Relaxed);
    READY.store(false, Ordering::Relaxed);
    // Configured by the computer in this attachment (`Bus::configured`;
    // the HAL's endpoint flag may still be set from a previous one).
    let configured = async {
        while !READY.load(Ordering::Relaxed) {
            Timer::after_millis(10).await;
        }
        w.ready().await;
    };
    if with_timeout(CONFIGURE_TIMEOUT, configured).await.is_err() {
        atj2157_hal::log::line(t!(b"kbd: not configured by a computer"));
        drop_job(state::NO_HOST);
        return;
    }
    // Let the computer's HID driver bind and start polling.
    Timer::after_millis(300).await;
    atj2157_hal::log::line(t!(b"kbd: ready"));
    let mut text = [0u8; TEXT_MAX];
    loop {
        if RELEASE.swap(false, Ordering::Relaxed) {
            let _ = Usb(w).send(&KeyboardReport::RELEASED).await;
        }
        let Some((n, cfg)) = take(&mut text) else {
            WAKE.wait().await;
            continue;
        };
        STATE.store(state::TYPING, Ordering::Relaxed);
        CANCEL.store(false, Ordering::Relaxed);
        let s = core::str::from_utf8(&text[..n]).unwrap_or("");
        let r = typist::type_text(
            &mut Usb(w),
            cfg,
            s,
            |ms| Timer::after_millis(u64::from(ms)),
            || CANCEL.load(Ordering::Relaxed) || SUSPENDED.load(Ordering::Relaxed),
        )
        .await;
        wipe(&mut text);
        let st = match r {
            Ok(k) => {
                TYPED.fetch_add(k, Ordering::Relaxed);
                state::DONE
            }
            Err(TypeError::Cancelled) => state::CANCELLED,
            Err(TypeError::Untypable(_)) => state::UNTYPABLE,
            Err(TypeError::Transport(())) => state::FAILED,
        };
        finish(st);
    }
}

/// The personality ended: stop typing; a job still queued fails.
pub fn ended() {
    ACTIVE.store(false, Ordering::Relaxed);
    READY.store(false, Ordering::Relaxed);
    CANCEL.store(true, Ordering::Relaxed);
    drop_job(state::NO_HOST);
}

/// Developer channel: `kbd type <text>` (ASCII, `\\n` = Enter), `kbd
/// status`, `kbd on|off`, `kbd layout us|uk|de|fr|setting`, `kbd keypad
/// on|off|setting`, `kbd delay <ms>`.
pub fn test_command(rest: &[u8]) -> bool {
    use crate::audio::reply;
    if let Some(t) = rest.strip_prefix(b"type ") {
        let mut buf = [0u8; TEXT_MAX];
        let mut n = 0;
        let mut i = 0;
        let mut bad = false;
        while i < t.len() {
            let (c, adv) = match (t[i], t.get(i + 1)) {
                (b'\\', Some(b'n')) => (b'\n', 2),
                (c, _) => (c, 1),
            };
            if !(c == b'\n' || (0x20..0x7F).contains(&c)) {
                bad = true;
            }
            if n < TEXT_MAX {
                buf[n] = c;
            }
            n += 1;
            i += adv;
        }
        if bad || n > TEXT_MAX {
            reply(|w| {
                w.s(t!(b"kbd: printable ASCII only, at most 64 characters"));
            });
            return true;
        }
        let s = core::str::from_utf8(&buf[..n]).unwrap_or("");
        let r = queue(s);
        wipe(&mut buf);
        reply(|w| {
            match r {
                Ok(()) => w.s(t!(b"kbd: queued ")).u(n as u64).s(t!(b" characters")),
                Err(QueueError::Busy) => w.s(t!(b"kbd: busy")),
                Err(QueueError::TooLong) => w.s(t!(b"kbd: too long")),
                Err(QueueError::CardReader) => w.s(t!(b"kbd: the card reader is attached (usb dev first)")),
                Err(QueueError::Untypable(c)) => {
                    let mut b = [0u8; 4];
                    w.s(t!(b"kbd: the layout cannot type ")).s(c.encode_utf8(&mut b).as_bytes())
                }
            };
        });
        return true;
    }
    match rest {
        b"status" => {
            let c = config();
            reply(|w| {
                w.s(t!(b"kbd: mode="))
                    .s(if ACTIVE.load(Ordering::Relaxed) { t!(b"keyboard") } else { t!(b"other") })
                    .s(t!(b" ready="))
                    .u(u64::from(READY.load(Ordering::Relaxed)))
                    .s(t!(b" suspended="))
                    .u(u64::from(SUSPENDED.load(Ordering::Relaxed)))
                    .s(t!(b" boot="))
                    .u(u64::from(BOOT_PROTOCOL.load(Ordering::Relaxed)))
                    .s(t!(b" leds="))
                    .h(u32::from(LEDS.load(Ordering::Relaxed)))
                    .s(t!(b" layout="))
                    .s(c.layout.code().as_bytes())
                    .s(t!(b" keypad="))
                    .u(u64::from(c.keypad_digits))
                    .s(t!(b" delay="))
                    .u(u64::from(c.delay_ms))
                    .s(t!(b" jobs="))
                    .u(u64::from(JOBS.load(Ordering::Relaxed)))
                    .s(t!(b" typed="))
                    .u(u64::from(TYPED.load(Ordering::Relaxed)))
                    .s(t!(b" last="))
                    .u(u64::from(STATE.load(Ordering::Relaxed)))
                    .s(t!(b" hs="))
                    .u(u64::from(atj2157_hal::usb::high_speed()));
            });
        }
        b"on" => {
            crate::usbdev::MODE.signal(crate::usbdev::Personality::Keyboard);
            reply(|w| {
                w.s(t!(b"kbd: keyboard personality (re-attach)"));
            });
        }
        b"off" => {
            crate::usbdev::MODE.signal(crate::usbdev::Personality::Dev);
            reply(|w| {
                w.s(t!(b"kbd: developer personality (re-attach)"));
            });
        }
        _ => {
            if let Some(l) = rest.strip_prefix(b"layout ") {
                let v = if l == b"setting" { Some(0xFF) } else { Layout::from_code(l).map(|l| l as u8) };
                let Some(v) = v else { return false };
                OVERRIDE_LAYOUT.store(v, Ordering::Relaxed);
            } else if let Some(k) = rest.strip_prefix(b"keypad ") {
                let v = match k {
                    b"on" => 1,
                    b"off" => 0,
                    b"setting" => 0xFF,
                    _ => return false,
                };
                OVERRIDE_KEYPAD.store(v, Ordering::Relaxed);
            } else if let Some(d) = rest.strip_prefix(b"delay ") {
                let mut v: u16 = 0;
                for &c in d {
                    if !c.is_ascii_digit() {
                        return false;
                    }
                    v = v.saturating_mul(10).saturating_add(u16::from(c - b'0'));
                }
                DELAY_MS.store(v.clamp(typist::MIN_DELAY_MS, typist::MAX_DELAY_MS), Ordering::Relaxed);
            } else {
                return false;
            }
            let c = config();
            reply(|w| {
                w.s(t!(b"kbd: layout="))
                    .s(c.layout.code().as_bytes())
                    .s(t!(b" keypad="))
                    .u(u64::from(c.keypad_digits))
                    .s(t!(b" delay="))
                    .u(u64::from(c.delay_ms));
            });
        }
    }
    true
}
