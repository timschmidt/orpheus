//! Profilers (WP-T-8, docs/plan/07-tooling-testing.md §5.6), off by
//! default: the **PC sampler** and the pager's **fault log**. Both write the
//! profiler block in SRAM3 (`orpheus_proto::prof`, 2 KiB at `0x307000`,
//! plain RAM outside the paging windows: writing it never faults); one
//! capture at a time.
//!
//! * PC sampler: timer **T2** (1 MHz, reload, the period given at the
//!   start; [DS] §4.5.5.7, the same normal-timer mode as T0/T1) raises its
//!   interrupt at **level 0** with its own vector entry ([`entry`]), so it
//!   also samples inside critical sections (BASEPRI masks levels 1–3 only)
//!   and inside the level-1 deadline handlers. It cannot preempt the pager
//!   (MemManage is level 0 too): a tick that falls into a line load is taken
//!   when the fault returns, and the stacked PC is then the faulting
//!   instruction, so time spent paging is charged to the code that faulted.
//!   A fault inside this handler would escalate to HardFault: the handler is
//!   resident (`check-resident`, a vector root) and touches only the stacked
//!   frame, T2 and the block.
//! * Fault log: [`log_fault`] is called by the MemManage handler
//!   (`pager::orpheus_pager_fault`) for every fault it serves while the log
//!   is on; with the log off the fault path pays one load and one branch
//!   ([`faults_on`]).

use core::sync::atomic::{AtomicBool, Ordering, compiler_fence};

use atj2157_pac::{cmu, irq, rtc};
use orpheus_proto::prof as p;

use crate::interrupt::{self, Level};

/// The sampler's timer.
const T_PROF: usize = 2;
/// Shortest and longest sampler period (µs).
pub const PERIOD_MIN_US: u32 = 100;
pub const PERIOD_MAX_US: u32 = 1_000_000;

static FAULTS_ON: AtomicBool = AtomicBool::new(false);
/// A PC capture was started in this boot and not stopped (the block's own
/// mode word may be left over from before a reset: SRAM3 is not cleared).
static PC_ON: AtomicBool = AtomicBool::new(false);

/// The block. Writers: the sampler interrupt while a PC capture runs, the
/// MemManage handler while the fault log is on, and the start/stop
/// functions below, which run only while neither writer can (the timer
/// stopped, the log flag clear).
#[inline(always)]
fn block() -> &'static mut p::Block {
    // SAFETY: `p::BASE` is 2 KiB of plain SRAM3 (02 §1.2, `mem::SRAM3_PROF`),
    // word aligned, used by nothing else; see above for who writes it when.
    unsafe { &mut *(p::BASE as *mut p::Block) }
}

/// Whether the fault log is on (the fault path's one branch).
#[inline(always)]
#[must_use]
pub fn faults_on() -> bool {
    FAULTS_ON.load(Ordering::Relaxed)
}

/// Record one fault the pager served: fault address, faulting PC, data
/// access (`MMARVALID`), line read from NOR (else re-mapped).
#[inline(never)]
#[unsafe(link_section = ".text.resident.prof_fault")]
pub fn log_fault(addr: u32, pc: u32, data: bool, loaded: bool) {
    let b = block();
    let since = crate::time::now_us_raw().wrapping_sub(b[p::W_START]);
    p::push(b, &p::encode_fault(addr, pc, loaded, data, since));
}

/// The sampler's vector entry (`rt::__vectors`, line `irq::TIMER2`): hands
/// the stacked frame to [`tick`].
///
/// # Safety
/// Only the vector table refers to it; it is entered by the hardware as
/// an exception handler, never called.
#[unsafe(naked)]
#[unsafe(link_section = ".text.resident.prof_entry")]
pub unsafe extern "C" fn entry() {
    core::arch::naked_asm!(
        "tst lr, #4",
        "ite eq",
        "mrseq r0, msp",
        "mrsne r0, psp",
        "b {t}",
        t = sym tick,
    )
}

