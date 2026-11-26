//! The synth's rests for thread mode (SYN-3, 2026-11-26): a wake-up at a
//! time for futures that must not run `embassy_time`'s code, and the early
//! end of a rest when thread mode has nothing left to do.
//!
//! `embassy_time::Timer`'s poll and `_embassy_time_now` are paged code
//! wherever the linker puts them; the rest's future, polled every few
//! milliseconds on a path whose code must fit the code cache, calls
//! [`rest_until`] instead, from its own poll. Not in [`crate::time`], whose
//! code is resident by name: this is thread-side code and pages like its
//! callers.
//!
//! **Early end** (`orpheus_power::pace::rest_over`). A rest exists so that
//! thread mode runs while the real-time executor renders; when thread mode
//! has run everything it had and goes to sleep, the rest of the rest is the
//! CPU idling while the PCM ring is short (2026-11-26, the player: 20
//! underruns in 107 s at a render load of 25–40 %, the PC profile 20 %
//! asleep in thread mode). The resting task leaves its waker here; the main
//! executor, about to sleep (WFE) during a rest, marks itself asleep and
//! has the resting task woken [`orpheus_power::pace::IDLE_PROBE_US`] later
//! ([`thread_sleeps`]); if thread mode is still asleep then, the rest ends.
//! A wake-up of thread mode in between (a task woken, an interrupt) clears
//! the mark first ([`thread_woke`]), so a rest never ends while thread mode
//! has work, and never on its first look (the mark is cleared as the rest
//! starts: thread mode must have run and gone back to sleep).

use core::sync::atomic::Ordering::Relaxed;
use core::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32};
use core::task::{RawWakerVTable, Waker};

use orpheus_power::pace;

/// The resting task's waker (data, vtable; null: no rest) and the rest's
/// end (low 32 bits of µs: a stale entry stops being probed then).
static REST_TASK: AtomicPtr<()> = AtomicPtr::new(core::ptr::null_mut());
static REST_VT: AtomicPtr<RawWakerVTable> = AtomicPtr::new(core::ptr::null_mut());
static REST_END: AtomicU32 = AtomicU32::new(0);
/// Thread mode is asleep (set right before its WFE, cleared after).
static ASLEEP: AtomicBool = AtomicBool::new(false);
/// Rests ended, of them ended early (thread mode asleep), and the latest a
/// rest's task resumed after its time, µs (the real-time executor's wake-up
/// latency: masked stretches, higher levels, line loads on the way).
static RESTS: AtomicU32 = AtomicU32::new(0);
static CUT: AtomicU32 = AtomicU32::new(0);
static LATE_MAX: AtomicU32 = AtomicU32::new(0);

/// The rest's poll: true when the rest until `at_us` (µs since
/// `time::init`, [`crate::time::now_us`]) is over — its time is up, or
/// thread mode went to sleep after the rest began. Else the waker is
/// woken at `at_us` (as `embassy_time::Timer` does when polled before its
/// deadline) and earlier by thread mode's probe.
#[inline(never)]
pub fn rest_until(at_us: u64, waker: &Waker) -> bool {
    let me = waker.data().cast_mut();
    let resting = REST_TASK.load(Relaxed) == me;
    let now = crate::time::now_us();
    if pace::rest_over(now, at_us, resting, ASLEEP.load(Relaxed)) {
        REST_TASK.store(core::ptr::null_mut(), Relaxed);
        RESTS.store(RESTS.load(Relaxed).wrapping_add(1), Relaxed);
        match now.checked_sub(at_us) {
            Some(late) => {
                LATE_MAX.fetch_max(late.min(u64::from(u32::MAX)) as u32, Relaxed);
            }
            None => CUT.store(CUT.load(Relaxed).wrapping_add(1), Relaxed),
        }
        return true;
    }
    if !resting {
        // The rest begins: thread mode must run and go to sleep again
        // before it can end early.
        ASLEEP.store(false, Relaxed);
        REST_END.store(at_us as u32, Relaxed);
        REST_VT.store(core::ptr::from_ref(waker.vtable()).cast_mut(), Relaxed);
        REST_TASK.store(me, Relaxed);
    }
    crate::time::schedule_wake(at_us, waker);
    false
}

/// The main executor is about to sleep: mark it asleep and, during a rest,
/// have the resting task look again [`pace::IDLE_PROBE_US`] from now.
/// Inlined into the main executor's loop: on the code-cache lines every
/// thread-mode slot runs anyway, not on a line of its own that the synth's
/// per-piece code would load again after each rest.
#[inline(always)]
pub fn thread_sleeps() {
    ASLEEP.store(true, Relaxed);
    let task = REST_TASK.load(Relaxed);
    let vt = REST_VT.load(Relaxed);
    if task.is_null() || vt.is_null() {
        return;
    }
    let now = crate::time::now_us();
    if !pace::probe_due(now as u32, REST_END.load(Relaxed)) {
        return;
    }
    // SAFETY: `task` and `vt` are the data and vtable of a waker that
    // `rest_until` was given: an executor task's (static, never freed),
    // whose clone and drop do nothing. The copy is never dropped.
    let w = core::mem::ManuallyDrop::new(unsafe { Waker::new(task, &*vt) });
    crate::time::schedule_wake(now + u64::from(pace::IDLE_PROBE_US), &w);
}

/// The main executor woke up.
#[inline(always)]
pub fn thread_woke() {
    ASLEEP.store(false, Relaxed);
}

/// Rests ended so far, and of them ended early because thread mode was
/// asleep.
#[must_use]
pub fn rests() -> (u32, u32) {
    (RESTS.load(Relaxed), CUT.load(Relaxed))
}

/// The latest a rest's task resumed after its time since the last call, µs.
#[must_use]
pub fn take_late_max() -> u32 {
    LATE_MAX.swap(0, Relaxed)
}
