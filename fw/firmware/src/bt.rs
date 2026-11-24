//! The Bluetooth task (WP BT-2, device side; 06 §4.2, bluetooth.md).
//!
//! Owns the transmitter module: `yoton_bt::driver::Driver` (state machine,
//! command layer, frame codec; ~2 KB, in a static) over `yoton_bsp::bt`
//! (UART1 with a DMA receive ring and DMA transmit, GPIO20 enable,
//! GPIO31 host-busy). No interrupt handler of its own: the receive ring is
//! polled every 10 ms while the module is on (2 ms with the CPU-FIFO
//! fallback); transmit waits on the DMA channel's completion (the generic
//! `atj2157_hal::dma` handler, resident already) with a time bound.
//!
//! * Off at boot (`yoton_bsp::bt::init` drives GPIO20 low). The UI sends
//!   [`Req::Boot`] with the settings: Bluetooth on → the module starts
//!   [`BOOT_ON_MS`] after boot, never in a host test (the test window).
//! * The Bluetooth setting switches it ([`Req::Power`]: off = enable line
//!   low, no shutdown command). The player's power-off calls
//!   [`shutdown_for_power_off`]: command 0x24 when the module is up, at
//!   most 1 s for its completion, then the enable line low (owner decision
//!   2026-11-24). No other path sends 0x24 except the `bt shutdown` test
//!   command, which runs the same sequence.
//! * Headset keys go to the player; playback start/stop goes to the
//!   module (command 0x05); the connected device's name and the last
//!   device (saved by the UI under [`LAST_KEY`]) go to the UI.
//! * Developer channel: `bt on|off|status|scan|connect N|connect pN|
//!   disconnect|listen|shutdown|rx dma|rx cpu|tx dma|tx cpu` ([`test_command`]).

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};

use critical_section::Mutex;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use yoton_bsp::bt::{self as hw, Link, Mode};
use yoton_bt::driver::{Driver, Port, SHUTDOWN_WAIT_MS};
use yoton_bt::transmitter::Device;
use yoton_bt::{Addr, Config, Key, Output, State};
use yoton_ui::event::{BtState, Source};

use crate::audio::Writer;

/// Settings-store key of the last connected device (6 bytes, as the
/// module sends the address).
pub const LAST_KEY: u16 = 0x06F0;
/// Uptime before the Bluetooth setting powers the module at boot.
pub const BOOT_ON_MS: u64 = 5000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Req {
    /// The settings as loaded at boot.
    Boot {
        on: bool,
        reconnect: bool,
        last: Option<Addr>,
    },
    /// Bluetooth on / off (setting or test).
    Power(bool),
    Reconnect(bool),
    Scan,
    /// Connect to a found device (`false`) or a paired one (`true`), by
    /// index.
    Connect {
        paired: bool,
        index: u8,
    },
    Disconnect,
    /// U-BT-1 board check, module off only ([`hw::Listen`]).
    Listen,
    /// Receive (`SetRx`) / transmit (`SetTx`) by DMA (`true`) or CPU;
    /// module off only.
    SetRx(bool),
    SetTx(bool),
    /// The player's power-off sequence.
    Shutdown,
    Status,
}

/// What the UI shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub state: BtState,
    /// The connected device's name (UTF-8 as sent, may be cut).
    pub name: [u8; 31],
    pub name_len: u8,
    /// The last connected device, and a count of changes (the UI saves it).
    pub last: Option<Addr>,
    pub last_gen: u16,
}

impl Snap {
    pub const OFF: Snap = Snap { state: BtState::Off, name: [0; 31], name_len: 0, last: None, last_gen: 0 };

    #[must_use]
    pub fn name(&self) -> &str {
        let b = &self.name[..usize::from(self.name_len)];
        match core::str::from_utf8(b) {
            Ok(s) => s,
            Err(e) => core::str::from_utf8(&b[..e.valid_up_to()]).unwrap_or(""),
        }
    }
}

