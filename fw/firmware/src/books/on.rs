//! Books with the firmware feature `books` (the module's documentation:
//! `crate::books`).

use core::cell::Cell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use atj2157_hal::time;
use critical_section::Mutex;
use embassy_futures::select::{select, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::Timer;
use orpheus_apps::books::{FACES, Face};
use orpheus_books::inflate::SCRATCH;
use orpheus_books::shelf::{Desk, Kind, LibPhase, Phase, Req, Scope, Shelf};
use orpheus_fs::Volume;
use yoton_bsp::card::Card;

use crate::media;

/// The data-ready tag that wakes the UI after card work changed the model
/// (the UI looks at [`CHANGED`] in every loop: `Fw::books_poll`).
pub const TAG: u16 = 0x102;

/// The model and the card side, in `.bss` (`init`, before the tasks).
static mut SHELF: MaybeUninit<Shelf> = MaybeUninit::uninit();
static mut DESK: MaybeUninit<Desk> = MaybeUninit::uninit();

/// Who holds the model ([`SHELF`]): nobody, the UI task, the card task.
const FREE: u8 = 0;
const UI: u8 = 1;
const CARD: u8 = 2;
static OWNER: AtomicU8 = AtomicU8::new(FREE);
/// The UI gave the model back (the card task's wait).
static FREED: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// A request since the card side last ran out of work (set by the UI
/// while it holds the model, cleared by the card task while it holds it:
/// no request is missed between the two).
static WORK: AtomicBool = AtomicBool::new(false);
/// The work waits for the idle card task: an EPUB's scratch, or a folder
/// to make.
static IDLE_WORK: AtomicBool = AtomicBool::new(false);
/// The model changed since the UI last looked ([`take_changed`]); the
/// wake-up ([`TAG`]) is out.
static CHANGED: AtomicBool = AtomicBool::new(false);
static WOKEN: AtomicBool = AtomicBool::new(false);
/// What the next step does (`Kind as u8`, `u8::MAX`: not known), for the
/// chore's measured need.
static NEXT: AtomicU8 = AtomicU8::new(u8::MAX);
/// `books test on`: the library is the suite's folder.
static TEST: AtomicBool = AtomicBool::new(false);
/// The idle arena given to something else since the scratch was last lent
/// ([`arena_taken`]), and the count the last lent step saw.
static ARENA_GEN: AtomicU32 = AtomicU32::new(0);
static LENT_GEN: AtomicU32 = AtomicU32::new(u32::MAX);

/// The model as the card task last left it (`books status`).
#[derive(Clone, Copy)]
struct Stat {
    lib: u8,
    count: u16,
    found: u16,
    book: u8,
    permille: u16,
    waiting: bool,
    top: u32,
    page: u32,
    pages: u32,
    complete: bool,
    steps: u32,
    /// Steps with the scratch lent, and conversions started again because
    /// it was taken back.
    lent: u32,
    retaken: u32,
    mkdir_wait: bool,
}

static STAT: Mutex<Cell<Stat>> = Mutex::new(Cell::new(Stat {
    lib: 0,
    count: 0,
    found: 0,
    book: 0,
    permille: 0,
    waiting: false,
    top: 0,
    page: u32::MAX,
    pages: 0,
    complete: false,
    steps: 0,
    lent: 0,
    retaken: 0,
    mkdir_wait: false,
}));

/// Once, from `main`, before the tasks start. Not inlined: the values are
/// built in this frame, which is gone when the tasks run (inlined into
/// `main`, 9 KB of frame stayed below the thread executor for good).
#[inline(never)]
pub fn init() {
    // SAFETY: called once before any task runs: the only references.
    unsafe {
        (*(&raw mut SHELF)).write(Shelf::new());
        (*(&raw mut DESK)).write(Desk::new());
    }
}

// ------------------------------------------------------------------ UI side

/// The model, for the UI task: taken at its first use in a UI loop and
/// held until [`ui_release`] at the loop's end. A card step in progress is
/// waited for (the card task preempts this one and gives the model back
/// after the step).
#[must_use]
pub fn ui_shelf() -> &'static mut Shelf {
    loop {
        match OWNER.compare_exchange(FREE, UI, Ordering::Acquire, Ordering::Acquire) {
            Ok(_) | Err(UI) => break,
            Err(_) => core::hint::spin_loop(),
        }
    }
    // SAFETY: `init` ran before the tasks; this task holds the model
    // (`OWNER` is `UI`) until `ui_release`, after which it keeps no
    // reference (the references handed out are tied to `Fw`'s borrows
    // within one UI loop).
    unsafe { (*(&raw mut SHELF)).assume_init_mut() }
}

