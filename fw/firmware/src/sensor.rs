//! Step counting (WP SENS-1): the accelerometer (`yoton_bsp::accel`), the
//! detector and the day history (`orpheus_steps`), the trace capture.
//!
//! **Sampling.** The `sensor` task (main executor) reads one sample every
//! 40 ms (25 Hz, the counting rate of 03 §15) while counting is wanted:
//! Steps › Count in background is "While on", or the Steps app is open, or
//! a trace runs. Otherwise the part is suspended and the task looks at the
//! flags once a second. A sample is one 6-byte burst read (≈ 1 ms of bus
//! at 100 kHz, interrupts open); the samples are kept in a batch and the
//! detector, the day check (RTC) and the history run once per batch (one
//! second), so the code touched 25 times a second is the read and the
//! batch push only (cluster `steps-sample` in `hot.toml`: one 1 KiB line
//! beside the playback path's lines, not the whole detector).
//!
//! **Steps file.** `/.orpheus/steps.bin` (`orpheus_steps::history`) is read
//! by the card task at mount and written by it when due: 10 minutes after
//! the first unsaved step, at a day change, at power-off and before the
//! card goes to USB; never while the clock is not valid (U-RTC). While a
//! track plays a write waits for room in the DAC's ring, like the battery
//! log (`audio::ROOMY`).
//!
//! **Trace capture** (`acc trace <secs>`): the samples also go into a
//! 64-sample ring; the card task appends them to
//! `/.orpheus/test/acc-<n>.csv` (`orpheus_steps::trace`) when 32 are
//! waiting, and at the end. Samples that find the ring full are counted
//! (`# lost N`).
//!
//! **RAM**: the state ([`Shared`]) lives in SRAM3 at [`SHARED_ADDR`], above
//! the battery logger's (`drain::SHARED_ADDR`), not in SRAM0 (02 §1).
//!
//! Developer commands (`usbdev`): `acc read`, `acc regs`, `acc trace
//! <secs>`, `acc trace stop`, `steps status`.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicU8, Ordering};

use critical_section::Mutex;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Ticker, Timer};
use orpheus_steps::trace::{self, Sample};
use orpheus_steps::{Detector, History, Sensitivity, history};
use yoton_bsp::accel::{self, Accel};

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

/// Samples per second while counting.
pub const RATE: u16 = 25;
/// Samples per detector batch (one second).
const BATCH: usize = RATE as usize;
/// Trace ring (samples) and the fill that makes a write due.
const TRACE_RING: usize = 64;
const TRACE_DUE: usize = 32;
/// The steps file is written this long after the first unsaved step.
const SAVE_AFTER_MS: u64 = 10 * 60_000;
/// Longest trace (`acc trace <secs>`).
const TRACE_MAX_S: u32 = 3600;

/// SRAM3 right after the battery logger's state (`drain::SHARED_END`,
/// 8-byte aligned); the radio's follows (`fm::FM_ADDR`), all below the
/// card I/O counter (`0x307FFC`). Chained so that no two overlap: at a
/// fixed `0x307A00` this state ran into the radio's (2026-11-24).
pub const SHARED_ADDR: usize = orpheus_proto::mem::sram3_next(crate::drain::SHARED_END);
/// End of [`Shared`].
pub const SHARED_END: usize = SHARED_ADDR + core::mem::size_of::<Shared>();

/// The trace capture.
struct Trace {
    on: bool,
    /// Stopped (time up, or `acc trace stop`): the rest and `# end` go out,
    /// then it is off.
    ending: bool,
    header_done: bool,
    /// File number (0: not chosen yet).
    file: u16,
    secs: u32,
    start_ms: u64,
    ring: [Sample; TRACE_RING],
    head: u8,
    len: u8,
    lost: u32,
    written: u32,
}

struct State {
    history: History,
    detector: Detector,
    /// The part answered with its id at boot.
    present: bool,
    id: u8,
    /// Sampling now.
    counting: bool,
    /// The clock is valid (days are dated).
    dated: bool,
    /// The card task read the steps file (or found none): saving may
    /// start.
    loaded: bool,
    /// When the first unsaved step came (ms), a day changed.
    dirty_since: Option<u64>,
    day_changed: bool,
    samples: u32,
    errors: u32,
    last: [i16; 3],
    batch: [[i16; 3]; BATCH],
    n: u8,
    saves: u32,
    trace: Trace,
}

