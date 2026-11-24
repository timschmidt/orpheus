//! The Bluetooth task (WP BT-2, device side; 06 §4.2, bluetooth.md).
//!
//! Owns the transmitter module: `orpheus_bt::driver::Driver` (state machine,
//! command layer, frame codec; ~1.3 KB, in a static) over `yoton_bsp::bt`
//! (UART1, GPIO20 enable, GPIO31 host-busy). Receive path, switchable for
//! the first hardware runs (`bt rx …`): a DMA ring (default, no
//! interrupt) or CPU polling of the FIFO. No interrupt handler of its
//! own, so nothing here needs to be resident. The task polls every
//! 10 ms while the module is on (1 ms with CPU polling); transmit waits on
//! the DMA channel's completion (the generic `atj2157_hal::dma` handler,
//! resident already) with a time bound.
//!
//! * Off at boot (`yoton_bsp::bt::init` drives GPIO20 low). The UI sends
//!   [`Req::Boot`] with the settings: Bluetooth on → the module starts
//!   [`BOOT_ON_MS`] after boot, never in a host test (the test window).
//! * The Bluetooth setting switches it ([`Req::Power`]: off = enable line
//!   low, no shutdown command). The player's power-off calls
//!   [`shutdown_for_power_off`]: command 0x24 when the module is up, at
//!   most 1 s for its completion, then the enable line low (owner decision
//!   2026-11-24). Nothing else sends 0x24 except the `bt shutdown` test
//!   command, which runs the same sequence by hand (CLAUDE.md rule 6: no
//!   other restricted command exists here).
//! * Headset keys go to the player; playback start/stop goes to the
//!   module (command 0x05); the connected device's name and the last
//!   device (saved by the UI under [`LAST_KEY`]) go to the UI.
//! * The Bluetooth app (WP UI-6a) asks through [`request`]: scan, stop,
//!   connect to an address, cancel, disconnect, forget, the sink's
//!   absolute volume. It sees [`SNAP`] (state, counters of connections
//!   made / failed / lost, the sink's volume) and [`LISTS`] (the found
//!   and paired devices, copied when they change). None of these is a
//!   restricted command.
//! * Developer channel ([`test_command`]): `bt probe|on|off|status|scan|
//!   connect N|connect pN|disconnect|listen|shutdown|rx dma|rx cpu|tx dma|
//!   tx cpu`.
//!
//! Text for the developer channel is built from [`t!`] statics: plain
//! byte-string literals land in the mergeable `.rodata.str1.1`, which
//! cannot be paged and would cost SRAM.

use core::cell::RefCell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering};

use atj2157_hal::uart::Rx;
use critical_section::Mutex;
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Duration, Instant, Timer, with_timeout};
use orpheus_apps::services::{BT_DEVICES, BtAddr, BtDevice, BtKind, BtList, BtPhase};
use orpheus_bt::driver::{Driver, Port, SHUTDOWN_WAIT_MS};
use orpheus_bt::transmitter::Device;
use orpheus_bt::{Addr, Config, Key, Output, State};
use orpheus_ui::event::{BtState, Source};
use yoton_bsp::bt::{self as hw, Link, Mode};

use crate::audio::Writer;

/// A byte string in its own (pageable) rodata section; see the module
/// docs.
macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

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
    /// Test: on, start-up exchange, report, off.
    Probe,
    Reconnect(bool),
    Scan,
    /// Connect to a found device (`false`) or a paired one (`true`), by
    /// index.
    Connect {
        paired: bool,
        index: u8,
    },
    Disconnect,
    /// The Bluetooth app (UI-6a): stop the inquiry; connect to an address
    /// (a running inquiry is stopped first); cancel an attempt; forget a
    /// pairing.
    StopScan,
    ConnectTo(Addr),
    CancelConnect,
    Forget(Addr),
    /// U-BT-1 board check, module off only ([`hw::Listen`]).
    Listen,
    /// Receive / transmit path; module off only.
    SetRx(Rx),
    SetTx(bool),
    /// The player's power-off sequence.
    Shutdown,
    Status,
}