/// The UI loop is over (or the UI waits for card work): the model goes
/// back.
pub fn ui_release() {
    if OWNER.compare_exchange(UI, FREE, Ordering::Release, Ordering::Relaxed).is_ok() {
        FREED.signal(());
    }
}

/// A screen's request: into the model (the UI holds it), the card task
/// woken for its work.
pub fn request(r: Req) {
    let test = TEST.load(Ordering::Relaxed);
    let r = match r {
        Req::Library(_) if test => Req::Library(Scope::Test),
        Req::Rescan(_) if test => Req::Rescan(Scope::Test),
        r => r,
    };
    ui_shelf().request(r);
    if !WORK.swap(true, Ordering::AcqRel) {
        // The idle loop waits for a request; while a track plays the
        // chores see `WORK`. A full queue: the idle loop looks at `WORK`
        // before it waits again.
        let _ = media::try_request(media::Req::Books);
    }
}

/// The model changed since the UI last looked (every UI loop).
#[must_use]
pub fn take_changed() -> bool {
    WOKEN.store(false, Ordering::Release);
    CHANGED.swap(false, Ordering::AcqRel)
}

/// Before power-off and a restart (the UI task): the reading position
/// written (the reader closes), waited for at most `max_ms`.
pub async fn save_for_shutdown(max_ms: u64) {
    let sh = ui_shelf();
    if sh.book.phase != Phase::Ready || !crate::state::CARD_READY.load(Ordering::Relaxed) {
        ui_release();
        return;
    }
    request(Req::Close);
    ui_release();
    let t0 = time::now_us() / 1000;
    while WORK.load(Ordering::Acquire) && time::now_us() / 1000 - t0 < max_ms {
        Timer::after_millis(10).await;
    }
}

// ---------------------------------------------------------------- card side

/// The model and the desk, for the card task while it holds the model.
///
/// # Safety
/// `OWNER` must be `CARD` (taken by this task) for as long as the result
/// is used.
unsafe fn card_parts() -> (&'static mut Shelf, &'static mut Desk) {
    // SAFETY: `init` ran before the tasks; the caller holds the model, and
    // the desk is only ever touched by the card task.
    unsafe { ((*(&raw mut SHELF)).assume_init_mut(), (*(&raw mut DESK)).assume_init_mut()) }
}

fn card_try() -> bool {
    OWNER.compare_exchange(FREE, CARD, Ordering::Acquire, Ordering::Relaxed).is_ok()
}

/// Take the model, waiting for the UI's loop to end.
async fn card_take() {
    while !card_try() {
        FREED.wait().await;
    }
}

fn card_release() {
    OWNER.store(FREE, Ordering::Release);
}

/// Card work is waiting for the card task (a chore while a track plays).
#[must_use]
pub fn wants() -> bool {
    WORK.load(Ordering::Acquire)
}

/// Card work is waiting for the idle loop (any work, or the scratch and
/// folders only it gives).
#[must_use]
pub fn wants_idle() -> bool {
    WORK.load(Ordering::Acquire) || IDLE_WORK.load(Ordering::Acquire)
}

/// The kind of the next step (`0..orpheus_books::shelf::KINDS`, the chore's
/// measured need; a read when not known).
#[must_use]
pub fn kind() -> usize {
    match NEXT.load(Ordering::Relaxed) {
        k if usize::from(k) < orpheus_books::shelf::KINDS => usize::from(k),
        _ => Kind::Read as usize,
    }
}

/// The idle loop gives the arena to something else (a request, a
/// picture, the index build, the logs, playback): the scratch lent to an
/// EPUB is no longer what its last step left.
pub fn arena_taken() {
    ARENA_GEN.fetch_add(1, Ordering::Relaxed);
}

