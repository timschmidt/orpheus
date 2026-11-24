//! Demand pager, device side (docs/plan/02-architecture.md §3). The policy
//! is `yoton_pager::PagerCore` (host-tested); this module owns the MPU, the
//! MemManage entry and the NOR line reads. **Resident**, like the NOR
//! driver: nothing here may reach paged code.
//!
//! MemManage arrives at priority 0, above everything BASEPRI can mask, so a
//! fault inside a critical section or an interrupt handler is served too
//! (it stalls everything for one line read, ~175 µs).

use core::cell::UnsafeCell;
use core::sync::atomic::{AtomicU32, Ordering};

use yoton_pager::{Action, PagerCore, RegionMap, mpu};

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

const MPU_CTRL: usize = 0xE000_ED94;
const MPU_RNR: usize = 0xE000_ED98;
const MPU_RBAR: usize = 0xE000_ED9C;
const MPU_RASR: usize = 0xE000_EDA0;
const SCB_CFSR: usize = 0xE000_ED28;
const SCB_MMFAR: usize = 0xE000_ED34;

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
/// instead of executing whatever SRAM2/3 hold.
pub fn arm_deny() {
    w(MPU_CTRL, mpu::CTRL_OFF);
    barrier();
    let (n, rbar, rasr) = mpu::INITIAL[0];
    set_region(n, rbar, rasr);
    w(MPU_CTRL, mpu::CTRL_ON);
    barrier();
}

/// Start paging: `store` is the NOR address of page 0, with `ncode` code
/// and `nro` rodata pages. Call before anything paged runs (from `main`,
/// which is resident). Keeps the stack-guard region 7.
pub fn init(store: u32, ncode: u32, nro: u32) {
    STORE.store(store, Ordering::Relaxed);
    // SAFETY: the MPU is about to be reprogrammed; no fault can use the
    // core concurrently (nothing paged has run yet).
    unsafe { (*CORE.0.get()).init(ncode, nro) };
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
pub fn stats() -> yoton_pager::Stats {
    // SAFETY: a plain read of counters; a torn read only skews statistics.
    unsafe { (*CORE.0.get()).stats() }
}

/// Called from the MemManage entry with the stacked frame. Returns normally
/// when the fault was served (the instruction is retried); diverges into
/// the crash path otherwise.
#[unsafe(no_mangle)]
#[unsafe(link_section = ".text.resident.pager")]
extern "C" fn yoton_pager_fault(frame: *const u32, exc_return: u32, ipsr: u32) {
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
    match core.fault(addr, pc) {
        Action::Remap(m) => {
            w(MPU_CTRL, mpu::CTRL_OFF);
            barrier();
            map(m);
            w(MPU_CTRL, mpu::CTRL_ON);
            barrier();
        }
        Action::Load { map: m, nor_offset, phys, unmap } => {
            crate::wdt::feed();
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
fn fatal(frame: *const u32, exc_return: u32, ipsr: u32, addr: u32) -> ! {
    crate::persist::record_fault(frame as usize, exc_return, ipsr);
    crate::persist::note_pager(addr, stats().loads);
    crate::wdt::reboot_after_fault()
}
