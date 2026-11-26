//! Timers and the Embassy time driver (docs/plan/03-hal-drivers.md §4).
//!
//! * **T0** — time base: free-running, reload `0xFFFF_FFFF`, clocked at
//!   1 MHz (CK24M/24, `CMU_TIMER0CLK = 1`). It counts down; its zero
//!   interrupt extends the count to 64 bits.
//! * **T1** — alarm: one-shot, programmed with the distance to the next
//!   deadline, capped at 2^31 µs and chained.
//!
//! Tick rate 1 MHz (`embassy-time-driver/tick-hz-1_000_000`).

use core::cell::{Cell, RefCell};
use core::task::Waker;

use atj2157_pac::{cmu, irq, rmu, rtc};
use critical_section::{CriticalSection, Mutex};
use embassy_time_driver::Driver;
use embassy_time_queue_utils::Queue;

use crate::interrupt::{self, Level};

const T_BASE: usize = 0;
const T_ALARM: usize = 1;

struct TimeDriver {
    /// Number of T0 wraps (each 2^32 ticks).
    periods: Mutex<Cell<u32>>,
    queue: Mutex<RefCell<Queue>>,
    /// Statistics: alarm interrupts taken.
    alarms: Mutex<Cell<u32>>,
}

embassy_time_driver::time_driver_impl!(static DRIVER: TimeDriver = TimeDriver {
    periods: Mutex::new(Cell::new(0)),
    queue: Mutex::new(RefCell::new(Queue::new())),
    alarms: Mutex::new(Cell::new(0)),
});

/// Bring the timer block up and start T0. Call once, before the executors.
pub fn init() {
    // Timer block start-up as the port does it [HW] (03 §4).
    rmu::RST0.clear_bits(rmu::rst0::TIMER);
    crate::delay::spin_us(5);
    rmu::RST0.set_bits(rmu::rst0::TIMER);
    cmu::CLKEN0.set_bits(cmu::clken0::TIMER);
    // The port's `0xC0000104 |= 1`: LOSC (32 kHz crystal) enable per the
    // datasheet (HAL-0 correction), kept as verified.
    cmu::LOSC_CTL.set_bits(1);
    rtc::CTL.set_bits(rtc::ctl::RTC32K_DIV_EN);
    rtc_update_handshake();

    for t in [T_BASE, T_ALARM] {
        rtc::t_ctl(t).write(rtc::tctl::ZIPD);
        cmu::timer_clk(t).write(cmu::TIMERCLK_1M);
    }
    rtc::t_val(T_BASE).write(u32::MAX);
    rtc::t_ctl(T_BASE).write(rtc::tctl::EN | rtc::tctl::RELO | rtc::tctl::ZIEN);
    rtc::t_val(T_ALARM).write(u32::MAX);

    interrupt::install(irq::TIMER0, Level::Deadline, on_t0);
    interrupt::install(irq::TIMER1, Level::Deadline, on_t1);
}

/// Write the SVCC update key and wait for the acknowledge (bounded).
pub fn rtc_update_handshake() -> bool {
    rtc::REGUPDATA.write(rtc::UPDATE_KEY);
    for _ in 0..100_000 {
        if rtc::REGUPDATA.read() == rtc::UPDATE_DONE {
            return true;
        }
    }
    false
}

fn on_t0() {
    critical_section::with(|cs| {
        let ctl = rtc::t_ctl(T_BASE);
        if ctl.read() & rtc::tctl::ZIPD != 0 {
            ctl.write(ctl.read()); // write-1-to-clear the pending bit
            let p = DRIVER.periods.borrow(cs);
            p.set(p.get().wrapping_add(1));
        }
    });
}

fn on_t1() {
    critical_section::with(|cs| {
        // One-shot: stop it and clear the pending bit.
        rtc::t_ctl(T_ALARM).write(rtc::tctl::ZIPD);
        let a = DRIVER.alarms.borrow(cs);
        a.set(a.get().wrapping_add(1));
        DRIVER.rearm(cs);
    });
}

impl TimeDriver {
    fn now_cs(&self, cs: CriticalSection) -> u64 {
        let mut periods = self.periods.borrow(cs).get();
        let cnt = rtc::t_cnt(T_BASE).read();
        let pending = rtc::t_ctl(T_BASE).read() & rtc::tctl::ZIPD != 0;
        let elapsed = u32::MAX - cnt;
        // A wrap that the interrupt has not counted yet: count it if the
        // counter was read after the wrap (small elapsed value).
        if pending && elapsed < 0x8000_0000 {
            periods = periods.wrapping_add(1);
        }
        (u64::from(periods) << 32) | u64::from(elapsed)
    }

    /// Program T1 for the queue's next deadline; loop while deadlines have
    /// already passed.
    fn rearm(&self, cs: CriticalSection) {
        let mut q = self.queue.borrow(cs).borrow_mut();
        loop {
            let now = self.now_cs(cs);
            let next = q.next_expiration(now);
            if next == u64::MAX {
                rtc::t_ctl(T_ALARM).write(rtc::tctl::ZIPD);
                return;
            }
            if next <= now {
                continue;
            }
            let delta = (next - now).min(0x8000_0000) as u32;
            rtc::t_ctl(T_ALARM).write(rtc::tctl::ZIPD);
            rtc::t_val(T_ALARM).write(delta.saturating_sub(1));
            rtc::t_ctl(T_ALARM).write(rtc::tctl::EN | rtc::tctl::ZIEN);
            // If the deadline passed while programming, go round again.
            if self.now_cs(cs) < next {
                return;
            }
        }
    }
}

impl Driver for TimeDriver {
    fn now(&self) -> u64 {
        critical_section::with(|cs| self.now_cs(cs))
    }

    // Inlined into its callers (`embassy_time::Timer`'s poll,
    // `wake::wake_at`): out of line it would be resident by its name.
    #[inline(always)]
    fn schedule_wake(&self, at: u64, waker: &Waker) {
        critical_section::with(|cs| {
            let changed = self.queue.borrow(cs).borrow_mut().schedule_wake(at, waker);
            if changed {
                self.rearm(cs);
            }
        });
    }
}

/// Microseconds since `init` (64 bit).
#[must_use]
pub fn now_us() -> u64 {
    DRIVER.now()
}

/// [`Driver::schedule_wake`] for [`crate::wake::wake_at`] (inlined there:
/// an out-of-line copy here would be resident, `atj2157_hal::time::*`).
#[inline(always)]
pub(crate) fn schedule_wake(at_us: u64, waker: &Waker) {
    DRIVER.schedule_wake(at_us, waker);
}

/// Alarm interrupts taken so far.
#[must_use]
pub fn alarm_count() -> u32 {
    critical_section::with(|cs| DRIVER.alarms.borrow(cs).get())
}

/// T0 wraps counted so far.
#[must_use]
pub fn period_count() -> u32 {
    critical_section::with(|cs| DRIVER.periods.borrow(cs).get())
}

/// Low 32 bits of the µs counter without a critical section (for the
/// pager, which runs above every masked level). Wraps every 71 minutes.
#[inline(always)]
#[must_use]
pub fn now_us_raw() -> u32 {
    u32::MAX - rtc::t_cnt(T_BASE).read()
}