/// State shared by the sensor task, the UI, the card task and the
/// developer channel.
#[repr(C)]
pub struct Shared {
    state: Mutex<RefCell<State>>,
    /// From the UI: the Steps app is open; Count in background; Sensitivity.
    app_open: AtomicBool,
    background: AtomicBool,
    sensitivity: AtomicU8,
    /// Something for the card (steps file, trace rows): mirrors of the
    /// state for the card task's hot loop.
    card_due: AtomicBool,
    /// Today's count changed (for the UI's event).
    changed: AtomicBool,
}

/// Below the card's I/O counter.
const _: () = assert!(SHARED_ADDR + core::mem::size_of::<Shared>() <= yoton_bsp::card::IO_ADDR);
/// Above the battery logger's state, aligned.
const _: () =
    assert!(crate::drain::SHARED_ADDR + core::mem::size_of::<crate::drain::Shared>() <= SHARED_ADDR);
const _: () = assert!(SHARED_ADDR % core::mem::align_of::<Shared>() == 0);

/// Commands for the sensor task (they need the bus or the chip's mode).
#[derive(Clone, Copy)]
pub enum Cmd {
    Read,
    Regs,
}

static CMD: Signal<CriticalSectionRawMutex, Cmd> = Signal::new();

/// Once, before the tasks start (from `audio::init_chain`).
#[inline(never)]
pub fn init() {
    let s = Shared {
        state: Mutex::new(RefCell::new(State {
            history: History::new(),
            detector: Detector::new(RATE, Sensitivity::Medium),
            present: false,
            id: 0,
            counting: false,
            dated: false,
            loaded: false,
            dirty_since: None,
            day_changed: false,
            samples: 0,
            errors: 0,
            last: [0; 3],
            batch: [[0; 3]; BATCH],
            n: 0,
            saves: 0,
            trace: Trace {
                on: false,
                ending: false,
                header_done: false,
                file: 0,
                secs: 0,
                start_ms: 0,
                ring: [Sample { ms: 0, x: 0, y: 0, z: 0 }; TRACE_RING],
                head: 0,
                len: 0,
                lost: 0,
                written: 0,
            },
        })),
        app_open: AtomicBool::new(false),
        background: AtomicBool::new(true),
        sensitivity: AtomicU8::new(1),
        card_due: AtomicBool::new(false),
        changed: AtomicBool::new(false),
    };
    // SAFETY: `SHARED_ADDR` is plain SRAM3 (enabled in crt0, inside the
    // MPU's RAM sub-region), aligned, used by nothing else (the asserts
    // above); written once before any task runs.
    unsafe { core::ptr::write(SHARED_ADDR as *mut Shared, s) };
}

fn shared() -> &'static Shared {
    // SAFETY: initialised by `init` before any task runs; accessed only
    // through atomics and the critical-section mutex.
    unsafe { &*(SHARED_ADDR as *const Shared) }
}

/// `f` on the state (a nested borrow gets the default, never a panic).
fn with<R: Default>(f: impl FnOnce(&mut State) -> R) -> R {
    critical_section::with(|cs| match shared().state.borrow(cs).try_borrow_mut() {
        Ok(mut s) => f(&mut s),
        Err(_) => R::default(),
    })
}

fn now_ms() -> u64 {
    atj2157_hal::time::now_us() / 1000
}

/// Today's day number, if the clock is valid (running, set this decade).
fn today() -> Option<u32> {
    let t = atj2157_hal::rtc_clock::now();
    (atj2157_hal::rtc_clock::running() && t.year >= 2026)
        .then(|| orpheus_steps::day_number(t.year, t.month, t.day))
}

// ---------------------------------------------------------------- the task

/// Counting is wanted.
fn wanted() -> bool {
    let sh = shared();
    sh.background.load(Ordering::Relaxed)
        || sh.app_open.load(Ordering::Relaxed)
        || with(|s| s.trace.on && !s.trace.ending)
}