/// What the UI shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Snap {
    pub state: BtState,
    /// The connected device's name, else the last device's if it is in
    /// the module's paired list (UTF-8 as sent, may be cut).
    pub name: [u8; 24],
    pub name_len: u8,
    /// The last connected device, and a count of changes (the UI saves it).
    pub last: Option<Addr>,
    pub last_gen: u16,
    /// What the app shows.
    pub phase: BtPhase,
    /// Counts of connections made, attempts failed, connections lost, and
    /// changes of [`LISTS`]: the UI turns changes into events.
    pub connects: u16,
    pub fails: u16,
    pub drops: u16,
    pub lists_gen: u16,
    /// Connected to a sink that reported absolute-volume support (the
    /// DAC then stays at the reference level).
    pub sink_abs: bool,
    /// The sink's volume change last taken (0–31) and a count of them:
    /// the player's volume follows.
    pub sink_local: u8,
    pub sink_gen: u16,
}

impl Snap {
    pub const OFF: Snap = Snap {
        state: BtState::Off,
        name: [0; 24],
        name_len: 0,
        last: None,
        last_gen: 0,
        phase: BtPhase::Off,
        connects: 0,
        fails: 0,
        drops: 0,
        lists_gen: 0,
        sink_abs: false,
        sink_local: 0,
        sink_gen: 0,
    };

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

/// The player's volume on the module's 0–31 scale, set by the UI on
/// every change ([`VOL_NONE`] until then). The task hands changes to the
/// transmitter, which sends them to a connected sink (command 0x26) and
/// again at every connection (06 §4.2; a BlueZ sink starts at 0).
pub static VOLUME: AtomicU8 = AtomicU8::new(VOL_NONE);
pub const VOL_NONE: u8 = 0xFF;

/// The module's device lists as the UI reads them (≈ 0.5 KB: 2 × 8
/// devices of 33 bytes), rewritten by [`publish`] when they change.
pub struct Lists {
    found: [BtDevice; BT_DEVICES],
    n_found: u8,
    paired: [BtDevice; BT_DEVICES],
    n_paired: u8,
}

impl Lists {
    const EMPTY: Lists = Lists {
        found: [BtDevice::EMPTY; BT_DEVICES],
        n_found: 0,
        paired: [BtDevice::EMPTY; BT_DEVICES],
        n_paired: 0,
    };

    fn of(&self, l: BtList) -> &[BtDevice] {
        match l {
            BtList::Found => self.found.get(..usize::from(self.n_found)).unwrap_or(&[]),
            BtList::Paired => self.paired.get(..usize::from(self.n_paired)).unwrap_or(&[]),
        }
    }

    #[must_use]
    pub fn count(&self, l: BtList) -> usize {
        self.of(l).len()
    }

