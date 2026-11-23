//! `critical-section` implementation with BASEPRI (never PRIMASK).
//!
//! Masks interrupt levels 1–3 and leaves level 0 (faults, the pager's
//! MemManage handler) live, so code inside a critical section may still take
//! a page fault (02 §3.2, CLAUDE.md).

use cortex_m::register::{basepri, basepri_max};

use crate::interrupt::CS_BASEPRI;

struct BasepriCs;
critical_section::set_impl!(BasepriCs);

// SAFETY: acquire returns the previous BASEPRI and raises it (BASEPRI_MAX
// never lowers it); release restores exactly that value. Nesting therefore
// restores correctly, and every maskable handler that touches shared state
// runs at a level masked by CS_BASEPRI.
unsafe impl critical_section::Impl for BasepriCs {
    unsafe fn acquire() -> critical_section::RawRestoreState {
        let old = basepri::read();
        basepri_max::write(CS_BASEPRI);
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        u32::from(old)
    }

    unsafe fn release(old: critical_section::RawRestoreState) {
        core::sync::atomic::compiler_fence(core::sync::atomic::Ordering::SeqCst);
        // SAFETY: restoring the value read in `acquire`.
        unsafe { basepri::write(old as u8) };
    }
}