#[embassy_executor::task]
pub async fn sensor() {
    let acc = Accel::new();
    let id = acc.id();
    let present = id == Ok(accel::CHIP_ID);
    with(|s| {
        s.present = present;
        s.id = id.unwrap_or(0);
    });
    // Suspended at boot (fm-i2c-imu.md §4).
    let _ = acc.suspend();
    let mut on = false;
    let mut ticker = Ticker::every(Duration::from_millis(1000 / u64::from(RATE)));
    loop {
        if let Some(c) = CMD.try_take() {
            command(&acc, c, on).await;
        }
        let want = present && wanted();
        if want != on {
            on = want && start(&acc).await;
            if !on {
                let _ = acc.suspend();
            }
            with(|s| s.counting = on);
            ticker.reset();
        }
        if !on {
            // Nothing counts: look again in a second (or at a command).
            if let Either::First(c) = select(CMD.wait(), Timer::after_secs(1)).await {
                command(&acc, c, on).await;
            }
            continue;
        }
        ticker.next().await;
        sample(&acc);
    }
}

/// Configure the part for counting; the detector starts afresh.
#[inline(never)]
async fn start(acc: &Accel) -> bool {
    let ok = acc.open().await.is_ok_and(|id| id == accel::CHIP_ID);
    let sens = Sensitivity::from_index(i16::from(shared().sensitivity.load(Ordering::Relaxed)));
    with(|s| {
        s.n = 0;
        s.detector.restart();
        s.detector.set_sensitivity(sens);
        if !ok {
            s.errors += 1;
        }
    });
    ok
}

/// One sample into the batch (and the trace); the batch through the
/// detector when full. The per-sample path: kept small (`hot.toml`).
#[inline(never)]
fn sample(acc: &Accel) {
    let r = acc.read();
    let (full, ended) = with(|s| {
        let was = s.trace.ending;
        match r {
            Ok(v) => {
                s.samples = s.samples.wrapping_add(1);
                s.last = v;
                if let Some(slot) = s.batch.get_mut(usize::from(s.n)) {
                    *slot = v;
                    s.n += 1;
                }
                if s.trace.on && !s.trace.ending {
                    trace_push(&mut s.trace, v);
                }
            }
            Err(_) => s.errors = s.errors.wrapping_add(1),
        }
        (usize::from(s.n) >= BATCH, s.trace.ending && !was)
    });
    if full {
        batch();
    }
    if ended {
        // The trace's time is up: its end goes to the card now (counting
        // may stop with it).
        publish_due(true);
    }
}

fn trace_push(t: &mut Trace, v: [i16; 3]) {
    let ms = (now_ms().saturating_sub(t.start_ms)).min(u64::from(u32::MAX)) as u32;
    if ms / 1000 >= t.secs {
        t.ending = true;
        return;
    }
    if usize::from(t.len) == TRACE_RING {
        t.lost += 1;
        return;
    }
    let i = (usize::from(t.head) + usize::from(t.len)) % TRACE_RING;
    if let Some(slot) = t.ring.get_mut(i) {
        *slot = Sample { ms, x: v[0], y: v[1], z: v[2] };
        t.len += 1;
    }
}

/// A second of samples: the day, the detector, the history; the card task
/// is told when something is due.
#[inline(never)]
fn batch() {
    let day = today();
    let now = now_ms();
    let sens = Sensitivity::from_index(i16::from(shared().sensitivity.load(Ordering::Relaxed)));
    let (changed, due) = with(|s| {
        roll(s, day);
        s.detector.set_sensitivity(sens);
        let mut k = 0;
        for v in s.batch.iter().take(usize::from(s.n)) {
            k += s.detector.feed(v[0], v[1], v[2]);
        }
        s.n = 0;
        s.history.add(k);
        if s.history.dirty() && s.dirty_since.is_none() {
            s.dirty_since = Some(now);
        }
        (k > 0, card_wants(s, now))
    });
    if changed {
        shared().changed.store(true, Ordering::Release);
    }
    publish_due(due);
}

