//! `i2c` — WP-FM-0 bring-up image (reads only; no device register writes).
//!
//! Counters:
//!  0 bitmap of answering addresses 0x00–0x1F   4 lines idle (SCL<<1 | SDA)
//!  1 bitmap 0x20–0x3F                          5 tuner id via 0x11 reg 0x00 (or error)
//!  2 bitmap 0x40–0x5F                          6 tuner sequential read bytes 8–9 (0x10)
//!  3 bitmap 0x60–0x7F                          7 accelerometer reg 0x01 (chip id)
//!                                              8 accelerometer regs 0x02–0x05 (raw)

#![no_std]
#![no_main]

use atj2157_hal::i2c_soft::{Error, SoftI2c};
use atj2157_hal::{self as hal, executor, log, pac, persist, wdt};
use embassy_executor::Spawner;
use static_cell::StaticCell;
use yoton_bsp::pins;
use yoton_proto::result::Verdict;

fn code(e: Error) -> u32 {
    0xEE00_0000
        | match e {
            Error::Nack => 1,
            Error::DataNack => 2,
            Error::BusStuck => 3,
        }
}

#[embassy_executor::task]
async fn run(test_id: u32) {
    let mut r = persist::Results::begin(test_id);
    let bus = SoftI2c::new(pins::I2C_SCL, pins::I2C_SDA, 5);
    let (scl, sda) = bus.lines();
    r.set(4, u32::from(scl) << 1 | u32::from(sda));
    if !(scl && sda) {
        bus.recover();
    }
    let mut maps = [0u32; 4];
    let mut stuck = false;
    for a in 0x08u8..0x78 {
        match bus.probe(a) {
            Ok(true) => maps[usize::from(a / 32)] |= 1 << (a % 32),
            Ok(false) => {}
            Err(_) => stuck = true,
        }
    }
    for (i, m) in maps.iter().enumerate() {
        r.set(i, *m);
    }
    let mut b = [0u8; 2];
    r.set(5, bus.write_read(0x11, &[0x00], &mut b).map_or_else(code, |()| u32::from(u16::from_be_bytes(b))));
    let mut seq = [0u8; 10];
    r.set(
        6,
        bus.read(0x10, &mut seq).map_or_else(code, |()| u32::from(u16::from_be_bytes([seq[8], seq[9]]))),
    );
    let mut id = [0u8; 1];
    r.set(7, bus.write_read(0x27, &[0x01], &mut id).map_or_else(code, |()| u32::from(id[0])));
    let mut raw = [0u8; 4];
    r.set(8, bus.write_read(0x27, &[0x02], &mut raw).map_or_else(code, |()| u32::from_le_bytes(raw)));
    let found_tuner = maps[0] & (1 << 0x10) != 0;
    let found_acc = maps[1] & (1 << (0x27 - 32)) != 0;
    let mut msg = log::LineBuf::new();
    msg.push_str("tuner=").push_u32(u32::from(found_tuner)).push_str(" acc=").push_u32(u32::from(found_acc));
    msg.push_str(" acc_id=").push_hex(r.get(7));
    let ok = found_tuner && found_acc && !stuck;
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
