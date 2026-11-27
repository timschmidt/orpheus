//! Settings › Storage's card work (05 §7.11; VER-13, DOC-2 findings
//! 2026-11-27): Clear caches, Check card, Format card.
//!
//! The UI asks with [`request`] (one at a time); the card task stops
//! playback and saves its state as for a USB hand-over (`audio::hand_over`),
//! runs [`run`] with the volume, then picks the card up as after one
//! (`audio::card_back`). The outcome is published for the UI
//! ([`outcome`]; `other::STORAGE` when it ends, `Fw::care_poll`).
//!
//! * Clear caches removes `services::CACHE_DIRS` (`/.orpheus/cache`,
//!   `/.orpheus/index`) through the volume's `/.orpheus` write prefix
//!   (CLAUDE.md rule 5).
//! * Check card reads every folder and follows every file's chain
//!   (`Volume::check`): no write; a request for the card stops it.
//! * Format card unmounts the volume and runs `orpheus_fs::format` on the
//!   card's own partition (FAT32 to 32 GiB, exFAT above; the partition
//!   table is not written), then mounts it again. Only the card: NOR is
//!   not touched. The user confirmed it in a destructive dialog.

use core::cell::Cell;
use core::sync::atomic::{AtomicBool, Ordering};

use crate::frame::Own;
use critical_section::Mutex;
use orpheus_apps::services::{CACHE_DIRS, CHECK_DEPTH, StorageOutcome, StorageRequest};
use orpheus_fs::{DirEntry, Volume};
use yoton_bsp::card::Card;

const IDLE: StorageOutcome =
    StorageOutcome { req: None, busy: false, ok: false, dirs: 0, files: 0, problems: 0, stopped: false };

static OUT: Mutex<Cell<StorageOutcome>> = Mutex::new(Cell::new(IDLE));
static CHANGED: AtomicBool = AtomicBool::new(false);

/// The last card work's outcome.
#[must_use]
pub fn outcome() -> StorageOutcome {
    critical_section::with(|cs| OUT.borrow(cs).get())
}

fn publish(o: StorageOutcome) {
    critical_section::with(|cs| OUT.borrow(cs).set(o));
    CHANGED.store(true, Ordering::Relaxed);
}

/// The outcome changed since the last call (the UI loop raises
/// `other::STORAGE`).
#[must_use]
pub fn take_changed() -> bool {
    CHANGED.swap(false, Ordering::Relaxed)
}

/// Ask the card task for `r` (not Rebuild library, which has its own
/// path). False if card work runs already or the queue is full.
pub fn request(r: StorageRequest) -> bool {
    if outcome().busy {
        return false;
    }
    if !crate::media::try_request(crate::media::Req::Care(r)) {
        return false;
    }
    critical_section::with(|cs| OUT.borrow(cs).set(StorageOutcome { req: Some(r), busy: true, ..IDLE }));
    true
}

/// Clear caches or Check card on the volume in `slot` (the card task,
/// after the hand-over). The card task calls this and [`format`] from
/// separate arms (`audio::release`): each future is polled in a frame of
/// its own, so the static stack's worst chain is the larger of them, not
/// their sum (`xtask stack`).
pub async fn run(slot: &mut Option<Volume<'static, Card>>, r: StorageRequest) {
    let mut o = StorageOutcome { req: Some(r), ok: false, ..IDLE };
    if let Some(v) = slot.as_mut() {
        match r {
            StorageRequest::ClearCaches => o.ok = Own(clear(v)).await,
            StorageRequest::CheckCard => Own(check(v, &mut o)).await,
            _ => {}
        }
    }
    finish(o);
}

/// The outcome, logged and published.
#[inline(never)]
fn finish(o: StorageOutcome) {
    log(&o);
    publish(o);
}

#[inline(never)]
async fn clear(v: &mut Volume<'static, Card>) -> bool {
    // In this future, not on the stack (the caches' paths are short:
    // `/.orpheus/cache/books/00abcdef.idx`).
    let mut path = [0u8; 128];
    let mut e = DirEntry::new();
    let mut ok = true;
    for d in CACHE_DIRS {
        ok &= v.remove_tree(d, &mut path, &mut e).await.is_ok();
    }
    ok & v.flush().await.is_ok()
}

#[inline(never)]
async fn check(v: &mut Volume<'static, Card>, o: &mut StorageOutcome) {
    let mut stack = [v.root(); CHECK_DEPTH];
    let mut e = DirEntry::new();
    // Any request for the card task stops it (the result says so).
    if let Ok(r) = v.check(&mut stack, &mut e, &mut |_| crate::audio::work_waiting()).await {
        (o.ok, o.dirs, o.files, o.problems, o.stopped) = (true, r.dirs, r.files, r.problems, r.stopped);
    }
}

/// Format card: the volume in `slot` flushed, the card formatted through
/// it, then the old volume dropped without a flush (its tables are the
/// old file system's) and the card mounted into `slot` again (`None`
/// there if that failed). `arena` (the idle arena) is the zero-fill
/// buffer. No `Card` is held in this future (608 B of the card task's
/// `.bss`): the card stays inside the volume until the mount takes it.
pub async fn format(slot: &mut Option<Volume<'static, Card>>, arena: &mut [u8]) {
    let mut ok = false;
    if let Some(v) = slot.as_mut() {
        crate::state::CARD_READY.store(false, Ordering::Relaxed);
        ok = v.flush().await.is_ok() && Own(format_card(v.device(), arena)).await;
    }
    if let Some(v) = slot.take() {
        *slot = Own(crate::audio::mount(v.into_device())).await;
        ok &= slot.is_some();
        crate::state::CARD_READY.store(slot.is_some(), Ordering::Relaxed);
    }
    finish(StorageOutcome { req: Some(StorageRequest::Format), ok, ..IDLE });
}

#[inline(never)]
async fn format_card(card: &mut Card, arena: &mut [u8]) -> bool {
    // Zero-fill in writes of up to 64 sectors (32 KiB of the arena).
    let (secs, _) = arena.as_chunks_mut::<512>();
    let n = secs.len().min(64);
    let serial = atj2157_hal::rtc_clock::epoch2000(atj2157_hal::rtc_clock::now());
    orpheus_fs::format::format(card, None, serial, &mut secs[..n]).await.is_ok()
}

fn log(o: &StorageOutcome) {
    let mut l = atj2157_hal::log::LineBuf::new();
    l.push_str("care: ")
        .push_str(match o.req {
            Some(StorageRequest::ClearCaches) => "clear",
            Some(StorageRequest::CheckCard) => "check",
            Some(StorageRequest::Format) => "format",
            _ => "?",
        })
        .push_str(if o.ok { " ok" } else { " failed" })
        .push_str(" dirs=")
        .push_u32(o.dirs)
        .push_str(" files=")
        .push_u32(o.files)
        .push_str(" problems=")
        .push_u32(o.problems)
        .push_str(if o.stopped { " stopped" } else { "" });
    atj2157_hal::log::line(l.as_bytes());
}
