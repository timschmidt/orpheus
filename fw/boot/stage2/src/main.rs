//! Boot stage 2 (docs/plan/02-architecture.md §2.2).
//!
//! Decides, in this order:
//! 1. mailbox `"ADFU"` at `0x137FF4` → boot ROM USB recovery;
//! 2. Next key held (ladder 1, 60..200) → USB recovery;
//! 3. three failed starts in a row (counter at `0x137FF8`) → USB recovery;
//! 4. the first valid `"YFW1"` slot of full slot A `0x001000`, full slot B
//!    `0x200000`, dual slot `0x762000` (magic, version, bounds, resident
//!    sum32; the firmware checks the header CRC and page store) → load its
//!    resident image and enter it, slot index in the boot-info low byte;
//! 5. otherwise USB recovery.
//!
//! The stock-firmware path (OK key, and the fallback) is gone with decision
//! D-2 (full layout): the stock firmware is erased.
//!
//! Writes its decision to `0x137FFC` (`BootInfo`). Never touches the rest
//! of the persistent area. No `.data`, no `.bss`, no panics; everything
//! after the image load runs from the trampoline in SRAM1.

#![no_std]
#![no_main]
// Size matters more than style here (1 KiB): range checks written out.
#![allow(clippy::manual_range_contains)]

use core::ptr::{read_volatile as rd, write_volatile as wr};

const MB_ADFU: *mut u32 = 0x0013_7FF4 as *mut u32;
const MB_COUNT: *mut u32 = 0x0013_7FF8 as *mut u32;
const MB_INFO: *mut u32 = 0x0013_7FFC as *mut u32;
const ADFU_MAGIC: u32 = 0x5546_4441;
const COUNT_TAG: u32 = 0x0059_4D42;

/// `yoton_proto::layout::SLOTS`.
const SLOTS: [u32; 3] = [0x0000_1000, 0x0020_0000, 0x0076_2000];
const HDR_TMP: u32 = 0x0010_2000; // below stage 2, above the ROM's use
const YFW1: u32 = 0x3157_4659;
/// Largest resident image: up to the top of the area the trampoline keeps.
const IMAGE_MAX: u32 = 0x0003_7C00;

const SPI_CTL: *mut u32 = 0xC00A_0000 as *mut u32;
const SPI_STAT: *const u32 = 0xC00A_0004 as *const u32;
const SPI_TX: *mut u32 = 0xC00A_0008 as *mut u32;
const SPI_RX: *const u32 = 0xC00A_000C as *const u32;
const SPI_BC: *mut u32 = 0xC00A_0010 as *mut u32;
const WDT: *mut u32 = 0xC003_001C as *mut u32;

core::arch::global_asm!(
    ".section .yst2_header, \"a\"",
    ".word 0x32545359, 0, 0, 0",
    ".section .yst2_entry, \"ax\"",
    ".global _stage2_start",
    ".thumb_func",
    "_stage2_start:",
    "    ldr r0, =0x00101800",
    "    mov sp, r0",
    "    bl {main}",
    "1:  b 1b",
    ".ltorg",
    main = sym stage2_main,
);

unsafe extern "C" {
    static __tramp_start: u32;
    static __tramp_end: u32;
    static __tramp_load: u32;
}

#[inline(always)]
fn feed() {
    // SAFETY: watchdog register.
    unsafe { wr(WDT, rd(WDT) | 1) };
}

