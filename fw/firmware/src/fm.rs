//! The FM radio task (WP FM-1; 03 §15, 05 §7.3, fm-i2c-imu.md §2).
//!
//! Runs `orpheus_fm::Radio` (host-tested: start sequence, tune, seek,
//! auto-scan, RDS, volume ramps, stop) over [`HwPort`]: the software I2C
//! master on GPIO24/25 (100 kHz; writes only ever go to the tuner at
//! `0x10`) and the FM bypass into the headphone amplifier
//! (`atj2157_hal::dac::fm_*`, the vendor's steps marked VERBATIM there).
//!
//! * The radio and the UI's radio text live in SRAM3 at [`FM_ADDR`]
//!   ([`FmRam`], 02 §1, after the step counter's state), not in SRAM0,
//!   whose statics come off the stack. The radio is touched only from tasks of the main
//!   (thread-mode) executor, in synchronous closures ([`with`]): this task
//!   polls it, the UI (`services::Fw`) reads its state, RDS text and scan
//!   results and makes requests ([`request`]), and the developer channel
//!   (USB task) does the same. Nothing is copied between them.
//! * The tuner is powered only while the radio plays: at boot one 2-byte
//!   write puts a tuner left running (by a reset of the SoC alone) into
//!   standby; stopping the radio ramps the amplifier gain down, switches
//!   the bypass off, restores the music gain (7) and puts the tuner into
//!   standby.
//! * Music and radio are exclusive: a starting radio stops the music (the
//!   UI asks first; this task asks again for a developer `fm on|tune`) and
//!   is held until the DAC has stopped (at most 2 s); music starting while
//!   the radio plays stops the radio.
//! * Nothing here runs in an interrupt or on the playback path; while the
//!   radio is off this task sleeps on [`WAKE`].
//! * Developer channel ([`test_command`]): `fm on|off|status|scan`,
//!   `fm tune KHZ`, `fm seek up|down`, `fm route on|off` (the bypass alone,
//!   U-FM-3). Replies wait for the operation (at most 25 s).
//!
//! Text for the developer channel comes from [`t!`] statics (pageable),
//! not plain literals (resident `.rodata.str1.1`).

use core::sync::atomic::{AtomicU8, Ordering};

use atj2157_hal::dac;
use atj2157_hal::i2c_soft::SoftI2c;
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use orpheus_fm::chip::{ADDR, ADDR_RANDOM, Analog, Port};
use orpheus_fm::radio::{Phase, Radio, Req};
use orpheus_ui::text::StrBuf;
use yoton_bsp::pins;

use crate::audio::Writer;

macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

/// The UI's copy of the radio's text: RDS name and text as UTF-8, the
/// mini-player's title (written by `services::Fw::sync_radio`).
pub struct RadioText {
    pub ps: StrBuf<24>,
    pub rt: StrBuf<96>,
    pub title: StrBuf<32>,
}

/// The radio's RAM (520 bytes).
#[repr(C)]
pub struct FmRam {
    radio: Radio,
    text: RadioText,
}

/// SRAM3 after the step counter's state (`sensor::SHARED_END`, which
/// follows the battery logger's), below the card I/O counter
/// (`yoton_bsp::card::IO_ADDR`): plain RAM, outside SRAM0 (02 §1).
pub const FM_ADDR: usize = (crate::sensor::SHARED_END + 7) & !7;

/// End of [`FmRam`] (the recorder's state follows, `rec::SHARED_ADDR`).
pub const FM_END: usize = FM_ADDR + core::mem::size_of::<FmRam>();

const _: () = assert!(crate::sensor::SHARED_END <= FM_ADDR);
const _: () = assert!(crate::drain::SHARED_END <= crate::sensor::SHARED_ADDR);
const _: () = assert!(FM_ADDR + core::mem::size_of::<FmRam>() <= yoton_bsp::card::IO_ADDR);
const _: () = assert!(FM_ADDR % core::mem::align_of::<FmRam>() == 0);

/// Once, from `audio::init_chain` (in `main`) before the tasks start (not
/// inlined: `main` is resident).
#[inline(never)]
pub fn init() {
    let ram = FmRam {
        radio: Radio::new(),
        text: RadioText { ps: StrBuf::new(), rt: StrBuf::new(), title: StrBuf::new() },
    };
    // SAFETY: `FM_ADDR` is plain SRAM3 (enabled in crt0, inside the MPU's
    // RAM sub-region), aligned (asserted above), used by nothing else;
    // written once before any task runs.
    unsafe { core::ptr::write(FM_ADDR as *mut FmRam, ram) };
}

