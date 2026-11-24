//! `audio` — WP-AU-0 bring-up image.
//!
//! Phases (1.5 s each, 1 kHz sine at −12 dBFS):
//!   0 left only, 44.1 kHz, chained single transfers (SRAM0 ring)
//!   1 right only, 44.1 kHz, chained
//!   2 both, 48 kHz, DMA reload mode with half/complete interrupts
//!   3 both, 44.1 kHz, reload mode, ring in SRAM3 (U-DMA-1)
//!   4 both, volume steps down then up (user check: even steps)
//! then mute and stop.
//!
//! Counters per phase p (base 4·p): measured rate in Hz·10, IRQs,
//! max IRQ gap µs, half-transfer IRQs seen. Counter 20: DAC regs[2] (PA_CTL).

#![no_std]
#![no_main]
// `&*(&raw const STATIC)` is the 2024-edition way to borrow a `static mut`.
#![allow(clippy::deref_addrof)]

use core::sync::atomic::{AtomicU32, Ordering};

use atj2157_hal::dma::{self, Channel};
use atj2157_hal::interrupt::Level;
use atj2157_hal::{self as hal, dac, executor, log, pac, persist, time, wdt};
use embassy_executor::Spawner;
use embassy_time::Timer;
use orpheus_proto::result::Verdict;
use static_cell::StaticCell;

const CH: usize = 5;
const HALF_FRAMES: usize = 1024;
const RING_WORDS: usize = HALF_FRAMES * 2; // one u32 = one stereo frame (L, R s16)

#[repr(C, align(4))]
struct Ring([u32; RING_WORDS]);
static mut RING0: Ring = Ring([0; RING_WORDS]);
/// SRAM3 ring for phase 3 (PCM ring location of the plan, 0x304000).
const RING3_ADDR: usize = 0x0030_4000;

static RING_ADDR: AtomicU32 = AtomicU32::new(0);
static MODE: AtomicU32 = AtomicU32::new(0); // 0 chained, 1 reload
static NEXT_HALF: AtomicU32 = AtomicU32::new(0);
static IRQS: AtomicU32 = AtomicU32::new(0);
static HF_IRQS: AtomicU32 = AtomicU32::new(0);
static FRAMES: AtomicU32 = AtomicU32::new(0);
static LAST_US: AtomicU32 = AtomicU32::new(0);
static MAX_GAP: AtomicU32 = AtomicU32::new(0);
static FIRST_US: AtomicU32 = AtomicU32::new(0);
static FIRST_FRAMES: AtomicU32 = AtomicU32::new(0);
static PHASE_INC: AtomicU32 = AtomicU32::new(0);
static PHASE: AtomicU32 = AtomicU32::new(0);
static CHANNELS: AtomicU32 = AtomicU32::new(3); // bit 0 left, bit 1 right

static mut SINE: [i16; 256] = [0; 256];

fn make_sine() {
    // Recurrence s[n] = 2cos(w)s[n-1] − s[n-2], w = 2π/256, amplitude 0.25 FS.
    let c = 2.0f32 * 0.999_698_8_f32;
    let a = 8191.0f32;
    let (mut s1, mut s2) = (a * 0.024_541_229_f32, 0.0f32);
    // SAFETY: written once before any interrupt reads it.
    let t = unsafe { &mut *(&raw mut SINE) };
    t[0] = 0;
    t[1] = s1 as i16;
    for x in t.iter_mut().skip(2) {
        let s = c * s1 - s2;
        s2 = s1;
        s1 = s;
        *x = s as i16;
    }
}

fn fill(half: usize) {
    let base = RING_ADDR.load(Ordering::Relaxed) as *mut u32;
    let inc = PHASE_INC.load(Ordering::Relaxed);
    let mut ph = PHASE.load(Ordering::Relaxed);
    let ch = CHANNELS.load(Ordering::Relaxed);
    // SAFETY: SINE is read-only after start-up.
    let sine = unsafe { &*(&raw const SINE) };
    for i in 0..HALF_FRAMES {
        let s = sine[(ph >> 24) as usize] as u16;
        ph = ph.wrapping_add(inc);
        let l = if ch & 1 != 0 { s } else { 0 };
        let r = if ch & 2 != 0 { s } else { 0 };
        // SAFETY: inside the ring (RING_WORDS words at `base`).
        unsafe {
            core::ptr::write_volatile(base.add(half * HALF_FRAMES + i), u32::from(l) | u32::from(r) << 16)
        };
    }
    PHASE.store(ph, Ordering::Relaxed);
}

fn start_half(half: usize) {
    let base = RING_ADDR.load(Ordering::Relaxed);
    // SAFETY: channel 5 is owned by this image.
    let ch = unsafe { Channel::steal(CH) };
    ch.start(
        dac::DMA_MODE_S16,
        base + (half * HALF_FRAMES * 4) as u32,
        dac::FIFO0_ADDR,
        (HALF_FRAMES * 4) as u32,
    );
}

fn on_dma() {
    let what = dma::clear(CH);
    let now = time::now_us() as u32;
    let last = LAST_US.swap(now, Ordering::Relaxed);
    if last != 0 {
        MAX_GAP.fetch_max(now.wrapping_sub(last), Ordering::Relaxed);
    } else {
        FIRST_US.store(now, Ordering::Relaxed);
    }
    let first = IRQS.fetch_add(1, Ordering::Relaxed) == 0;
    let _ = first;
    if MODE.load(Ordering::Relaxed) == 0 {
        if what & 1 != 0 {
            // Chained: start the other half at once, then refill this one.
            let h = NEXT_HALF.load(Ordering::Relaxed) as usize;
            start_half(h);
            NEXT_HALF.store((h ^ 1) as u32, Ordering::Relaxed);
            FRAMES.fetch_add(HALF_FRAMES as u32, Ordering::Relaxed);
            fill(h ^ 1);
        }
    } else {
        if what & 2 != 0 {
            HF_IRQS.fetch_add(1, Ordering::Relaxed);
            FRAMES.fetch_add(HALF_FRAMES as u32, Ordering::Relaxed);
            fill(0);
        }
        if what & 1 != 0 {
            FRAMES.fetch_add(HALF_FRAMES as u32, Ordering::Relaxed);
            fill(1);
        }
    }
    if first {
        FIRST_FRAMES.store(FRAMES.load(Ordering::Relaxed), Ordering::Relaxed);
    }
}