/// One step on the model (held by this task); afterwards the observables,
/// the next step's kind and, if the model changed, the UI's wake-up.
async fn step_once(vol: &mut Volume<'static, Card>, scratch: Option<&mut [u8]>, mkdir: bool) -> bool {
    // SAFETY: the callers took the model (`card_try`, `card_take`) and
    // give it back after this step.
    let (sh, desk) = unsafe { card_parts() };
    desk.mkdir = mkdir;
    let lent = scratch.is_some();
    let face: &Face = sh.params.as_ref().map_or(&FACES[0], Face::of_params);
    let changed = desk.step_with(sh, vol, face, scratch).await;
    if !changed {
        WORK.store(false, Ordering::Release);
        IDLE_WORK.store(desk.needs_scratch(sh) || desk.wants_mkdir(), Ordering::Release);
    }
    NEXT.store(desk.next_kind(sh).map_or(u8::MAX, |k| k as u8), Ordering::Relaxed);
    publish(sh, desk, lent);
    if changed {
        CHANGED.store(true, Ordering::Release);
        if !WOKEN.swap(true, Ordering::AcqRel) {
            media::ready(TAG);
        }
    }
    changed
}

fn publish(sh: &Shelf, desk: &Desk, lent: bool) {
    let (l, b) = (&sh.lib, &sh.book);
    critical_section::with(|cs| {
        let c = STAT.borrow(cs);
        let mut s = c.get();
        s.lib = l.phase as u8;
        s.count = l.count;
        s.found = l.found;
        s.book = b.phase as u8;
        s.permille = b.permille;
        s.waiting = b.waiting;
        s.top = b.top.offset;
        s.page = b.page_no.unwrap_or(u32::MAX);
        s.pages = b.pages;
        s.complete = b.complete;
        s.steps = sh.steps;
        s.lent += u32::from(lent);
        s.mkdir_wait = desk.wants_mkdir();
        c.set(s);
    });
}

/// A chore while a track plays (`audio::chores`): steps of the kind it
/// was admitted for ([`kind`]) while the chore's budget lasts (the first
/// always), no scratch, no folder made. Nothing if the UI holds the model
/// (its loop is in progress): the next chore. A step of another kind
/// waits for a chore of its own: its need is not the one this chore was
/// measured and admitted with (BOOK-1/2b: a write after the reads of an
/// open ran inside a `job:bread` piece).
pub async fn chore(vol: &mut Volume<'static, Card>) {
    if !card_try() {
        return;
    }
    let admitted = kind();
    while crate::audio::piece_left() {
        if !step_once(vol, None, false).await || kind() != admitted {
            break;
        }
    }
    card_release();
}

/// Paused (`audio::pause`, the DAC stopped): steps until the work is done,
/// a request waits, or about 150 ms have passed (the paused loop goes on);
/// folders may be made, the scratch is not lent (the decoder is loaded).
/// After each step this task sleeps as long as it took (at least 2 ms), as
/// the index build does: the card spins, and the UI keeps half the CPU.
pub async fn run_paused(vol: &mut Volume<'static, Card>) {
    let t0 = time::now_us();
    while wants() && media::REQS.is_empty() && media::PLAY.is_empty() && time::now_us() - t0 < 150_000 {
        card_take().await;
        let s0 = time::now_us();
        let more = step_once(vol, None, true).await;
        card_release();
        if !more {
            break;
        }
        rest(time::now_us() - s0).await;
    }
}

/// No track loaded (the idle loop, and `Req::Books` served there): steps
/// until the work is done or other work waits for the idle loop, with
/// folders made and, while the desk asks for it, the arena's first
/// 35,072 bytes lent (unchanged between the lent steps unless the idle
/// loop gave the arena to something else: then one step without it, and
/// an entry half decoded starts again). Paced as [`run_paused`].
pub async fn run_idle(vol: &mut Volume<'static, Card>, arena: &mut [u8]) {
    while wants_idle() && !crate::audio::work_waiting() {
        card_take().await;
        // SAFETY: this task holds the model (`card_take`) until
        // `card_release` below; the borrow ends before the step.
        let lend = unsafe {
            let (sh, desk) = card_parts();
            desk.needs_scratch(sh)
        };
        let seen = ARENA_GEN.load(Ordering::Relaxed);
        let retake = lend && LENT_GEN.swap(seen, Ordering::Relaxed) != seen;
        if retake {
            critical_section::with(|cs| {
                let c = STAT.borrow(cs);
                let mut s = c.get();
                s.retaken += 1;
                c.set(s);
            });
        }
        let scratch = (lend && !retake).then(|| arena.get_mut(..SCRATCH)).flatten();
        let s0 = time::now_us();
        let more = step_once(vol, scratch, true).await;
        card_release();
        // A step taken without the scratch it asked for may have had
        // nothing to do (the entry waits): again, with it.
        if !more && !retake {
            break;
        }
        rest(time::now_us() - s0).await;
    }
}