/// The radio text, for the UI (`services::Fw::new`, called once; the UI
/// task is its only user).
pub fn ui_text() -> &'static mut RadioText {
    let p = FM_ADDR as *mut FmRam;
    // SAFETY: initialised by `init`; called once (the UI task's `Fw`), so
    // this is the only reference to the field, which `with` (the `radio`
    // field only) never touches.
    unsafe { &mut (*p).text }
}

/// Wakes this task after a request.
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The radio, for the length of `f` (main executor only; never nested).
pub fn with<R>(f: impl FnOnce(&mut Radio) -> R) -> R {
    let p = FM_ADDR as *mut FmRam;
    // SAFETY: initialised by `init` before any task runs. Only tasks of
    // the main (thread-mode, cooperative) executor call `with`: this task,
    // the UI task and the USB task. `f` is synchronous and never calls
    // `with`, so no reference to `radio` outlives it, crosses an await or
    // overlaps another; no interrupt handler touches it. The UI's
    // references (`ui_parts`) are to other fields.
    f(unsafe { &mut (*p).radio })
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

/// Ask the radio for something (main executor only).
pub fn request(r: Req) {
    let now = now_ms();
    with(|x| x.request(r, now));
    WAKE.signal(());
}

/// The player's power-off: the radio stops (gain ramp, bypass off, tuner
/// standby). Returns within about 300 ms.
pub async fn stop_for_power_off() {
    request(Req::Off);
    for _ in 0..30 {
        if matches!(with(|r| r.phase()), Phase::Off | Phase::Failed) {
            return;
        }
        Timer::after_millis(10).await;
    }
}

/// A developer command waiting for its reply.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Report {
    On,
    Off,
    Tune,
    Seek,
    Scan,
}

/// The developer command waiting (0 = none), and its deadline.
static REPORT: AtomicU8 = AtomicU8::new(0);
static REPORT_ROUTE: AtomicU8 = AtomicU8::new(0);

const REPORT_MS: u64 = 25_000;

fn report_code(r: Report) -> u8 {
    match r {
        Report::On => 1,
        Report::Off => 2,
        Report::Tune => 3,
        Report::Seek => 4,
        Report::Scan => 5,
    }
}

fn report_of(c: u8) -> Option<Report> {
    Some(match c {
        1 => Report::On,
        2 => Report::Off,
        3 => Report::Tune,
        4 => Report::Seek,
        5 => Report::Scan,
        _ => return None,
    })
}

fn parse_khz(s: &[u8]) -> Option<u32> {
    if s.is_empty() || s.len() > 6 || !s.iter().all(u8::is_ascii_digit) {
        return None;
    }
    let v = s.iter().fold(0u32, |a, d| a * 10 + u32::from(d - b'0'));
    (64_000..=108_000).contains(&v).then_some(v)
}

/// `fm …` developer-channel commands; false if unknown. Main executor
/// (USB task).
pub fn test_command(rest: &[u8]) -> bool {
    let (req, rep) = match rest {
        b"on" => (Req::On, Report::On),
        b"off" => (Req::Off, Report::Off),
        b"scan" => (Req::Scan, Report::Scan),
        b"seek up" => (Req::Seek(true), Report::Seek),
        b"seek down" => (Req::Seek(false), Report::Seek),
        b"status" => {
            status_reply(t!(b"status"));
            return true;
        }
        b"route on" | b"route off" => {
            REPORT_ROUTE.store(if rest == b"route on" { 1 } else { 2 }, Ordering::Relaxed);
            WAKE.signal(());
            return true;
        }
        _ => {
            let Some(k) = rest.strip_prefix(b"tune ").and_then(parse_khz) else { return false };
            (Req::Tune(k), Report::Tune)
        }
    };
    // Seek and scan need the radio on.
    if matches!(rep, Report::Seek | Report::Scan) && !with(|r| r.phase().is_on()) {
        crate::audio::reply(|w| {
            w.s(t!(b"fm: radio is off (fm on first)"));
        });
        return true;
    }
    crate::audio::reply(|w| {
        w.s(t!(b"fm: busy"));
    });
    REPORT.store(report_code(rep), Ordering::Relaxed);
    request(req);
    true
}

/// The tuner on the bus and the bypass at the amplifier.
struct HwPort {
    i2c: SoftI2c,
}

