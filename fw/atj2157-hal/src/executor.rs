//! The two executors (docs/plan/02-architecture.md §4).
//!
//! * **Main** (thread mode): UI and services. Sleeps with WFE between polls
//!   and accounts the time spent asleep (power statistics); its sleep
//!   during a synth rest ends the rest early (`wake`).
//! * **RT** (interrupt mode): audio and data pumps, on a software-pended NVIC
//!   line with no hardware source, at level 2 so deadline interrupts
//!   (level 1) still preempt it and it preempts the main executor.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::{InterruptExecutor, SendSpawner, Spawner, raw};

use crate::interrupt::{self, Irq, Level};

/// NVIC line for the RT executor: no source in the datasheet table (U-IRQ-2).
pub const RT_IRQ: u16 = 9;

static RT: InterruptExecutor = InterruptExecutor::new();

/// Microseconds the RT executor ran (its interrupt handler): taken out of
/// the main executor's sleeps (AQ-0).
static RT_US: AtomicU32 = AtomicU32::new(0);

fn on_rt() {
    let t0 = crate::time::now_us_raw();
    // SAFETY: called only from the RT line's handler, after `start_rt`.
    unsafe { RT.on_interrupt() };
    // Only this handler writes it, and it does not nest.
    let dt = crate::time::now_us_raw().wrapping_sub(t0);
    RT_US.store(RT_US.load(Ordering::Relaxed).wrapping_add(dt), Ordering::Relaxed);
}

/// Start the RT executor; returns its spawner.
pub fn start_rt() -> SendSpawner {
    interrupt::unpend(RT_IRQ);
    crate::rt::bind(RT_IRQ, on_rt);
    interrupt::set_level(RT_IRQ, Level::Rt);
    RT.start(Irq(RT_IRQ))
}

/// Microseconds the main executor spent in WFE with nothing running (the
/// RT executor's time in it taken out, AQ-0): statistics and the
/// governor's idle share.
static IDLE_US: AtomicU32 = AtomicU32::new(0);
static POLLS: AtomicU32 = AtomicU32::new(0);

#[must_use]
pub fn idle_us() -> u32 {
    IDLE_US.load(Ordering::Relaxed)
}

#[must_use]
pub fn polls() -> u32 {
    POLLS.load(Ordering::Relaxed)
}

/// Thread-mode executor with idle accounting.
pub struct MainExecutor {
    inner: raw::Executor,
}

impl Default for MainExecutor {
    fn default() -> Self {
        Self::new()
    }
}

impl MainExecutor {
    #[must_use]
    pub fn new() -> Self {
        // Context `usize::MAX` makes the platform pender execute SEV.
        Self { inner: raw::Executor::new(usize::MAX as *mut ()) }
    }

    pub fn run(&'static mut self, init: impl FnOnce(Spawner)) -> ! {
        init(self.inner.spawner());
        main_loop(&self.inner)
    }
}

/// The main executor's loop: poll, feed the watchdog, sleep. A function of
/// its own, not generic and not inlined into [`MainExecutor::run`], so that
/// `hot.toml` can place it (`synth-block`: on the code-cache line of the
/// key scan's tick and the synth's wake-up entry). Inside `run` it sat on
/// physical lines 9–10, the render's last and the per-piece code's first:
/// every slot of thread mode while a MIDI file played loaded two lines and
/// the render loaded them again (SYN-3, 2026-11-26). Small to share that
/// line (1 KiB) with the tick and the wake-up's entry: the sleep timed on
/// the low 32 bits of the µs counter (no critical section; a sleep is far
/// shorter than its 71-minute wrap), the counters written only here (a
/// plain read-modify-write, not a compare-and-swap loop).
#[inline(never)]
fn main_loop(inner: &'static raw::Executor) -> ! {
    loop {
        // SAFETY: polled only from this thread-mode loop.
        unsafe { inner.poll() };
        POLLS.store(POLLS.load(Ordering::Relaxed).wrapping_add(1), Ordering::Relaxed);
        crate::wdt::feed_if_allowed();
        // During a synth rest, thread mode's sleep ends it early
        // (`wake`: the resting task looks again shortly).
        crate::wake::thread_sleeps();
        let t0 = crate::time::now_us_raw();
        let rt0 = RT_US.load(Ordering::Relaxed);
        // Not `cortex_m::asm::wfe` (`nomem`): the asleep mark's stores
        // before and after must stay on their side of the sleep.
        // SAFETY: WFE only waits for an event; no memory or stack effect.
        unsafe { core::arch::asm!("wfe", options(nostack, preserves_flags)) };
        crate::wake::thread_woke();
        let dt = crate::time::now_us_raw().wrapping_sub(t0);
        let rt = RT_US.load(Ordering::Relaxed).wrapping_sub(rt0);
        let idle = orpheus_power::governor::sleep_idle_us(dt, rt);
        IDLE_US.store(IDLE_US.load(Ordering::Relaxed).wrapping_add(idle), Ordering::Relaxed);
    }
}