fn reset_stats() {
    IRQS.store(0, Ordering::Relaxed);
    HF_IRQS.store(0, Ordering::Relaxed);
    FRAMES.store(0, Ordering::Relaxed);
    LAST_US.store(0, Ordering::Relaxed);
    MAX_GAP.store(0, Ordering::Relaxed);
}

/// Run one phase and return (rate·10, irqs, max gap, hf irqs).
async fn phase(rate: u32, reload: bool, ring: u32, channels: u32, ms: u64) -> [u32; 4] {
    // SAFETY: channel 5 is owned by this image.
    let ch = unsafe { Channel::steal(CH) };
    ch.abort();
    dma::clear(CH);
    dac::set_rate(rate);
    RING_ADDR.store(ring, Ordering::Relaxed);
    CHANNELS.store(channels, Ordering::Relaxed);
    PHASE_INC.store(((1000u64 << 32) / u64::from(rate)) as u32, Ordering::Relaxed);
    fill(0);
    fill(1);
    reset_stats();
    MODE.store(u32::from(reload), Ordering::Relaxed);
    let t0 = time::now_us();
    if reload {
        ch.start(dac::DMA_MODE_S16 | dma::mode::RELOAD, ring, dac::FIFO0_ADDR, (RING_WORDS * 4) as u32);
    } else {
        NEXT_HALF.store(1, Ordering::Relaxed);
        start_half(0);
    }
    Timer::after_millis(ms).await;
    let _ = t0;
    let frames = FRAMES.load(Ordering::Relaxed).wrapping_sub(FIRST_FRAMES.load(Ordering::Relaxed));
    let us = LAST_US.load(Ordering::Relaxed).wrapping_sub(FIRST_US.load(Ordering::Relaxed));
    ch.abort();
    MODE.store(0, Ordering::Relaxed);
    [
        (u64::from(frames) * 10_000_000 / u64::from(us.max(1))) as u32,
        IRQS.load(Ordering::Relaxed),
        MAX_GAP.load(Ordering::Relaxed),
        HF_IRQS.load(Ordering::Relaxed),
    ]
}

#[embassy_executor::task]
async fn run(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    make_sine();
    let vol = dac::VOL_0DB - 32; // −12 dB
    dac::power_up(44100, vol);
    r.set(20, dac::regs()[2]);
    // SAFETY: channel 5 is the DAC channel (01-hardware §6).
    let ch = unsafe { Channel::steal(CH) };
    ch.enable_irq_with(on_dma, Level::Deadline);
    // SAFETY: the ring is only touched by `fill` (this task and the DMA ISR
    // never run `fill` on the same half at once).
    let ring0 = (&raw mut RING0) as u32;

    let phases: [(u32, bool, u32, u32, u64); 4] = [
        (44100, false, ring0, 1, 1500),
        (44100, false, ring0, 2, 1500),
        (48000, true, ring0, 3, 1500),
        (44100, true, RING3_ADDR as u32, 3, 1500),
    ];
    let mut ok = true;
    for (i, &(rate, reload, ring, chans, ms)) in phases.iter().enumerate() {
        let s = phase(rate, reload, ring, chans, ms).await;
        for (j, v) in s.iter().enumerate() {
            r.set(4 * i + j, *v);
        }
        // Measured rate within 0.5 % of nominal.
        let want = rate * 10;
        ok &= s[0] > want - want / 200 && s[0] < want + want / 200;
    }
    // Phase 4: volume steps (user check), reload mode at 44.1 kHz.
    // SAFETY: as above.
    let ch = unsafe { Channel::steal(CH) };
    dac::set_rate(44100);
    RING_ADDR.store(ring0, Ordering::Relaxed);
    CHANNELS.store(3, Ordering::Relaxed);
    fill(0);
    fill(1);
    MODE.store(1, Ordering::Relaxed);
    ch.start(dac::DMA_MODE_S16 | dma::mode::RELOAD, ring0, dac::FIFO0_ADDR, (RING_WORDS * 4) as u32);
    for k in (0..8u8).chain((0..8u8).rev()) {
        let v = vol - 8 * k;
        dac::set_volume(v, v);
        Timer::after_millis(150).await;
    }
    dac::set_volume(0, 0);
    Timer::after_millis(100).await;
    ch.abort();
    r.set(16, dac::regs()[0]);
    r.set(17, dac::regs()[1]);
    let mut msg = log::LineBuf::new();
    msg.push_str("44k1=")
        .push_u32(r.get(0))
        .push_str(" 48k=")
        .push_u32(r.get(8))
        .push_str(" sram3=")
        .push_u32(r.get(12));
    r.flush(if ok { Verdict::Pass } else { Verdict::Fail }, msg.as_bytes());
    wdt::reboot(wdt::Target::Adfu);
}

static MAIN: StaticCell<executor::MainExecutor> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    log::init();
    hal::init();
    wdt::start(pac::rtc::wd::PERIOD_5S6);
    wdt::feed_from_idle(true);
    let test_id = persist::test_request().map_or(0, |(id, _)| id);
    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(run(test_id).unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