impl Port for HwPort {
    fn write(&mut self, data: &[u8]) -> bool {
        // Three attempts, as the vendor's driver (fm-i2c-imu.md §2.3).
        for _ in 0..3 {
            if self.i2c.write(ADDR, data).is_ok() {
                return true;
            }
            self.i2c.recover();
        }
        false
    }

    fn read(&mut self, out: &mut [u8]) -> bool {
        for _ in 0..3 {
            if self.i2c.read(ADDR, out).is_ok() {
                return true;
            }
            self.i2c.recover();
        }
        false
    }

    fn read_reg(&mut self, reg: u8, out: &mut [u8]) -> bool {
        // Random access at 0x11 (register index, repeated start, read):
        // the chip id when the sequential method gives none.
        for _ in 0..3 {
            if self.i2c.write_read(ADDR_RANDOM, &[reg], out).is_ok() {
                return true;
            }
            self.i2c.recover();
        }
        false
    }

    fn analog(&mut self, a: Analog) {
        match a {
            Analog::Precharge => dac::fm_precharge(),
            Analog::Connect => dac::fm_connect(),
            Analog::Release => dac::fm_release(),
            Analog::Disconnect => dac::fm_disconnect(),
            Analog::Gain(g) => dac::set_pa_gain(u32::from(g)),
        }
    }
}

/// The DAC plays music (a track open; the radio must not mix into it).
fn music_on() -> bool {
    crate::audio::PLAYING.load(Ordering::Relaxed)
}

#[embassy_executor::task]
pub async fn fm() {
    // 5 µs half period: 100 kHz (the speed the bring-up scan used).
    let mut port = HwPort { i2c: SoftI2c::new(pins::I2C_SCL, pins::I2C_SDA, 5) };
    // A tuner left playing by a reset of the SoC alone: standby.
    let parked = with(|r| r.park(&mut port));
    atj2157_hal::log::line(if parked { t!(b"fm: tuner parked") } else { t!(b"fm: no answer at 0x10") });
    let mut report_at = 0u64;
    let mut hold_since: Option<u64> = None;
    let mut stop_asked = false;
    loop {
        let now = now_ms();
        // Music and radio are exclusive.
        let phase = with(|r| r.phase());
        let mut hold = false;
        if music_on() && phase.is_on() {
            if phase == Phase::Starting {
                // Starting the radio stops the music (the UI asked
                // already; a developer `fm on|tune` did not): ask once,
                // then wait for the DAC.
                if !stop_asked {
                    stop_asked = true;
                    crate::media::play(crate::media::PlayReq::Stop);
                }
                let since = *hold_since.get_or_insert(now);
                if now.saturating_sub(since) > 2000 {
                    with(|r| r.request(Req::Off, now));
                } else {
                    hold = true;
                }
            } else {
                with(|r| r.request(Req::Off, now));
            }
        } else {
            hold_since = None;
            stop_asked = false;
        }
        // `fm route on|off`: the bypass alone (with the vendor's waits).
        match REPORT_ROUTE.swap(0, Ordering::Relaxed) {
            1 => {
                dac::fm_precharge();
                Timer::after_millis(100).await;
                dac::fm_connect();
                Timer::after_millis(100).await;
                dac::fm_release();
                status_reply(t!(b"route on"));
            }
            2 => {
                dac::fm_disconnect();
                status_reply(t!(b"route off"));
            }
            _ => {}
        }
        let next = if hold { Some(now + 20) } else { with(|r| r.poll(&mut port, now)) };
        // A developer command's reply once its operation is done.
        let pending = report_of(REPORT.load(Ordering::Relaxed));
        if let Some(rep) = pending {
            if report_at == 0 {
                report_at = now + REPORT_MS;
            }
            let (phase, settled) = with(|r| (r.phase(), r.settled()));
            let done = match rep {
                Report::On | Report::Tune => settled || phase == Phase::Failed || phase == Phase::Off,
                Report::Off => matches!(phase, Phase::Off | Phase::Failed),
                Report::Seek => phase != Phase::Seeking && (settled || !phase.is_on()),
                Report::Scan => phase != Phase::Scanning && (settled || !phase.is_on()),
            };
            if done || now >= report_at {
                REPORT.store(0, Ordering::Relaxed);
                report_at = 0;
                let head = match (rep, done) {
                    (_, false) => t!(b"timed out"),
                    (Report::On, _) => t!(b"on"),
                    (Report::Off, _) => t!(b"off"),
                    (Report::Tune, _) => t!(b"tuned"),
                    (Report::Seek, _) => t!(b"seek done"),
                    (Report::Scan, _) => t!(b"scan done"),
                };
                if rep == Report::Scan && done {
                    scan_reply();
                } else {
                    status_reply(head);
                }
            }
        } else {
            report_at = 0;
        }
        // Sleep until the radio's deadline, a request, or (with a reply
        // waiting) at most a second.
        let mut wait = next.map(|t| t.saturating_sub(now_ms()));
        if pending.is_some() {
            wait = Some(wait.unwrap_or(1000).min(1000));
        }
        if with(|r| r.phase().is_on()) {
            // Music starting while the radio plays is noticed within 100 ms.
            wait = Some(wait.unwrap_or(100).min(100));
        }
        match wait {
            Some(ms) => {
                let _ = select(WAKE.wait(), Timer::after_millis(ms)).await;
            }
            None => WAKE.wait().await,
        }
    }
}