/// The history to today (`None`: the clock is not valid, nothing moves).
fn roll(s: &mut State, day: Option<u32>) {
    s.dated = day.is_some();
    if let Some(d) = day {
        let before = s.history.current_day();
        s.history.roll(d);
        if before.is_some_and(|b| b != d) {
            s.day_changed = true;
        }
    }
}

/// The card has something to do for the sensor.
fn card_wants(s: &State, now: u64) -> bool {
    let save = s.loaded
        && s.dated
        && s.history.dirty()
        && (s.day_changed || s.dirty_since.is_some_and(|t| now.saturating_sub(t) >= SAVE_AFTER_MS));
    let t = &s.trace;
    let trace = t.on && (t.file == 0 || usize::from(t.len) >= TRACE_DUE || t.ending);
    save || trace
}

fn publish_due(due: bool) {
    let was = shared().card_due.swap(due, Ordering::AcqRel);
    if due && !was {
        let _ = media::try_request(media::Req::Sensor);
    }
}

// ---------------------------------------------------------------- the UI

/// Each UI loop: the Steps app's visibility and the settings.
pub fn set_ui(app_open: bool, background: bool, sensitivity: u8) {
    let sh = shared();
    sh.app_open.store(app_open, Ordering::Relaxed);
    sh.background.store(background, Ordering::Relaxed);
    sh.sensitivity.store(sensitivity, Ordering::Relaxed);
}

/// Today's count changed since the last call.
pub fn take_changed() -> bool {
    shared().changed.swap(false, Ordering::AcqRel)
}

/// For the Steps app: sensor present, counting, dated.
#[must_use]
#[inline(never)]
pub fn info() -> orpheus_apps::services::StepsInfo {
    with(|s| orpheus_apps::services::StepsInfo { sensor: s.present, counting: s.counting, dated: s.dated })
}

/// Steps `ago` days before today.
#[must_use]
#[inline(never)]
pub fn day(ago: u8) -> u32 {
    // Today's row first moves to today (counting may have been off since
    // midnight).
    let d = if ago == 0 { today() } else { None };
    with(|s| {
        if d.is_some() {
            roll(s, d);
        }
        s.history.steps_ago(usize::from(ago))
    })
}

/// Steps › Reset today.
#[inline(never)]
pub fn reset_today() {
    let now = now_ms();
    with(|s| {
        s.history.reset_today();
        s.dirty_since.get_or_insert(now);
    });
    shared().changed.store(true, Ordering::Release);
}

// ---------------------------------------------------------------- the card

/// Something for the card task (steps file or trace rows).
#[must_use]
pub fn card_due() -> bool {
    shared().card_due.load(Ordering::Acquire)
}

/// The steps file was read at mount (`None`: none, or not valid).
pub fn loaded(h: Option<History>) {
    with(|s| {
        if let Some(h) = h {
            s.history.merge_loaded(h);
        }
        s.loaded = true;
    });
}

/// The steps file's bytes if a save is due (or `force`d: power-off, card
/// hand-over), while the clock is valid.
pub fn begin_save(force: bool) -> Option<[u8; history::FILE_LEN]> {
    let now = now_ms();
    with(|s| {
        let due = s.loaded
            && s.dated
            && s.history.dirty()
            && (force
                || s.day_changed
                || s.dirty_since.is_some_and(|t| now.saturating_sub(t) >= SAVE_AFTER_MS));
        due.then(|| {
            s.history.clear_dirty();
            s.dirty_since = None;
            s.day_changed = false;
            s.history.encode()
        })
    })
}

/// The write started by [`begin_save`] is over.
pub fn end_save(ok: bool) {
    let now = now_ms();
    let due = with(|s| {
        if ok {
            s.saves += 1;
        } else {
            // Try again at the next due time.
            s.dirty_since = Some(now);
        }
        card_wants(s, now)
    });
    publish_due(due);
}

/// A trace waits for its file number.
#[must_use]
pub fn trace_needs_file() -> bool {
    with(|s| s.trace.on && s.trace.file == 0)
}

/// The trace's file number: one more than the largest found.
pub fn set_trace_file(largest: u16) {
    with(|s| s.trace.file = largest.saturating_add(1).max(1));
}

