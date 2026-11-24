//! USB: the developer channel on EP0 (personalities come with USB-4).

use core::sync::atomic::{AtomicU16, Ordering};

use atj2157_hal::{executor, log, time, usb, wdt};
use embassy_time::Timer;
use embassy_usb::{Builder, Config};
use static_cell::{ConstStaticCell, StaticCell};
use yoton_proto::devchan::{self as dc, Info};
use yoton_proto::{logring, mem};
use yoton_usb::devchan::{DevChannel, Target};

const REATTACH: u16 = 0x10;
static REBOOT: AtomicU16 = AtomicU16::new(0xFFFF);

struct Fw;

impl Target for Fw {
    fn info(&mut self) -> Info {
        Info {
            version: dc::VERSION,
            layout: 0,
            personality: 0,
            uptime_ms: (time::now_us() / 1000) as u32,
            battery_mv: crate::state::snapshot().millivolts,
            flags: u16::from(usb::high_speed()),
            build_id: *b"yotonrs-fw\0\0\0\0\0\0",
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
        for &(k, down) in events {
            if let Some(key) = yoton_input::Key::from_index(k) {
                let _ = crate::KEYS.try_send((key, down));
            }
        }
    }
    fn request_reboot(&mut self, target: u16) {
        REBOOT.store(target, Ordering::Release);
    }
    fn development(&self) -> bool {
        true
    }
    fn set_time(&mut self, t: [u8; 7]) -> bool {
        atj2157_hal::rtc_clock::set(atj2157_hal::rtc_clock::DateTime {
            year: u16::from_le_bytes([t[0], t[1]]),
            month: t[2],
            day: t[3],
            hour: t[4],
            minute: t[5],
            second: t[6],
        })
    }
    fn time(&mut self) -> Option<[u8; 7]> {
        if !atj2157_hal::rtc_clock::running() {
            return None;
        }
        let n = atj2157_hal::rtc_clock::now();
        let y = n.year.to_le_bytes();
        Some([y[0], y[1], n.month, n.day, n.hour, n.minute, n.second])
    }
    fn test_command(&mut self, data: &[u8]) -> bool {
        if data == b"stop" {
            crate::audio::request_stop();
            return true;
        }
        let mut c = crate::audio::Cmd {
            len: data.len().min(crate::audio::CMD_LEN),
            text: [0; crate::audio::CMD_LEN],
        };
        c.text[..c.len].copy_from_slice(&data[..c.len]);
        crate::audio::CMDS.try_send(c).is_ok()
    }
    fn test_reply(&mut self, buf: &mut [u8]) -> usize {
        critical_section::with(|cs| {
            let r = crate::audio::REPLY.borrow(cs).borrow();
            let n = r.1.min(buf.len());
            buf[..n].copy_from_slice(&r.0[..n]);
            n
        })
    }
}

pub async fn supervisor() {
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

static CONFIG_DESC: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
static BOS_DESC: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
static MSOS_DESC: ConstStaticCell<[u8; 256]> = ConstStaticCell::new([0; 256]);
static CONTROL_BUF: ConstStaticCell<[u8; 4096]> = ConstStaticCell::new([0; 4096]);
static DEVCHAN: StaticCell<DevChannel<Fw>> = StaticCell::new();

#[embassy_executor::task]
pub async fn usb() {
    let driver = usb::Driver::new();
    let mut config = Config::new(dc::DEV_VID, dc::DEV_PID);
    config.manufacturer = Some("yotonrs");
    config.product = Some("YM03");
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
    let mut func = builder.function(0xFF, 0, 0);
    let mut iface = func.interface();
    let _alt = iface.alt_setting(0xFF, 0, 0, None);
    drop(func);
    let mut dev = builder.build();
    embassy_futures::join::join(dev.run(), supervisor()).await;
}
