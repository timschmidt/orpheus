//! Start-up code, vector table, interrupt dispatch and fault capture
//! (docs/plan/03-hal-drivers.md §3).
//!
//! The image is entered at its first byte (`_start`, Thumb) by whatever
//! loaded it: `ymon` during development (interrupts may be enabled, VTOR
//! points into ymon) or the NOR boot stage 2. `_start` takes over the
//! machine before touching memory, brings SRAM2/3 up, sets the stack,
//! zeroes `.bss` and calls the application's `main` (`#[unsafe(no_mangle)]
//! extern "C" fn main() -> !`, provided by the image crate).
//!
//! Interrupts: every NVIC vector points at [`irq_dispatch`], which looks up
//! the handler registered with [`bind`]. Faults go to an assembly shim that
//! hands the stacked frame to [`fault_entry`], which writes the persistent
//! fault record and reboots (03 §3, 02 §8).

use core::sync::atomic::{AtomicUsize, Ordering};

use atj2157_pac::irq;

core::arch::global_asm!(
    ".section .init, \"ax\"",
    ".global _start",
    ".type _start, %function",
    ".thumb_func",
    "_start:",
    "    cpsid i",
    // Disable and un-pend every NVIC line the loader may have left enabled.
    "    ldr r0, =0xe000e180",
    "    mvn r1, #0",
    "    str r1, [r0]",
    "    str r1, [r0, #4]",
    "    ldr r0, =0xe000e280",
    "    str r1, [r0]",
    "    str r1, [r0, #4]",
    // Stop SysTick.
    "    ldr r0, =0xe000e010",
    "    movs r1, #0",
    "    str r1, [r0]",
    // VTOR -> our table.
    "    ldr r0, =0xe000ed08",
    "    ldr r1, =__vectors",
    "    str r1, [r0]",
    "    dsb",
    "    isb",
    // SRAM2/SRAM3 bring-up [HW] (01-hardware §3): cache-controller clocks,
    // reset pulse, then the address windows.
    "    ldr r0, =0xc0001008",
    "    ldr r1, [r0]",
    "    orr r1, r1, #0x600",
    "    str r1, [r0]",
    "    ldr r0, =0xc0000000",
    "    ldr r1, [r0]",
    "    bic r1, r1, #0x600",
    "    str r1, [r0]",
    "    orr r1, r1, #0x600",
    "    str r1, [r0]",
    "    ldr r0, =0xc0040000",
    "    ldr r1, =0x00200000",
    "    str r1, [r0, #0x10]",
    "    ldr r1, =0x00207fff",
    "    str r1, [r0, #0x14]",
    "    ldr r1, =0x00300000",
    "    str r1, [r0, #0x18]",
    "    ldr r1, =0x00307fff",
    "    str r1, [r0, #0x1c]",
    "    ldr r0, =0xc0040300",
    "    ldr r1, =0x00200000",
    "    str r1, [r0, #0x00]",
    "    movs r1, #0",
    "    str r1, [r0, #0x04]",
    "    ldr r1, =0x00300000",
    "    str r1, [r0, #0x08]",
    "    movs r1, #0",
    "    str r1, [r0, #0x0c]",
    "    dsb",
    "    isb",
    // One stack (MSP) for everything; thread mode stays on MSP.
    "    ldr r0, =__stack_top",
    "    msr msp, r0",
    "    movs r0, #0",
    "    msr control, r0",
    "    isb",
    // Zero .bss.
    "    ldr r0, =__bss_start",
    "    ldr r1, =__bss_end",
    "    movs r2, #0",
    "1:  cmp r0, r1",
    "    it lo",
    "    strlo r2, [r0], #4",
    "    blo 1b",
    // Paint the stack (all but the top 64 bytes) for high-water marks.
    "    ldr r0, =__stack_bottom",
    "    ldr r1, =__stack_top",
    "    sub r1, r1, #64",
    "    ldr r2, =0x5354434b",
    "2:  cmp r0, r1",
    "    it lo",
    "    strlo r2, [r0], #4",
    "    blo 2b",
    // MemManage, BusFault, UsageFault enabled; divide-by-zero traps.
    "    ldr r0, =0xe000ed24",
    "    ldr r1, [r0]",
    "    orr r1, r1, #0x70000",
    "    str r1, [r0]",
    "    ldr r0, =0xe000ed14",
    "    ldr r1, [r0]",
    "    orr r1, r1, #0x10",
    "    str r1, [r0]",
    // FPU: CP10/CP11 full access (lazy stacking is the reset default).
    "    ldr r0, =0xe000ed88",
    "    ldr r1, [r0]",
    "    orr r1, r1, #0xf00000",
    "    str r1, [r0]",
    "    dsb",
    "    isb",
    // Mask everything but level 0 with BASEPRI, then clear PRIMASK: from
    // here on PRIMASK is never used (a page fault under PRIMASK is fatal).
    "    movs r0, #0x40",
    "    msr basepri, r0",
    "    cpsie i",
    "    bl main",
    "3:  b 3b",
    ".ltorg",
    "",
    // Fault shim: r0 = stacked frame, r1 = EXC_RETURN, r2 = IPSR.
    ".section .text.yoton_fault_shim, \"ax\"",
    ".global yoton_fault_shim",
    ".type yoton_fault_shim, %function",
    ".thumb_func",
    "yoton_fault_shim:",
    "    tst lr, #4",
    "    ite eq",
    "    mrseq r0, msp",
    "    mrsne r0, psp",
    "    mov r1, lr",
    "    mrs r2, ipsr",
    // Continue on the emergency stack: the fault may be a stack overflow
    // into the guard region, where pushing would fault again.
    "    ldr r3, ={emerg}",
    "    add r3, r3, #{emerg_size}",
    "    mov sp, r3",
    "    b {fault}",
    fault = sym fault_entry,
    emerg = sym EMERGENCY_STACK,
    emerg_size = const EMERGENCY_STACK_BYTES,
);

