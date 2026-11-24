//! Battery logger (WP PWR-5, the PWR-4 measurement tool): while it runs,
//! the power task feeds it a sample every second and every minute a row
//! goes into a small RAM log (`orpheus_power::drain::Log`, 8 rows); the
//! card task appends the rows to `/.orpheus/test/drain-<n>-<label>.csv`
//! (next free `n`) every 5 rows, when the log stops and at power-off.
//!
//! The card writes are a low-priority chore of the card task, through its
//! normal file API:
//!
//! * idle: at once (a [`media::Req::Drain`] wakes the idle loop);
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
//! Developer commands (`usbdev`): `drain start [LABEL] [stay]`,
//! `drain stop`, `drain status`. UI: Settings › System › Developer.

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, Ordering};

use critical_section::Mutex;
use orpheus_apps::services::DrainLog;
use orpheus_fs::{DirEntry, OpenMode, Volume};
use orpheus_power::drain::{self, End, Log, Sample};
use yoton_bsp::card::Card;

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

static LOG: Mutex<RefCell<Log>> = Mutex::new(RefCell::new(Log::new()));
/// Mirror of `Log::due()` for the card task's hot loop (no critical
/// section per decoded block).
static DUE: AtomicBool = AtomicBool::new(false);
/// Running and not ending: the power task samples.
static ACTIVE: AtomicBool = AtomicBool::new(false);

fn with<R>(f: impl FnOnce(&mut Log) -> R) -> R {
    critical_section::with(|cs| f(&mut LOG.borrow(cs).borrow_mut()))
}

/// Mirror the log's state; wake the card task when something is due.
fn publish((due, active): (bool, bool)) {
    DUE.store(due, Ordering::Release);
    ACTIVE.store(active, Ordering::Release);
    if due {
        // A full queue: the idle loop also looks before it waits, and the
        // next second asks again.
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
pub fn start(label: &[u8], stay: bool) {
    let perf_max = crate::PERF_MAX.load(Ordering::Relaxed);
    update(|l| l.start(label, stay, perf_max));
}

/// Stop: the partial row and `# stop` are written, then it is off.
pub fn stop() {
    update(|l| l.end(End::Stop));
}

/// Power-off (UI, before the card task's shutdown flush).
pub fn power_off() {
    update(|l| l.end(End::PowerOff));
}

pub fn set_stay(stay: bool) {
    with(|l| l.set_stay(stay));
}

/// Auto-off is suppressed (`drain start … stay`).
#[must_use]
pub fn stay() -> bool {
    with(|l| l.stay())
}

/// The power task samples.
#[must_use]
pub fn active() -> bool {
    ACTIVE.load(Ordering::Acquire)
}

/// Something for the card.
#[must_use]
pub fn due() -> bool {
    DUE.load(Ordering::Acquire)
}

/// A second's sample (power task).
pub fn second(s: Sample) {
    let minute = s.uptime_s.is_multiple_of(60);
    // The card task is asked when a write becomes due, and again once a
    // minute while it stays due (the request queue may have been full);
    // not every second, which would fill the queue the UI uses.
    if with(|l| l.second(s)) && (!DUE.load(Ordering::Acquire) || minute) {
        publish((true, true));
    }
}

/// For the Developer rows.
#[must_use]
pub fn info(stay_pref: bool) -> DrainLog {
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
    match critical_section::with(|cs| crate::bt::SNAP.borrow(cs).borrow().phase) {
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

/// Choose the file's number if not done yet: one more than the largest
/// `drain-<n>` in the folder (created if missing). Reads only, apart from
/// creating the folder once. Idle loop, or a request served while playing.
pub async fn prepare(vol: &mut Volume<'static, Card>) {
    if with(|l| !l.on() || l.file != 0) {
        return;
    }
    let dir = drain::dir();
    let mut cur = match vol.open_dir(dir).await {
        Ok(c) => c,
        Err(_) => {
            // "/.orpheus", then "/.orpheus/test".
            let _ = vol.create_dir(dir.get(..9).unwrap_or(dir)).await;
            let _ = vol.create_dir(dir).await;
            match vol.open_dir(dir).await {
                Ok(c) => c,
                Err(_) => return,
            }
        }
    };
    let mut e = DirEntry::new();
    let mut max = 0u16;
    while let Ok(true) = vol.read_dir(&mut cur, &mut e).await {
        if let Some(n) = drain::file_number(e.name()) {
            max = max.max(n);
        }
    }
    with(|l| l.file = max.saturating_add(1).max(1));
}

/// [`prepare`], then [`flush_rows`] (idle loop, power-off, hand-over).
pub async fn flush(vol: &mut Volume<'static, Card>, buf: &mut [u8]) {
    prepare(vol).await;
    flush_rows(vol, buf).await;
}

/// Append what is due (header, rows, lost-row note, end) to the file,
/// formatted in `buf` (the decoder arena's spare bytes: 2 KiB or more).
/// Does nothing before [`prepare`] has chosen the file.
pub async fn flush_rows(vol: &mut Volume<'static, Card>, buf: &mut [u8]) {
    let (on, file, header_done, lost) = with(|l| (l.on(), l.file, l.header_done, l.lost));
    if !on || file == 0 || buf.len() < drain::HEADER_MAX + drain::ROW_MAX + 32 {
        return;
    }
    DUE.store(false, Ordering::Release);
    let mut n = 0;
    if !header_done {
        n += with(|l| l.header(buf));
    }
    if lost > 0 {
        n += drain::format_lost(lost, &mut buf[n..]);
        with(|l| l.lost = l.lost.saturating_sub(lost));
    }
    let mut rows = 0u32;
    while buf.len() - n >= drain::ROW_MAX + 16 {
        let Some(r) = with(Log::pop) else { break };
        n += drain::format_row(&r, &mut buf[n..]);
        rows += 1;
    }
    let end = with(|l| l.ending().filter(|_| l.pending() == 0));
    if let Some(e) = end {
        n += drain::format_end(e, &mut buf[n..]);
    }
    // "/.orpheus/test/" + "drain-<n>-<label>.csv"
    let mut path = [0u8; 64];
    let dir = drain::dir().as_bytes();
    let d = dir.len();
    path[..d].copy_from_slice(dir);
    path[d] = b'/';
    let pl = d + 1 + with(|l| l.file_name(&mut path[d + 1..]));
    let ok = match core::str::from_utf8(&path[..pl]) {
        Ok(p) => match vol.open(p, OpenMode::Create).await {
            Ok(mut f) => {
                f.set_position(f.size());
                let wrote = vol.write(&mut f, &buf[..n]).await.is_ok();
                vol.close(f).await.is_ok() && wrote && vol.flush().await.is_ok()
            }
            Err(_) => false,
        },
        Err(_) => false,
    };
    update(|l| {
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
    status_reply();
    true
}

fn status_reply() {
    critical_section::with(|cs| status_text(&LOG.borrow(cs).borrow()));
}

fn status_text(l: &Log) {
    let mut name = [0u8; 48];
    let nl = l.file_name(&mut name);
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
            w.s(t!(b"(not chosen yet)"));
        } else {
            w.s(drain::dir().as_bytes()).s(t!(b"/")).s(&name[..nl]);
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