fn hex(w: &mut Writer<'_>, v: u32, digits: u32) {
    for i in (0..digits).rev() {
        let d = ((v >> (i * 4)) & 15) as u8;
        w.s(&[if d < 10 { b'0' + d } else { b'A' + d - 10 }]);
    }
}

fn phase_name(p: Phase) -> &'static [u8] {
    match p {
        Phase::Off => t!(b"off"),
        Phase::Starting => t!(b"starting"),
        Phase::Playing => t!(b"playing"),
        Phase::Seeking => t!(b"seeking"),
        Phase::Scanning => t!(b"scanning"),
        Phase::Stopping => t!(b"stopping"),
        Phase::Failed => t!(b"not responding"),
    }
}

fn printable(w: &mut Writer<'_>, s: &[u8]) {
    for &c in s {
        w.s(&[if (0x20..0x7F).contains(&c) { c } else { b'?' }]);
    }
}

/// `fm HEAD: phase=… khz=… rssi=… st=… mute=… vol=… pa=… chip=… ps="…"
/// rt="…" groups=… bad=… err=… regs 02h–07h, 0Ah–0Fh, antipop, pa_ctl`.
fn status_reply(head: &[u8]) {
    with(|r| {
        let i = r.info();
        let regs = r.tuner().regs();
        let st = r.last_status();
        let rds = r.rds();
        crate::audio::reply(|w| {
            w.s(t!(b"fm ")).s(head).s(t!(b": phase=")).s(phase_name(i.phase));
            w.s(t!(b" khz=")).u(u64::from(i.khz));
            w.s(t!(b" rssi=")).u(u64::from(i.rssi));
            w.s(t!(b" st=")).u(u64::from(i.stereo));
            w.s(t!(b" mute=")).u(u64::from(i.muted));
            w.s(t!(b" vol=")).u(u64::from(r.volume()));
            w.s(t!(b" pa=")).u(u64::from(r.gain()));
            w.s(t!(b" chip="));
            hex(w, u32::from(i.chip), 4);
            w.s(t!(b"\nrds ps=\""));
            printable(w, rds.ps());
            w.s(t!(b"\" rt=\""));
            printable(w, rds.rt());
            w.s(t!(b"\" pi="));
            hex(w, u32::from(rds.pi()), 4);
            w.s(t!(b" groups=")).u(u64::from(rds.groups));
            w.s(t!(b" bad=")).u(u64::from(rds.bad));
            w.s(t!(b" err=")).u(u64::from(i.errors));
            w.s(t!(b"\nregs 02-07:"));
            for v in regs {
                w.s(t!(b" "));
                hex(w, u32::from(v), 4);
            }
            w.s(t!(b" 0A-0F:"));
            for v in [st.r0a, st.r0b, st.blocks[0], st.blocks[1], st.blocks[2], st.blocks[3]] {
                w.s(t!(b" "));
                hex(w, u32::from(v), 4);
            }
            w.s(t!(b"\nantipop="));
            hex(w, dac::antipop(), 8);
            w.s(t!(b" pa_ctl="));
            hex(w, dac::regs()[2], 8);
        });
    });
}

/// After `fm scan`: the stations found, strongest first (added to the
/// status reply).
fn scan_reply() {
    with(|r| {
        let found = r.scan_results();
        crate::audio::reply(|w| {
            w.s(t!(b"fm scan done: ")).u(found.len() as u64).s(t!(b" stations, khz/rssi:"));
            for s in found {
                w.s(t!(b" ")).u(u64::from(s.khz())).s(t!(b"/")).u(u64::from(s.rssi));
            }
            let i = r.info();
            w.s(t!(b"\nnow khz=")).u(u64::from(i.khz)).s(t!(b" phase=")).s(phase_name(i.phase));
        });
    });
}
