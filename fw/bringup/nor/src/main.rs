//! `nor` — NOR driver bring-up (PG-1 / PG-2 groundwork).
//!
//! Writes only the scratch sector 0x763000 (the unused second half of the
//! installed page-store header page), and leaves it erased.
//!
//! Counters:
//!  0 JEDEC id                         6 DMA 1 KiB line µs at clock 0x200
//!  1 "YST2" at 0x7DF400 (1 = found)   7 DMA 8 KiB µs at clock 0x200
//!  2 polled 8 KiB µs at 0x101         8 erase µs
//!  3 DMA vs polled match at 0x101     9 program 4 KiB µs
//!  4 DMA 1 KiB line µs at 0x101      10 verify ok (1/0)
//!  5 DMA vs polled match at 0x200    11 erased again (1/0)

#![no_std]
#![no_main]

use atj2157_hal::{self as hal, executor, log, nor, pac, persist, time, wdt};
use embassy_executor::Spawner;
use static_cell::{ConstStaticCell, StaticCell};
use yoton_proto::hash::crc32;
use yoton_proto::result::Verdict;

const SCRATCH: u32 = 0x0076_3000;
const PAGE: u32 = 0x0076_4000;
/// DMA target: SRAM2 slot 0 (not used by anything in a RAM image).
const SLOT: u32 = 0x0020_0000;

#[repr(C, align(4))]
struct Buf([u8; 8192]);
static REF: ConstStaticCell<Buf> = ConstStaticCell::new(Buf([0; 8192]));

fn slot() -> &'static [u8] {
    // SAFETY: SRAM2 slot 0 is plain RAM in a RAM image (crt0 enabled it)
    // and only this test touches it.
    unsafe { core::slice::from_raw_parts(SLOT as *const u8, 8192) }
}

fn dma_check(word: u32, r: &mut persist::Results, base: usize, reference: &[u8]) -> bool {
    nor::set_clock(word);
    // SAFETY: SRAM2 slot 0 is free in this image; DMA channel 0 is unused.
    unsafe { core::ptr::write_bytes(SLOT as *mut u8, 0x5A, 8192) };
    let t0 = time::now_us();
    // SAFETY: as above.
    let ok1 = unsafe { nor::read_dma(PAGE, SLOT, 1024, None) }.is_ok();
    let line = (time::now_us() - t0) as u32;
    let t0 = time::now_us();
    // SAFETY: as above.
    let ok8 = unsafe { nor::read_dma(PAGE, SLOT, 8192, None) }.is_ok();
    let page = (time::now_us() - t0) as u32;
    let m = ok1 && ok8 && crc32(slot()) == crc32(reference);
    r.set(base, u32::from(m));
    r.set(base + 1, line);
    if base == 5 {
        r.set(7, page);
    }
    m
}

#[embassy_executor::task]
async fn run(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    nor::init();
    let id = nor::jedec_id();
    r.set(0, id);
    let mut m = [0u8; 4];
    let _ = nor::read(0x7D_F400, &mut m);
    r.set(1, u32::from(&m == b"YST2"));
    let reference = &mut REF.take().0;
    let t0 = time::now_us();
    let _ = nor::read(PAGE, reference);
    r.set(2, (time::now_us() - t0) as u32);
    let ok_a = dma_check(0x101, &mut r, 3, reference);
    let ok_b = dma_check(0x200, &mut r, 5, reference);
    nor::set_clock(0x101);

    // Scratch sector: must be erased (all 0xFF) before we touch it.
    let mut buf = [0u8; 4096];
    let _ = nor::read(SCRATCH, &mut buf);
    let mut ok_w = false;
    let mut erased_after = false;
    if buf.iter().all(|&b| b == 0xFF) {
        for (i, b) in buf.iter_mut().enumerate() {
            *b = (i as u8) ^ 0xA5;
        }
        let t0 = time::now_us();
        let e = nor::erase_sector(SCRATCH);
        r.set(8, (time::now_us() - t0) as u32);
        let mut scratch = [0u8; 256];
        let t0 = time::now_us();
        let w = nor::write(SCRATCH, &buf, &mut scratch);
        r.set(9, (time::now_us() - t0) as u32);
        let mut back = [0u8; 4096];
        let _ = nor::read(SCRATCH, &mut back);
        ok_w = e.is_ok() && w.is_ok() && back == buf;
        r.set(10, u32::from(ok_w));
        let _ = nor::erase_sector(SCRATCH);
        let _ = nor::read(SCRATCH, &mut back);
        erased_after = back.iter().all(|&b| b == 0xFF);
        r.set(11, u32::from(erased_after));
    }
    // Guard: writes outside the free region must be refused.
    let refused = nor::erase_sector(0x0076_0000) == Err(nor::Error::Refused)
        && nor::program_page(0, &[0]) == Err(nor::Error::Refused);
    let ok = id == 0x0017_405E && ok_a && ok_w && erased_after && refused;
    let mut msg = log::LineBuf::new();
    msg.push_str("id=").push_hex(id).push_str(" dma=").push_u32(u32::from(ok_a)).push_u32(u32::from(ok_b));
    msg.push_str(" write=").push_u32(u32::from(ok_w)).push_str(" guard=").push_u32(u32::from(refused));
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