pub static SNAP: Mutex<RefCell<Snap>> = Mutex::new(RefCell::new(Snap::OFF));

/// Requests, with "from the developer channel" (a reply is written).
static REQS: Channel<CriticalSectionRawMutex, (Req, bool), 4> = Channel::new();
static SHUT_DONE: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Frames the UART did not get out (DMA or FIFO timeout).
static TX_FAIL: AtomicU32 = AtomicU32::new(0);

/// Ask the Bluetooth task (dropped if its queue is full).
pub fn request(r: Req) {
    let _ = REQS.try_send((r, false));
}

/// Player power-off: the module's shutdown (command 0x24 when it is up,
/// bounded wait), enable line low. Returns within about 1.7 s.
pub async fn shutdown_for_power_off() {
    SHUT_DONE.reset();
    if with_timeout(Duration::from_millis(200), REQS.send((Req::Shutdown, false))).await.is_err() {
        return;
    }
    let _ = with_timeout(Duration::from_millis(SHUTDOWN_WAIT_MS + 500), SHUT_DONE.wait()).await;
}

/// `bt …` developer-channel commands; false if unknown.
pub fn test_command(rest: &[u8]) -> bool {
    let r = match rest {
        b"on" => Req::Power(true),
        b"off" => Req::Power(false),
        b"status" => Req::Status,
        b"scan" => Req::Scan,
        b"disconnect" => Req::Disconnect,
        b"listen" => Req::Listen,
        b"shutdown" => Req::Shutdown,
        b"rx dma" => Req::SetRx(true),
        b"rx cpu" => Req::SetRx(false),
        b"tx dma" => Req::SetTx(true),
        b"tx cpu" => Req::SetTx(false),
        _ => {
            let Some(arg) = rest.strip_prefix(b"connect ") else { return false };
            let (paired, digits) = match arg.split_first() {
                Some((b'p', d)) => (true, d),
                _ => (false, arg),
            };
            if digits.is_empty() || digits.len() > 2 || !digits.iter().all(u8::is_ascii_digit) {
                return false;
            }
            let index = digits.iter().fold(0u8, |a, d| a * 10 + (d - b'0'));
            Req::Connect { paired, index }
        }
    };
    REQS.try_send((r, true)).is_ok()
}

static mut DRIVER: MaybeUninit<Driver> = MaybeUninit::uninit();
static mut RING: [u8; hw::RING] = [0; hw::RING];

/// The driver, built in place in its static (not in the task's future).
#[inline(never)]
fn driver() -> &'static mut Driver {
    let cfg = Config { forget_all_on_start: false, ..Config::default() };
    // SAFETY: called once, by the task, before anything else uses DRIVER.
    unsafe { (*(&raw mut DRIVER)).write(Driver::new(cfg)) }
}

struct HwPort {
    link: Option<Link>,
    mode: Mode,
}

impl Port for HwPort {
    fn set_enabled(&mut self, on: bool) {
        if let Some(l) = self.link.take() {
            hw::down(l);
        }
        if on {
            // SAFETY: UART1, DMA channels 8/9 and RING belong to this task;
            // the previous link (if any) was taken down above.
            self.link = Some(unsafe { hw::up(self.mode, &mut *(&raw mut RING)) });
        }
    }