    #[must_use]
    pub fn get(&self, l: BtList, i: usize) -> Option<BtDevice> {
        self.of(l).get(i).copied()
    }
}

pub static LISTS: Mutex<RefCell<Lists>> = Mutex::new(RefCell::new(Lists::EMPTY));

/// Requests, with "from the developer channel" (a reply is written).
static REQS: Channel<CriticalSectionRawMutex, (Req, bool), 3> = Channel::new();
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
        b"probe" => Req::Probe,
        b"on" => Req::Power(true),
        b"off" => Req::Power(false),
        b"status" => Req::Status,
        b"scan" => Req::Scan,
        b"disconnect" => Req::Disconnect,
        b"listen" => Req::Listen,
        b"shutdown" => Req::Shutdown,
        b"rx dma" => Req::SetRx(Rx::Dma),
        b"rx cpu" => Req::SetRx(Rx::Cpu),
        b"tx dma" => Req::SetTx(true),
        b"tx cpu" => Req::SetTx(false),
        _ => {
            // `bt vol N`: the player volume (0–31) the task sends as 0x26.
            if let Some(n) = rest.strip_prefix(b"vol ") {
                if n.is_empty() || n.len() > 2 || !n.iter().all(u8::is_ascii_digit) {
                    return false;
                }
                let v = n.iter().fold(0u8, |a, d| a * 10 + (d - b'0')).min(31);
                VOLUME.store(v, Ordering::Relaxed);
                return true;
            }
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
    Probe,
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
    last: Option<Addr>,
    last_gen: u16,
    report: Option<(Report, u64)>,
    saw_connected: bool,
    saw_failed: bool,
    saw_inquiry_done: bool,
    gave_up: bool,
    keys: u32,
    sink_volume: Option<u8>,
    /// Absolute volume taken by the connected sink (0x26 acknowledged,
    /// or the sink reported a volume); `sink_no_abs`: it said it has none.
    sink_abs: bool,
    sink_no_abs: bool,
    sink_local: u8,
    sink_gen: u16,
    /// [`VOLUME`] as last handed to the transmitter.
    vol_sent: u8,
    /// The module was given up on (no answer after its restarts).
    dead: bool,
    /// The device asked for by the app (shown while connecting).
    target: Option<Addr>,
    connects: u16,
    fails: u16,
    drops: u16,
    lists_gen: u16,
}

impl Session {
    fn on_output(&mut self, o: Output) {
        use orpheus_player::Command;
        match o {
            Output::Enable(true) => {
                self.restart_pending = false;
                self.dead = false;
            }
            Output::Connected(a) => {
                self.saw_connected = true;
                self.connected = Some(a);
                self.target = None;
                self.connects = self.connects.wrapping_add(1);
                if self.last != Some(a) {
                    self.last = Some(a);
                    self.last_gen = self.last_gen.wrapping_add(1);
                }
            }
            Output::Disconnected => {
                self.connected = None;
                self.sink_volume = None;
                self.sink_abs = false;
                self.sink_no_abs = false;
                self.drops = self.drops.wrapping_add(1);
            }
            Output::ConnectFailed => {
                self.saw_failed = true;
                self.target = None;
                self.fails = self.fails.wrapping_add(1);
            }
            Output::InquiryDone => self.saw_inquiry_done = true,
            Output::LinkDown { restarting } => {
                self.restart_pending = restarting;
                self.gave_up |= !restarting;
                self.dead |= !restarting;
                self.lost_connection();
            }
            Output::State(State::Off) => self.lost_connection(),
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
            // Out of AVRCP's 0–127 range: the module's "no absolute
            // volume" answer (128 seen with a BlueZ sink, 2026-11-24).
            Output::SinkVolume { raw, .. } if raw > 127 => {
                self.sink_volume = None;
                self.sink_abs = false;
                self.sink_no_abs = true;
            }
            Output::SinkVolume { raw, local } => {
                // A change made on the sink: the player's volume follows.
                // What the UI will store back is already the
                // transmitter's, so it is not sent again.
                use orpheus_bt::proto::{local_from_ui, ui_from_local};
                self.sink_volume = Some(raw);
                self.sink_abs = true;
                self.sink_local = local;
                self.sink_gen = self.sink_gen.wrapping_add(1);
                self.vol_sent = local_from_ui(ui_from_local(local));
            }
            Output::AbsVolume(v) => {
                // The sink's answer to 0x25: `None` or a value above 127 =
                // no absolute volume, the DAC goes back to the player's
                // volume. Measured: a BlueZ sink without absolute volume
                // answers 128 (2026-11-24).
                let v = v.filter(|&x| x <= 127);
                self.sink_volume = v;
                self.sink_abs = v.is_some();
                self.sink_no_abs = v.is_none();
            }
            // 0x26 acknowledged: that is the module's acknowledgement, not
            // the sink's, so it proves nothing about the sink scaling (a
            // sink without absolute volume is acknowledged too, measured
            // 2026-11-24). Only a volume reported by the sink counts.
            Output::VolumeAcked => {}
            _ => {}
        }
    }

    fn lost_connection(&mut self) {
        if self.connected.take().is_some() {
            self.drops = self.drops.wrapping_add(1);
        }
        if self.target.take().is_some() {
            self.fails = self.fails.wrapping_add(1);
        }
        self.sink_volume = None;
        self.sink_abs = false;
        self.sink_no_abs = false;
    }

