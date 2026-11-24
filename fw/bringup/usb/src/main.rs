//! `usb` — WP-USB-1 bring-up image: the controller driver under
//! `embassy-usb`, a vendor-only device (`1209:0001`) with the developer
//! channel on EP0. Driven live by `cargo xtask dev usbtest`.
//!
//! Extra REBOOT targets for tests: `0x10` = detach 300 ms and re-attach.

#![no_std]
#![no_main]

use core::sync::atomic::{AtomicU16, AtomicU32, Ordering};

use atj2157_hal::{self as hal, executor, log, pac, time, usb, wdt};
use embassy_executor::Spawner;
use embassy_time::Timer;
use embassy_usb::{Builder, Config};
use orpheus_proto::devchan::{self as dc, Info};
use orpheus_proto::{logring, mem};
use orpheus_usb::devchan::{DevChannel, Target};
use static_cell::{ConstStaticCell, StaticCell};

const REATTACH: u16 = 0x10;
static REBOOT: AtomicU16 = AtomicU16::new(0xFFFF);
static KEYS: AtomicU32 = AtomicU32::new(0);

struct Fw;

impl Target for Fw {
    fn info(&mut self) -> Info {
        Info {
            version: dc::VERSION,
            layout: 0,
            personality: 0,
            uptime_ms: (time::now_us() / 1000) as u32,
            battery_mv: 0,
            flags: u16::from(usb::high_speed()),
            build_id: *b"bringup-usb\0\0\0\0\0",
            reserved: usb::SETUPS.load(Ordering::Relaxed),
        }
    }
    fn read_mem(&mut self, addr: u32, buf: &mut [u8]) -> bool {
        for (i, b) in buf.iter_mut().enumerate() {
            // SAFETY: development-build PEEK: the host names the address;
            // reads of RAM/ROM/registers on this SoC do not fault.
            *b = unsafe { core::ptr::read_volatile((addr as usize + i) as *const u8) };
        }
        true
    }
    fn read_frame(&mut self, offset: u32, buf: &mut [u8]) -> usize {
        let n = buf.len().min((mem::FB_SIZE.saturating_sub(offset)) as usize);
        self.read_mem(mem::FB_BASE + offset, &mut buf[..n]);
        n
    }
    fn read_log(&mut self, offset: u32, buf: &mut [u8]) -> usize {
        let n = buf.len().min(logring::SIZE.saturating_sub(offset as usize));
        self.read_mem(logring::ALT_BASE + offset, &mut buf[..n]);
        n
    }
    fn stats(&mut self, _group: u16, out: &mut [u32]) -> usize {
        out[0] = usb::RESETS.load(Ordering::Relaxed);
        out[1] = usb::SETUPS.load(Ordering::Relaxed);
        out[2] = usb::DMA_ERRORS.load(Ordering::Relaxed);
        out[3] = executor::idle_us();
        4
    }
    fn keys(&mut self, events: &[(u8, bool)]) {
        KEYS.fetch_add(events.len() as u32, Ordering::Relaxed);
    }
    fn request_reboot(&mut self, target: u16) {
        REBOOT.store(target, Ordering::Release);
    }
    fn development(&self) -> bool {
        true
    }
}

type UsbDev = embassy_usb::UsbDevice<'static, usb::Driver>;

#[embassy_executor::task]
async fn usb_task(mut dev: UsbDev) -> ! {
    dev.run().await
}

#[embassy_executor::task]
async fn supervisor() {
    loop {
        Timer::after_millis(20).await;
        let r = REBOOT.swap(0xFFFF, Ordering::AcqRel);
        match r {
            0xFFFF => {}
            REATTACH => {
                // Let the status stage of the request finish first.
                Timer::after_millis(50).await;
                usb::set_attached(false);
                Timer::after_millis(300).await;
                usb::set_attached(true);
            }
            t => {
                Timer::after_millis(50).await;
                log::line(b"usb: reboot requested");
                wdt::reboot(if t == dc::REBOOT_ADFU { wdt::Target::Adfu } else { wdt::Target::Normal });
            }
        }
    }
}

static MAIN: StaticCell<executor::MainExecutor> = StaticCell::new();
static CONFIG_DESC: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
static BOS_DESC: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
static MSOS_DESC: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
static CONTROL_BUF: ConstStaticCell<[u8; 4096]> = ConstStaticCell::new([0; 4096]);
static DEVCHAN: StaticCell<DevChannel<Fw>> = StaticCell::new();

#[unsafe(no_mangle)]
extern "C" fn main() -> ! {
    log::init();
    log::line(b"usb: start");
    hal::init();
    wdt::start(pac::rtc::wd::PERIOD_5S6);
    wdt::feed_from_idle(true);

    let driver = usb::Driver::new();
    let mut config = Config::new(dc::DEV_VID, dc::DEV_PID);
    config.manufacturer = Some("orpheus");
    config.product = Some("YM03 bring-up (usb)");
    config.serial_number = Some("0001");
    config.max_power = 100;
    config.max_packet_size_0 = 64;
    let mut builder = Builder::new(
        driver,
        config,
        CONFIG_DESC.take(),
        BOS_DESC.take(),
        MSOS_DESC.take(),
        CONTROL_BUF.take(),
    );
    builder.handler(DEVCHAN.init(DevChannel::new(Fw)));
    // An empty vendor interface so the configuration is not empty.
    let mut func = builder.function(0xFF, 0, 0);
    let mut iface = func.interface();
    let _alt = iface.alt_setting(0xFF, 0, 0, None);
    drop(func);
    let dev = builder.build();

    MAIN.init(executor::MainExecutor::new()).run(|s: Spawner| {
        s.spawn(usb_task(dev).unwrap());
        s.spawn(supervisor().unwrap());
    })
}

#[panic_handler]
fn panic(info: &core::panic::PanicInfo) -> ! {
    hal::panic(info)
}