#[unsafe(link_section = ".text.resident.prof_tick")]
unsafe extern "C" fn tick(frame: *const u32) {
    let ctl = rtc::t_ctl(T_PROF);
    ctl.write(ctl.read()); // write-1-to-clear the pending bit
    let b = block();
    if !p::running(b, p::KIND_PC) {
        ctl.write(rtc::tctl::ZIPD);
        return;
    }
    // SAFETY: the hardware-stacked frame of this exception (PC at word 6,
    // xPSR at word 7 in basic and extended frames).
    let (pc, xpsr) = unsafe { (frame.add(6).read_volatile(), frame.add(7).read_volatile()) };
    p::push(b, &[p::encode_pc(pc, xpsr & 0x1FF != 0)]);
    if b[p::W_MODE] & p::FULL != 0 {
        // A burst capture is complete: stop the timer.
        ctl.write(rtc::tctl::ZIPD);
    }
}

fn timer_off() {
    rtc::t_ctl(T_PROF).write(rtc::tctl::ZIPD);
    interrupt::disable(irq::TIMER2);
    interrupt::unpend(irq::TIMER2);
}

/// Whether a capture of either kind runs (started in this boot).
#[must_use]
pub fn busy() -> bool {
    faults_on() || PC_ON.load(Ordering::Relaxed)
}

fn magic_ok() -> bool {
    // SAFETY: an aligned word of the block.
    unsafe { core::ptr::read_volatile((p::BASE as usize + 4 * p::W_MAGIC) as *const u32) == p::MAGIC }
}

/// Start a PC capture with the given period (clamped to
/// `PERIOD_MIN_US..=PERIOD_MAX_US`); `burst` stops when the ring is full
/// instead of thinning. False if a capture runs already.
pub fn pc_start(period_us: u32, burst: bool) -> bool {
    if busy() {
        return false;
    }
    timer_off();
    let period = period_us.clamp(PERIOD_MIN_US, PERIOD_MAX_US);
    p::start(block(), p::KIND_PC, burst, period, crate::time::now_us_raw());
    // The header is written before the timer can tick (plain stores are
    // not ordered against the register writes otherwise).
    compiler_fence(Ordering::SeqCst);
    cmu::timer_clk(T_PROF).write(cmu::TIMERCLK_1M);
    rtc::t_val(T_PROF).write(period - 1);
    interrupt::set_level(irq::TIMER2, Level::Fault);
    interrupt::enable(irq::TIMER2);
    PC_ON.store(true, Ordering::Relaxed);
    rtc::t_ctl(T_PROF).write(rtc::tctl::EN | rtc::tctl::RELO | rtc::tctl::ZIEN);
    true
}

/// Start the fault log (`burst` as for [`pc_start`]). False if a capture
/// runs already.
pub fn faults_start(burst: bool) -> bool {
    if busy() {
        return false;
    }
    p::start(block(), p::KIND_FAULTS, burst, 0, crate::time::now_us_raw());
    FAULTS_ON.store(true, Ordering::Release);
    true
}

/// Stop whatever runs; the block keeps the capture for the host.
pub fn stop() {
    FAULTS_ON.store(false, Ordering::Release);
    timer_off();
    PC_ON.store(false, Ordering::Relaxed);
    // Nothing writes the block from here on (single core: a compiler fence
    // keeps the stores below after the ones above).
    compiler_fence(Ordering::SeqCst);
    if magic_ok() {
        p::stop(block(), crate::time::now_us_raw());
    }
}

/// The header words (for `prof status`); zeros when the block was never
/// written since power-up.
#[must_use]
pub fn header() -> [u32; p::HEADER] {
    let mut h = [0u32; p::HEADER];
    if magic_ok() {
        for (i, w) in h.iter_mut().enumerate() {
            // SAFETY: aligned words of the block.
            *w = unsafe { core::ptr::read_volatile((p::BASE as usize + 4 * i) as *const u32) };
        }
    }
    h
}
