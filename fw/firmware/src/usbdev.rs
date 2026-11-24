//! USB: the developer channel on EP0 (personalities come with USB-4).

use core::sync::atomic::{AtomicU16, Ordering};

use atj2157_hal::{executor, log, time, usb, wdt};
use embassy_time::Timer;
use embassy_usb::{Builder, Config};
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
        if data == b"usb msc" {
            return MODE.try_send(Personality::FileTransfer).is_ok();
        }
        if data == b"usb dev" {
            return MODE.try_send(Personality::Dev).is_ok();
        }
        if data == b"status" {
            crate::audio::status_reply();
            return true;
        }
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

static mut CONFIG_DESC: [u8; 256] = [0; 256];
static mut BOS_DESC: [u8; 256] = [0; 256];
static mut MSOS_DESC: [u8; 256] = [0; 256];
static mut CONTROL_BUF: [u8; 4096] = [0; 4096];
static mut DEVCHAN: DevChannel<Fw> = DevChannel { target: Fw, requests: 0 };
static mut MSC_CLASS: MscClass = MscClass;

/// Personality requests (from the UI's USB dialog or the TEST channel).
pub static MODE: embassy_sync::channel::Channel<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    Personality,
    2,
> = embassy_sync::channel::Channel::new();

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Personality {
    /// Developer channel only.
    Dev,
    /// Card reader (+ developer channel).
    FileTransfer,
}

/// Class requests of the mass-storage interface: GET MAX LUN = 0, reset.
struct MscClass;
impl embassy_usb::Handler for MscClass {
    fn control_in<'a>(
        &'a mut self,
        req: embassy_usb::control::Request,
        buf: &'a mut [u8],
    ) -> Option<embassy_usb::control::InResponse<'a>> {
        use embassy_usb::control::{InResponse, Recipient, RequestType};
        if req.request_type == RequestType::Class
            && req.recipient == Recipient::Interface
            && req.request == 0xFE
        {
            buf[0] = 0;
            return Some(InResponse::Accepted(&buf[..1]));
        }
        None
    }
    fn control_out(
        &mut self,
        req: embassy_usb::control::Request,
        _data: &[u8],
    ) -> Option<embassy_usb::control::OutResponse> {
        use embassy_usb::control::{OutResponse, Recipient, RequestType};
        if req.request_type == RequestType::Class
            && req.recipient == Recipient::Interface
            && req.request == 0xFF
        {
            return Some(OutResponse::Accepted);
        }
        None
    }
}

#[repr(C, align(4))]
struct MscBuf([u8; 16384]);
static mut MSC_BUF: MscBuf = MscBuf([0; 16384]);

/// Card as the MSC block device (512-byte blocks).
struct CardDev<'a>(&'a mut yoton_bsp::card::Card);
impl yoton_usb::msc::BlockDevice for CardDev<'_> {
    type Error = ();
    async fn read(&mut self, lba: u64, buf: &mut [u8]) -> Result<(), ()> {
        let (s, _) = buf.as_chunks_mut::<512>();
        yoton_bsp::card::BlockDevice::read(self.0, lba as u32, s).await.map_err(|_| ())
    }
    async fn write(&mut self, lba: u64, buf: &[u8]) -> Result<(), ()> {
        let (s, _) = buf.as_chunks::<512>();
        yoton_bsp::card::BlockDevice::write(self.0, lba as u32, s).await.map_err(|_| ())
    }
}

fn builder(product: &'static str) -> Builder<'static, usb::Driver> {
    let driver = usb::Driver::new();
    let mut config = Config::new(dc::DEV_VID, dc::DEV_PID);
    config.manufacturer = Some("yotonrs");
    config.product = Some(product);
    config.serial_number = Some("0001");
    config.max_power = 100;
    config.max_packet_size_0 = 64;
    // SAFETY: one UsbDevice exists at a time; the previous one is dropped
    // before the next builder borrows these buffers again.
    let mut b = unsafe {
        Builder::new(
            driver,
            config,
            &mut *(&raw mut CONFIG_DESC),
            &mut *(&raw mut BOS_DESC),
            &mut *(&raw mut MSOS_DESC),
            &mut *(&raw mut CONTROL_BUF),
        )
    };
    // SAFETY: as above.
    b.handler(unsafe { &mut *(&raw mut DEVCHAN) });
    b
}

async fn run_dev() -> Personality {
    let mut b = builder("YM03");
    let mut func = b.function(0xFF, 0, 0);
    let mut iface = func.interface();
    let _alt = iface.alt_setting(0xFF, 0, 0, None);
    drop(func);
    let mut dev = b.build();
    let r = embassy_futures::select::select3(dev.run(), supervisor(), MODE.receive()).await;
    usb::set_attached(false);
    match r {
        embassy_futures::select::Either3::Third(p) => p,
        _ => Personality::Dev,
    }
}

async fn run_msc() -> Personality {
    // Get the card from the audio task.
    let mut c = crate::audio::Cmd { len: 7, text: [0; crate::audio::CMD_LEN] };
    c.text[..7].copy_from_slice(b"release");
    crate::audio::CMDS.send(c).await;
    let mut card = crate::audio::CARD_OUT.receive().await;
    let blocks = u64::from(yoton_bsp::card::BlockDevice::sector_count(&card));
    let next = {
        let mut b = builder("YM03 card reader");
        // SAFETY: as in `builder`.
        b.handler(unsafe { &mut *(&raw mut MSC_CLASS) });
        let mut func = b.function(0x08, 0x06, 0x50);
        let mut iface = func.interface();
        let mut alt = iface.alt_setting(0x08, 0x06, 0x50, None);
        let ep_in = alt.endpoint_bulk_in(None, 512);
        let ep_out = alt.endpoint_bulk_out(None, 512);
        drop(func);
        let mut dev = b.build();
        let unit = yoton_usb::msc::scsi::Unit {
            kind: yoton_usb::msc::scsi::DeviceKind::Disk,
            removable: true,
            block_size: 512,
            blocks,
            read_only: false,
            present: true,
            vendor: *b"yotonrs ",
            product: *b"YM03 microSD    ",
            revision: *b"0001",
        };
        // SAFETY: the buffer is used only by this BOT instance.
        let buf = unsafe { &mut (*(&raw mut MSC_BUF)).0 };
        let mut bot = yoton_usb::msc::bot::Bot::new(ep_in, ep_out, CardDev(&mut card), unit, buf);
        let pump = async {
            loop {
                embassy_usb_driver::Endpoint::wait_enabled(&mut bot.ep_in).await;
                while bot.serve_one().await.is_ok() {
                    if bot.scsi.events.eject {
                        return Personality::Dev;
                    }
                }
            }
        };
        let r = embassy_futures::select::select3(dev.run(), pump, MODE.receive()).await;
        usb::set_attached(false);
        match r {
            embassy_futures::select::Either3::Second(p) | embassy_futures::select::Either3::Third(p) => p,
            embassy_futures::select::Either3::First(_) => Personality::Dev,
        }
    };
    crate::audio::CARD_BACK.send(card).await;
    next
}

#[embassy_executor::task]
pub async fn usb() {
    let mut mode = Personality::Dev;
    loop {
        mode = match mode {
            Personality::Dev => run_dev().await,
            Personality::FileTransfer => run_msc().await,
        };
        usb::shutdown();
        Timer::after_millis(200).await;
    }
}
