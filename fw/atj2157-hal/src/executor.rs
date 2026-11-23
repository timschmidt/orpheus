//! The two executors (docs/plan/02-architecture.md §4).
//!
//! * **Main** (thread mode): UI and services. Sleeps with WFE between polls
//!   and accounts the time spent asleep (power statistics).
//! * **RT** (interrupt mode): audio and data pumps, on a software-pended NVIC
//!   line with no hardware source, at level 2 so deadline interrupts
//!   (level 1) still preempt it and it preempts the main executor.

use core::sync::atomic::{AtomicU32, Ordering};

use embassy_executor::{InterruptExecutor, SendSpawner, Spawner, raw};

use crate::interrupt::{self, Irq, Level};

/// NVIC line for the RT executor: no source in the datasheet table (U-IRQ-2).
pub const RT_IRQ: u16 = 9;

static RT: InterruptExecutor = InterruptExecutor::new();

fn on_rt() {
    // SAFETY: called only from the RT line's handler, after `start_rt`.
    unsafe { RT.on_interrupt() };
}

/// Start the RT executor; returns its spawner.
pub fn start_rt() -> SendSpawner {
    interrupt::unpend(RT_IRQ);
    crate::rt::bind(RT_IRQ, on_rt);
    interrupt::set_level(RT_IRQ, Level::Rt);
    RT.start(Irq(RT_IRQ))
}

/// Microseconds the main executor spent in WFE (statistics).
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
        loop {
            // SAFETY: polled only from this thread-mode loop.
            unsafe { self.inner.poll() };
            POLLS.fetch_add(1, Ordering::Relaxed);
            crate::wdt::feed_if_allowed();
            let t0 = crate::time::now_us();
            cortex_m::asm::wfe();
            let dt = crate::time::now_us().wrapping_sub(t0);
            IDLE_US.fetch_add(dt as u32, Ordering::Relaxed);
        }
    }
}