const EMERGENCY_STACK_BYTES: usize = 512;

/// Stack for the fault path (never used otherwise).
#[unsafe(link_section = ".uninit.emergency_stack")]
static mut EMERGENCY_STACK: [u64; EMERGENCY_STACK_BYTES / 8] = [0; EMERGENCY_STACK_BYTES / 8];

/// Size of the MPU stack guard at the bottom of the stack.
pub const STACK_GUARD_BYTES: u32 = 256;
/// MPU region used for the guard (02 §1.2: region 7, highest precedence).
const GUARD_REGION: u32 = 7;

/// Arm the stack guard: a no-access MPU region over the lowest 256 bytes of
/// the stack. Faults there are recorded as `StackGuard`.
pub fn enable_stack_guard() {
    let base = ((&raw const __stack_bottom) as u32 + 255) & !255;
    let mpu = |off: usize, v: u32| {
        // SAFETY: MPU registers of the core.
        unsafe { core::ptr::write_volatile((0xE000_ED90 + off) as *mut u32, v) };
    };
    mpu(0x08, GUARD_REGION); // RNR
    mpu(0x0C, base); // RBAR
    // RASR: XN, AP = 0 (no access), SIZE = log2(256) - 1 = 7, ENABLE.
    mpu(0x10, 1 << 28 | 7 << 1 | 1);
    // CTRL: ENABLE | PRIVDEFENA (default map for everything else).
    mpu(0x04, 0b101);
    cortex_m::asm::dsb();
    cortex_m::asm::isb();
}

/// Address range of the guard (for the fault record).
#[must_use]
pub fn stack_guard_range() -> (u32, u32) {
    let base = ((&raw const __stack_bottom) as u32 + 255) & !255;
    (base, base + STACK_GUARD_BYTES)
}

unsafe extern "C" {
    fn _start();
    fn yoton_fault_shim();
    static __stack_top: u32;
    static __stack_bottom: u32;
}

