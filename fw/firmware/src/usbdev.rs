//! USB: the developer channel on EP0 in every personality (USB-4):
//! developer only, card reader, and the keyboard (USB-5, `crate::kbd`).

use core::sync::atomic::{AtomicU16, Ordering};

use atj2157_hal::{executor, log, time, usb, wdt};
use embassy_time::Timer;
use embassy_usb::{Builder, Config};
use orpheus_proto::devchan::{self as dc, Info};
use orpheus_proto::{logring, mem};
use orpheus_usb::devchan::{DevChannel, Target};

const REATTACH: u16 = 0x10;
static REBOOT: AtomicU16 = AtomicU16::new(0xFFFF);

struct Fw;

impl Target for Fw {
    fn info(&mut self) -> Info {
        Info {
            version: dc::VERSION,
            layout: 0,
            personality: CURRENT.load(Ordering::Relaxed),
            uptime_ms: (time::now_us() / 1000) as u32,
            battery_mv: crate::state::snapshot().millivolts,
            flags: u16::from(usb::high_speed()),
            build_id: *b"orpheus-fw\0\0\0\0\0\0",
            // Stack high-water: free bytes (low half) of the total (high).
            reserved: (atj2157_hal::rt::stack_unused().min(0xFFFF) as u32)
                | ((atj2157_hal::rt::stack_size().min(0xFFFF) as u32) << 16),
        }
    }
    fn read_mem(&mut self, addr: u32, buf: &mut [u8]) -> bool {
        // Word-aligned requests are read as 32-bit words: several peripheral
        // blocks (the DAC at 0xC0180000 among them) read as 0 when accessed
        // by byte.
        if addr.is_multiple_of(4) && buf.len().is_multiple_of(4) {
            for (i, w) in buf.chunks_exact_mut(4).enumerate() {
                // SAFETY: as below, aligned word reads.
                let v = unsafe { core::ptr::read_volatile((addr as usize + 4 * i) as *const u32) };
                w.copy_from_slice(&v.to_le_bytes());
            }
            return true;
        }
        for (i, b) in buf.iter_mut().enumerate() {
            // SAFETY: development-build PEEK: the host names the address;
            // reads of RAM/ROM/registers on this SoC do not fault.
            *b = unsafe { core::ptr::read_volatile((addr as usize + i) as *const u8) };
        }
        true
    }
    fn read_frame(&mut self, offset: u32, buf: &mut [u8]) -> usize {
        // Hold the UI's drawing while a screenshot is read in pieces, so it
        // shows one frame (a scrolling row otherwise mixes two frames).
        let n = buf.len().min((mem::FB_SIZE.saturating_sub(offset)) as usize);
        let until = if offset as usize + n >= mem::FB_SIZE as usize {
            0
        } else {
            (time::now_us() / 1000) as u32 + 300
        };
        crate::ui::SHOT_HOLD_UNTIL_MS.store(until, Ordering::Relaxed);
        self.read_mem(mem::FB_BASE + offset, &mut buf[..n]);
        n
    }
    fn read_log(&mut self, offset: u32, buf: &mut [u8]) -> usize {
        let n = buf.len().min(logring::SIZE.saturating_sub(offset as usize));
        self.read_mem(logring::ALT_BASE + offset, &mut buf[..n]);
        n
    }
    fn stats(&mut self, group: u16, out: &mut [u32]) -> usize {
        if group == 1 {
            // Interrupts per NVIC line.
            let n = out.len().min(atj2157_hal::rt::IRQ_COUNT.len());
            for (o, c) in out[..n].iter_mut().zip(&atj2157_hal::rt::IRQ_COUNT) {
                *o = c.load(Ordering::Relaxed);
            }
            return n;
        }
        out[0] = usb::RESETS.load(Ordering::Relaxed);
        out[1] = usb::SETUPS.load(Ordering::Relaxed);
        out[2] = usb::DMA_ERRORS.load(Ordering::Relaxed);
        out[3] = executor::idle_us();
        let p = atj2157_hal::pager::stats();
        let n = out.len().min(21);
        let v = [
            p.loads,
            p.code_faults,
            p.ro_faults,
            p.evictions,
            (time::now_us() / 1000) as u32,
            crate::ui::LOOPS.load(Ordering::Relaxed),
            crate::ui::BUSY_US.load(Ordering::Relaxed),
            crate::ui::ROWS.load(Ordering::Relaxed),
            crate::ui::SET_WRITES.load(Ordering::Relaxed),
            crate::ui::SET_LAST.load(Ordering::Relaxed),
            crate::ui::T_DRAW.load(Ordering::Relaxed),
            crate::ui::T_FLUSH.load(Ordering::Relaxed),
            crate::audio::T_DECODE.load(Ordering::Relaxed),
            crate::audio::T_FILL.load(Ordering::Relaxed),
            crate::audio::T_SERVE.load(Ordering::Relaxed),
            atj2157_hal::clock::current() as u32,
            atj2157_hal::clock::switches(),
        ];
        out[4..n].copy_from_slice(&v[..n - 4]);
        n
    }
    fn keys(&mut self, events: &[(u8, bool)]) {
        for &(k, down) in events {
            if let Some(key) = orpheus_input::Key::from_index(k) {
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
    fn put(&mut self, data: &[u8]) -> bool {
        crate::audio::put_chunk(data)
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
        if data == b"usb msc" || data == b"usb msc ro" {
            MSC_READ_ONLY.store(data.len() > 7, core::sync::atomic::Ordering::Relaxed);
            MODE.signal(Personality::FileTransfer);
            return true;
        }
        if data == b"usb dev" {
            MODE.signal(Personality::Dev);
            return true;
        }
        if data == b"usb kbd" {
            MODE.signal(Personality::Keyboard);
            return true;
        }
        // USB-5: the HID keyboard (`kbd type <text>`, `kbd status`, …).
        if let Some(rest) = data.strip_prefix(b"kbd ") {
            return crate::kbd::test_command(rest);
        }
        if data == b"hang" {
            // Watchdog test: block the main executor; the watchdog task
            // stops feeding and the watchdog resets the player (~6 s).
            atj2157_hal::log::line(b"test: hang");
            loop {
                core::hint::spin_loop();
            }
        }
        if let Some(rest) = data.strip_prefix(b"sound ") {
            // Test: "sound g0 g1 g2 g3 g4 bass crossfeed" (dB, preset level).
            let mut v = [0i8; 7];
            for (i, w) in rest.split(|&b| b == b' ').filter(|w| !w.is_empty()).take(7).enumerate() {
                let (neg, digits) = match w.split_first() {
                    Some((b'-', d)) => (true, d),
                    _ => (false, w),
                };
                let mut n = 0i8;
                for &d in digits {
                    n = n.saturating_mul(10).saturating_add((d.wrapping_sub(b'0') % 10) as i8);
                }
                v[i] = if neg { -n } else { n };
            }
            crate::media::publish_sound(crate::media::Sound {
                eq: [v[0], v[1], v[2], v[3], v[4]],
                bass: v[5],
                crossfeed: v[6] as u8,
                ..crate::media::Sound::default()
            });
            return true;
        }
        if let Some(l) = data.strip_prefix(b"clock ") {
            // Test: pin the clock level (the governor may move it again).
            use atj2157_hal::clock::{Level, set_level};
            set_level(match l {
                b"96" => Level::Mid,
                b"144" => Level::High,
                b"192" => Level::Turbo,
                b"288" => Level::Max,
                _ => Level::Base,
            });
            return true;
        }
        if data == b"charge pause" || data == b"charge resume" {
            crate::CHARGE_PAUSE.store(data == b"charge pause", Ordering::Relaxed);
            return true;
        }
        if data == b"crc on" || data == b"crc off" {
            crate::audio::CRC_ON.store(data == b"crc on", Ordering::Relaxed);
            return true;
        }
        if data == b"gov on" || data == b"gov off" {
            crate::GOV_ON.store(data == b"gov on", Ordering::Relaxed);
            return true;
        }
        if data == b"dsp on" || data == b"dsp off" {
            crate::audio::DSP_ON.store(data == b"dsp on", Ordering::Relaxed);
            return true;
        }
        if data == b"status" {
            crate::audio::status_reply();
            return true;
        }
        if data == b"stop" {
            crate::media::play(crate::media::PlayReq::Stop);
            crate::audio::request_stop();
            return true;
        }
        if let Some(rest) = data.strip_prefix(b"bt ") {
            return crate::bt::test_command(rest);
        }
        if let Some(rest) = data.strip_prefix(b"fm ") {
            return crate::fm::test_command(rest);
        }
        if let Some(rest) = data.strip_prefix(b"drain ") {
            return crate::drain::test_command(rest);
        }
        // SENS-1: the accelerometer and the step counter; UI-5d: the RTC
        // alarm.
        if let Some(rest) = data.strip_prefix(b"acc ") {
            return crate::sensor::acc_command(rest);
        }
        if data == b"steps status" {
            crate::sensor::steps_status();
            return true;
        }
        if data == b"alarm status" {
            crate::services::alarm_status();
            return true;
        }
        if CARD_WITH_USB.load(Ordering::Relaxed) {
            crate::audio::busy_reply();
            return true;
        }
        if data.starts_with(b"playdir ") {
            // The card task reads commands only between tracks: stop the
            // one playing (and so the queue) first; the new queue starts
            // when the command is served.
            crate::audio::request_stop();
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
/// 1 KiB: the host moves at most 1 KiB per control transfer (stack room, 2026-11-24).
static mut CONTROL_BUF: [u8; 1024] = [0; 1024];
static mut DEVCHAN: DevChannel<Fw> = DevChannel { target: Fw, requests: 0 };
static mut MSC_CLASS: MscClass = MscClass;
static mut HID_STATE: embassy_usb::class::hid::State<'static> = embassy_usb::class::hid::State::new();
static mut KBD_REQUESTS: crate::kbd::Requests = crate::kbd::Requests;
static mut KBD_BUS: crate::kbd::Bus = crate::kbd::Bus;
/// The personality presented (INFO): 0 developer, 1 card reader, 2 keyboard.
pub static CURRENT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
pub const CURRENT_CARD_READER: u8 = 1;

/// Personality requests (from the UI's USB dialog or the TEST channel).
/// Latest request wins; sending never fails.
pub static MODE: embassy_sync::signal::Signal<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    Personality,
> = embassy_sync::signal::Signal::new();

/// The card is with the USB task (card reader); card commands answer busy.
pub static CARD_WITH_USB: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

/// Present the card write-protected (automated tests: a desktop automount
/// must never write to the owner's card, CLAUDE.md rule 5).
pub static MSC_READ_ONLY: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(false);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Personality {
    /// Developer channel only.
    Dev,
    /// Card reader (+ developer channel).
    FileTransfer,
    /// HID boot keyboard (+ developer channel): USB-5.
    Keyboard,
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

/// Card as the MSC block device (512-byte blocks).
struct CardDev<'a>(&'a mut yoton_bsp::card::Card);
impl orpheus_usb::msc::BlockDevice for CardDev<'_> {
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
    config.manufacturer = Some("orpheus");
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
    let r = embassy_futures::select::select3(dev.run(), supervisor(), MODE.wait()).await;
    usb::set_attached(false);
    match r {
        embassy_futures::select::Either3::Third(p) => p,
        _ => Personality::Dev,
    }
}

async fn run_msc() -> Personality {
    // Get the card from the audio task.
    crate::media::REQS.send(crate::media::Req::Release).await;
    CARD_WITH_USB.store(true, Ordering::Relaxed);
    let mut card = crate::audio::CARD_OUT.receive().await;
    atj2157_hal::log::line(b"usb: card reader on");
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
        let unit = orpheus_usb::msc::scsi::Unit {
            kind: orpheus_usb::msc::scsi::DeviceKind::Disk,
            removable: true,
            block_size: 512,
            blocks,
            read_only: MSC_READ_ONLY.load(core::sync::atomic::Ordering::Relaxed),
            present: true,
            vendor: *b"orpheus ",
            product: *b"YM03 microSD    ",
            revision: *b"0001",
        };
        // SAFETY: the card is with this task until `CARD_BACK.send` below,
        // after this scope (and the borrow) ends.
        let buf = unsafe { crate::audio::usb_buffer() };
        let mut bot = orpheus_usb::msc::bot::Bot::new(ep_in, ep_out, CardDev(&mut card), unit, buf);
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
        let r = embassy_futures::select::select3(dev.run(), pump, MODE.wait()).await;
        usb::set_attached(false);
        match r {
            embassy_futures::select::Either3::Second(p) | embassy_futures::select::Either3::Third(p) => p,
            embassy_futures::select::Either3::First(_) => Personality::Dev,
        }
    };
    // A host that still had the card mounted may have stopped in the middle
    // of a transfer: reset the controller and the card before handing it
    // back (`card: remount failed` after `usb msc ro` / `usb dev`,
    // 2026-11-24). Here, where the card already lives in this task's
    // state; in the card task it would add its size to that task's.
    if card.reinit().await.is_err() {
        atj2157_hal::log::line(b"usb: card re-init failed");
    }
    crate::audio::CARD_BACK.send(card).await;
    CARD_WITH_USB.store(false, Ordering::Relaxed);
    atj2157_hal::log::line(b"usb: card returned");
    next
}

/// The keyboard personality: the developer channel's interface and a HID
/// boot keyboard (1 interrupt IN endpoint, 8-byte reports, 4 ms interval:
/// bInterval 4 is 4 ms at full speed and 2^3 microframes = 1 ms at high
/// speed). Back to the developer personality when no computer configures
/// it within `kbd::CONFIGURE_TIMEOUT`.
async fn run_kbd() -> Personality {
    use embassy_usb::class::hid::{Config as HidConfig, HidBootProtocol, HidSubclass, HidWriter};
    let mut b = builder("YM03 keyboard");
    let mut func = b.function(0xFF, 0, 0);
    let mut iface = func.interface();
    let _alt = iface.alt_setting(0xFF, 0, 0, None);
    drop(func);
    // SAFETY: as in `builder`: one device at a time; the previous one
    // (and its borrows of these statics) is gone.
    b.handler(unsafe { &mut *(&raw mut KBD_BUS) });
    let config = HidConfig {
        report_descriptor: &orpheus_hid::descriptor::BOOT_KEYBOARD,
        // SAFETY: as above.
        request_handler: Some(unsafe { &mut *(&raw mut KBD_REQUESTS) }),
        poll_ms: 4,
        max_packet_size: 8,
        hid_subclass: HidSubclass::Boot,
        hid_boot_protocol: HidBootProtocol::Keyboard,
    };
    // SAFETY: as above.
    let mut w = HidWriter::<_, 8>::new(&mut b, unsafe { &mut *(&raw mut HID_STATE) }, config);
    let mut dev = b.build();
    let pump = async {
        crate::kbd::pump(&mut w).await;
        Personality::Dev
    };
    let r = embassy_futures::select::select4(dev.run(), supervisor(), MODE.wait(), pump).await;
    usb::set_attached(false);
    crate::kbd::ended();
    match r {
        embassy_futures::select::Either4::Third(p) | embassy_futures::select::Either4::Fourth(p) => p,
        _ => Personality::Dev,
    }
}

#[embassy_executor::task]
pub async fn usb() {
    let mut mode = Personality::Dev;
    loop {
        CURRENT.store(
            match mode {
                Personality::Dev => 0,
                Personality::FileTransfer => 1,
                Personality::Keyboard => 2,
            },
            Ordering::Relaxed,
        );
        mode = match mode {
            Personality::Dev => run_dev().await,
            Personality::FileTransfer => run_msc().await,
            Personality::Keyboard => run_kbd().await,
        };
        usb::shutdown();
        Timer::after_millis(200).await;
    }
}