    fn read(&mut self, buf: &mut [u8]) -> usize {
        self.link.as_mut().map_or(0, |l| l.read(buf))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Report {
    On,
    Off,
    Scan,
    Connect,
    Disconnect,
}

/// The task's own small state (in its future: keep it small).
struct Session {
    boot_on_at: Option<u64>,
    restart_pending: bool,
    audio: bool,
    connected: Option<Addr>,
    snap: Snap,
    report: Option<(Report, u64)>,
    saw_connected: bool,
    saw_failed: bool,
    saw_inquiry_done: bool,
    gave_up: bool,
    keys: u32,
    sink_volume: Option<u8>,
}

impl Session {
    fn on_output(&mut self, o: Output) {
        use yoton_player::Command;
        match o {
            Output::Enable(true) => self.restart_pending = false,
            Output::Connected(a) => {
                self.saw_connected = true;
                self.connected = Some(a);
                if self.snap.last != Some(a) {
                    self.snap.last = Some(a);
                    self.snap.last_gen = self.snap.last_gen.wrapping_add(1);
                }
            }
            Output::Disconnected => self.connected = None,
            Output::ConnectFailed => self.saw_failed = true,
            Output::InquiryDone => self.saw_inquiry_done = true,
            Output::LinkDown { restarting } => {
                self.restart_pending = restarting;
                self.gave_up |= !restarting;
                self.connected = None;
            }
            Output::State(State::Off) => self.connected = None,
            Output::Key(k) => {
                // AVRCP pass-through from the headset (bluetooth.md §3.5):
                // play/pause toggle as the vendor treats them.
                self.keys += 1;
                let r = match k {
                    Key::Play | Key::Pause => crate::media::PlayReq::Cmd(Command::Toggle),
                    Key::Stop => crate::media::PlayReq::Stop,
                    Key::Next => crate::media::PlayReq::Cmd(Command::Next),
                    Key::Previous => crate::media::PlayReq::Cmd(Command::Prev),
                    Key::Other(_) => return,
                };
                crate::media::play(r);
            }
            Output::SinkVolume { raw, .. } => self.sink_volume = Some(raw),
            Output::AbsVolume(v) => self.sink_volume = v,
            _ => {}
        }
    }

    fn start_report(&mut self, r: Report, now: u64) {
        let secs = match r {
            Report::On => 15,
            Report::Off => 0,
            Report::Scan => 30,
            Report::Connect => 25,
            Report::Disconnect => 5,
        };
        self.saw_connected = false;
        self.saw_failed = false;
        self.saw_inquiry_done = false;
        self.gave_up = false;
        self.report = Some((r, now + secs * 1000));
    }
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

fn ui_state(s: State) -> BtState {
    match s {
        State::Off => BtState::Off,
        State::Starting | State::Idle | State::Inquiry | State::Connecting => BtState::On,
        State::Connected => BtState::Connected,
        State::Streaming => BtState::Streaming,
    }
}

fn find(d: &Driver, a: Addr) -> Option<&Device> {
    let t = d.transmitter();
    t.paired().iter().chain(t.found()).find(|x| x.addr == a)
}

#[embassy_executor::task]
pub async fn bt() {
    hw::init();
    let d = driver();
    let mut port = HwPort { link: None, mode: Mode::DEFAULT };
    let mut s = Session {
        boot_on_at: None,
        restart_pending: false,
        audio: false,
        connected: None,
        snap: Snap::OFF,
        report: None,
        saw_connected: false,
        saw_failed: false,
        saw_inquiry_done: false,
        gave_up: false,
        keys: 0,
        sink_volume: None,
    };
    loop {
        let idle = d.is_off() && !s.restart_pending && s.boot_on_at.is_none() && s.report.is_none();
        let req = if idle {
            Some(REQS.receive().await)
        } else {
            let ms = if port.mode.rx_dma { 10 } else { 2 };
            match select(REQS.receive(), Timer::after_millis(ms)).await {
                Either::First(r) => Some(r),
                Either::Second(()) => None,
            }
        };
        let now = now_ms();
        if let Some((r, test)) = req {
            handle(r, test, d, &mut port, &mut s, now).await;
        }
        if let Some(t) = s.boot_on_at
            && now >= t
        {
            s.boot_on_at = None;
            d.power(true, now, &mut port, &mut |o| s.on_output(o));
        }
        d.poll(now_ms(), &mut port, &mut |o| s.on_output(o));
        flush(d, &mut port).await;
        // Playback start/stop to the module (command 0x05 when connected).
        let playing =
            critical_section::with(|cs| crate::media::PLAYER.borrow(cs).borrow().source) == Source::Playing;
        if playing != s.audio {
            s.audio = playing;
            d.request(now_ms(), &mut port, &mut |o| s.on_output(o), |t, now, tx| {
                t.set_audio_active(playing, now, tx);
            });
            flush(d, &mut port).await;
        }
        publish(d, &mut s);
        if let Some((r, deadline)) = s.report {
            let st = d.state();
            let done = match r {
                Report::On => d.transmitter().module_up() || (st == State::Off && !s.restart_pending),
                Report::Off => true,
                Report::Scan => s.saw_inquiry_done || st != State::Inquiry,
                Report::Connect => s.saw_connected || s.saw_failed || st == State::Off,
                Report::Disconnect => !matches!(st, State::Connected | State::Streaming),
            };
            if done || now >= deadline {
                s.report = None;
                status_reply(d, &port, &s, if done { b"done" } else { b"timed out" });
            }
        }
    }
}

async fn handle(r: Req, test: bool, d: &mut Driver, port: &mut HwPort, s: &mut Session, now: u64) {
    let mut refused = false;
    match r {
        Req::Boot { on, reconnect, last } => {
            s.snap.last = last;
            d.request(now, port, &mut |o| s.on_output(o), |t, _, _| {
                t.set_last_device(last);
                t.set_config(Config { auto_reconnect: reconnect, ..*t.config() });
            });
            if on {
                s.boot_on_at = Some(now.max(BOOT_ON_MS));
            }
        }
        Req::Reconnect(on) => {
            d.request(now, port, &mut |o| s.on_output(o), |t, _, _| {
                t.set_config(Config { auto_reconnect: on, ..*t.config() });
            });
        }
        Req::Power(on) => {
            s.boot_on_at = None;
            if test {
                s.start_report(if on { Report::On } else { Report::Off }, now);
            }
            d.power(on, now, port, &mut |o| s.on_output(o));
            if !on {
                s.restart_pending = false;
            }
        }
        Req::Scan => {
            refused =
                d.request(now, port, &mut |o| s.on_output(o), |t, now, tx| t.start_inquiry(now, tx)).is_err();
            if test && !refused {
                s.start_report(Report::Scan, now);
            }
        }
        Req::Connect { paired, index } => {
            let t = d.transmitter();
            let list = if paired { t.paired() } else { t.found() };
            match list.get(usize::from(index)).map(|x| x.addr) {
                Some(a) => {
                    refused = d
                        .request(now, port, &mut |o| s.on_output(o), |t, now, tx| t.connect(a, now, tx))
                        .is_err();
                }
                None => refused = true,
            }
            if test && !refused {
                s.start_report(Report::Connect, now);
            }
        }
        Req::Disconnect => {
            refused =
                d.request(now, port, &mut |o| s.on_output(o), |t, now, tx| t.disconnect(now, tx)).is_err();
            if test && !refused {
                s.start_report(Report::Disconnect, now);
            }
        }
        Req::SetRx(dma) | Req::SetTx(dma) => {
            if port.link.is_some() {
                refused = true;
            } else if matches!(r, Req::SetRx(_)) {
                port.mode.rx_dma = dma;
            } else {
                port.mode.tx_dma = dma;
            }
        }
        Req::Listen => {
            if port.link.is_some() || !d.is_off() {
                refused = true;
            } else {
                listen().await;
                return;
            }
        }
        Req::Shutdown => {
            s.boot_on_at = None;
            s.restart_pending = false;
            let sent = d.begin_shutdown(now, port, &mut |o| s.on_output(o));
            flush(d, port).await;
            if sent {
                // Bounded inside the driver (SHUTDOWN_WAIT_MS); this loop
                // only adds a guard.
                let until = now + SHUTDOWN_WAIT_MS + 200;
                while !d.is_off() && now_ms() < until {
                    Timer::after_millis(10).await;
                    d.poll(now_ms(), port, &mut |o| s.on_output(o));
                    flush(d, port).await;
                }
                if !d.is_off() {
                    d.power(false, now_ms(), port, &mut |o| s.on_output(o));
                }
            }
            SHUT_DONE.signal(());
            if test {
                status_reply(
                    d,
                    port,
                    s,
                    if sent { b"shutdown: 0x24 sent" } else { b"shutdown: module not up, line dropped" },
                );
            }
            return;
        }
        Req::Status => {}
    }
    if test {
        let head: &[u8] = if refused {
            b"refused"
        } else if s.report.is_some() {
            b"started"
        } else {
            b"ok"
        };
        status_reply(d, port, s, head);
    }
}

/// Send the queued frame, if any.
async fn flush(d: &mut Driver, port: &mut HwPort) {
    if d.tx_pending().is_empty() {
        return;
    }
    // The frame lives in the DRIVER static (SRAM0, DMA-visible) and is not
    // touched until `tx_sent`.
    let ok = match port.link.as_mut() {
        Some(l) => l.send(d.tx_pending()).await,
        None => true,
    };
    if !ok {
        TX_FAIL.fetch_add(1, Ordering::Relaxed);
    }
    d.tx_sent();
}

/// State and the connected device's name to the UI (on change only).
fn publish(d: &Driver, s: &mut Session) {
    let mut n = s.snap;
    n.state = ui_state(d.state());
    n.name_len = 0;
    if let Some(dev) = s.connected.and_then(|a| find(d, a))
        && matches!(n.state, BtState::Connected | BtState::Streaming)
    {
        let b = dev.name();
        let k = b.len().min(n.name.len());
        n.name[..k].copy_from_slice(&b[..k]);
        n.name_len = k as u8;
    }
    if n != s.snap {
        s.snap = n;
        critical_section::with(|cs| *SNAP.borrow(cs).borrow_mut() = n);
    }
}

/// `bt listen`: which UART pad the module drives while it starts (U-BT-1,
/// board side). Neither pad is driven by the player.
async fn listen() {
    let mut l = hw::Listen::begin();
    // ≈ 4 s of 10-ms sampling slices, the executor runs in between.
    for _ in 0..400 {
        l.sample(10_000);
        Timer::after_millis(1).await;
    }
    let (falls, lows, n) = (l.falls, l.lows, l.samples);
    l.end();
    crate::audio::reply(|w| {
        w.s(b"bt listen: gpio38 falls=")
            .u(u64::from(falls.0))
            .s(b" low=")
            .u(u64::from(lows.0))
            .s(b" gpio39 falls=")
            .u(u64::from(falls.1))
            .s(b" low=")
            .u(u64::from(lows.1))
            .s(b" samples=")
            .u(u64::from(n))
            .s(b"\nexpected: gpio39 (module TX) toggles, gpio38 (module RX) stays high");
    });
}

fn hex(w: &mut Writer<'_>, v: u32, digits: u32) {
    const H: &[u8; 16] = b"0123456789ABCDEF";
    for i in (0..digits).rev() {
        w.s(&[H[((v >> (i * 4)) & 15) as usize]]);
    }
}

fn state_name(s: State) -> &'static [u8] {
    match s {
        State::Off => b"off",
        State::Starting => b"starting",
        State::Idle => b"idle",
        State::Inquiry => b"inquiry",
        State::Connecting => b"connecting",
        State::Connected => b"connected",
        State::Streaming => b"streaming",
    }
}

fn device_line(w: &mut Writer<'_>, i: usize, x: &Device) {
    w.s(b"\n  ").u(i as u64).s(b" ").s(&x.addr.to_text()).s(b" cod=");
    for b in x.cod {
        hex(w, u32::from(b), 2);
    }
    w.s(b" \"");
    for &c in x.name() {
        w.s(&[if (0x20..0x7F).contains(&c) { c } else { b'?' }]);
    }
    w.s(b"\"");
}

fn status_reply(d: &Driver, port: &HwPort, s: &Session, head: &[u8]) {
    let t = d.transmitter();
    let st = d.stats();
    let ls = t.link_stats();
    let ds = t.decode_stats();
    crate::audio::reply(|w| {
        w.s(b"bt ").s(head).s(b": state=").s(state_name(t.state()));
        w.s(if t.shutting_down() { b" (shutting down)" } else { b"" });
        w.s(b" module=\"");
        for &c in t.identity() {
            if (0x20..0x7F).contains(&c) {
                w.s(&[c]);
            }
        }
        w.s(b"\" rx=").s(if port.mode.rx_dma { b"dma" } else { b"cpu" }).s(b" tx=").s(if port.mode.tx_dma {
            b"dma"
        } else {
            b"cpu"
        });
        if let Some(l) = port.link.as_ref() {
            let (sta, ctl, rem, run, peak) = l.regs();
            w.s(b"\nuart sta=0x");
            hex(w, sta, 8);
            w.s(b" ctl=0x");
            hex(w, ctl, 8);
            w.s(b" ring_rem=")
                .u(u64::from(rem))
                .s(b" ring_run=")
                .u(u64::from(run))
                .s(b" peak=")
                .u(u64::from(peak));
            w.s(b" rx_err=").u(u64::from(l.rx_errors));
        }
        w.s(b"\nrx bytes=")
            .u(u64::from(st.rx_bytes))
            .s(b" frames=")
            .u(u64::from(ds.frames))
            .s(b" skipped=")
            .u(u64::from(ds.skipped))
            .s(b" badlen=")
            .u(u64::from(ds.bad_length))
            .s(b" badsum=")
            .u(u64::from(ds.bad_checksum));
        w.s(b"\ntx frames=")
            .u(u64::from(st.tx_frames))
            .s(b" bytes=")
            .u(u64::from(st.tx_bytes))
            .s(b" dropped=")
            .u(u64::from(st.tx_dropped))
            .s(b" fail=")
            .u(u64::from(TX_FAIL.load(Ordering::Relaxed)))
            .s(b" dma_timeouts=")
            .u(u64::from(atj2157_hal::uart::TX_DMA_TIMEOUTS.load(Ordering::Relaxed)))
            .s(b" dma_no_irq=")
            .u(u64::from(atj2157_hal::uart::TX_DMA_NO_IRQ.load(Ordering::Relaxed)));
        w.s(b"\ncmd sent=")
            .u(u64::from(ls.sent))
            .s(b" resent=")
            .u(u64::from(ls.resent))
            .s(b" timeouts=")
            .u(u64::from(ls.timeouts))
            .s(b" stray_acks=")
            .u(u64::from(ls.stray_acks))
            .s(b" power_ups=")
            .u(u64::from(st.power_ups))
            .s(b" shutdowns=")
            .u(u64::from(st.shutdowns))
            .s(b"/")
            .u(u64::from(st.shutdowns_completed))
            .s(b" gave_up=")
            .u(u64::from(s.gave_up))
            .s(b" keys=")
            .u(u64::from(s.keys));
        if let Some(v) = s.sink_volume {
            w.s(b" sink_vol=").u(u64::from(v));
        }
        if let Some(a) = s.connected {
            w.s(b"\nconnected ").s(&a.to_text());
        }
        w.s(b"\npaired=").u(t.paired().len() as u64);
        for (i, x) in t.paired().iter().enumerate() {
            device_line(w, i, x);
        }
        w.s(b"\nfound=").u(t.found().len() as u64);
        for (i, x) in t.found().iter().enumerate() {
            device_line(w, i, x);
        }
        w.s(b"\nrx tail:");
        let (a, b) = st.last_rx();
        for &c in a.iter().chain(b) {
            w.s(b" ");
            hex(w, u32::from(c), 2);
        }
    });
}