/// Number of NVIC lines with vectors.
const N_IRQ: usize = irq::COUNT;

/// A vector table entry.
#[derive(Clone, Copy)]
#[repr(C)]
pub union Vector {
    handler: unsafe extern "C" fn(),
    reserved: usize,
}

const RESERVED: Vector = Vector { reserved: 0 };
const FAULT: Vector = Vector { handler: yoton_fault_shim };
const DISPATCH: Vector = Vector { handler: irq_dispatch };

/// The vector table (aligned to 512 bytes by the linker script; 80 entries).
#[unsafe(link_section = ".vectors")]
#[unsafe(no_mangle)]
#[used]
pub static __vectors: [Vector; 16 + N_IRQ] = {
    let mut v = [DISPATCH; 16 + N_IRQ];
    v[0] = RESERVED; // initial SP: unused, `_start` sets MSP itself
    v[1] = Vector { handler: _start };
    v[2] = FAULT; // NMI
    v[3] = FAULT; // HardFault
    v[4] = FAULT; // MemManage (the pager hooks in here later)
    v[5] = FAULT; // BusFault
    v[6] = FAULT; // UsageFault
    v[7] = RESERVED;
    v[8] = RESERVED;
    v[9] = RESERVED;
    v[10] = RESERVED;
    v[11] = FAULT; // SVCall: unused
    v[12] = FAULT; // DebugMonitor
    v[13] = RESERVED;
    v[14] = FAULT; // PendSV: unused
    v[15] = FAULT; // SysTick: unused (follows the core clock)
    v
};

/// Registered IRQ handlers (0 = none). Written with the line disabled.
static HANDLERS: [AtomicUsize; N_IRQ] = [const { AtomicUsize::new(0) }; N_IRQ];

/// Register `handler` for NVIC line `n`. Call before enabling the line.
pub fn bind(n: u16, handler: fn()) {
    if let Some(slot) = HANDLERS.get(usize::from(n)) {
        slot.store(handler as usize, Ordering::Release);
    }
}

/// Common entry for all NVIC vectors.
unsafe extern "C" fn irq_dispatch() {
    let ipsr: u32;
    // SAFETY: reading IPSR has no side effects.
    unsafe { core::arch::asm!("mrs {}, ipsr", out(reg) ipsr, options(nomem, nostack, preserves_flags)) };
    let n = (ipsr & 0x1FF).wrapping_sub(16) as usize;
    let h = HANDLERS.get(n).map_or(0, |s| s.load(Ordering::Acquire));
    if h == 0 {
        unhandled(n);
    } else {
        // SAFETY: only `bind` stores into HANDLERS, always a valid `fn()`.
        let f: fn() = unsafe { core::mem::transmute::<usize, fn()>(h) };
        f();
    }
}

#[cold]
fn unhandled(n: usize) {
    // Disable the line so it cannot storm, and record it.
    crate::interrupt::disable(n as u16);
    crate::persist::note_unhandled_irq(n as u32);
}

/// Called by the fault shim with the stacked exception frame.
#[unsafe(no_mangle)]
extern "C" fn fault_entry(frame: *const u32, exc_return: u32, ipsr: u32) -> ! {
    crate::persist::record_fault(frame as usize, exc_return, ipsr);
    crate::wdt::reboot_after_fault()
}

/// Bytes of the stack never touched since start-up (paint still intact).
#[must_use]
pub fn stack_unused() -> usize {
    // Start above the MPU guard (reading it faults).
    let bottom = stack_guard_range().1 as usize;
    let top = (&raw const __stack_top) as usize;
    let mut p = bottom;
    while p < top {
        // SAFETY: `p` walks the stack region, which is mapped RAM.
        if unsafe { core::ptr::read_volatile(p as *const u32) } != 0x5354_434B {
            break;
        }
        p += 4;
    }
    p - bottom
}

/// Total stack size.
#[must_use]
pub fn stack_size() -> usize {
    (&raw const __stack_top) as usize - (&raw const __stack_bottom) as usize
}
