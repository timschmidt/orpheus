//! Battery logger (WP PWR-5, the PWR-4 measurement tool): while it runs,
//! the power task feeds it a sample every second and every minute a row
//! goes into a small RAM log (`orpheus_power::drain::Log`, 6 rows); the
//! card task appends the rows to `/.orpheus/test/drain-<n>-<label>.csv`
//! (next free `n`) every 5 rows, when the log stops and at power-off.
//!
//! The card writes are a low-priority chore of the card task, through its
//! normal file API:
//!
//! * idle: at once (a `media::Req::Drain` wakes the idle loop);
//! * playing: only between ring refills while the DAC has room to spare
//!   (`audio::ROOMY`), never forced like the other chores; while paused,
//!   with the DAC stopped;
//! * the file's number is chosen (a scan of the folder) only where the
//!   card task may read freely (idle loop, a request served while playing).
//!
//! The logger keeps nothing awake: it adds no timer (the power task ticks
//! once a second anyway) and does not touch auto-off, unless started with
//! `stay` (then auto-off waits until it stops). It runs with the cable in
//! or out; with USB connected the battery is not discharging (the player
//! runs from VBUS), which the `cable` and `charge` columns show.
//!
//! **RAM**: its state ([`Shared`]: the log and the flags the UI, the power
//! task and the governor share) lives in SRAM3 at [`SHARED_ADDR`], not in
//! SRAM0: every byte of SRAM0 statics comes off the stack, which has no
//! margin left during playback (a 424-byte build reached the RT executor's
//! state below the stack, 2026-11-24). Entry points the UI calls are not
//! inlined, so their frames are not added to the UI's.
//!
//! Developer commands (`usbdev`): `drain start [LABEL] [stay]`,
//! `drain stop`, `drain status`. UI: Settings › System › Developer.
//!
//! No panic sites (each would keep this file's path in a resident string);
//! the card task's part (async, so with an implicit one) is in `audio`
//! (`drain_prepare`, `drain_flush`, `drain_flush_rows`).

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use critical_section::Mutex;
use orpheus_apps::services::DrainLog;
use orpheus_power::drain::{self, End, Log, Sample};

use crate::media;

/// A byte string in a paged `static` (a literal may land in a resident
/// mergeable section).
macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

/// SRAM3 above the PCM ring (`0x304000–0x306FFF`) and the 2 KiB reserved
/// for the persistent log ring (`orpheus_proto::logring::BASE`,
/// `0x307000`): `0x307800–0x307FFF` is free plain RAM (02 §1 memory map).
pub const SHARED_ADDR: usize = 0x0030_7800;

/// State shared by the logger, the UI, the power task and the governor.
#[repr(C)]
pub struct Shared {
    log: Mutex<RefCell<Log>>,
    /// Mirror of `Log::due()` for the card task's hot loop.
    due: AtomicBool,
    /// Running and not ending: the power task samples.
    active: AtomicBool,
    /// Settings › System › Performance = Maximum (D-4), from the UI.
    pub perf_max: AtomicBool,
    /// Backlight level (0–16) and panel on, from the UI.
    pub backlight: AtomicU8,
    pub screen_on: AtomicBool,
    /// Developer rows shown; Keep awake chosen for the next log.
    pub developer: AtomicBool,
    pub stay_pref: AtomicBool,
}

/// Below the card's I/O counter (`yoton_bsp::card::IO_ADDR`).
const _: () = assert!(SHARED_ADDR + core::mem::size_of::<Shared>() <= yoton_bsp::card::IO_ADDR);

/// Once, from `audio::init_chain` (in `main`) before the tasks start (not inlined: `main` is
/// resident).
#[inline(never)]
pub fn init() {
    let s = Shared {
        log: Mutex::new(RefCell::new(Log::new())),
        due: AtomicBool::new(false),
        active: AtomicBool::new(false),
        perf_max: AtomicBool::new(false),
        backlight: AtomicU8::new(12),
        screen_on: AtomicBool::new(true),
        developer: AtomicBool::new(false),
        stay_pref: AtomicBool::new(false),
    };
    // SAFETY: `SHARED_ADDR` is plain SRAM3 (enabled in crt0, inside the
    // MPU's RAM sub-region), aligned, used by nothing else; written once
    // before any task runs.
    unsafe { core::ptr::write(SHARED_ADDR as *mut Shared, s) };
}