/// Start a trace write: the header (once), the samples waiting, a lost
/// note and the end, formatted into `buf`; the file's path into `path`.
/// Returns (bytes, path length); `None` if there is nothing to write.
pub fn begin_trace_write(buf: &mut [u8], path: &mut [u8; 48]) -> Option<(usize, usize)> {
    let now = now_ms();
    let r = with(|s| {
        let t = &mut s.trace;
        if !t.on || t.file == 0 || buf.len() < 128 + TRACE_RING * trace::LINE_MAX {
            return None;
        }
        // Time up while nothing was sampled (counting stopped).
        if !t.ending && now.saturating_sub(t.start_ms) / 1000 >= u64::from(t.secs) {
            t.ending = true;
        }
        let mut n = 0;
        if !t.header_done {
            n += trace::format_header(t.file, RATE, accel::ODR_COUNTING, t.secs, buf);
            t.header_done = true;
        }
        if t.lost > 0 {
            n += trace::format_lost(t.lost, buf.get_mut(n..).unwrap_or(&mut []));
            t.lost = 0;
        }
        while t.len > 0 {
            let Some(smp) = t.ring.get(usize::from(t.head)).copied() else { break };
            n += trace::format_sample(&smp, buf.get_mut(n..).unwrap_or(&mut []));
            t.head = ((usize::from(t.head) + 1) % TRACE_RING) as u8;
            t.len -= 1;
            t.written += 1;
        }
        if t.ending {
            n += trace::format_end(buf.get_mut(n..).unwrap_or(&mut []));
            t.on = false;
        }
        let pl = trace::file_path(t.file, path);
        Some((n, pl))
    });
    let due = with(|s| card_wants(s, now));
    publish_due(due);
    r
}

// ---------------------------------------------------------------- commands

/// `acc read`, `acc regs`, `acc trace <secs>`, `acc trace stop`.
pub fn acc_command(rest: &[u8]) -> bool {
    if matches!(rest, [b'r', b'e', b'a', b'd']) {
        CMD.signal(Cmd::Read);
    } else if matches!(rest, [b'r', b'e', b'g', b's']) {
        CMD.signal(Cmd::Regs);
    } else if rest == t!(b"trace stop") {
        with(|s| {
            if s.trace.on {
                s.trace.ending = true;
            }
        });
        publish_due(true);
        trace_reply();
    } else if let Some(n) = rest.strip_prefix(t!(b"trace ")) {
        let secs = parse_u32(n).unwrap_or(0).min(TRACE_MAX_S);
        if secs == 0 {
            return false;
        }
        if !with(|s| s.present) {
            crate::audio::reply(|w| {
                w.s(t!(b"acc trace: no sensor answered"));
            });
            return true;
        }
        let now = now_ms();
        with(|s| {
            s.trace = Trace {
                on: true,
                ending: false,
                header_done: false,
                file: 0,
                secs,
                start_ms: now,
                ring: s.trace.ring,
                head: 0,
                len: 0,
                lost: 0,
                written: 0,
            };
        });
        publish_due(true);
        trace_reply();
    } else {
        return false;
    }
    true
}

fn parse_u32(b: &[u8]) -> Option<u32> {
    let b = b.trim_ascii();
    if b.is_empty() || b.len() > 9 {
        return None;
    }
    b.iter().try_fold(0u32, |v, c| c.is_ascii_digit().then(|| v * 10 + u32::from(c - b'0')))
}

fn trace_reply() {
    let (on, file, secs, written, lost) =
        with(|s| (s.trace.on, s.trace.file, s.trace.secs, s.trace.written, s.trace.lost));
    let mut path = [0u8; 48];
    let pl = if file == 0 { 0 } else { trace::file_path(file, &mut path) };
    crate::audio::reply(|w| {
        w.s(t!(b"acc trace ")).s(if on { t!(b"on") } else { t!(b"off") }).s(t!(b" secs=")).u(u64::from(secs));
        w.s(t!(b" file="));
        if file == 0 {
            w.s(t!(b"(not yet)"));
        } else {
            w.s(path.get(..pl).unwrap_or(&[]));
        }
        w.s(t!(b" written=")).u(u64::from(written)).s(t!(b" lost=")).u(u64::from(lost));
    });
}

