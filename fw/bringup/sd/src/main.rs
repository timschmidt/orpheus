//! `sd` — WP-SD-1 bring-up image (read-only: no card writes).
//!
//! Counters:
//!  0 init result (0 ok, else error code)   7 MBR partition 1 size (blocks)
//!  1 capacity in blocks (low 32 bits)       8 multi-vs-single read mismatches
//!  2 capacity high 32 bits                  9 sequential read KiB/s (64 KiB reads)
//!  3 CSD version << 8 | TRAN_SPEED         10 sequential read KiB/s (4 KiB reads)
//!  4 CID manufacturer << 16 | OEM          11 commands issued
//!  5 MBR signature (0xAA55 expected)       12 timeouts
//!  6 MBR partition 1 type << 24 | start    13 CRC errors
//!                                          14 busy polls

#![no_std]
#![no_main]

use atj2157_hal::sd::{self, Error, Sd};
use atj2157_hal::{self as hal, dma, executor, log, pac, persist, time, wdt};
use embassy_executor::Spawner;
use orpheus_proto::hash::crc32;
use orpheus_proto::result::Verdict;
use static_cell::{ConstStaticCell, StaticCell};
use yoton_bsp::pins;

/// 64 KiB DMA buffer in SRAM0 (word aligned).
#[repr(C, align(4))]
struct Buf([u8; 65536]);
static BUF: ConstStaticCell<Buf> = ConstStaticCell::new(Buf([0; 65536]));
static BUF2: ConstStaticCell<Buf> = ConstStaticCell::new(Buf([0; 65536]));

fn err_code(e: Error) -> u32 {
    match e {
        Error::Timeout => 1,
        Error::Crc => 2,
        Error::Controller(s) => 0x1000_0000 | s,
        Error::Card(s) => 0x2000_0000 | (s >> 4),
        Error::NoCard => 4,
        Error::Unsupported => 5,
        Error::Dma => 6,
    }
}

#[embassy_executor::task]
async fn run(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    let buf = &mut BUF.take().0;
    let buf2 = &mut BUF2.take().0;
    // SAFETY: channel 4 is the SD channel (01-hardware §6), used by nothing
    // else here.
    let mut card =
        Sd::new(&sd::Pins { pads: &pins::SD_PADS, func: pins::SD_FUNC }, unsafe { dma::Channel::steal(4) });
    let res = card.init_card().await;
    r.set(0, res.map_or_else(err_code, |()| 0));
    let mut ok = res.is_ok();
    if ok {
        let blocks = card.blocks();
        r.set(1, blocks as u32);
        r.set(2, (blocks >> 32) as u32);
        if let Some(c) = card.csd {
            r.set(3, u32::from(c.version) << 8 | u32::from(c.tran_speed));
        }
        if let Some(c) = card.cid {
            r.set(4, u32::from(c.manufacturer) << 16 | u32::from(c.oem[0]) << 8 | u32::from(c.oem[1]));
        }
        // MBR.
        match card.read(0, &mut buf[..512]).await {
            Ok(()) => {
                r.set(5, u32::from(u16::from_le_bytes([buf[510], buf[511]])));
                let e = &buf[446..462];
                let start = u32::from_le_bytes([e[8], e[9], e[10], e[11]]);
                let size = u32::from_le_bytes([e[12], e[13], e[14], e[15]]);
                r.set(6, u32::from(e[4]) << 24 | (start & 0xFF_FFFF));
                r.set(7, size);
            }
            Err(e) => {
                r.set(5, err_code(e));
                ok = false;
            }
        }
        // Consistency: 128 blocks multi-block vs block by block.
        let base = 2048u32;
        let mut mism = 0u32;
        if card.read(base, &mut buf[..65536]).await.is_ok() {
            for b in 0..128usize {
                if card.read(base + b as u32, &mut buf2[..512]).await.is_err()
                    || crc32(&buf[b * 512..(b + 1) * 512]) != crc32(&buf2[..512])
                {
                    mism += 1;
                }
            }
        } else {
            mism = 0xFFFF;
        }
        r.set(8, mism);
        ok &= mism == 0;
        // Sequential read speed, 4 MiB in 64 KiB and in 4 KiB requests.
        let t0 = time::now_us();
        let mut fails = 0;
        for i in 0..64u32 {
            if card.read(base + i * 128, &mut buf[..]).await.is_err() {
                fails += 1;
            }
        }
        let us = (time::now_us() - t0).max(1);
        r.set(9, (4096 * 1_000_000 / us) as u32);
        let t0 = time::now_us();
        for i in 0..256u32 {
            if card.read(base + i * 8, &mut buf[..4096]).await.is_err() {
                fails += 1;
            }
        }
        let us = (time::now_us() - t0).max(1);
        r.set(10, (1024 * 1_000_000 / us) as u32);
        ok &= fails == 0;
    }
    r.set(11, card.stats.commands);
    r.set(12, card.stats.timeouts);
    r.set(13, card.stats.crc_errors);
    r.set(14, card.stats.busy_polls);
    r.set(15, card.stats.last_err_state);
    r.set(16, card.stats.last_err_cmd);
    let mut msg = log::LineBuf::new();
    msg.push_str("init=")
        .push_hex(res.map_or_else(err_code, |()| 0))
        .push_str(" blocks=")
        .push_u32(card.blocks() as u32);
    let v = if res == Err(Error::NoCard) {
        Verdict::Skip
    } else if ok {
        Verdict::Pass
    } else {
        Verdict::Fail
    };
    r.flush(v, msg.as_bytes());
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
