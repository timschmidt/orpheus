//! `pagetoy` — WP-T-3 toy for `cargo xtask pagelink` and
//! `cargo xtask check-resident`.
//!
//! About 70 KiB of generated code and 50 KiB of tables (build.rs), linked
//! into a resident image plus several code and rodata pages. Host-side
//! only for now: running it needs the device pager of WP-PG-1 (the MPU and
//! the MemManage entry), which `_start` does not set up yet.
//!
//! [`toy_memmanage`] stands in for the MemManage handler: a resident root
//! (`resident.toml`) that drives `PagerCore` and reaches a helper and a
//! table that no rule names, so pagelink must make them resident through
//! the closure. `xtask test pagelink` plants a violation by forcing
//! [`planted_violation`] into a page and checks that `check-resident`
//! reports the chain.

#![no_std]
#![no_main]

mod generated {
    include!(concat!(env!("OUT_DIR"), "/generated.rs"));
}

use core::hint::black_box;

use atj2157_hal::{self as hal, executor, log, persist, wdt};
use embassy_executor::Spawner;
use orpheus_pager::{Action, PagerCore};
use orpheus_proto::result::Verdict;
use static_cell::StaticCell;

unsafe extern "C" {
    /// Page counts set by the pass-2 linker script (absolute symbols).
    static __orpheus_code_pages: u8;
    static __orpheus_rodata_pages: u8;
}

static HELPER_TABLE: [u32; 64] = {
    let mut t = [0u32; 64];
    let mut i = 0;
    while i < 64 {
        t[i] = (i as u32).wrapping_mul(0x0101_0101) ^ 0x5A5A_0000;
        i += 1;
    }
    t
};

static PLANTED_TABLE: [u32; 16] = [3, 1, 4, 1, 5, 9, 2, 6, 5, 3, 5, 8, 9, 7, 9, 3];

#[inline(never)]
fn handler_helper(addr: u32) -> u32 {
    HELPER_TABLE[(addr as usize >> 10) & 63].rotate_left(addr & 31)
}

/// Reached from the handler only for one address; `xtask test pagelink`
/// forces it into a page to plant a resident-set violation.
#[inline(never)]
fn planted_violation(pc: u32) -> u32 {
    PLANTED_TABLE[(pc as usize) & 15].wrapping_mul(pc)
}

/// Stand-in MemManage handler (resident root of this image).
#[unsafe(no_mangle)]
pub extern "C" fn toy_memmanage(addr: u32, pc: u32) -> u32 {
    let ncode = (&raw const __orpheus_code_pages) as u32;
    let nro = (&raw const __orpheus_rodata_pages) as u32;
    let mut core = PagerCore::new();
    core.init(ncode, nro);
    let mut code = handler_helper(addr);
    if addr == 0xDEAD_0000 {
        code ^= planted_violation(pc);
    }
    match core.fault(addr, pc) {
        Action::Remap(m) => code ^ m.rasr(),
        Action::Load { map, nor_offset, phys, unmap } => {
            code ^ map.rasr() ^ nor_offset ^ phys ^ unmap.map_or(0, |u| u.rasr())
        }
        Action::Fatal(_) => code ^ 1,
    }
}

#[embassy_executor::task]
async fn run(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    let mut x = black_box(0x1234_5678u32);
    for f in &generated::WORK {
        x = f(x);
    }
    x ^= generated::work_17(x);
    x ^= toy_memmanage(black_box(0x0020_0000), black_box(0x0010_0000));
    r.set(0, x);
    r.flush(Verdict::Pass, b"pagetoy");
    log::line(b"pagetoy: done, rebooting");
    wdt::reboot(wdt::Target::Adfu);
}

static MAIN: StaticCell<executor::MainExecutor> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    log::init();
    hal::init();
    let test_id = persist::test_request().map_or(0, |(id, _)| id);
    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(run(test_id).unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