/// Sleep as long as the step took (2–100 ms), or until a request comes.
async fn rest(dt_us: u64) {
    let _ = select(
        Timer::after_micros(dt_us.clamp(2_000, 100_000)),
        select3(
            crate::audio::CMDS.ready_to_receive(),
            media::REQS.ready_to_receive(),
            media::PLAY.ready_to_receive(),
        ),
    )
    .await;
}

/// The card goes to the USB task (`Server::card_leaving`, idle): the
/// reading state written, the files the jobs left closed, the sector
/// cache written back; the desk starts afresh and the open book is opened
/// again when the card is back (its position kept).
pub async fn card_leaving(vol: &mut Volume<'static, Card>) {
    card_take().await;
    // SAFETY: this task holds the model until `card_release`.
    let (sh, desk) = unsafe { card_parts() };
    desk.suspend(sh, vol, 64).await;
    WORK.store(true, Ordering::Release);
    IDLE_WORK.store(false, Ordering::Release);
    LENT_GEN.store(u32::MAX, Ordering::Relaxed);
    publish(sh, desk, false);
    card_release();
    CHANGED.store(true, Ordering::Release);
    media::ready(TAG);
}

// ---------------------------------------------------------------- test hooks

/// Developer `books …` (USB task): `status`, `test on|off`.
pub fn test_command(rest: &[u8]) -> bool {
    match rest {
        b"status" => status_reply(),
        b"test on" | b"test off" => {
            let on = rest == b"test on";
            TEST.store(on, Ordering::Relaxed);
            crate::audio::reply(|w| {
                w.s(if on { b"books test on" } else { b"books test off" });
            });
        }
        _ => return false,
    }
    true
}

/// `books: lib=ready n=2 found=2 book=ready conv=1000 wait=0 top=0 page=0
/// pages=4 laid=1 steps=123 lent=0 retaken=0 mkdir=0 test=1`.
fn status_reply() {
    let s = critical_section::with(|cs| STAT.borrow(cs).get());
    let lib: &[u8] = match s.lib {
        x if x == LibPhase::Loading as u8 => b"loading",
        x if x == LibPhase::Searching as u8 => b"searching",
        x if x == LibPhase::Ready as u8 => b"ready",
        _ => b"unknown",
    };
    let book: &[u8] = match s.book {
        x if x == Phase::Opening as u8 => b"opening",
        x if x == Phase::Converting as u8 => b"converting",
        x if x == Phase::Ready as u8 => b"ready",
        x if x == Phase::Failed as u8 => b"failed",
        _ => b"closed",
    };
    crate::audio::reply(|w| {
        w.s(b"books: lib=").s(lib).s(b" n=").u(u64::from(s.count)).s(b" found=").u(u64::from(s.found));
        w.s(b" book=").s(book).s(b" conv=").u(u64::from(s.permille)).s(b" wait=").u(u64::from(s.waiting));
        w.s(b" top=").u(u64::from(s.top)).s(b" page=");
        if s.page == u32::MAX {
            w.s(b"-");
        } else {
            w.u(u64::from(s.page));
        }
        w.s(b" pages=").u(u64::from(s.pages)).s(b" laid=").u(u64::from(s.complete));
        w.s(b" steps=").u(u64::from(s.steps)).s(b" lent=").u(u64::from(s.lent));
        w.s(b" retaken=").u(u64::from(s.retaken)).s(b" mkdir=").u(u64::from(s.mkdir_wait));
        w.s(b" test=").u(u64::from(TEST.load(Ordering::Relaxed)));
    });
}