/// The shared state.
#[must_use]
pub fn shared() -> &'static Shared {
    // SAFETY: initialised by `init` before any task runs; only accessed
    // through atomics and the critical-section mutex.
    unsafe { &*(SHARED_ADDR as *const Shared) }
}

/// `f` on the log (inside a critical section nothing else holds it; a
/// nested call would get the default instead of a panic site).
fn with<R: Default>(f: impl FnOnce(&mut Log) -> R) -> R {
    critical_section::with(|cs| match shared().log.borrow(cs).try_borrow_mut() {
        Ok(mut l) => f(&mut l),
        Err(_) => R::default(),
    })
}

/// Mirror the log's state; wake the card task when something is due.
fn publish((due, active): (bool, bool)) {
    shared().due.store(due, Ordering::Release);
    shared().active.store(active, Ordering::Release);
    if due {
        // A full queue: the idle loop also looks before it waits, and the
        // next row asks again.
        let _ = media::try_request(media::Req::Drain);
    }
}

fn update(f: impl FnOnce(&mut Log)) {
    publish(with(|l| {
        f(l);
        (l.due(), l.sampling())
    }));
}

/// Start a new log (a running one ends without its unwritten rows).
#[inline(never)]
pub fn start(label: &[u8], stay: bool) {
    let perf_max = shared().perf_max.load(Ordering::Relaxed);
    update(|l| l.start(label, stay, perf_max));
}

/// Stop: the partial row and `# stop` are written, then it is off.
#[inline(never)]
pub fn stop() {
    update(|l| l.end(End::Stop));
}

/// Power-off (UI, before the card task's shutdown flush).
#[inline(never)]
pub fn power_off() {
    update(|l| l.end(End::PowerOff));
}

#[inline(never)]
pub fn set_stay(stay: bool) {
    with(|l| l.set_stay(stay));
}

/// Auto-off is suppressed (`drain start … stay`).
#[must_use]
#[inline(never)]
pub fn stay() -> bool {
    with(|l| l.stay())
}

/// The power task samples.
#[must_use]
pub fn active() -> bool {
    shared().active.load(Ordering::Acquire)
}

/// Something for the card.
#[must_use]
pub fn due() -> bool {
    shared().due.load(Ordering::Acquire)
}

/// A second's sample (power task).
#[inline(never)]
pub fn second(s: Sample) {
    let (due, new_row) = with(|l| {
        let before = l.pending();
        let due = l.second(s);
        (due, l.pending() != before)
    });
    // The card task is asked when a write becomes due, and again with
    // every new row while it stays due (the request queue may have been
    // full); not every second, which would fill the queue the UI uses.
    if due && (!shared().due.load(Ordering::Acquire) || new_row) {
        publish((true, true));
    }
}

/// For the Developer rows.
#[must_use]
#[inline(never)]
pub fn info() -> DrainLog {
    let stay_pref = shared().stay_pref.load(Ordering::Relaxed);
    with(|l| DrainLog {
        on: l.on(),
        stay: if l.sampling() { l.stay_choice() } else { stay_pref },
        file: l.file,
        rows: l.written,
    })
}

/// The Bluetooth state for the log (`orpheus_power::drain::BT_WORDS`).
#[must_use]
pub fn bt_code() -> u8 {
    use orpheus_apps::services::BtPhase;
    let phase = critical_section::with(|cs| crate::bt::SNAP.borrow(cs).try_borrow().map(|s| s.phase));
    match phase.unwrap_or(BtPhase::Off) {
        BtPhase::Off => 0,
        BtPhase::Starting => 1,
        BtPhase::Ready => 2,
        BtPhase::Scanning => 3,
        BtPhase::Connecting => 4,
        BtPhase::Connected => 5,
        BtPhase::Streaming => 6,
        _ => 7,
    }
}

/// The file's number is still to be chosen (`audio::drain_prepare`).
#[must_use]
pub fn needs_file() -> bool {
    with(|l| l.on() && l.file == 0)
}

