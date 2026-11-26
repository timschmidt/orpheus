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
            // The running slot's build id (UPD-1: the host checks that an
            // update started).
            build_id: crate::update::running_build().unwrap_or(*b"orpheus-fw\0\0\0\0\0\0"),
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
        let n = out.len().min(dc::usb::WORDS);
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
            // VER-12 (soak oracles): the DAC headroom low-water since the
            // last read (frames; 0x7FFFFFFF none), failed card operations,
            // failed pager line loads.
            crate::audio::take_head_min(),
            yoton_bsp::card::error_count(),
            atj2157_hal::pager::LOAD_ERRORS.load(Ordering::Relaxed),
            // USB-W1: the controller's counters and the liveness state
            // (`orpheus_proto::devchan::usb`).
            usb::STALLS.load(Ordering::Relaxed),
            usb::ABORTS.load(Ordering::Relaxed),
            usb::REINITS.load(Ordering::Relaxed),
            usb::last_error(),
            usb::INITS.load(Ordering::Relaxed),
            usb::COMABTS.load(Ordering::Relaxed),
            LIVE_STATE.load(Ordering::Relaxed),
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
        // USB-6: `usb image <path> [cd|disk|fixed] [ro|rw]`, `usb image status`.
        if let Some(rest) = data.strip_prefix(b"usb image ") {
            if crate::drive::test_command(rest) {
                MODE.signal(Personality::Image);
            }
            return true;
        }
        if data == b"usb kbd" {
            MODE.signal(Personality::Keyboard);
            return true;
        }
        // USB-W1: `usb delay setup|data|status <ms>` holds the next control
        // transfer at that stage (a late USB task; `xtask dev usb-cancel`);
        // `usb recover` asks for a recovery as the liveness check would.
        if let Some(rest) = data.strip_prefix(b"usb delay ") {
            let (stage, ms) = rest.split_at(rest.iter().position(|&b| b == b' ').unwrap_or(rest.len()));
            let stage = match stage {
                b"setup" => usb::Stage::Setup,
                b"data" => usb::Stage::Data,
                b"status" => usb::Stage::Status,
                _ => return false,
            };
            let ms = ms
                .iter()
                .filter(|d| d.is_ascii_digit())
                .fold(0u32, |n, d| n.saturating_mul(10).saturating_add(u32::from(d - b'0')));
            usb::delay_next(stage, ms);
            return true;
        }
        if data == b"usb recover" {
            request_recovery(dc::usb::ERR_RECOVER_SETUP);
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
        // T-6: stage 2's record of this start, and the fallback test.
        if data == b"boot status" {
            boot_status_reply();
            return true;
        }
        if data == b"boot crash-test" {
            // The running slot fails its next BOOT_COUNT_LIMIT starts (from
            // the next reboot on; nothing written to NOR). Stage 2 then
            // starts the next slot, whose start disarms the test.
            let slot = atj2157_hal::persist::boot_info().and_then(orpheus_proto::mailbox::BootInfo::slot);
            if let Some(s) = slot {
                atj2157_hal::persist::arm_crash_test(s, orpheus_proto::mailbox::BOOT_COUNT_LIMIT);
            }
            crate::audio::reply(|w| {
                match slot {
                    Some(s) => w
                        .s(b"crash test armed: slot ")
                        .s(orpheus_proto::boot::slot_name(s).as_bytes())
                        .s(b" fails its next ")
                        .u(u64::from(orpheus_proto::mailbox::BOOT_COUNT_LIMIT))
                        .s(b" starts; reboot to begin"),
                    None => w.s(b"crash test refused: not started from a slot by stage 2"),
                };
            });
            return true;
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
        // WP-T-8: the PC sampler and the fault log.
        if let Some(rest) = data.strip_prefix(b"prof ") {
            prof_command(rest);
            return true;
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
        // AU-13: the recorder (`start` goes to the card task, which must
        // have the card).
        if let Some(rest) = data.strip_prefix(b"rec ") {
            if rest.starts_with(b"start") && CARD_WITH_USB.load(Ordering::Relaxed) {
                crate::audio::busy_reply();
                return true;
            }
            return crate::rec::test_command(rest);
        }
        if data == b"alarm status" {
            crate::services::alarm_status();
            return true;
        }
        // VER-7: the UI's top screen for the on-device suite and fuzzer.
        if data == b"ui top" {
            crate::ui::top_reply();
            return true;
        }
        if data == b"ui test-look on" || data == b"ui test-look off" {
            crate::ui::set_test_look(data.ends_with(b"on"));
            return true;
        }
        // VER-12: the long soaks (session-only settings, screen off).
        if let Some(rest) = data.strip_prefix(b"settings ") {
            return crate::ui::settings_command(rest);
        }
        if data == b"ui screen-off" {
            crate::ui::request_screen_off();
            return true;
        }
        if CARD_WITH_USB.load(Ordering::Relaxed) {
            crate::audio::busy_reply();
            return true;
        }
        if orpheus_proto::devchan::stops_track_first(data) {
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
    fn provision(&mut self, uri: &[u8]) -> bool {
        crate::vault::provision(uri)
    }
    fn update(&mut self, op: u16, index: u16, data: &[u8]) -> bool {
        crate::update::usb_out(op, index, data)
    }
    fn update_status(&mut self, buf: &mut [u8]) -> usize {
        crate::update::usb_status(buf)
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

/// `prof pc on|burst [PERIOD_US]`, `prof faults on|burst`, `prof off` (also
/// `prof pc off`, `prof faults off`), `prof status`. The reply is always
/// the status line (`orpheus_proto::prof` describes the block the host
/// reads afterwards): `prof: pc on n=12/500 seen=96 shift=3 period=1000us
/// t=812ms` / `prof: refused (running)` / `prof: off`.
fn prof_command(cmd: &[u8]) {
    use atj2157_hal::prof;
    use orpheus_proto::prof as p;
    let mut words = cmd.split(|&b| b == b' ').filter(|w| !w.is_empty());
    let what = words.next().unwrap_or(b"");
    let how = words.next().unwrap_or(b"");
    let num = words.next().map(|d| {
        d.iter()
            .filter(|c| c.is_ascii_digit())
            .fold(0u32, |n, c| n.saturating_mul(10).saturating_add(u32::from(c - b'0')))
    });
    let ok = match (what, how) {
        (b"pc", b"on" | b"burst") => prof::pc_start(num.unwrap_or(1000), how == b"burst"),
        (b"faults", b"on" | b"burst") => prof::faults_start(how == b"burst"),
        (b"off" | b"stop", _) | (b"pc" | b"faults", b"off") => {
            prof::stop();
            true
        }
        _ => true,
    };
    let h = prof::header();
    crate::audio::reply(|w| {
        w.s(b"prof: ");
        if !ok {
            w.s(b"refused (running) ");
        }
        let mode = h[p::W_MODE];
        match mode & p::KIND_MASK {
            p::KIND_PC => w.s(b"pc"),
            p::KIND_FAULTS => w.s(b"faults"),
            _ => {
                w.s(b"off");
                return;
            }
        };
        w.s(if mode & p::RUNNING != 0 && prof::busy() { b" on" } else { b" stopped" });
        if mode & p::BURST != 0 {
            w.s(b" burst");
        }
        if mode & p::FULL != 0 {
            w.s(b" full");
        }
        let end = if mode & p::RUNNING != 0 { atj2157_hal::time::now_us_raw() } else { h[p::W_STOP] };
        w.s(b" n=")
            .u(u64::from(h[p::W_COUNT]))
            .s(b"/")
            .u(u64::from(h[p::W_CAP]))
            .s(b" seen=")
            .u(u64::from(h[p::W_SEEN]))
            .s(b" shift=")
            .u(u64::from(h[p::W_SHIFT]))
            .s(b" period=")
            .u(u64::from(h[p::W_PERIOD]))
            .s(b"us t=")
            .u(u64::from(end.wrapping_sub(h[p::W_START]) / 1000))
            .s(b"ms at 0x")
            .h(p::BASE);
    });
}

/// `boot status`: `slot B, start 1, after A failed` (or `slot A, start 1`;
/// `not started by stage 2`), and whether a crash test is armed.
fn boot_status_reply() {
    use orpheus_proto::boot::slot_name;
    let info = atj2157_hal::persist::boot_info();
    let m = atj2157_hal::persist::mailbox();
    crate::audio::reply(|w| {
        match info.and_then(|i| i.slot().map(|s| (i, s))) {
            Some((i, s)) => {
                w.s(b"slot ").s(slot_name(s).as_bytes()).s(b", start ").u(u64::from(i.tries()));
                if i.fell_back() {
                    w.s(b", after");
                    for t in 0..3u8 {
                        if i.tripped() & 1 << t != 0 {
                            w.s(b" ").s(slot_name(t).as_bytes());
                        }
                    }
                    w.s(b" failed");
                }
            }
            None => {
                w.s(b"not started by stage 2");
            }
        }
        if orpheus_proto::mailbox::Request::from_word(m.request) == orpheus_proto::mailbox::Request::CrashTest
        {
            w.s(b"; crash test armed for slot ")
                .s(slot_name(m.args[0] as u8).as_bytes())
                .s(b", ")
                .u(u64::from(m.args[1]))
                .s(b" left");
        }
    });
}

pub async fn supervisor() {
    loop {
        Timer::after_millis(20).await;
        // A firmware update over the channel: read-back and slot switch.
        crate::update::poll();
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
                if t == dc::REBOOT_NORMAL {
                    // Through the UI's restart (saves the place, the game,
                    // the recording); rebooted below if it has not happened
                    // within 3 s.
                    crate::ui::request_restart();
                    Timer::after_millis(3000).await;
                }
                // RAM survives a reboot into the ROM's USB mode: no
                // authenticator key may be left in it (06 §5.2).
                crate::vault::wipe_secrets();
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
/// The personality presented (INFO): 0 developer, 1 card reader, 2
/// keyboard, 3 disk image.
pub static CURRENT: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);
pub const CURRENT_CARD_READER: u8 = 1;

/// Personality requests (from the UI's USB dialog or the TEST channel).
/// Latest request wins; sending never fails.
pub static MODE: embassy_sync::signal::Signal<
    embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex,
    Personality,
> = embassy_sync::signal::Signal::new();

/// A recovery (USB-W1): the running personality ends, the controller is
/// shut down and the same personality starts again on a fresh controller
/// (soft detach, controller reset, re-enumeration). Carries the reason code.
static REINIT: embassy_sync::signal::Signal<embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex, u8> =
    embassy_sync::signal::Signal::new();

/// The liveness state word for `STATS` (`orpheus_proto::devchan::usb::W_STATE`).
static LIVE_STATE: core::sync::atomic::AtomicU32 = core::sync::atomic::AtomicU32::new(0);

fn request_recovery(code: u8) {
    usb::note_recovery(code);
    let mut l = log::LineBuf::new();
    l.push_str("usb: recovery (").push_str(dc::usb::error_name(code)).push_str("), setups ");
    l.push_u32(usb::SETUPS.load(Ordering::Relaxed)).push_str(" resets ");
    l.push_u32(usb::RESETS.load(Ordering::Relaxed)).push_str(" aborts ");
    l.push_u32(usb::ABORTS.load(Ordering::Relaxed)).push_str(" last ");
    l.push_str(dc::usb::error_name(usb::last_error() as u8));
    l.emit();
    REINIT.signal(code);
}

/// The USB liveness check, once a second from the `watchdog` task (which
/// runs whatever the USB task does): asks the USB task for a recovery when
/// the host goes unanswered, and holds the watchdog (`wdt::USB_WEDGED`)
/// when recoveries cannot help (`orpheus_usb::liveness`).
pub fn liveness_tick(l: &mut orpheus_usb::liveness::Liveness) {
    use orpheus_usb::liveness::{Sample, Verdict};
    let s = Sample {
        now_ms: (time::now_us() / 1000) as u32,
        vbus: usb::vbus_present(),
        setups: usb::SETUPS.load(Ordering::Relaxed),
        resets: usb::RESETS.load(Ordering::Relaxed),
        setup_waiting: usb::setup_waiting(),
        inits: usb::INITS.load(Ordering::Relaxed),
    };
    let was = wdt::USB_WEDGED.load(Ordering::Relaxed);
    let v = l.step(s);
    match v {
        Verdict::Ok => {}
        Verdict::Reinit(r) => request_recovery(r.code()),
        Verdict::Wedged(w) => {
            if !was {
                usb::note_error(w.code());
                let mut b = log::LineBuf::new();
                b.push_str("usb: wedged (").push_str(dc::usb::error_name(w.code()));
                b.push_str("): watchdog not fed");
                b.emit();
            }
        }
    }
    let wedged = matches!(v, Verdict::Wedged(_));
    wdt::USB_WEDGED.store(wedged, Ordering::Relaxed);
    LIVE_STATE
        .store(dc::usb::Counters::state_word(s.vbus, s.setup_waiting, wedged, l.failed()), Ordering::Relaxed);
}

/// A personality switch (`MODE`) or a recovery (`REINIT`: the same
/// personality `current` again).
async fn next_mode(current: Personality) -> Personality {
    match embassy_futures::select::select(MODE.wait(), REINIT.wait()).await {
        embassy_futures::select::Either::First(p) => p,
        embassy_futures::select::Either::Second(_) => current,
    }
}

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
    /// An image file as a disk or CD-ROM (+ developer channel): USB-6,
    /// `crate::drive`.
    Image,
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
    // A recovery asked for before this fresh controller is already done.
    REINIT.reset();
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
    // Settings › USB › Developer channel (VER-13): off, the device answers
    // no developer request. On by default, and always while a host test
    // or a test session runs (`ui::dev_channel_held`).
    if dev_channel() {
        // SAFETY: as above.
        b.handler(unsafe { &mut *(&raw mut DEVCHAN) });
    }
    b
}

/// The developer channel is offered (VER-13; `ui::DEV_CHANNEL`, published
/// by the UI from the setting).
fn dev_channel() -> bool {
    crate::ui::DEV_CHANNEL.load(Ordering::Relaxed)
}

async fn run_dev() -> Personality {
    let mut b = builder("YM03");
    // The developer interface with WebUSB and MS OS 2.0 descriptors (WEB-3):
    // the browser installer opens it without a driver. With the developer
    // channel off the device has no interface (charging only; the host
    // still enumerates it, so the USB liveness check sees it answered).
    if dev_channel() {
        orpheus_usb::webusb::dev_interface(&mut b, true);
    }
    let mut dev = b.build();
    let r = embassy_futures::select::select3(dev.run(), supervisor(), next_mode(Personality::Dev)).await;
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
        let r = embassy_futures::select::select3(dev.run(), pump, next_mode(Personality::FileTransfer)).await;
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

/// USB Drive (USB-6): the card task maps the wanted image (`crate::drive`)
/// and hands the card over as for the card reader; the image is served
/// through its extent table, then the card goes back re-initialised.
async fn run_image() -> Personality {
    use orpheus_apps::services::DrivePhase;
    use orpheus_usb::msc::extent::{ExtentTable, ImageDevice};
    // No mounted card: the card task would never answer. (Straight after
    // the card reader the card is being mounted again: wait for it.)
    for _ in 0..30 {
        if crate::state::CARD_READY.load(Ordering::Relaxed) {
            break;
        }
        Timer::after_millis(100).await;
    }
    if !crate::state::CARD_READY.load(Ordering::Relaxed) {
        crate::drive::update(|i| {
            i.phase = DrivePhase::Failed;
            i.refusal = Some(orpheus_apps::services::DriveRefusal::Io);
        });
        return Personality::Dev;
    }
    CARD_WITH_USB.store(true, Ordering::Relaxed);
    crate::drive::MAPPED.reset();
    crate::media::REQS.send(crate::media::Req::Image).await;
    let mapped = match crate::drive::MAPPED.wait().await {
        Ok(m) => m,
        Err(r) => {
            CARD_WITH_USB.store(false, Ordering::Relaxed);
            crate::drive::update(|i| {
                i.phase = DrivePhase::Failed;
                i.refusal = Some(r);
            });
            atj2157_hal::log::line(b"usb: image refused");
            return Personality::Dev;
        }
    };
    let mut card = crate::audio::CARD_OUT.receive().await;
    let e = mapped.export;
    crate::drive::update(|i| {
        i.phase = DrivePhase::Attached;
        i.medium = e.medium;
        i.read_only = e.read_only;
        i.size = e.bytes;
        i.runs = mapped.runs as u16;
    });
    atj2157_hal::log::line(b"usb: image on");
    let next = {
        let mut b = builder("YM03 USB Drive");
        // SAFETY: as in `builder`.
        b.handler(unsafe { &mut *(&raw mut MSC_CLASS) });
        let mut func = b.function(0x08, 0x06, 0x50);
        let mut iface = func.interface();
        let mut alt = iface.alt_setting(0x08, 0x06, 0x50, None);
        let ep_in = alt.endpoint_bulk_in(None, 512);
        let ep_out = alt.endpoint_bulk_out(None, 512);
        drop(func);
        let mut dev = b.build();
        // SAFETY: the card is with this task until `CARD_BACK.send` below,
        // after this scope (and these borrows) end; the card task filled
        // the extents before handing the card over.
        let (buf, area) = unsafe { (crate::audio::usb_buffer(), crate::audio::usb_extents()) };
        let table = ExtentTable::from_raw(crate::drive::as_extents(area), mapped.runs);
        let image =
            table.and_then(|t| ImageDevice::new(CardDev(&mut card), t, e.bytes, e.medium.block_size()).ok());
        match image {
            None => {
                crate::drive::update(|i| {
                    i.phase = DrivePhase::Failed;
                    i.refusal = Some(orpheus_apps::services::DriveRefusal::Io);
                });
                Personality::Dev
            }
            Some(mut image) => {
                image.set_writable(!e.read_only);
                let mut bot = orpheus_usb::msc::bot::Bot::new(ep_in, ep_out, image, e.unit(), buf);
                bot.scsi.medium_changed();
                let pump = async {
                    loop {
                        embassy_usb_driver::Endpoint::wait_enabled(&mut bot.ep_in).await;
                        crate::drive::update(|i| i.host = true);
                        while bot.serve_one().await.is_ok() {
                            let st = bot.stats;
                            crate::drive::update(|i| {
                                i.bytes_read = st.bytes_read;
                                i.bytes_written = st.bytes_written;
                            });
                            if bot.scsi.events.eject {
                                // Ejected on the computer: as the card
                                // reader, the card comes back.
                                return Personality::Dev;
                            }
                        }
                        crate::drive::update(|i| i.host = false);
                    }
                };
                // A recovery ends the drive (the computer has lost it
                // anyway); the card goes back as after an eject.
                let r = embassy_futures::select::select3(dev.run(), pump, next_mode(Personality::Dev)).await;
                usb::set_attached(false);
                match r {
                    embassy_futures::select::Either3::Second(p)
                    | embassy_futures::select::Either3::Third(p) => p,
                    embassy_futures::select::Either3::First(_) => Personality::Dev,
                }
            }
        }
    };
    // As the card reader: reset the controller and the card first.
    if card.reinit().await.is_err() {
        atj2157_hal::log::line(b"usb: card re-init failed");
    }
    crate::audio::CARD_BACK.send(card).await;
    CARD_WITH_USB.store(false, Ordering::Relaxed);
    crate::drive::update(|i| {
        if i.phase != DrivePhase::Failed {
            i.phase = DrivePhase::Off;
        }
        i.host = false;
    });
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
    if dev_channel() {
        orpheus_usb::webusb::dev_interface(&mut b, false);
    }
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
    let r = embassy_futures::select::select4(dev.run(), supervisor(), next_mode(Personality::Keyboard), pump)
        .await;
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
                Personality::Image => 3,
            },
            Ordering::Relaxed,
        );
        mode = match mode {
            Personality::Dev => run_dev().await,
            Personality::FileTransfer => run_msc().await,
            Personality::Keyboard => run_kbd().await,
            Personality::Image => run_image().await,
        };
        usb::shutdown();
        Timer::after_millis(200).await;
    }
}
