//! `clocks` — WP-PWR-1 level self-test.
//!
//! For each level (Base at 1.0 V, Low, Mid, High, Turbo, Base again):
//! CPU benchmark time against T0, 64 KiB RAM pattern test, CRC of 64 KiB of
//! NOR read by DMA at the level's SPI clock compared with a polled read.
//!
//! Counters per level i (base 4·i): benchmark µs, RAM ok, NOR ok, VDD code.
//! Counter 24: VDD code at start (reset default expected 0x14).

#![no_std]
#![no_main]

use atj2157_hal::clock::{self, Level};
use atj2157_hal::{self as hal, executor, log, nor, pac, persist, time, wdt};
use embassy_executor::Spawner;
use orpheus_proto::hash::crc32;
use orpheus_proto::result::Verdict;
use static_cell::{ConstStaticCell, StaticCell};

#[repr(C, align(4))]
struct Buf([u32; 16384]);
static RAM: ConstStaticCell<Buf> = ConstStaticCell::new(Buf([0; 16384]));
static NORBUF: ConstStaticCell<Buf> = ConstStaticCell::new(Buf([0; 16384]));
const NOR_ADDR: u32 = 0x0076_4000;

#[inline(never)]
fn bench() -> u32 {
    // Integer and memory mix, no I/O: ~2 M iterations.
    let mut x: u32 = 0x1234_5678;
    let mut acc: u32 = 0;
    for i in 0..2_000_000u32 {
        x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        acc = acc.rotate_left(5) ^ x ^ i;
    }
    core::hint::black_box(acc)
}

fn ram_test(buf: &mut [u32]) -> bool {
    for (i, w) in buf.iter_mut().enumerate() {
        *w = (i as u32).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_5A5A;
    }
    let ok1 = buf.iter().enumerate().all(|(i, w)| *w == (i as u32).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_5A5A);
    for w in buf.iter_mut() {
        *w = !*w;
    }
    ok1 && buf.iter().enumerate().all(|(i, w)| *w == !((i as u32).wrapping_mul(0x9E37_79B9) ^ 0xA5A5_5A5A))
}

fn as_bytes(b: &[u32]) -> &[u8] {
    // SAFETY: u32 → u8 view of the same memory, length scaled by 4.
    unsafe { core::slice::from_raw_parts(b.as_ptr().cast(), b.len() * 4) }
}

#[embassy_executor::task]
async fn run(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    nor::init();
    r.set(24, clock::vdd_code());
    let ram = &mut RAM.take().0;
    let nb = &mut NORBUF.take().0;
    // Reference CRC: polled read at the ROM clock.
    // SAFETY: view as bytes for the read.
    let nbb = unsafe { core::slice::from_raw_parts_mut(nb.as_mut_ptr().cast::<u8>(), 65536) };
    let _ = nor::read(NOR_ADDR, nbb);
    let reference = crc32(nbb);
    let levels = [Level::Base, Level::Low, Level::Mid, Level::High, Level::Turbo, Level::Base];
    let mut ok = true;
    let mut base_us = 0u32;
    for (i, &l) in levels.iter().enumerate() {
        clock::set_level(l);
        let t0 = time::now_us();
        bench();
        let us = (time::now_us() - t0) as u32;
        if i == 0 {
            base_us = us;
        }
        let ram_ok = ram_test(ram);
        nb.fill(0);
        // SAFETY: NORBUF is SRAM0 RAM owned by this task; DMA channel 0 is free.
        let dma_ok = unsafe { nor::read_dma(NOR_ADDR, nb.as_ptr() as u32, 65536, None) }.is_ok();
        let nor_ok = dma_ok && crc32(as_bytes(nb)) == reference;
        r.set(4 * i, us);
        r.set(4 * i + 1, u32::from(ram_ok));
        r.set(4 * i + 2, u32::from(nor_ok));
        r.set(4 * i + 3, clock::vdd_code());
        // Benchmark scales with the clock within 3 %.
        let expect = u64::from(base_us) * 48_000_000 / u64::from(l.hz());
        let dev = (i64::from(us) - expect as i64).unsigned_abs();
        let scale_ok = dev * 100 <= expect * 3;
        ok &= ram_ok && nor_ok && scale_ok;
        let mut m = log::LineBuf::new();
        m.push_str("lvl ").push_u32(l.hz() / 1_000_000).push_str(" us=").push_u32(us);
        m.push_str(" ram=").push_u32(u32::from(ram_ok)).push_str(" nor=").push_u32(u32::from(nor_ok));
        m.emit();
    }
    clock::set_level(Level::Base);
    let mut msg = log::LineBuf::new();
    msg.push_str("vdd0=")
        .push_hex(r.get(24))
        .push_str(" base_us=")
        .push_u32(base_us)
        .push_str(" turbo_us=")
        .push_u32(r.get(16));
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