/// Boot ROM USB recovery (ROM entry points [HW], as the C stage).
#[inline(never)]
fn adfu(path: u32) -> ! {
    info(path, 0);
    // SAFETY: core registers; hand the ROM the interrupt state of a cold
    // boot (ROM vectors, no NVIC line enabled or pending) whoever ran
    // before us (a RAM monitor during tests).
    unsafe {
        core::arch::asm!(
            "cpsid i",
            "ldr r0, =0xe000e180",
            "mvn r1, #0",
            "str r1, [r0]",
            "str r1, [r0, #4]",
            "str r1, [r0, #0x100]",
            "str r1, [r0, #0x104]",
            "ldr r0, =0xe000ed08",
            "movs r1, #0",
            "str r1, [r0]",
            "dsb",
            "isb",
            "cpsie i",
            out("r0") _, out("r1") _,
        );
    }
    // SAFETY: ROM functions at fixed addresses (Thumb); they do not return
    // except the cleanups.
    unsafe {
        let c1: extern "C" fn() = core::mem::transmute(0x11D5usize);
        let c2: extern "C" fn() = core::mem::transmute(0x1259usize);
        let a: extern "C" fn() -> ! = core::mem::transmute(0x1145usize);
        c1();
        c2();
        a()
    }
}

fn info(path: u32, attempt: u32) {
    // SAFETY: persistent mailbox word. `attempt` carries the slot index in
    // its low byte when shifted by the caller (`attempt << 8 | slot`).
    unsafe { wr(MB_INFO, 0x4200_0000 | path << 16 | attempt) };
}

/// Byte-polled NOR READ into `dst` (or only summed when `dst` is null);
/// returns the byte sum, or `None` on a controller timeout. Lives in the
/// trampoline because it runs after the image overwrote stage 2.
#[unsafe(link_section = ".tramp.read")]
#[inline(never)]
extern "C" fn nor_read(addr: u32, dst: *mut u8, len: u32, err: *mut u32) -> u32 {
    let mut sum = 0u32;
    // SAFETY: SPI0 registers; `dst` is RAM chosen by the caller.
    unsafe {
        let m = |v: u32| wr(SPI_CTL, (rd(SPI_CTL) & !3) | v);
        wr(SPI_CTL, rd(SPI_CTL) | 8);
        wr(SPI_CTL, rd(SPI_CTL) & !8);
        m(0);
        m(2);
        wr(SPI_TX, 3);
        wr(SPI_TX, addr >> 16);
        wr(SPI_TX, addr >> 8);
        wr(SPI_TX, addr);
        wait(0x10, 0x10, err);
        wait(0x40, 0, err);
        m(0);
        let mut left = len;
        let mut p = dst;
        while left > 0 {
            let c = if left > 0x1000 { 0x1000 } else { left };
            wr(SPI_CTL, rd(SPI_CTL) & !0x30);
            wr(SPI_CTL, rd(SPI_CTL) | 0x30);
            wr(SPI_BC, c);
            m(1);
            let mut i = 0;
            while i < c {
                wait(4, 0, err);
                let b = rd(SPI_RX) as u8;
                sum = sum.wrapping_add(u32::from(b));
                if !p.is_null() {
                    wr(p, b);
                    p = p.add(1);
                }
                i += 1;
            }
            m(0);
            left -= c;
            wr(WDT, rd(WDT) | 1);
        }
        wr(SPI_CTL, rd(SPI_CTL) | 8);
    }
    sum
}

#[unsafe(link_section = ".tramp.wait")]
#[inline(never)]
fn wait(mask: u32, val: u32, err: *mut u32) {
    let mut i = 0u32;
    // SAFETY: SPI status register.
    while unsafe { rd(SPI_STAT) } & mask != val {
        i += 1;
        if i > 200_000 {
            // SAFETY: caller's error word.
            unsafe { wr(err, 1) };
            return;
        }
    }
}

/// Runs from the trampoline: load `len` bytes from `src` to 0x100000 and
/// jump to `entry`. The stack moves to SRAM1 first (stage 2 and its stack
/// are about to be overwritten).
#[unsafe(link_section = ".tramp.load")]
#[unsafe(naked)]
extern "C" fn load_and_jump(src: u32, len: u32, entry: u32) -> ! {
    core::arch::naked_asm!(
        "ldr r3, =0x0015FBF0",
        "mov sp, r3",
        "mov r4, r2",           // entry
        "mov r2, r1",           // len
        "ldr r1, =0x00100000",  // dst
        "mov r3, sp",           // &err (scratch word on the new stack)
        "bl {read}",
        "bx r4",
        ".ltorg",
        read = sym nor_read,
    )
}