    fn start_report(&mut self, r: Report, now: u64) {
        let secs = match r {
            Report::On => 15,
            // Long enough for a dead module's three power cycles.
            Report::Probe => 25,
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
        last: None,
        last_gen: 0,
        report: None,
        saw_connected: false,
        saw_failed: false,
        saw_inquiry_done: false,
        gave_up: false,
        keys: 0,
        sink_volume: None,
        sink_abs: false,
        sink_no_abs: false,
        sink_local: 0,
        sink_gen: 0,
        vol_sent: VOL_NONE,
        dead: false,
        target: None,
        connects: 0,
        fails: 0,
        drops: 0,
        lists_gen: 0,
    };
    loop {
        let idle = d.is_off() && !s.restart_pending && s.boot_on_at.is_none() && s.report.is_none();
        let req = if idle {
            Some(REQS.receive().await)
        } else {
            let ms = if port.mode.rx == Rx::Cpu { 1 } else { 10 };
            match select(REQS.receive(), Timer::after_millis(ms)).await {
                Either::First(r) => Some(r),
                Either::Second(()) => None,
            }
        };
        let now = now_ms();
        if let Some((r, test)) = req {
            // `handle` is synchronous; the two requests that wait run here
            // (keeps the task's future small).
            match handle(r, test, d, &mut port, &mut s, now) {
                Then::Nothing => {}
                Then::Listen => listen().await,
                Then::ShutdownWait(sent) => shutdown_wait(sent, test, d, &mut port, &mut s, now).await,
            }
        }
        if let Some(t) = s.boot_on_at
            && now >= t
        {
            s.boot_on_at = None;
            d.power(true, now, &mut port, &mut |o| s.on_output(o));
        }
        d.poll(now_ms(), &mut port, &mut |o| s.on_output(o));
        flush(d, &mut port).await;
        // The player's volume to the transmitter (sent when connected).
        let vol = VOLUME.load(Ordering::Relaxed);
        if vol != VOL_NONE && vol != s.vol_sent {
            s.vol_sent = vol;
            let _ = d.request(now_ms(), &mut port, &mut |o| s.on_output(o), |t, now, tx| {
                t.set_volume(vol, now, tx)
            });
            flush(d, &mut port).await;
        }
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
                Report::On | Report::Probe => {
                    d.transmitter().module_up() || (st == State::Off && !s.restart_pending)
                }
                Report::Off => true,
                Report::Scan => s.saw_inquiry_done || st != State::Inquiry,
                Report::Connect => s.saw_connected || s.saw_failed || st == State::Off,
                Report::Disconnect => !matches!(st, State::Connected | State::Streaming),
            };
            if done || now >= deadline {
                s.report = None;
                let head = match (r, done) {
                    (Report::Probe, true) => t!(b"probe done, module off again"),
                    (Report::Probe, false) => t!(b"probe timed out, module off again"),
                    (_, true) => t!(b"done"),
                    (_, false) => t!(b"timed out"),
                };
                status_reply(d, &port, &s, head);
                if r == Report::Probe {
                    d.power(false, now_ms(), &mut port, &mut |o| s.on_output(o));
                    s.restart_pending = false;
                    publish(d, &mut s);
                }
            }
        }
    }
}

/// What the task still has to wait for after [`handle`].
enum Then {
    Nothing,
    Listen,
    /// The shutdown command went out (`true`) or the line was dropped.
    ShutdownWait(bool),
}