/// In the sensor task (it owns the chip's mode): one sample, or the
/// register map.
#[inline(never)]
async fn command(acc: &Accel, c: Cmd, on: bool) {
    // A suspended part is woken for the read and put back.
    let woke = !on && matches!(c, Cmd::Read) && acc.open().await.is_ok();
    if woke {
        Timer::after_millis(40).await;
    }
    match c {
        Cmd::Read => {
            let id = acc.id();
            let odr = acc.reg(accel::reg::ODR_AXIS);
            let mode = acc.reg(accel::reg::MODE_BW);
            let v = acc.read();
            let (present, counting, samples, errors) = with(|s| (s.present, s.counting, s.samples, s.errors));
            crate::audio::reply(|w| {
                match v {
                    Ok(v) => {
                        w.s(t!(b"acc x=")).i(v[0]).s(t!(b" y=")).i(v[1]).s(t!(b" z=")).i(v[2]);
                        // Magnitude in milli-g.
                        let sq = |a: i16| i64::from(a) * i64::from(a);
                        let m = orpheus_steps::detector::isqrt(
                            (sq(v[0]) + sq(v[1]) + sq(v[2])).min(i64::from(u32::MAX)) as u32,
                        );
                        w.s(t!(b" magnitude_mg=")).u(u64::from(m) * 1000 / 1024);
                    }
                    Err(_) => {
                        w.s(t!(b"acc read failed"));
                    }
                }
                w.s(t!(b" chip_id="))
                    .h(u32::from(id.unwrap_or(0)))
                    .s(t!(b" odr="))
                    .h(u32::from(odr.unwrap_or(0)));
                w.s(t!(b" mode=")).h(u32::from(mode.unwrap_or(0)));
                w.s(t!(b" rate=")).u(u64::from(RATE)).s(t!(b" present=")).u(u64::from(present));
                w.s(t!(b" counting=")).u(u64::from(counting)).s(t!(b" samples=")).u(u64::from(samples));
                w.s(t!(b" i2c_errors=")).u(u64::from(errors));
            });
        }
        Cmd::Regs => {
            // Registers 0x00–0x3F (reads only), for identifying the part
            // (U-ACC-1).
            let mut r = [0u8; 64];
            let ok = acc.regs(0, &mut r).is_ok();
            crate::audio::reply(|w| {
                w.s(t!(b"acc regs:"));
                if !ok {
                    w.s(t!(b" failed"));
                    return;
                }
                for (i, b) in r.iter().enumerate() {
                    if i % 16 == 0 {
                        w.s(t!(b"\n")).h(i as u32).s(t!(b":"));
                    }
                    w.s(t!(b" ")).h(u32::from(*b));
                }
            });
        }
    }
    if woke {
        let _ = acc.suspend();
    }
}

/// `steps status`.
pub fn steps_status() {
    let (present, counting, dated, loaded, total, samples, errors, saves) =
        with(|s| (s.present, s.counting, s.dated, s.loaded, s.detector.total, s.samples, s.errors, s.saves));
    let days: [u32; 7] = core::array::from_fn(|k| day(k as u8));
    let sh = shared();
    crate::audio::reply(|w| {
        w.s(t!(b"steps today=")).u(u64::from(days[0])).s(t!(b" week="));
        for (k, d) in days.iter().enumerate() {
            if k > 0 {
                w.s(t!(b","));
            }
            w.u(u64::from(*d));
        }
        w.s(t!(b" present=")).u(u64::from(present)).s(t!(b" counting=")).u(u64::from(counting));
        w.s(t!(b" background=")).u(u64::from(sh.background.load(Ordering::Relaxed)));
        w.s(t!(b" app=")).u(u64::from(sh.app_open.load(Ordering::Relaxed)));
        w.s(t!(b" dated=")).u(u64::from(dated)).s(t!(b" file_loaded=")).u(u64::from(loaded));
        w.s(t!(b" detected=")).u(u64::from(total)).s(t!(b" samples=")).u(u64::from(samples));
        w.s(t!(b" i2c_errors=")).u(u64::from(errors)).s(t!(b" saves=")).u(u64::from(saves));
    });
}