/// NOR read callable before the trampoline matters (it is also linked at
/// its SRAM1 address; the copy happens first thing).
fn nor_read_main(addr: u32, dst: *mut u8, len: u32, err: &mut u32) -> u32 {
    nor_read(addr, dst, len, err)
}

fn adc(reg: u32) -> u32 {
    let (mut lo, mut hi) = (0x3FFu32, 0u32);
    for _ in 0..16 {
        // SAFETY: LRADC data register.
        let a = unsafe { rd(reg as *const u32) } & 0x3FF;
        lo = lo.min(a);
        hi = hi.max(a);
        for _ in 0..3000 {
            core::hint::spin_loop();
        }
    }
    if hi - lo < 40 { hi } else { 0x3FF }
}

extern "C" fn stage2_main() -> ! {
    feed();
    // SAFETY: mailbox words in the persistent area.
    unsafe {
        if rd(MB_ADFU) == ADFU_MAGIC {
            wr(MB_ADFU, 0);
            adfu(4);
        }
        // Copy the trampoline to SRAM1.
        let s = &raw const __tramp_start as *mut u32;
        let e = &raw const __tramp_end;
        let mut l = &raw const __tramp_load;
        let mut d = s;
        while (d as usize) < e as usize {
            wr(d, rd(l));
            d = d.add(1);
            l = l.add(1);
        }
        // Key ADCs as the key driver sets them up [HW].
        wr(0xC01C_0048 as *mut u32, 0x303B);
        wr(0xC01C_0304 as *mut u32, 0x3001);
        wr(0xC000_1008 as *mut u32, rd(0xC000_1008 as *const u32) | 1 << 27);
        wr(0xC001_0038 as *mut u32, rd(0xC001_0038 as *const u32) | 0x210);
    }
    for _ in 0..200_000 {
        core::hint::spin_loop();
    }
    let k1 = adc(0xC001_0050);
    if k1 >= 60 && k1 <= 200 {
        adfu(3);
    }
    // SAFETY: boot counter word.
    let tries = unsafe {
        let a = rd(MB_COUNT);
        let t = if a >> 8 == COUNT_TAG { a & 0xFF } else { 0 };
        if t >= 3 {
            wr(MB_COUNT, 0);
            adfu(5);
        }
        wr(MB_COUNT, COUNT_TAG << 8 | (t + 1));
        t + 1
    };
    // Slot headers (first 128 bytes) into scratch RAM below stage 2.
    let hdr = HDR_TMP;
    let w = |o: u32| {
        // SAFETY: the header copy.
        unsafe { rd((hdr + o) as *const u32) }
    };
    let mut i = 0;
    while i < SLOTS.len() {
        let slot = SLOTS[i];
        let mut err = 0u32;
        nor_read_main(slot, hdr as *mut u8, 128, &mut err);
        if err == 0 && w(0) == YFW1 && w(4) == 1 {
            let off = w(0x10);
            let len = w(0x14);
            let load = w(0x18);
            let entry = w(0x1C);
            if load == 0x0010_0000 && len > 0 && len <= IMAGE_MAX && off < 0x0007_8000 && entry & 1 == 1 {
                let sum = nor_read_main(slot + off, core::ptr::null_mut(), len, &mut err);
                if err == 0 && sum == w(0x20) {
                    info(1, tries << 8 | i as u32);
                    let jump: extern "C" fn(u32, u32, u32) -> ! = load_and_jump;
                    jump(slot + off, len, entry);
                }
            }
        }
        i += 1;
    }
    adfu(6)
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    adfu(7)
}
