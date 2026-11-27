//! Demand pager, device side (docs/plan/02-architecture.md §3). The policy
//! is `orpheus_pager::PagerCore` (host-tested); this module owns the MPU, the
//! MemManage entry and the NOR line reads. **Resident**, like the NOR
//! driver: nothing here may reach paged code.
//!
//! MemManage arrives at priority 0, above everything BASEPRI can mask, so a
//! fault inside a critical section or an interrupt handler is served too
//! (it stalls everything for one line read, ~175 µs).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

#[cfg(feature = "cold-code")]
use orpheus_pager::bounce::{self, FETCH_HALFWORDS, SLOT_HALFWORDS};
use orpheus_pager::{Action, PagerCore, RegionMap, mpu};

struct Shared(UnsafeCell<PagerCore>);
// SAFETY: touched only from the MemManage handler (priority 0, never
// preempted by anything that uses it) and from `init` before the MPU is on.
unsafe impl Sync for Shared {}

static CORE: Shared = Shared(UnsafeCell::new(PagerCore::new()));
/// NOR address of the page store (slot base + `pages_off`).
static STORE: AtomicU32 = AtomicU32::new(0);
pub static LOAD_ERRORS: AtomicU32 = AtomicU32::new(0);
/// Cycles of T0 (µs) spent loading lines, and the worst single load.
pub static LOAD_US: AtomicU32 = AtomicU32::new(0);
pub static LOAD_MAX_US: AtomicU32 = AtomicU32::new(0);
/// Set by [`arm_deny`]: region 7 is SRAM3's RAM half, not the stack guard
/// (`rt::enable_stack_guard` then uses the DWT and the canary).
static ARMED: AtomicBool = AtomicBool::new(false);

/// Nesting depths a faulting context can have: thread mode and interrupt
/// levels 3, 2, 1 (four levels, MemManage at 0 preempts no level-0
/// handler), one spare. Deeper is impossible; it would share the last slot.
#[cfg(feature = "cold-code")]
const DEPTHS: usize = 5;
/// Trampolines for bounced instructions (`orpheus_pager::bounce`), one per
/// exception nesting depth: a trampoline in use belongs to a context that
/// only something deeper can preempt. Plain SRAM0, executable by the
/// default memory map.
#[cfg(feature = "cold-code")]
#[repr(C, align(4))]
struct Trampolines(UnsafeCell<[[u16; SLOT_HALFWORDS]; DEPTHS]>);
// SAFETY: written only by the MemManage handler (priority 0), each slot
// only for the context at its depth, which is not running while written.
#[cfg(feature = "cold-code")]
unsafe impl Sync for Trampolines {}
// Not zeroed (`.uninit`): a slot is written before it is entered, and an
// image that never pages keeps it out of its loaded RAM (payloads).
#[cfg(feature = "cold-code")]
#[unsafe(link_section = ".uninit.pager_trampolines")]
static TRAMPOLINES: Trampolines = Trampolines(UnsafeCell::new([[0; SLOT_HALFWORDS]; DEPTHS]));
/// The bounced instruction's halfwords, read from the page store.
#[cfg(feature = "cold-code")]
#[repr(C, align(4))]
struct Fetch(UnsafeCell<[u16; FETCH_HALFWORDS]>);
// SAFETY: as `Trampolines` (MemManage only).
#[cfg(feature = "cold-code")]
unsafe impl Sync for Fetch {}
#[cfg(feature = "cold-code")]
#[unsafe(link_section = ".uninit.pager_fetch")]
static FETCH: Fetch = Fetch(UnsafeCell::new([0; FETCH_HALFWORDS]));

const MPU_CTRL: usize = 0xE000_ED94;
const MPU_RNR: usize = 0xE000_ED98;
const MPU_RBAR: usize = 0xE000_ED9C;
const MPU_RASR: usize = 0xE000_EDA0;
const SCB_CFSR: usize = 0xE000_ED28;
const SCB_MMFAR: usize = 0xE000_ED34;
#[cfg(feature = "cold-code")]
const SCB_SHCSR: usize = 0xE000_ED24;
#[cfg(feature = "cold-code")]
const NVIC_IABR0: usize = 0xE000_E300;
#[cfg(feature = "cold-code")]
const NVIC_IABR1: usize = 0xE000_E304;
/// `SHCSR` active bits of the exceptions other than MemManage that it can
/// preempt: BusFault, UsageFault, SVCall, DebugMonitor, PendSV, SysTick.
#[cfg(feature = "cold-code")]
const SHCSR_OTHER_ACTIVE: u32 = 0xD8A;