/// The file's number: one more than `largest` found in the folder.
pub fn set_file(largest: u16) {
    with(|l| l.file = largest.saturating_add(1).max(1));
}

/// Start a write (`audio::drain_flush_rows`): what is due formatted into
/// `buf` (header, lost-row note, rows, end) and the file's path into
/// `path`. Returns (bytes, rows, path length); `None` if nothing can be
/// written (off, no file yet, `buf` too small).
pub fn begin_write(buf: &mut [u8], path: &mut [u8; 64]) -> Option<(usize, u32, usize)> {
    let (on, file, header_done, lost) = with(|l| (l.on(), l.file, l.header_done, l.lost));
    if !on || file == 0 || buf.len() < drain::HEADER_MAX + drain::ROW_MAX + 32 {
        return None;
    }
    shared().due.store(false, Ordering::Release);
    let mut n = 0;
    if !header_done {
        n += with(|l| l.header(buf));
    }
    if lost > 0 {
        n += drain::format_lost(lost, buf.get_mut(n..).unwrap_or(&mut []));
        with(|l| l.lost = l.lost.saturating_sub(lost));
    }
    let mut rows = 0u32;
    while buf.len().saturating_sub(n) >= drain::ROW_MAX + 16 {
        let Some(r) = with(Log::pop) else { break };
        n += drain::format_row(&r, buf.get_mut(n..).unwrap_or(&mut []));
        rows += 1;
    }
    if let Some(e) = with(|l| l.ending().filter(|_| l.pending() == 0)) {
        n += drain::format_end(e, buf.get_mut(n..).unwrap_or(&mut []));
    }
    let pl = with(|l| file_path_of(l, path));
    Some((n, rows, pl))
}

/// The write started by [`begin_write`] is over.
pub fn end_write(ok: bool, rows: u32) {
    update(|l| {
        let mut rows = rows;
        if ok {
            l.header_done = true;
        } else {
            // The rows are gone; the next write says so.
            l.lost += rows;
            rows = 0;
        }
        // Ending with nothing left: off (also when the last write failed).
        l.written_out(rows);
    });
}

/// `drain start [LABEL] [stay]`, `drain stop`, `drain status`.
pub fn test_command(rest: &[u8]) -> bool {
    if let Some(args) = rest.strip_prefix(t!(b"start")) {
        let mut label: &[u8] = &[];
        let mut stay = false;
        for w in args.split(|&b| b == b' ').filter(|w| !w.is_empty()) {
            if w == t!(b"stay") {
                stay = true;
            } else if label.is_empty() {
                label = w;
            }
        }
        start(label, stay);
    } else if rest == t!(b"stop") {
        stop();
    } else if rest != t!(b"status") {
        return false;
    }
    critical_section::with(|cs| {
        if let Ok(l) = shared().log.borrow(cs).try_borrow() {
            status_text(&l);
        }
    });
    true
}

fn status_text(l: &Log) {
    let mut path = [0u8; 64];
    let pl = if l.file == 0 { 0 } else { file_path_of(l, &mut path) };
    crate::audio::reply(|w| {
        w.s(t!(b"drain "))
            .s(if !l.on() {
                t!(b"off")
            } else if l.ending().is_some() {
                t!(b"ending")
            } else {
                t!(b"on")
            })
            .s(t!(b" label="))
            .s(l.label())
            .s(t!(b" file="));
        if l.file == 0 {
            w.s(t!(b"(not yet)"));
        } else {
            w.s(path.get(..pl).unwrap_or(&[]));
        }
        w.s(t!(b" written="))
            .u(u64::from(l.written))
            .s(t!(b" pending="))
            .u(l.pending() as u64)
            .s(t!(b" lost="))
            .u(u64::from(l.lost))
            .s(t!(b" stay="))
            .u(u64::from(l.stay()));
    });
}

/// As [`file_path`], for a log already borrowed.
fn file_path_of(l: &Log, out: &mut [u8; 64]) -> usize {
    let mut w = drain::Writer::new(out);
    w.s(drain::dir().as_bytes()).b(b'/');
    let n = w.len();
    n + l.file_name(out.get_mut(n..).unwrap_or(&mut []))
}