fn handle(r: Req, test: bool, d: &mut Driver, port: &mut HwPort, s: &mut Session, now: u64) -> Then {
    let mut refused = false;
    match r {
        Req::Boot { on, reconnect, last } => {
            s.last = last;
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
            s.dead = false;
            if test {
                s.start_report(if on { Report::On } else { Report::Off }, now);
            }
            d.power(on, now, port, &mut |o| s.on_output(o));
            if !on {
                s.restart_pending = false;
            }
        }
        Req::Probe => {
            if d.is_off() {
                s.boot_on_at = None;
                s.start_report(Report::Probe, now);
                d.power(true, now, port, &mut |o| s.on_output(o));
            } else {
                refused = true;
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
        Req::StopScan => {
            refused =
                d.request(now, port, &mut |o| s.on_output(o), |t, now, tx| t.stop_inquiry(now, tx)).is_err();
        }
        Req::ConnectTo(a) => {
            refused =
                d.request(now, port, &mut |o| s.on_output(o), |t, now, tx| t.connect(a, now, tx)).is_err();
            if refused {
                // Said at once, so the app's progress screen ends.
                s.fails = s.fails.wrapping_add(1);
            } else if s.connected != Some(a) {
                s.target = Some(a);
            }
        }
        Req::CancelConnect => {
            refused = d
                .request(now, port, &mut |o| s.on_output(o), |t, now, tx| t.cancel_connect(now, tx))
                .is_err();
            if !refused {
                s.target = None;
            }
        }
        Req::Forget(a) => {
            refused = d
                .request(now, port, &mut |o| s.on_output(o), |t, now, tx| t.forget(Some(a), now, tx))
                .is_err();
            if !refused && s.last == Some(a) {
                s.last = None;
            }
        }
        Req::SetRx(rx) => {
            refused = port.link.is_some();
            if !refused {
                port.mode.rx = rx;
            }
        }
        Req::SetTx(dma) => {
            refused = port.link.is_some();
            if !refused {
                port.mode.tx_dma = dma;
            }
        }
        Req::Listen => {
            if port.link.is_some() || !d.is_off() {
                refused = true;
            } else {
                return Then::Listen;
            }
        }
        Req::Shutdown => {
            s.boot_on_at = None;
            s.restart_pending = false;
            return Then::ShutdownWait(d.begin_shutdown(now, port, &mut |o| s.on_output(o)));
        }
        Req::Status => {}
    }
    if test {
        let head: &[u8] = if refused {
            t!(b"refused")
        } else if s.report.is_some() {
            t!(b"started")
        } else {
            t!(b"ok")
        };
        status_reply(d, port, s, head);
    }
    Then::Nothing
}

/// The rest of the power-off sequence: the shutdown frame out, the
/// module's completion awaited (bounded inside the driver by
/// `SHUTDOWN_WAIT_MS`; this loop only adds a guard), the line low.
async fn shutdown_wait(sent: bool, test: bool, d: &mut Driver, port: &mut HwPort, s: &mut Session, now: u64) {
    flush(d, port).await;
    if sent {
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
    publish(d, s);
    SHUT_DONE.signal(());
    if test {
        let head = if sent {
            t!(b"shutdown: 0x24 sent")
        } else {
            t!(b"shutdown: module not up, enable line dropped")
        };
        status_reply(d, port, s, head);
    }
}

/// Send the pending frame, if any.
async fn flush(d: &mut Driver, port: &mut HwPort) {
    if d.tx_pending().is_empty() {
        return;
    }
    // The frame lives in the DRIVER static (SRAM0, DMA-visible) and is not
    // touched until the next call into the driver.
    let ok = match port.link.as_mut() {
        Some(l) => l.send(d.tx_pending()).await,
        None => true,
    };
    if !ok {
        TX_FAIL.fetch_add(1, Ordering::Relaxed);
    }
    d.tx_sent();
}

fn ui_phase(st: State, s: &Session) -> BtPhase {
    match st {
        State::Off if s.dead => BtPhase::NotResponding,
        State::Off if s.boot_on_at.is_some() || s.restart_pending => BtPhase::Starting,
        State::Off => BtPhase::Off,
        State::Starting => BtPhase::Starting,
        State::Idle => BtPhase::Ready,
        State::Inquiry => BtPhase::Scanning,
        State::Connecting => BtPhase::Connecting,
        State::Connected => BtPhase::Connected,
        State::Streaming => BtPhase::Streaming,
    }
}

/// Copy the module's lists into [`LISTS`] where they differ; true if
/// anything changed. In place, under one critical section (no 0.5 KB
/// temporary on the stack).
#[inline(never)]
fn publish_lists(d: &Driver, connected: Option<Addr>) -> bool {
    let t = d.transmitter();
    let conv = |x: &Device, found: bool| {
        let kind = if found { BtKind::from_cod(x.cod) } else { BtKind::Unknown };
        BtDevice::new(BtAddr(x.addr.0), kind, x.name(), Some(x.addr) == connected)
    };
    critical_section::with(|cs| {
        let mut guard = LISTS.borrow(cs).borrow_mut();
        let l = &mut *guard;
        let mut changed = false;
        for (found, src) in [(true, t.found()), (false, t.paired())] {
            let n = src.len().min(BT_DEVICES);
            let (dst, len) =
                if found { (&mut l.found, &mut l.n_found) } else { (&mut l.paired, &mut l.n_paired) };
            for (o, x) in dst.iter_mut().zip(src) {
                let v = conv(x, found);
                if *o != v {
                    *o = v;
                    changed = true;
                }
            }
            if usize::from(*len) != n {
                *len = n as u8;
                changed = true;
            }
        }
        changed
    })
}

/// State and the device's name to the UI (on change only).
fn publish(d: &Driver, s: &mut Session) {
    let connected = if matches!(d.state(), State::Connected | State::Streaming) { s.connected } else { None };
    if publish_lists(d, connected) {
        s.lists_gen = s.lists_gen.wrapping_add(1);
    }
    let mut n = Snap {
        state: ui_state(d.state()),
        last: s.last,
        last_gen: s.last_gen,
        phase: ui_phase(d.state(), s),
        connects: s.connects,
        fails: s.fails,
        drops: s.drops,
        lists_gen: s.lists_gen,
        sink_abs: connected.is_some() && s.sink_abs,
        sink_local: s.sink_local,
        sink_gen: s.sink_gen,
        ..Snap::OFF
    };
    let who = match n.phase {
        BtPhase::Connected | BtPhase::Streaming => s.connected,
        BtPhase::Connecting => s.target.or(s.last),
        _ => s.last,
    };
    if let Some(dev) = who.and_then(|a| find(d, a)) {
        let b = dev.name();
        let k = b.len().min(n.name.len());
        n.name[..k].copy_from_slice(&b[..k]);
        n.name_len = k as u8;
    }
    critical_section::with(|cs| {
        let mut cur = SNAP.borrow(cs).borrow_mut();
        if *cur != n {
            *cur = n;
        }
    });
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
        w.s(t!(b"bt listen: gpio38 falls="))
            .u(u64::from(falls.0))
            .s(t!(b" low="))
            .u(u64::from(lows.0))
            .s(t!(b" gpio39 falls="))
            .u(u64::from(falls.1))
            .s(t!(b" low="))
            .u(u64::from(lows.1))
            .s(t!(b" samples="))
            .u(u64::from(n))
            .s(t!(b"\nexpected: gpio39 (module TX) toggles, gpio38 (module RX) stays high"));
    });
}

fn hex(w: &mut Writer<'_>, v: u32, digits: u32) {
    for i in (0..digits).rev() {
        let d = ((v >> (i * 4)) & 15) as u8;
        w.s(&[if d < 10 { b'0' + d } else { b'A' + d - 10 }]);
    }
}

fn state_name(s: State) -> &'static [u8] {
    match s {
        State::Off => t!(b"off"),
        State::Starting => t!(b"starting"),
        State::Idle => t!(b"idle"),
        State::Inquiry => t!(b"inquiry"),
        State::Connecting => t!(b"connecting"),
        State::Connected => t!(b"connected"),
        State::Streaming => t!(b"streaming"),
    }
}

