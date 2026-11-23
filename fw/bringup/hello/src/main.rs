//! `hello` — WP-HAL-1 bring-up image.
//!
//! Proves: start-up code, BASEPRI critical section, T0/T1 time driver,
//! both executors (RT preempting main), cross-executor signalling, watchdog
//! feeding, GPIO output (backlight blink), result block, reboot to ADFU.
//!
//! Counters (result block):
//!  0 T0 µs over the measurement window       7 alarm interrupts
//!  1 RTC DHMS at start                        8 main executor idle µs
//!  2 RTC DHMS at end                          9 main executor polls
//!  3 RT ticker iterations (1 kHz)            10 stack bytes never used
//!  4 max RT lateness µs                      11 stack size
//!  5 mean RT lateness µs                     12 cross-executor signals
//!  6 backlight toggles                       13 T0 wraps

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use atj2157_hal::{self as hal, executor, gpio, log, pac, persist, time, wdt};
use embassy_executor::Spawner;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Ticker, Timer};
use static_cell::StaticCell;
use yoton_proto::result::Verdict;

const WINDOW_MS: u64 = 10_000;
const BACKLIGHT_PIN: u8 = 22;

static STOP: AtomicBool = AtomicBool::new(false);
static RT_ITERS: AtomicU32 = AtomicU32::new(0);
static RT_MAX_LATE: AtomicU32 = AtomicU32::new(0);
static RT_SUM_LATE: AtomicU32 = AtomicU32::new(0);
static BLINKS: AtomicU32 = AtomicU32::new(0);
static SIGNALS: AtomicU32 = AtomicU32::new(0);
static TICK_SIGNAL: Signal<CriticalSectionRawMutex, u32> = Signal::new();

#[embassy_executor::task]
async fn rt_ticker() {
    let mut ticker = Ticker::every(Duration::from_millis(1));
    let mut expected = Instant::now();
    while !STOP.load(Ordering::Relaxed) {
        expected += Duration::from_millis(1);
        ticker.next().await;
        let late = Instant::now().saturating_duration_since(expected).as_micros() as u32;
        RT_MAX_LATE.fetch_max(late, Ordering::Relaxed);
        RT_SUM_LATE.fetch_add(late, Ordering::Relaxed);
        let n = RT_ITERS.fetch_add(1, Ordering::Relaxed) + 1;
        if n.is_multiple_of(1000) {
            TICK_SIGNAL.signal(n);
        }
    }
}

#[embassy_executor::task]
async fn listener() {
    while !STOP.load(Ordering::Relaxed) {
        let _ = TICK_SIGNAL.wait().await;
        SIGNALS.fetch_add(1, Ordering::Relaxed);
    }
}

#[embassy_executor::task]
async fn blink() {
    let bl = gpio::Output::new(BACKLIGHT_PIN, true);
    while !STOP.load(Ordering::Relaxed) {
        Timer::after_millis(250).await;
        bl.toggle();
        BLINKS.fetch_add(1, Ordering::Relaxed);
    }
    bl.set(true);
}

#[embassy_executor::task]
async fn supervisor(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    if test_id == yoton_proto::hash::fnv1a(b"fault") {
        // Deliberate UsageFault: the HIL harness checks that a decoded
        // fault record comes back.
        log::line(b"hello: deliberate fault");
        Timer::after_millis(100).await;
        // SAFETY: `udf` raises a UsageFault by design; the fault handler
        // records it and reboots.
        unsafe { core::arch::asm!("udf #0x42") };
    }
    let rtc0 = pac::rtc::DHMS.read();
    let t0 = time::now_us();
    log::line(b"hello: measuring");
    Timer::after_millis(WINDOW_MS).await;
    let t1 = time::now_us();
    let rtc1 = pac::rtc::DHMS.read();
    STOP.store(true, Ordering::Relaxed);
    Timer::after_millis(20).await;

    let iters = RT_ITERS.load(Ordering::Relaxed);
    let max_late = RT_MAX_LATE.load(Ordering::Relaxed);
    let mean_late = RT_SUM_LATE.load(Ordering::Relaxed) / iters.max(1);
    let window_us = (t1 - t0) as u32;
    r.set(0, window_us);
    r.set(1, rtc0);
    r.set(2, rtc1);
    r.set(3, iters);
    r.set(4, max_late);
    r.set(5, mean_late);
    r.set(6, BLINKS.load(Ordering::Relaxed));
    r.set(7, time::alarm_count());
    r.set(8, executor::idle_us());
    r.set(9, executor::polls());
    r.set(10, hal::rt::stack_unused() as u32);
    r.set(11, hal::rt::stack_size() as u32);
    r.set(12, SIGNALS.load(Ordering::Relaxed));
    r.set(13, time::period_count());

    // Plausibility: the ticker ran at ~1 kHz for the window, wake-ups are
    // prompt, signals crossed executors, the main executor slept.
    let expect = (WINDOW_MS as u32) - 50;
    let ok = iters >= expect
        && max_late < 2_000
        && SIGNALS.load(Ordering::Relaxed) >= 5
        && BLINKS.load(Ordering::Relaxed) >= 30
        && executor::idle_us() > window_us / 2;
    let mut msg = log::LineBuf::new();
    msg.push_str("window_us=")
        .push_u32(window_us)
        .push_str(" iters=")
        .push_u32(iters)
        .push_str(" max_late=")
        .push_u32(max_late);
    msg.emit();
    r.flush(if ok { Verdict::Pass } else { Verdict::Fail }, msg.as_bytes());
    log::line(b"hello: done, rebooting");
    wdt::reboot(wdt::Target::Adfu);
}

static MAIN: StaticCell<executor::MainExecutor> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    log::init();
    log::line(b"hello: start");
    hal::init();
    wdt::start(pac::rtc::wd::PERIOD_5S6);
    wdt::feed_from_idle(true);

    let test_id = persist::test_request().map_or(0, |(id, _)| id);

    let rt = executor::start_rt();
    rt.spawn(rt_ticker().unwrap());

    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(listener().unwrap());
        s.spawn(blink().unwrap());
        s.spawn(supervisor(test_id).unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
