//! NVIC access and the priority plan (docs/plan/02-architecture.md §4.1).
//!
//! Four levels (two priority bits):
//!
//! | Level | Byte | Use |
//! |---|---|---|
//! | 0 | `0x00` | Faults and the pager; the profiler's PC sampler (T2, only while `prof pc` runs; resident, never faults). Never masked by BASEPRI |
//! | 1 | `0x40` | Deadline interrupts: time driver, audio DMA, UART DMA |
//! | 2 | `0x80` | The RT executor (software-pended line) |
//! | 3 | `0xC0` | Everything else |
//!
//! The critical section raises BASEPRI to `0x40` (masks levels 1–3).

use atj2157_pac::irq;
use cortex_m::interrupt::InterruptNumber;

/// Interrupt priority level.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Level {
    Fault = 0,
    Deadline = 1,
    Rt = 2,
    Normal = 3,
}

impl Level {
    #[must_use]
    pub const fn byte(self) -> u8 {
        irq::prio(self as u8)
    }
}

/// BASEPRI value used by the critical section.
pub const CS_BASEPRI: u8 = irq::prio(1);

/// An NVIC line number, usable with `cortex_m` and Embassy.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Irq(pub u16);

// SAFETY: `number` returns the line number the struct was built from, which
// callers take from `atj2157_pac::irq` (all < 64 lines on this SoC).
unsafe impl InterruptNumber for Irq {
    fn number(self) -> u16 {
        self.0
    }
}

const NVIC_ISER: usize = 0xE000_E100;
const NVIC_ICER: usize = 0xE000_E180;
const NVIC_ISPR: usize = 0xE000_E200;
const NVIC_ICPR: usize = 0xE000_E280;
const NVIC_IPR: usize = 0xE000_E400;
const SCB_SHPR: usize = 0xE000_ED18;

fn w(addr: usize, v: u32) {
    // SAFETY: NVIC/SCB registers of the core; word writes have no memory
    // safety implications.
    unsafe { core::ptr::write_volatile(addr as *mut u32, v) }
}

pub fn enable(n: u16) {
    w(NVIC_ISER + 4 * usize::from(n / 32), 1 << (n % 32));
}

pub fn disable(n: u16) {
    w(NVIC_ICER + 4 * usize::from(n / 32), 1 << (n % 32));
}

pub fn pend(n: u16) {
    w(NVIC_ISPR + 4 * usize::from(n / 32), 1 << (n % 32));
}

pub fn unpend(n: u16) {
    w(NVIC_ICPR + 4 * usize::from(n / 32), 1 << (n % 32));
}

pub fn set_level(n: u16, level: Level) {
    // SAFETY: byte write to the line's priority register.
    unsafe { core::ptr::write_volatile((NVIC_IPR + usize::from(n)) as *mut u8, level.byte()) }
}

/// Set every NVIC line to `Normal` and the system handlers to the plan:
/// MemManage/BusFault/UsageFault at level 0, SVCall/PendSV/SysTick at 3.
pub fn init_priorities() {
    for n in 0..irq::COUNT as u16 {
        set_level(n, Level::Normal);
    }
    // SHPR1: MemManage, BusFault, UsageFault = 0.
    w(SCB_SHPR, 0);
    // SHPR2: SVCall (byte 3) = level 3.
    w(SCB_SHPR + 4, u32::from(Level::Normal.byte()) << 24);
    // SHPR3: PendSV (byte 2), SysTick (byte 3) = level 3.
    w(SCB_SHPR + 8, u32::from(Level::Normal.byte()) << 16 | u32::from(Level::Normal.byte()) << 24);
}

/// Bind a handler, set the level and enable the line.
pub fn install(n: u16, level: Level, handler: fn()) {
    disable(n);
    unpend(n);
    crate::rt::bind(n, handler);
    set_level(n, level);
    enable(n);
}
