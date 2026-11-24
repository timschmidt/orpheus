//! ymon payload: erase/program NOR for the full layout (02 §2.4, WP LAY-1).
//!
//! Arguments at `0x11F800`: `[op, addr, len]`; op 1 erases the 4 KiB sector
//! at `addr`, op 2 programs `len` ≤ 256 bytes (within one page) from the
//! buffer at `0x122000` without erasing (so it can also clear single bits),
//! op 3 only reports the JEDEC id. Result: four words `[status, NOR status
//! register, op, JEDEC id]`; status 0 = done, `0xBAD` = refused by the
//! guard, `0x7173` = the chip did not finish in time.
//!
//! The guard is the HAL's full-layout guard: sector 0 (stage 1; only
//! `install-boot` writes it) and the vendor data above `0x7E0000` are never
//! written.

#![no_std]
#![no_main]

use atj2157_hal::nor;

const ARGS: *const u32 = 0x0011_F800 as *const u32;
const DATA: *const u8 = 0x0012_2000 as *const u8;

#[repr(C)]
pub struct Res {
    addr: *const u32,
    size: u32,
}

static mut OUT: [u32; 4] = [0; 4];
static mut RES: Res = Res { addr: core::ptr::null(), size: 0 };

unsafe extern "C" {
    static mut __bss_start: u32;
    static mut __bss_end: u32;
}

#[unsafe(no_mangle)]
#[unsafe(link_section = ".entry")]
pub extern "C" fn payload_entry() -> *const Res {
    // SAFETY: ymon does not zero .bss; the region is this payload's own.
    unsafe {
        let mut p = &raw mut __bss_start;
        while p < &raw mut __bss_end {
            p.write_volatile(0);
            p = p.add(1);
        }
    }
    // SAFETY: the argument block ymon filled.
    let (op, addr, len) =
        unsafe { (ARGS.read_volatile(), ARGS.add(1).read_volatile(), ARGS.add(2).read_volatile()) };
    nor::init();
    nor::allow_full_layout();
    let r = match op {
        1 => nor::erase_sector(addr),
        2 => {
            // SAFETY: the host wrote `len` bytes to the data buffer.
            let data = unsafe { core::slice::from_raw_parts(DATA, len.min(256) as usize) };
            nor::program_page(addr, data)
        }
        _ => Ok(()),
    };
    let status = match r {
        Ok(()) => 0,
        Err(nor::Error::Refused) => 0xBAD,
        Err(_) => 0x7173,
    };
    // SAFETY: single-threaded payload; the host reads the result after we
    // return.
    unsafe {
        OUT = [status, u32::from(nor::status()), op, nor::jedec_id()];
        RES = Res { addr: (&raw const OUT).cast(), size: 16 };
        &raw const RES
    }
}

#[panic_handler]
fn panic(_: &core::panic::PanicInfo) -> ! {
    loop {
        core::hint::spin_loop();
    }
}