#[inline(always)]
fn w(a: usize, v: u32) {
    // SAFETY: core MPU/SCB registers.
    unsafe { core::ptr::write_volatile(a as *mut u32, v) }
}
#[inline(always)]
fn r(a: usize) -> u32 {
    // SAFETY: core SCB registers.
    unsafe { core::ptr::read_volatile(a as *const u32) }
}
#[inline(always)]
fn barrier() {
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

fn set_region(n: u8, rbar: u32, rasr: u32) {
    w(MPU_RNR, u32::from(n));
    w(MPU_RBAR, rbar);
    w(MPU_RASR, rasr);
}

fn map(m: RegionMap) {
    set_region(m.region, m.rbar(), m.rasr());
}

/// First thing after reset in a paged image: deny the paging windows so an
/// access to paged code or data before [`init`] faults (and is recorded)
/// instead of executing whatever SRAM2/3 hold. Never inlined: it is a
/// `check-resident` root (`resident.toml`).
#[inline(never)]
pub fn arm_deny() {
    w(MPU_CTRL, mpu::CTRL_OFF);
    barrier();
    // The windows denied, SRAM3's RAM half granted (PG-2: no longer a
    // hole in the deny region).
    for i in [usize::from(mpu::DENY_REGION), usize::from(mpu::RAM_REGION)] {
        if let Some(&(n, rbar, rasr)) = mpu::INITIAL.get(i) {
            set_region(n, rbar, rasr);
        }
    }
    ARMED.store(true, Ordering::Relaxed);
    w(MPU_CTRL, mpu::CTRL_ON);
    barrier();
}

/// Whether [`arm_deny`] ran (a paged image: MPU region 7 is taken).
#[must_use]
pub fn armed() -> bool {
    ARMED.load(Ordering::Relaxed)
}

/// Start paging: `store` is the NOR address of page 0, with `ncode` code
/// and `nro` rodata pages, the last `ro_code` of which hold cold code. Call
/// before anything paged runs (from `main`, which is resident).
pub fn init(store: u32, ncode: u32, nro: u32, ro_code: u32) {
    STORE.store(store, Ordering::Relaxed);
    ARMED.store(true, Ordering::Relaxed);
    // SAFETY: the MPU is about to be reprogrammed; no fault can use the
    // core concurrently (nothing paged has run yet).
    unsafe { (*CORE.0.get()).init_with(ncode, nro, ro_code) };
    w(MPU_CTRL, mpu::CTRL_OFF);
    barrier();
    for (n, rbar, rasr) in mpu::INITIAL {
        set_region(n, rbar, rasr);
    }
    w(MPU_CTRL, mpu::CTRL_ON);
    barrier();
}

/// Statistics of the core (faults, loads, remaps…).
#[must_use]
pub fn stats() -> orpheus_pager::Stats {
    // SAFETY: a plain read of counters; a torn read only skews statistics.
    unsafe { (*CORE.0.get()).stats() }
}

/// Exceptions active below this MemManage: the nesting depth of the
/// context that faulted (0 = thread mode).
#[cfg(feature = "cold-code")]
fn depth() -> usize {
    ((r(SCB_SHCSR) & SHCSR_OTHER_ACTIVE).count_ones()
        + r(NVIC_IABR0).count_ones()
        + r(NVIC_IABR1).count_ones()) as usize
}

/// Without the feature: no trampoline (a bounce is fatal, as before PG-2).
#[cfg(not(feature = "cold-code"))]
fn prepare_bounce(_core: &PagerCore, _pc: u32, _xpsr: u32) -> Option<(u32, u32)> {
    None
}

/// Build the trampoline for the instruction at `pc` (the stacked frame's)
/// in this depth's slot. Returns its address and the xPSR to return with,
/// or `None` when the instruction cannot be bounced or its bytes cannot be
/// read.
// Out of line: its locals are not in the frame of every fault (`xtask
// stack`, the pager context).
#[cfg(feature = "cold-code")]
#[inline(never)]
#[unsafe(link_section = ".text.resident.pager_bounce")]
fn prepare_bounce(core: &PagerCore, pc: u32, xpsr: u32) -> Option<(u32, u32)> {
    let off = core.store_offset(pc)?;
    let fetch = FETCH.0.get();
    let store = STORE.load(Ordering::Relaxed);
    // SAFETY: `FETCH` is the handler's own buffer in SRAM0; DMA channel 0
    // is the pager's.
    unsafe { crate::nor::read_dma(store + off, fetch as u32, 2 * FETCH_HALFWORDS as u32, None) }.ok()?;
    // SAFETY: the DMA transfer has completed; only this handler uses it.
    let code = unsafe { &*fetch };
    let slot = depth().min(DEPTHS - 1);
    // SAFETY: the slot of this depth: its context is the one that faulted
    // (not running), and no shallower context can be in it (02 §3.6).
    let buf = unsafe { &mut (*TRAMPOLINES.0.get())[slot] };
    let (_, _, xpsr) = bounce::build_into(buf, pc, xpsr, code).ok()?;
    Some((buf.as_ptr() as u32, xpsr))
}

/// Called from the MemManage entry with the stacked frame. Returns normally
/// when the fault was served (the instruction is retried, or for a bounce
/// runs from its trampoline); diverges into the crash path otherwise.
// Not `no_mangle`: the MemManage shim names it with `sym`, so an image
// without the shim (a payload) drops it with its statics.
#[unsafe(link_section = ".text.resident.pager")]
pub(crate) extern "C" fn orpheus_pager_fault(frame: *mut u32, exc_return: u32, ipsr: u32) {
    let cfsr = r(SCB_CFSR);
    let mmfar = r(SCB_MMFAR);
    // SAFETY: `frame` is the stacked exception frame handed over by the
    // entry shim; word 6 is the stacked PC.
    let pc = unsafe { core::ptr::read_volatile(frame.add(6)) };
    let addr = mpu::fault_address(cfsr, mmfar, pc);
    // Clear the MemManage status bits (write 1 to clear).
    w(SCB_CFSR, cfsr & 0xFF);
    // SAFETY: only this handler mutates the core after `init`.
    let core = unsafe { &mut *CORE.0.get() };
    let action = core.fault(addr, pc);
    #[cfg(feature = "prof")]
    if crate::prof::faults_on() {
        // WP-T-8: off by default; one load and one branch here when off.
        if !matches!(action, Action::Fatal(_)) {
            let loaded = matches!(action, Action::Load { .. });
            crate::prof::log_fault(addr, pc, cfsr & mpu::CFSR_MMARVALID != 0, loaded);
        }
    }
    match action {
        Action::Remap(m) => {
            w(MPU_CTRL, mpu::CTRL_OFF);
            barrier();
            map(m);
            w(MPU_CTRL, mpu::CTRL_ON);
            barrier();
        }
        Action::Load { map: m, nor_offset, phys, unmap, bounce } => {
            // The instruction gives up its own line: copy it out first
            // (from the page store; its line is about to be overwritten).
            let tramp = if bounce {
                // SAFETY: word 7 of the stacked frame is the xPSR.
                let xpsr = unsafe { core::ptr::read_volatile(frame.add(7)) };
                match prepare_bounce(core, pc, xpsr) {
                    Some(t) => Some(t),
                    None => fatal(frame, exc_return, ipsr, addr),
                }
            } else {
                None
            };
            // No watchdog feed here: a line read takes ~175 us, and a
            // system that only faults pages in is not making progress.
            w(MPU_CTRL, mpu::CTRL_OFF);
            barrier();
            if let Some(u) = unmap {
                map(u);
            }
            let t0 = crate::time::now_us_raw();
            let store = STORE.load(Ordering::Relaxed);
            // SAFETY: `phys` is a line of SRAM2/3 that no mapped region
            // covers (the core unmapped it); DMA channel 0 is the pager's.
            let ok = unsafe { crate::nor::read_dma(store + nor_offset, phys, 1024, None) }.is_ok();
            let dt = crate::time::now_us_raw().wrapping_sub(t0);
            LOAD_US.fetch_add(dt, Ordering::Relaxed);
            LOAD_MAX_US.fetch_max(dt, Ordering::Relaxed);
            if ok {
                map(m);
                if let Some((at, xpsr)) = tramp {
                    // Return into the trampoline (the barrier below makes
                    // its instructions visible to the fetch).
                    // SAFETY: words 6 and 7 of the stacked frame: PC, xPSR.
                    unsafe {
                        core::ptr::write_volatile(frame.add(6), at);
                        core::ptr::write_volatile(frame.add(7), xpsr);
                    }
                }
                w(MPU_CTRL, mpu::CTRL_ON);
                barrier();
            } else {
                LOAD_ERRORS.fetch_add(1, Ordering::Relaxed);
                if let Some(fix) = core.abort_load(phys) {
                    map(fix);
                }
                w(MPU_CTRL, mpu::CTRL_ON);
                barrier();
                fatal(frame, exc_return, ipsr, addr);
            }
        }
        Action::Fatal(_) => fatal(frame, exc_return, ipsr, addr),
    }
}

#[cold]
fn fatal(frame: *mut u32, exc_return: u32, ipsr: u32, addr: u32) -> ! {
    crate::persist::record_fault(frame as usize, exc_return, ipsr);
    crate::persist::note_pager(addr, stats().loads);
    crate::wdt::reboot_after_fault()
}