fn device_line(w: &mut Writer<'_>, i: usize, x: &Device) {
    w.s(t!(b"\n  ")).u(i as u64).s(t!(b" ")).s(&x.addr.to_text()).s(t!(b" cod="));
    for b in x.cod {
        hex(w, u32::from(b), 2);
    }
    w.s(t!(b" \""));
    for &c in x.name() {
        w.s(&[if (0x20..0x7F).contains(&c) { c } else { b'?' }]);
    }
    w.s(t!(b"\""));
}

fn rx_name(rx: Rx) -> &'static [u8] {
    match rx {
        Rx::Dma => t!(b"dma"),
        Rx::Cpu => t!(b"cpu"),
    }
}

fn counter(w: &mut Writer<'_>, label: &[u8], v: u32) {
    w.s(label).u(u64::from(v));
}

fn status_reply(d: &Driver, port: &HwPort, s: &Session, head: &[u8]) {
    use atj2157_hal::uart;
    let t = d.transmitter();
    let st = d.stats();
    let ls = t.link_stats();
    let ds = t.decode_stats();
    // Pads: enable, host busy, UART TX, UART RX (read only).
    let pads = [20usize, 31, 38, 39].map(|p| atj2157_hal::pac::gpio::mfp(p).read());
    crate::audio::reply(|w| {
        w.s(t!(b"bt ")).s(head).s(t!(b": state=")).s(state_name(t.state()));
        if t.shutting_down() {
            w.s(t!(b" (shutting down)"));
        }
        w.s(t!(b" module=\""));
        for &c in t.identity() {
            if (0x20..0x7F).contains(&c) {
                w.s(&[c]);
            }
        }
        w.s(t!(b"\" rx=")).s(rx_name(port.mode.rx)).s(t!(b" tx="));
        w.s(if port.mode.tx_dma { t!(b"dma") } else { t!(b"cpu") });
        w.s(t!(b"\npads 20/31/38/39 ="));
        for p in pads {
            w.s(t!(b" "));
            hex(w, p, 4);
        }
        if let Some(l) = port.link.as_ref() {
            let (sta, ctl, rem, run, peak) = l.regs();
            w.s(t!(b"\nuart sta=0x"));
            hex(w, sta, 8);
            w.s(t!(b" ctl=0x"));
            hex(w, ctl, 8);
            counter(w, t!(b" ring_rem="), rem);
            counter(w, t!(b" ring_run="), u32::from(run));
            counter(w, t!(b" peak="), peak);
            counter(w, t!(b" rx_err="), l.rx_errors);
        }
        counter(w, t!(b"\nrx bytes="), st.rx_bytes);
        counter(w, t!(b" frames="), ds.frames);
        counter(w, t!(b" skipped="), ds.skipped);
        counter(w, t!(b" badlen="), ds.bad_length);
        counter(w, t!(b" badsum="), ds.bad_checksum);
        counter(w, t!(b"\ntx frames="), st.tx_frames);
        counter(w, t!(b" bytes="), st.tx_bytes);
        counter(w, t!(b" superseded="), st.tx_dropped);
        counter(w, t!(b" fail="), TX_FAIL.load(Ordering::Relaxed));
        counter(w, t!(b" dma_timeouts="), uart::TX_DMA_TIMEOUTS.load(Ordering::Relaxed));
        counter(w, t!(b" dma_no_irq="), uart::TX_DMA_NO_IRQ.load(Ordering::Relaxed));
        counter(w, t!(b"\ncmd sent="), ls.sent);
        counter(w, t!(b" resent="), ls.resent);
        counter(w, t!(b" timeouts="), ls.timeouts);
        counter(w, t!(b" stray_acks="), ls.stray_acks);
        counter(w, t!(b" power_ups="), st.power_ups);
        counter(w, t!(b" shutdowns="), st.shutdowns);
        counter(w, t!(b"/"), st.shutdowns_completed);
        counter(w, t!(b" gave_up="), u32::from(s.gave_up));
        counter(w, t!(b" keys="), s.keys);
        if let Some(v) = s.sink_volume {
            counter(w, t!(b" sink_vol="), u32::from(v));
        }
        if let Some(a) = s.connected {
            w.s(t!(b"\nconnected ")).s(&a.to_text());
        }
        counter(w, t!(b"\npaired="), t.paired().len() as u32);
        for (i, x) in t.paired().iter().enumerate() {
            device_line(w, i, x);
        }
        counter(w, t!(b"\nfound="), t.found().len() as u32);
        for (i, x) in t.found().iter().enumerate() {
            device_line(w, i, x);
        }
        w.s(t!(b"\nrx tail:"));
        let (a, b) = st.last_rx();
        for &c in a.iter().chain(b) {
            w.s(t!(b" "));
            hex(w, u32::from(c), 2);
        }
    });
}
