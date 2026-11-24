//! Transmitter-role state machine (06-connectivity §4.2, bluetooth.md
//! §3.3, §4):
//!
//! ```text
//! Off → Starting → Idle ⇄ Inquiry
//!                  Idle → Connecting → Connected ⇄ Streaming
//! ```
//!
//! Start-up (bluetooth.md §4.1, minus the restricted steps): enable line
//! high; wait up to 5 s for event 0x23 (ready), else probe with `06 00`;
//! `06 02` (identity); `30 01 01`; `2A FF` and collect the paired list
//! (up to 5 s, or until `total` records arrived) — or `2B FF` when the
//! player has no saved record; then Idle, and a bounded auto-reconnect to
//! the last device. The vendor's settings read/write (0x2C/0x7F) is
//! skipped (CLAUDE.md rule 6; the module keeps its own settings).
//! Switching Bluetooth off drops the enable line ([`Transmitter::power_off`]);
//! the player's power-off first sends the shutdown command 0x24 through
//! [`Transmitter::shutdown`] (owner decision 2026-11-24, WP BT-2) and drops
//! the line on the module's completion event or after a bounded wait.
//!
//! A command that is not acknowledged after its resends declares the link
//! down: the enable line goes low, and after a pause the module is started
//! again, at most [`Config::max_restarts`] times in a row.
//!
//! Sans-I/O: every method takes the time (ms, monotonic) and a
//! [`Transport`]; results come out of [`Transmitter::next_output`].

use crate::frame::{Decoder, Frame, MAX_TX_FRAME};
use crate::link::{CommandLayer, DEFAULT_RETRIES, Poll, Transport};
use crate::proto::{Addr, Command, Event, Key, LinkState, volume_from_sink, volume_to_sink};

/// Paired records the module reports at most.
pub const MAX_PAIRED: usize = 8;
/// Inquiry results kept (the vendor keeps 10; 8 saves SRAM).
pub const MAX_FOUND: usize = 8;
/// Longest device name kept, bytes (the vendor keeps 31; a 160-pixel
/// line shows about 20 characters, and every byte here costs 16 bytes of
/// SRAM).
pub const MAX_NAME: usize = 24;
/// Command queue, bytes: frames are kept encoded, 3 bytes of header
/// each. Holds a whole metadata burst ([`META_QUEUE_BYTES`]) or a few
/// dozen small commands; a byte ring instead of fixed slots saves RAM
/// (most frames are 5–11 bytes, metadata fields up to 78).
const QUEUE_BYTES: usize = 264;
/// Queue bytes one [`Transmitter::send_metadata`] needs: seven frames
/// (6 + 8 + 78 + 58 + 26 + 58 + 6 bytes) and their headers.
const META_QUEUE_BYTES: usize = 240 + 7 * 3;
// A whole metadata burst fits an empty queue.
const _: () = assert!(META_QUEUE_BYTES <= QUEUE_BYTES);
/// Outputs between two drains. The firmware drains after every call;
/// should one be lost anyway, [`Transmitter::state`] says whether the
/// enable line belongs high (the driver checks it).
const OUTPUTS: usize = 12;
/// After a connection, sink volume reports that differ from the
/// player's are its start-up value and are answered with ours.
pub const VOLUME_SYNC_MS: u64 = 3000;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    Off,
    Starting,
    Idle,
    Inquiry,
    Connecting,
    Connected,
    Streaming,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Config {
    /// Reconnect to the last device after start-up.
    pub auto_reconnect: bool,
    /// Connection attempts after start-up (bounded).
    pub reconnect_attempts: u8,
    /// Pause between two reconnect attempts.
    pub reconnect_gap_ms: u64,
    /// A connection attempt is cancelled (`29`) after this long.
    pub connect_timeout_ms: u64,
    /// Wait for the ready event before probing.
    pub ready_timeout_ms: u64,
    /// Wait for paired-list records.
    pub paired_timeout_ms: u64,
    /// Module power-cycles after a link failure before giving up.
    pub max_restarts: u8,
    /// Pause with the enable line low before a restart.
    pub restart_delay_ms: u64,
    /// The player has no saved Bluetooth record (first start, settings
    /// reset): forget all pairings in the module instead of reading them.
    pub forget_all_on_start: bool,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            auto_reconnect: true,
            reconnect_attempts: 3,
            reconnect_gap_ms: 2000,
            connect_timeout_ms: 15_000,
            ready_timeout_ms: 5000,
            paired_timeout_ms: 5000,
            max_restarts: 2,
            restart_delay_ms: 500,
            forget_all_on_start: false,
        }
    }
}

/// A found or paired device.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Device {
    pub addr: Addr,
    /// Class of device (inquiry results only).
    pub cod: [u8; 3],
    name: [u8; MAX_NAME],
    name_len: u8,
}

impl Device {
    fn new(addr: Addr, cod: [u8; 3], name: &[u8]) -> Self {
        let mut d = Self { addr, cod, ..Self::default() };
        d.set_name(name);
        d
    }

    fn set_name(&mut self, name: &[u8]) {
        let n = name.len().min(MAX_NAME);
        self.name[..n].copy_from_slice(&name[..n]);
        self.name_len = n as u8;
    }

    /// Name bytes as the module sent them (normally UTF-8; may be cut).
    #[must_use]
    pub fn name(&self) -> &[u8] {
        &self.name[..usize::from(self.name_len).min(MAX_NAME)]
    }
}

/// Things the firmware acts on.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Output {
    /// Drive the module enable line (GPIO20).
    Enable(bool),
    State(State),
    /// The identity string is available ([`Transmitter::identity`]).
    Identity,
    /// Paired list complete ([`Transmitter::paired`]).
    PairedList,
    /// Inquiry result at this index of [`Transmitter::found`].
    Found(u8),
    /// A name arrived for this index of [`Transmitter::found`].
    NameResolved(u8),
    InquiryDone,
    /// Connected; save as the last device.
    Connected(Addr),
    Disconnected,
    ConnectFailed,
    /// Headset key for the playback service.
    Key(Key),
    /// The sink changed its volume: raw 0–127 and the player's 0–31.
    SinkVolume {
        raw: u8,
        local: u8,
    },
    /// Absolute-volume support of the sink, with its volume.
    AbsVolume(Option<u8>),
    /// The module acknowledged an absolute volume (command 0x26) while
    /// connected: the sink scales from now on (06 §4.2).
    VolumeAcked,
    /// A command went unacknowledged; the module is being restarted
    /// (`restarting`) or given up on.
    LinkDown {
        restarting: bool,
    },
}

/// Request refused in the current state (or the command queue is full).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Refused;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    None,
    WaitReady { until: u64 },
    Probe,
    Setup,
    Paired { until: u64 },
}

/// Encoded commands waiting for the link: `id, retries, len, frame…`
/// per entry, in a byte ring.
struct Queue {
    buf: [u8; QUEUE_BYTES],
    head: usize,
    len: usize,
}

impl Queue {
    const fn new() -> Self {
        Self { buf: [0; QUEUE_BYTES], head: 0, len: 0 }
    }

    fn free(&self) -> usize {
        QUEUE_BYTES - self.len
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn clear(&mut self) {
        self.len = 0;
    }

    fn push(&mut self, id: u8, retries: u8, frame: &[u8]) -> bool {
        if frame.len() > MAX_TX_FRAME || 3 + frame.len() > self.free() {
            return false;
        }
        for &b in [id, retries, frame.len() as u8].iter().chain(frame) {
            self.buf[(self.head + self.len) % QUEUE_BYTES] = b;
            self.len += 1;
        }
        true
    }

    fn take_byte(&mut self) -> u8 {
        let b = self.buf[self.head % QUEUE_BYTES];
        self.head = (self.head + 1) % QUEUE_BYTES;
        self.len -= 1;
        b
    }

    /// The oldest entry into `out`: (id, retries, frame length).
    fn pop(&mut self, out: &mut [u8; MAX_TX_FRAME]) -> Option<(u8, u8, usize)> {
        if self.len < 3 {
            return None;
        }
        let id = self.take_byte();
        let retries = self.take_byte();
        // `push` stored at most MAX_TX_FRAME bytes.
        let n = usize::from(self.take_byte()).min(MAX_TX_FRAME);
        for o in &mut out[..n] {
            *o = self.take_byte();
        }
        Some((id, retries, n))
    }
}

/// Track metadata for the sink (command 0x31).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct TrackInfo<'a> {
    pub length_s: u16,
    /// Title, then three more ID3-style fields (which of artist / album /
    /// genre the sink shows where is unknown, bluetooth.md §3.6).
    pub fields: [&'a str; 4],
}

pub struct Transmitter {
    cfg: Config,
    state: State,
    phase: Phase,
    link: CommandLayer,
    decoder: Decoder,
    queue: Queue,
    out: [Output; OUTPUTS],
    o_head: usize,
    o_len: usize,
    /// Outputs dropped because nobody drained them.
    pub outputs_dropped: u32,
    identity: [u8; 8],
    identity_len: u8,
    paired: [Device; MAX_PAIRED],
    paired_len: u8,
    paired_total: u8,
    found: [Device; MAX_FOUND],
    found_len: u8,
    last_device: Option<Addr>,
    audio_active: bool,
    /// The player's volume (0–31): pushed to the sink at every connection
    /// and on every change while connected (06 §4.2; the sink's absolute
    /// volume is 0 after every connection on BlueZ, so without this the
    /// stream is silent).
    volume: Option<u8>,
    /// Until then, volume reports from the sink that differ from ours are
    /// answered with ours (its start-up value, not a user's change).
    volume_sync_until: u64,
    /// Connection the firmware asked for (after a disconnect, or a
    /// reconnect attempt).
    want: Option<Addr>,
    connect_deadline: Option<u64>,
    reconnect_left: u8,
    reconnect_at: Option<u64>,
    restarts: u8,
    restart_at: Option<u64>,
    /// The shutdown command (0x24) is out: no restarts, no other commands.
    shutting_down: bool,
}

impl Transmitter {
    #[must_use]
    pub fn new(cfg: Config) -> Self {
        Self {
            cfg,
            state: State::Off,
            phase: Phase::None,
            link: CommandLayer::new(),
            decoder: Decoder::new(),
            queue: Queue::new(),
            out: [Output::Disconnected; OUTPUTS],
            o_head: 0,
            o_len: 0,
            outputs_dropped: 0,
            identity: [0; 8],
            identity_len: 0,
            paired: [Device::default(); MAX_PAIRED],
            paired_len: 0,
            paired_total: 0,
            found: [Device::default(); MAX_FOUND],
            found_len: 0,
            last_device: None,
            audio_active: false,
            volume: None,
            volume_sync_until: 0,
            want: None,
            connect_deadline: None,
            reconnect_left: 0,
            reconnect_at: None,
            restarts: 0,
            restart_at: None,
            shutting_down: false,
        }
    }

    // ----- accessors -----

    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    #[must_use]
    pub fn identity(&self) -> &[u8] {
        &self.identity[..usize::from(self.identity_len).min(8)]
    }

    #[must_use]
    pub fn paired(&self) -> &[Device] {
        &self.paired[..usize::from(self.paired_len).min(MAX_PAIRED)]
    }

    #[must_use]
    pub fn found(&self) -> &[Device] {
        &self.found[..usize::from(self.found_len).min(MAX_FOUND)]
    }

    /// The last frame given to the transport ([`CommandLayer::last_frame`]).
    #[must_use]
    pub fn last_frame(&self) -> &[u8] {
        self.link.last_frame()
    }

    #[must_use]
    pub fn link_stats(&self) -> crate::link::LinkStats {
        self.link.stats
    }

    #[must_use]
    pub fn decode_stats(&self) -> crate::frame::DecodeStats {
        self.decoder.stats
    }

    /// Set the last connected device (from the player's saved record).
    pub fn set_last_device(&mut self, a: Option<Addr>) {
        self.last_device = a;
    }

    /// Next output for the firmware, oldest first.
    pub fn next_output(&mut self) -> Option<Output> {
        if self.o_len == 0 {
            return None;
        }
        let o = self.out[self.o_head % OUTPUTS];
        self.o_head = (self.o_head + 1) % OUTPUTS;
        self.o_len -= 1;
        Some(o)
    }

    // ----- internals -----

    fn emit(&mut self, o: Output) {
        if self.o_len == OUTPUTS {
            self.outputs_dropped += 1;
            return;
        }
        self.out[(self.o_head + self.o_len) % OUTPUTS] = o;
        self.o_len += 1;
    }

    fn set_state(&mut self, s: State) {
        if self.state != s {
            self.state = s;
            self.emit(Output::State(s));
        }
    }

    fn enqueue(&mut self, c: &Command<'_>, retries: u8) -> Result<(), Refused> {
        let mut f = [0u8; MAX_TX_FRAME];
        let n = c.encode(&mut f).map_err(|_| Refused)?;
        let f = f.get(..n).ok_or(Refused)?;
        if self.queue.push(c.id(), retries, f) { Ok(()) } else { Err(Refused) }
    }

    fn pump(&mut self, now: u64, tx: &mut impl Transport) {
        if self.link.is_busy() || self.queue.is_empty() || self.state == State::Off || self.shutting_down {
            return;
        }
        let mut f = [0u8; MAX_TX_FRAME];
        if let Some((id, retries, n)) = self.queue.pop(&mut f) {
            // Not busy, and the frame came from `Command::encode`.
            let _ = self.link.send_encoded(id, f.get(..n).unwrap_or(&[]), retries, now, tx);
        }
    }

    fn clear_session(&mut self) {
        self.queue.clear();
        self.link.reset();
        self.decoder.reset();
        self.phase = Phase::None;
        self.want = None;
        self.connect_deadline = None;
        self.reconnect_at = None;
    }

    fn start(&mut self, now: u64) {
        self.clear_session();
        self.shutting_down = false;
        self.paired_len = 0;
        self.paired_total = 0;
        self.restart_at = None;
        self.reconnect_left = if self.cfg.auto_reconnect { self.cfg.reconnect_attempts } else { 0 };
        self.emit(Output::Enable(true));
        self.set_state(State::Starting);
        self.phase = Phase::WaitReady { until: now + self.cfg.ready_timeout_ms };
    }

    /// The module is ready (event 0x23 or a probe acknowledge): run the
    /// set-up commands.
    fn ready(&mut self, was_connected: bool) {
        self.clear_session();
        if was_connected {
            self.emit(Output::Disconnected);
        }
        self.set_state(State::Starting);
        self.phase = Phase::Setup;
        self.paired_len = 0;
        self.paired_total = 0;
        let _ = self.enqueue(&Command::Identity(2), DEFAULT_RETRIES);
        let _ = self.enqueue(&Command::StartupMagic, DEFAULT_RETRIES);
        let last = if self.cfg.forget_all_on_start { Command::ForgetAll } else { Command::ReadPaired };
        let _ = self.enqueue(&last, DEFAULT_RETRIES);
    }

    fn finish_start(&mut self, now: u64) {
        self.phase = Phase::None;
        self.restarts = 0;
        self.emit(Output::PairedList);
        self.set_state(State::Idle);
        if let Some(a) = self.last_device
            && self.reconnect_left > 0
            && self.paired().iter().any(|d| d.addr == a)
        {
            self.reconnect_left -= 1;
            self.begin_connect(a, now);
        }
    }

    fn begin_connect(&mut self, a: Addr, now: u64) {
        if self.enqueue(&Command::Connect(a), DEFAULT_RETRIES).is_ok() {
            self.want = Some(a);
            self.connect_deadline = Some(now + self.cfg.connect_timeout_ms);
            self.set_state(State::Connecting);
        }
    }

    fn link_down(&mut self, now: u64) {
        self.clear_session();
        self.emit(Output::Enable(false));
        let was = self.state;
        self.state = State::Off;
        if was != State::Off {
            self.emit(Output::State(State::Off));
        }
        let restarting = self.restarts < self.cfg.max_restarts;
        if restarting {
            self.restarts += 1;
            self.restart_at = Some(now + self.cfg.restart_delay_ms);
        }
        self.emit(Output::LinkDown { restarting });
    }

    fn connect_failed(&mut self, now: u64) {
        self.connect_deadline = None;
        self.want = None;
        self.emit(Output::ConnectFailed);
        self.set_state(State::Idle);
        if self.reconnect_left > 0 {
            self.reconnect_at = Some(now + self.cfg.reconnect_gap_ms);
        }
    }

    fn on_ack(&mut self, id: u8, now: u64) {
        match (id, self.phase) {
            (0x06, Phase::Probe) => self.ready(false),
            (0x2A, Phase::Setup) => {
                self.phase = Phase::Paired { until: now + self.cfg.paired_timeout_ms };
            }
            (0x2B, Phase::Setup) => self.finish_start(now),
            (0x01, _) if self.state == State::Inquiry => self.inquiry_finished(now),
            (0x26, _) if matches!(self.state, State::Connected | State::Streaming) => {
                self.emit(Output::VolumeAcked);
            }
            _ => {}
        }
    }

    fn on_frame(&mut self, f: &Frame, now: u64) {
        if let Some(id) = self.link.on_frame(f) {
            self.on_ack(id, now);
            return;
        }
        match Event::parse(f) {
            Event::Ready => {
                // Ready at power-up, or a module restart at any time.
                if self.state != State::Off && !self.shutting_down {
                    let was = matches!(self.state, State::Connected | State::Streaming);
                    self.ready(was);
                }
            }
            Event::Identity(t) => {
                let n = t.len().min(8);
                self.identity[..n].copy_from_slice(&t[..n]);
                self.identity_len = n as u8;
                self.emit(Output::Identity);
            }
            Event::Paired { total, addr, name, .. } => {
                self.paired_total = total.min(MAX_PAIRED as u8);
                if !self.paired().iter().any(|d| d.addr == addr)
                    && let Some(slot) = self.paired.get_mut(usize::from(self.paired_len))
                {
                    *slot = Device::new(addr, [0; 3], name);
                    self.paired_len += 1;
                }
                if matches!(self.phase, Phase::Paired { .. }) && self.paired_len >= self.paired_total {
                    self.finish_start(now);
                } else if self.phase == Phase::None && self.paired_len >= self.paired_total {
                    self.emit(Output::PairedList);
                }
            }
            Event::InquiryResult { addr, cod, name, .. } => {
                let i = match self.found().iter().position(|d| d.addr == addr) {
                    Some(i) => i,
                    None if usize::from(self.found_len) < MAX_FOUND => {
                        self.found_len += 1;
                        usize::from(self.found_len) - 1
                    }
                    None => return,
                };
                if let Some(d) = self.found.get_mut(i) {
                    *d = Device::new(addr, cod, name);
                    self.emit(Output::Found(i as u8));
                }
            }
            Event::InquiryDone => {
                if self.state == State::Inquiry {
                    self.inquiry_finished(now);
                }
            }
            Event::RemoteName { addr, name } => {
                if let Some(i) = self.found().iter().position(|d| d.addr == addr)
                    && let Some(d) = self.found.get_mut(i)
                {
                    d.set_name(name);
                    self.emit(Output::NameResolved(i as u8));
                }
            }
            Event::Link { state, addr, .. } => self.on_link(state, addr, now),
            Event::CancelConnect { status } => {
                if status == 0 && self.state == State::Connecting {
                    self.connect_failed(now);
                }
            }
            Event::Key(k) => self.emit(Output::Key(k)),
            // Above AVRCP's 0–127: the module's "no absolute volume" (128
            // from a BlueZ sink). Reported, but the player's volume stays
            // (it used to become up to 62, or 31 for 128, so the next
            // connection pushed the maximum; VER-4 finding F-BT-1).
            Event::SinkVolume(v) if v > 127 => {
                self.emit(Output::SinkVolume { raw: v, local: volume_from_sink(v) });
            }
            Event::SinkVolume(v) => {
                if now < self.volume_sync_until && self.differs(v) {
                    self.push_volume();
                } else {
                    let local = volume_from_sink(v);
                    self.volume = Some(local);
                    self.emit(Output::SinkVolume { raw: v, local });
                }
            }
            Event::AbsVolume(v) => {
                // The answer to our query: support, not a user's change.
                if let Some(raw) = v
                    && self.differs(raw)
                {
                    self.push_volume();
                }
                self.emit(Output::AbsVolume(v));
            }
            Event::ShutdownComplete => {
                self.clear_session();
                self.shutting_down = false;
                self.restart_at = None;
                self.emit(Output::Enable(false));
                self.set_state(State::Off);
            }
            Event::Ack { .. } | Event::Upgrade { .. } | Event::Settings { .. } | Event::Other { .. } => {}
        }
    }

    /// The inquiry ended (its own end, or a stop): Idle; then the
    /// connection asked for during it, or the names of the devices found
    /// without one (command 0x02 each, as the vendor's UI does).
    fn inquiry_finished(&mut self, now: u64) {
        self.set_state(State::Idle);
        self.emit(Output::InquiryDone);
        if let Some(a) = self.want.take() {
            self.begin_connect(a, now);
            return;
        }
        for i in 0..self.found().len() {
            if let Some(&d) = self.found.get(i)
                && d.name_len == 0
                && self.enqueue(&Command::RemoteName(d.addr), DEFAULT_RETRIES).is_err()
            {
                break;
            }
        }
    }

    /// A newly connected device is paired now (the module pairs on
    /// connect): add it to the paired list with the name it was found
    /// under, so the list is right without reading it again.
    fn note_paired(&mut self, a: Addr) {
        if self.paired().iter().any(|d| d.addr == a) {
            return;
        }
        let name = self.found().iter().find(|d| d.addr == a).map(|d| (d.name, d.name_len));
        if let Some(slot) = self.paired.get_mut(usize::from(self.paired_len)) {
            *slot = Device { addr: a, ..Device::default() };
            if let Some((n, l)) = name {
                slot.name = n;
                slot.name_len = l;
            }
            self.paired_len += 1;
            self.emit(Output::PairedList);
        }
    }

    fn on_link(&mut self, st: LinkState, addr: Addr, now: u64) {
        match st {
            LinkState::Connected => {
                self.note_paired(addr);
                self.want = None;
                self.connect_deadline = None;
                self.reconnect_left = 0;
                self.reconnect_at = None;
                self.last_device = Some(addr);
                self.emit(Output::Connected(addr));
                self.push_volume();
                self.volume_sync_until = now + VOLUME_SYNC_MS;
                if self.audio_active && self.enqueue(&Command::Audio(true), DEFAULT_RETRIES).is_ok() {
                    self.set_state(State::Streaming);
                } else {
                    self.set_state(State::Connected);
                }
            }
            LinkState::Disconnected => {
                let was = matches!(self.state, State::Connected | State::Streaming);
                if let Some(a) = self.want
                    && was
                {
                    // Disconnect-then-connect requested by `connect`.
                    self.emit(Output::Disconnected);
                    self.set_state(State::Idle);
                    self.begin_connect(a, now);
                } else if was {
                    self.emit(Output::Disconnected);
                    self.set_state(State::Idle);
                } else if self.state == State::Connecting {
                    self.connect_failed(now);
                }
            }
            LinkState::Failed => {
                if self.state == State::Connecting {
                    self.connect_failed(now);
                }
            }
            LinkState::Connecting => {
                // Also sent for reconnects the module starts itself.
                if self.state == State::Idle {
                    self.set_state(State::Connecting);
                }
            }
            LinkState::ControlUp => {
                // The vendor arms a volume sync here: ask the sink.
                let _ = self.enqueue(&Command::ReadAbsVolume, DEFAULT_RETRIES);
            }
            LinkState::ControlDown | LinkState::Other(_) => {}
        }
    }

    // ----- inputs -----

    /// Bytes from the UART.
    pub fn on_bytes(&mut self, bytes: &[u8], now: u64, tx: &mut impl Transport) {
        // Decode in small batches so frames are handled in order (a byte
        // completes at most two frames: a rejected candidate's rescan can
        // release one ahead of the current one).
        let mut frames = [Frame::new(0, &[]); 4];
        let mut rest = bytes;
        while !rest.is_empty() {
            let mut n = 0;
            while let [b, tail @ ..] = rest
                && n < frames.len() - 1
            {
                self.decoder.push(*b, &mut |f| {
                    if let Some(slot) = frames.get_mut(n) {
                        *slot = f;
                        n += 1;
                    }
                });
                rest = tail;
            }
            for f in frames.iter().take(n) {
                self.on_frame(f, now);
            }
        }
        self.pump(now, tx);
    }

    /// Call every 10 ms or so: timeouts, restarts, reconnects, sending.
    pub fn tick(&mut self, now: u64, tx: &mut impl Transport) {
        if let Some(t) = self.restart_at
            && now >= t
            && self.state == State::Off
        {
            self.start(now);
        }
        match self.link.poll(now, tx) {
            Poll::Timeout { .. } if self.shutting_down => {
                // No acknowledge of the shutdown command: power off anyway.
                self.power_off();
                return;
            }
            Poll::Timeout { .. } => {
                self.link_down(now);
                return;
            }
            Poll::Idle | Poll::Waiting | Poll::Resent { .. } => {}
        }
        match self.phase {
            Phase::WaitReady { until } if now >= until => {
                self.phase = Phase::Probe;
                let _ = self.enqueue(&Command::Identity(0), DEFAULT_RETRIES);
            }
            Phase::Paired { until } if now >= until => self.finish_start(now),
            _ => {}
        }
        if let Some(d) = self.connect_deadline
            && now >= d
            && self.state == State::Connecting
        {
            self.connect_deadline = None;
            let _ = self.enqueue(&Command::CancelConnect, DEFAULT_RETRIES);
        }
        if let Some(t) = self.reconnect_at
            && now >= t
        {
            self.reconnect_at = None;
            if self.state == State::Idle
                && let Some(a) = self.last_device
                && self.reconnect_left > 0
            {
                self.reconnect_left -= 1;
                self.begin_connect(a, now);
            }
        }
        self.pump(now, tx);
    }

    /// Bluetooth on: power the module and run the start-up.
    pub fn power_on(&mut self, now: u64, tx: &mut impl Transport) {
        if self.state == State::Off {
            self.restarts = 0;
            self.start(now);
        }
        self.pump(now, tx);
    }

    /// Bluetooth off: enable line low, no shutdown command (the player's
    /// power-off uses [`Self::shutdown`] first).
    pub fn power_off(&mut self) {
        self.clear_session();
        self.shutting_down = false;
        self.restart_at = None;
        if self.state != State::Off {
            self.emit(Output::Enable(false));
            self.set_state(State::Off);
        }
    }

    /// The module finished its start-up and takes commands (Idle and the
    /// states after it).
    #[must_use]
    pub fn module_up(&self) -> bool {
        matches!(
            self.state,
            State::Idle | State::Inquiry | State::Connecting | State::Connected | State::Streaming
        )
    }

    /// Commands queued or awaiting their acknowledge.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.link.is_busy() || !self.queue.is_empty()
    }

    /// The shutdown command is out; waiting for the module.
    #[must_use]
    pub fn shutting_down(&self) -> bool {
        self.shutting_down
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    /// Change the configuration (takes effect at the next start-up).
    pub fn set_config(&mut self, cfg: Config) {
        self.cfg = cfg;
    }

    /// **Player power-off only** (owner decision 2026-11-24, WP BT-2; the
    /// one caller of a restricted command in this crate): drop everything
    /// queued and send the shutdown command 0x24 once, without resend.
    /// The module answers with its acknowledge and event 0x24, which gives
    /// `Enable(false)` and `State(Off)`; without an acknowledge within
    /// [`crate::link::ACK_TIMEOUT_MS`] the same happens at the next
    /// [`Self::tick`]. Refused unless the module is up
    /// ([`Self::module_up`]): during its start-up the enable line is
    /// simply dropped ([`Self::power_off`]). The caller bounds the wait
    /// for the completion event and calls `power_off` when it runs out.
    pub fn shutdown(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if !self.module_up() || self.shutting_down {
            return Err(Refused);
        }
        self.clear_session();
        self.restart_at = None;
        self.reconnect_left = 0;
        self.shutting_down = true;
        crate::restricted::send_restricted(
            &mut self.link,
            &crate::restricted::RestrictedCommand::Shutdown,
            0,
            now,
            tx,
        )
        .map_err(|_| Refused)
    }

    pub fn start_inquiry(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if self.state != State::Idle {
            return Err(Refused);
        }
        self.enqueue(&Command::StartInquiry, DEFAULT_RETRIES)?;
        self.found_len = 0;
        self.set_state(State::Inquiry);
        self.pump(now, tx);
        Ok(())
    }

    pub fn stop_inquiry(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if self.state != State::Inquiry {
            return Err(Refused);
        }
        self.enqueue(&Command::StopInquiry, DEFAULT_RETRIES)?;
        self.pump(now, tx);
        Ok(())
    }

    /// Ask for the names of found devices that came without one.
    pub fn resolve_names(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if !matches!(self.state, State::Idle | State::Connected | State::Streaming) {
            return Err(Refused);
        }
        for i in 0..self.found().len() {
            if let Some(&d) = self.found.get(i)
                && d.name_len == 0
            {
                self.enqueue(&Command::RemoteName(d.addr), DEFAULT_RETRIES)?;
            }
        }
        self.pump(now, tx);
        Ok(())
    }

    /// Connect to `a`, disconnecting another device first.
    pub fn connect(&mut self, a: Addr, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        self.reconnect_left = 0;
        self.reconnect_at = None;
        match self.state {
            State::Idle => self.begin_connect(a, now),
            State::Inquiry => {
                // Stop the inquiry; the connection follows its end.
                if self.want.is_none() {
                    self.enqueue(&Command::StopInquiry, DEFAULT_RETRIES)?;
                }
                self.want = Some(a);
            }
            State::Connected | State::Streaming => {
                if self.last_device == Some(a) {
                    return Ok(());
                }
                self.enqueue(&Command::Disconnect, DEFAULT_RETRIES)?;
                self.want = Some(a);
            }
            _ => return Err(Refused),
        }
        self.pump(now, tx);
        Ok(())
    }

    pub fn cancel_connect(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if self.state == State::Inquiry && self.want.is_some() {
            // Asked for during an inquiry, not started yet.
            self.want = None;
            return Ok(());
        }
        if self.state != State::Connecting {
            return Err(Refused);
        }
        self.reconnect_left = 0;
        self.enqueue(&Command::CancelConnect, DEFAULT_RETRIES)?;
        self.pump(now, tx);
        Ok(())
    }

    pub fn disconnect(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if !matches!(self.state, State::Connected | State::Streaming) {
            return Err(Refused);
        }
        self.want = None;
        self.enqueue(&Command::Disconnect, DEFAULT_RETRIES)?;
        self.pump(now, tx);
        Ok(())
    }

    /// Forget one paired device, or all (`None`).
    pub fn forget(&mut self, a: Option<Addr>, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if matches!(self.state, State::Off | State::Starting) {
            return Err(Refused);
        }
        self.enqueue(&a.map_or(Command::ForgetAll, Command::Forget), DEFAULT_RETRIES)?;
        match a {
            Some(a) => {
                let n = self.paired().len();
                if let Some(i) = self.paired().iter().position(|d| d.addr == a) {
                    if let Some(tail) = self.paired.get_mut(i..n) {
                        tail.rotate_left(1);
                    }
                    self.paired_len -= 1;
                }
            }
            None => self.paired_len = 0,
        }
        if a.is_none() || a == self.last_device {
            self.last_device = None;
        }
        self.pump(now, tx);
        Ok(())
    }

    /// Re-read the paired list (Idle only).
    pub fn refresh_paired(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if self.state != State::Idle {
            return Err(Refused);
        }
        self.enqueue(&Command::ReadPaired, DEFAULT_RETRIES)?;
        self.paired_len = 0;
        self.paired_total = 0;
        self.pump(now, tx);
        Ok(())
    }

    /// Playback output started/stopped (command 0x05). Remembered while
    /// not connected and sent on connection.
    pub fn set_audio_active(&mut self, on: bool, now: u64, tx: &mut impl Transport) {
        self.audio_active = on;
        match (self.state, on) {
            (State::Connected, true) if self.enqueue(&Command::Audio(true), DEFAULT_RETRIES).is_ok() => {
                self.set_state(State::Streaming);
            }
            (State::Streaming, false) if self.enqueue(&Command::Audio(false), DEFAULT_RETRIES).is_ok() => {
                self.set_state(State::Connected);
            }
            _ => {}
        }
        self.pump(now, tx);
    }

    /// The player's volume 0–31: remembered, and sent to the sink
    /// (absolute volume, command 0x26) now if connected and again at
    /// every connection. Refused only when the command queue is full.
    pub fn set_volume(&mut self, local: u8, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        let local = local.min(31);
        self.volume = Some(local);
        if matches!(self.state, State::Connected | State::Streaming) {
            self.enqueue(&Command::SetAbsVolume(volume_to_sink(local)), DEFAULT_RETRIES)?;
            self.pump(now, tx);
        }
        Ok(())
    }

    /// The player's volume as last set or taken from the sink.
    #[must_use]
    pub fn volume(&self) -> Option<u8> {
        self.volume
    }

    fn differs(&self, raw: u8) -> bool {
        self.volume.is_some_and(|v| volume_to_sink(v) != raw)
    }

    /// Queue the player's volume for the sink (connected only).
    fn push_volume(&mut self) {
        if let Some(v) = self.volume {
            let _ = self.enqueue(&Command::SetAbsVolume(volume_to_sink(v)), DEFAULT_RETRIES);
        }
    }

    /// Ask the sink for its absolute volume (answer: `Output::AbsVolume`).
    pub fn query_volume(&mut self, now: u64, tx: &mut impl Transport) -> Result<(), Refused> {
        if !matches!(self.state, State::Connected | State::Streaming) {
            return Err(Refused);
        }
        self.enqueue(&Command::ReadAbsVolume, DEFAULT_RETRIES)?;
        self.pump(now, tx);
        Ok(())
    }

    /// Send track metadata (seven commands, each resent up to three times
    /// as the vendor does).
    pub fn send_metadata(
        &mut self,
        t: &TrackInfo<'_>,
        now: u64,
        tx: &mut impl Transport,
    ) -> Result<(), Refused> {
        if !matches!(self.state, State::Connected | State::Streaming) || self.queue.free() < META_QUEUE_BYTES
        {
            return Err(Refused);
        }
        self.enqueue(&Command::MetaBegin, 3)?;
        self.enqueue(&Command::MetaLength(t.length_s), 3)?;
        for (i, f) in t.fields.iter().enumerate() {
            self.enqueue(&Command::MetaField(i as u8, f), 3)?;
        }
        self.enqueue(&Command::MetaEnd, 3)?;
        self.pump(now, tx);
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::{checksum, encode_unchecked, is_restricted};
    use std::vec::Vec;

    #[derive(Default)]
    struct Tx(Vec<Vec<u8>>);
    impl Transport for Tx {
        fn send(&mut self, f: &[u8]) {
            // Every frame the state machine sends must be well formed and
            // must not be restricted. (Metadata frames exceed the 44-byte
            // limit of the receive side, so check the format directly.)
            assert!(!is_restricted(f[3]), "restricted command {:#04x} sent", f[3]);
            assert_eq!(f[0], 0xAA);
            assert_eq!(usize::from(u16::from_be_bytes([f[1], f[2]])), f.len() - 4, "{f:02X?}");
            assert_eq!(checksum(&f[1..]), 0, "{f:02X?}");
            self.0.push(f.to_vec());
        }
    }
    impl Tx {
        fn take(&mut self) -> Vec<Vec<u8>> {
            core::mem::take(&mut self.0)
        }
    }

    fn ev(id: u8, p: &[u8]) -> Vec<u8> {
        let mut b = [0u8; 64];
        let n = encode_unchecked(id, p, &mut b).unwrap();
        b[..n].to_vec()
    }
    fn ack(id: u8) -> Vec<u8> {
        ev(0, &[id])
    }
    fn outputs(t: &mut Transmitter) -> Vec<Output> {
        core::iter::from_fn(|| t.next_output()).collect()
    }

    const HEADSET: Addr = Addr([0x11, 0x22, 0x33, 0x44, 0x55, 0x66]);

    /// The start-up exchange preserved in the RAM dump (bluetooth.md
    /// §4.1), replayed byte for byte. The vendor sent 06 02, 2C, 30 01 01,
    /// 2A FF; we skip 2C, so its acknowledge and event arrive unasked and
    /// are ignored.
    #[test]
    fn replay_ram_dump_startup() {
        let mut t = Transmitter::new(Config::default());
        let mut tx = Tx::default();
        t.power_on(0, &mut tx);
        assert_eq!(outputs(&mut t), [Output::Enable(true), Output::State(State::Starting)]);
        assert!(tx.take().is_empty());
        // Stray 00, then ready.
        t.on_bytes(&[0x00, 0xAA, 0x00, 0x01, 0x23, 0xDC], 120, &mut tx);
        assert_eq!(tx.take(), [std::vec![0xAA, 0x00, 0x02, 0x06, 0x02, 0xF6]]);
        t.on_bytes(&[0xAA, 0x00, 0x02, 0x00, 0x06, 0xF8], 130, &mut tx);
        assert_eq!(tx.take(), [std::vec![0xAA, 0x00, 0x03, 0x30, 0x01, 0x01, 0xCB]]);
        t.on_bytes(b"\xAA\x00\x09\x06ATS2851\x00\x39", 135, &mut tx);
        assert_eq!(t.identity(), b"ATS2851\0");
        // Unsolicited (for us) settings exchange from the dump.
        t.on_bytes(
            &[0xAA, 0x00, 0x02, 0x00, 0x2C, 0xD2, 0xAA, 0x00, 0x03, 0x2C, 0x06, 0x07, 0xC4],
            140,
            &mut tx,
        );
        assert!(tx.take().is_empty());
        t.on_bytes(&[0xAA, 0x00, 0x02, 0x00, 0x30, 0xCE], 150, &mut tx);
        assert_eq!(tx.take(), [std::vec![0xAA, 0x00, 0x02, 0x2A, 0xFF, 0xD5]]);
        t.on_bytes(&[0xAA, 0x00, 0x02, 0x00, 0x2A, 0xD4], 160, &mut tx);
        // No records followed: Idle after the paired-list wait.
        t.tick(5159, &mut tx);
        assert_eq!(t.state(), State::Starting);
        t.tick(5160, &mut tx);
        assert_eq!(t.state(), State::Idle);
        // Rest of the dump: ack 24, event 29 01, event 24 (shutdown by the
        // vendor); we treat the shutdown event as power-off.
        t.on_bytes(&[0xAA, 0x00, 0x02, 0x00, 0x24, 0xDA, 0xAA, 0x00, 0x02, 0x29, 0x01, 0xD4], 6000, &mut tx);
        assert_eq!(t.state(), State::Idle);
        t.on_bytes(&[0xAA, 0x00, 0x01, 0x24, 0xDB], 6010, &mut tx);
        assert_eq!(t.state(), State::Off);
        let o = outputs(&mut t);
        assert!(o.contains(&Output::Identity));
        assert_eq!(&o[o.len() - 2..], &[Output::Enable(false), Output::State(State::Off)]);
        assert_eq!(t.link_stats().stray_acks, 2);
        assert_eq!(t.decode_stats().skipped, 1);
    }

    fn started(cfg: Config, paired: &[Addr], last: Option<Addr>) -> (Transmitter, Tx) {
        let mut t = Transmitter::new(cfg);
        t.set_last_device(last);
        let mut tx = Tx::default();
        t.power_on(0, &mut tx);
        t.on_bytes(&ev(0x23, &[]), 10, &mut tx);
        t.on_bytes(&ack(0x06), 20, &mut tx);
        t.on_bytes(&ack(0x30), 30, &mut tx);
        t.on_bytes(&ack(0x2A), 40, &mut tx);
        for (i, a) in paired.iter().enumerate() {
            let mut p = std::vec![paired.len() as u8, i as u8];
            p.extend_from_slice(&a.0);
            p.extend_from_slice(b"Phones");
            t.on_bytes(&ev(0x2A, &p), 50, &mut tx);
        }
        if paired.is_empty() {
            t.tick(10_000, &mut tx);
        }
        tx.take();
        (t, tx)
    }

    #[test]
    fn queue_ring_wraps_and_refuses_when_full() {
        let mut q = Queue::new();
        let mut out = [0u8; MAX_TX_FRAME];
        let big = [0x5Au8; MAX_TX_FRAME];
        // Fill and drain repeatedly so entries straddle the ring's end.
        for round in 0..20u8 {
            let small = [round; 7];
            assert!(q.push(0x31, 3, &big));
            assert!(q.push(0x06, 1, &small));
            assert_eq!(q.pop(&mut out), Some((0x31, 3, MAX_TX_FRAME)));
            assert_eq!(out, big);
            assert_eq!(q.pop(&mut out), Some((0x06, 1, 7)));
            assert_eq!(&out[..7], &small);
            assert!(q.is_empty());
        }
        // Full: a frame that does not fit is refused, nothing is lost.
        let mut n = 0;
        while q.push(0x01, 1, &big) {
            n += 1;
        }
        assert_eq!(n, QUEUE_BYTES / (3 + MAX_TX_FRAME));
        assert!(q.push(0x02, 1, &[1; 2]) == (q.free() >= 5));
        for _ in 0..n {
            assert_eq!(q.pop(&mut out).map(|e| e.0), Some(0x01));
        }
    }

    #[test]
    fn ram_footprint() {
        let n = core::mem::size_of::<Transmitter>();
        std::println!("Transmitter: {n} bytes");
        assert!(n <= 2048, "{n}");
    }

    #[test]
    fn ready_timeout_probe() {
        let mut t = Transmitter::new(Config::default());
        let mut tx = Tx::default();
        t.power_on(0, &mut tx);
        t.tick(4999, &mut tx);
        assert!(tx.take().is_empty());
        t.tick(5000, &mut tx);
        assert_eq!(tx.take(), [std::vec![0xAA, 0, 2, 0x06, 0x00, 0xF8]]);
        t.on_bytes(&ack(0x06), 5100, &mut tx);
        assert_eq!(tx.take(), [std::vec![0xAA, 0, 2, 0x06, 0x02, 0xF6]]);
    }

    #[test]
    fn auto_reconnect_bounded() {
        let cfg = Config { reconnect_attempts: 2, ..Config::default() };
        let (mut t, mut tx) = started(cfg, &[HEADSET], Some(HEADSET));
        assert_eq!(t.state(), State::Connecting);
        assert_eq!(t.paired()[0].name(), b"Phones");
        let connect = ev(0x03, &HEADSET.0);
        // Sent with the last paired record (taken by `started`).
        assert_eq!(t.link.pending(), Some(0x03));
        t.on_bytes(&ack(0x03), 60, &mut tx);
        // Attempt 1 fails.
        let mut st = std::vec![4, 0];
        st.extend_from_slice(&HEADSET.0);
        t.on_bytes(&ev(0x02, &st), 1000, &mut tx);
        assert_eq!(t.state(), State::Idle);
        t.tick(2999, &mut tx);
        assert!(tx.take().is_empty());
        t.tick(3000, &mut tx);
        assert_eq!(tx.take(), [connect]);
        t.on_bytes(&ack(0x03), 3010, &mut tx);
        // Attempt 2 times out: cancel after 15 s, module confirms.
        t.tick(18_000, &mut tx);
        assert_eq!(tx.take(), [ev(0x29, &[])]);
        t.on_bytes(&ack(0x29), 18_010, &mut tx);
        t.on_bytes(&ev(0x29, &[0]), 18_020, &mut tx);
        assert_eq!(t.state(), State::Idle);
        // No third attempt.
        t.tick(60_000, &mut tx);
        assert!(tx.take().is_empty());
        let o = outputs(&mut t);
        assert_eq!(o.iter().filter(|o| **o == Output::ConnectFailed).count(), 2);
    }

    #[test]
    fn connect_stream_volume_keys_metadata() {
        let (mut t, mut tx) = started(Config::default(), &[], None);
        assert_eq!(t.state(), State::Idle);
        // The firmware drains outputs after every call; here, in a few goes.
        outputs(&mut t);
        // Inquiry.
        t.start_inquiry(0, &mut tx).unwrap();
        assert_eq!(tx.take(), [ev(0x00, &[0x0F, 0x0C, 0x03])]);
        t.on_bytes(&ack(0x00), 10, &mut tx);
        let mut r = HEADSET.0.to_vec();
        r.extend_from_slice(&[0xC5, 0x04, 0x04, 0x24]);
        t.on_bytes(&ev(0x03, &r), 100, &mut tx);
        t.on_bytes(&ev(0x04, &[]), 200, &mut tx);
        assert_eq!(t.state(), State::Idle);
        assert_eq!(t.found().len(), 1);
        // No name: asked for at the end of the inquiry.
        assert_eq!(tx.take(), [ev(0x02, &HEADSET.0)]);
        t.on_bytes(&ack(0x02), 220, &mut tx);
        let mut n = HEADSET.0.to_vec();
        n.extend_from_slice(b"BT Speaker\0");
        t.on_bytes(&ev(0x05, &n), 300, &mut tx);
        assert_eq!(t.found()[0].name(), b"BT Speaker");
        let mut o = outputs(&mut t);
        // Playback already running; connect.
        t.set_audio_active(true, 300, &mut tx);
        t.connect(HEADSET, 310, &mut tx).unwrap();
        assert_eq!(tx.take(), [ev(0x03, &HEADSET.0)]);
        t.on_bytes(&ack(0x03), 320, &mut tx);
        let link = |s: u8| {
            let mut p = std::vec![s, 0];
            p.extend_from_slice(&HEADSET.0);
            ev(0x02, &p)
        };
        t.on_bytes(&link(5), 400, &mut tx);
        t.on_bytes(&link(0), 2000, &mut tx);
        assert_eq!(t.state(), State::Streaming);
        assert_eq!(tx.take(), [ev(0x05, &[0x14, 0])]);
        t.on_bytes(&ack(0x05), 2010, &mut tx);
        t.on_bytes(&link(2), 2100, &mut tx);
        assert_eq!(tx.take(), [ev(0x25, &[])]);
        t.on_bytes(&ack(0x25), 2110, &mut tx);
        t.on_bytes(&ev(0x25, &[0x40]), 2120, &mut tx);
        t.on_bytes(&ev(0x26, &[127]), 2130, &mut tx);
        t.on_bytes(&ev(0x01, &[0x4C]), 2140, &mut tx);
        t.set_volume(31, 2150, &mut tx).unwrap();
        assert_eq!(tx.take(), [ev(0x26, &[127])]);
        t.on_bytes(&ack(0x26), 2160, &mut tx);
        o.extend(outputs(&mut t));
        // Metadata: seven frames, one at a time.
        t.send_metadata(
            &TrackInfo { length_s: 215, fields: ["Song", "Artist", "Album", "Genre"] },
            2200,
            &mut tx,
        )
        .unwrap();
        let mut sent = tx.take();
        for k in 0..7 {
            t.on_bytes(&ack(0x31), 2300 + k, &mut tx);
            sent.extend(tx.take());
        }
        assert_eq!(sent.len(), 7);
        assert_eq!(sent[0], ev(0x31, &[0]));
        assert_eq!(sent[1], ev(0x31, &[1, 0, 215]));
        assert_eq!(sent[2].len(), 5 + 73);
        assert_eq!(sent[4].len(), 5 + 21);
        assert_eq!(sent[6], ev(0x31, &[0xFF]));
        o.extend(outputs(&mut t));
        // Stop playback, disconnect.
        t.set_audio_active(false, 3000, &mut tx);
        assert_eq!(t.state(), State::Connected);
        assert_eq!(tx.take(), [ev(0x05, &[0x15, 0])]);
        t.on_bytes(&ack(0x05), 3010, &mut tx);
        t.disconnect(3020, &mut tx).unwrap();
        assert_eq!(tx.take(), [ev(0x04, &[0])]);
        t.on_bytes(&ack(0x04), 3030, &mut tx);
        t.on_bytes(&link(1), 3100, &mut tx);
        assert_eq!(t.state(), State::Idle);
        o.extend(outputs(&mut t));
        assert_eq!(t.outputs_dropped, 0);
        for want in [
            Output::Found(0),
            Output::NameResolved(0),
            Output::Connected(HEADSET),
            Output::AbsVolume(Some(0x40)),
            Output::SinkVolume { raw: 127, local: 31 },
            Output::Key(Key::Previous),
            Output::Disconnected,
        ] {
            assert!(o.contains(&want), "{want:?} missing from {o:?}");
        }
    }

    fn link_ev(st: u8, a: Addr) -> Vec<u8> {
        let mut p = std::vec![st, 0];
        p.extend_from_slice(&a.0);
        ev(0x02, &p)
    }

    fn found_ev(a: Addr, name: &[u8]) -> Vec<u8> {
        let mut r = a.0.to_vec();
        r.extend_from_slice(&[0xC5, 0x04, 0x04, 0x24]);
        r.extend_from_slice(name);
        ev(0x03, &r)
    }

    /// OK on a device while the inquiry runs: stop it, then connect; the
    /// device joins the paired list with its name once connected.
    #[test]
    fn connect_during_inquiry_then_paired() {
        let (mut t, mut tx) = started(Config::default(), &[], None);
        t.start_inquiry(0, &mut tx).unwrap();
        t.on_bytes(&ack(0x00), 10, &mut tx);
        tx.take();
        t.on_bytes(&found_ev(HEADSET, b"Buds\0"), 100, &mut tx);
        t.on_bytes(&found_ev(Addr([7; 6]), b""), 120, &mut tx);
        outputs(&mut t);
        t.connect(HEADSET, 200, &mut tx).unwrap();
        assert_eq!(tx.take(), [ev(0x01, &[0])], "stop first");
        // A second OK only moves the target; no second stop.
        t.connect(HEADSET, 205, &mut tx).unwrap();
        assert!(tx.take().is_empty());
        t.on_bytes(&ack(0x01), 210, &mut tx);
        assert_eq!(t.state(), State::Connecting);
        assert_eq!(tx.take(), [ev(0x03, &HEADSET.0)], "connect, no name requests");
        t.on_bytes(&ack(0x03), 220, &mut tx);
        t.on_bytes(&link_ev(0, HEADSET), 900, &mut tx);
        assert_eq!(t.state(), State::Connected);
        assert_eq!(t.paired().len(), 1);
        assert_eq!(t.paired()[0].name(), b"Buds");
        let o = outputs(&mut t);
        assert!(o.contains(&Output::PairedList) && o.contains(&Output::Connected(HEADSET)), "{o:?}");
        // Connected again later: not added twice.
        t.on_bytes(&link_ev(1, HEADSET), 1000, &mut tx);
        t.connect(HEADSET, 1010, &mut tx).unwrap();
        t.on_bytes(&ack(0x03), 1020, &mut tx);
        t.on_bytes(&link_ev(0, HEADSET), 1100, &mut tx);
        assert_eq!(t.paired().len(), 1);
    }

    /// Back on the connect screen before the inquiry stopped: nothing is
    /// connected; unnamed devices are asked for their names.
    #[test]
    fn cancel_before_inquiry_stops() {
        let (mut t, mut tx) = started(Config::default(), &[], None);
        t.start_inquiry(0, &mut tx).unwrap();
        t.on_bytes(&ack(0x00), 10, &mut tx);
        let other = Addr([7; 6]);
        t.on_bytes(&found_ev(other, b""), 120, &mut tx);
        t.connect(HEADSET, 200, &mut tx).unwrap();
        t.cancel_connect(210, &mut tx).unwrap();
        tx.take();
        t.on_bytes(&ack(0x01), 220, &mut tx);
        assert_eq!(t.state(), State::Idle);
        assert_eq!(tx.take(), [ev(0x02, &other.0)]);
        assert_eq!(t.cancel_connect(230, &mut tx), Err(Refused));
    }

    /// Found on the player with a BlueZ sink (lead, 2026-11-24): its
    /// absolute volume is 0 after every connection. The player's volume
    /// goes out on connect (before the audio start), its start-up report
    /// of 0 is answered with ours, a later change by the user is taken.
    #[test]
    fn connect_sends_player_volume() {
        let (mut t, mut tx) = started(Config::default(), &[HEADSET], None);
        // Set while not connected: remembered, nothing sent.
        t.set_volume(16, 100, &mut tx).unwrap();
        assert!(tx.take().is_empty());
        t.set_audio_active(true, 110, &mut tx);
        t.connect(HEADSET, 120, &mut tx).unwrap();
        t.on_bytes(&ack(0x03), 130, &mut tx);
        tx.take();
        let raw = volume_to_sink(16);
        t.on_bytes(&link_ev(0, HEADSET), 1000, &mut tx);
        assert_eq!(tx.take(), [ev(0x26, &[raw])], "volume first");
        t.on_bytes(&ack(0x26), 1010, &mut tx);
        assert!(outputs(&mut t).contains(&Output::VolumeAcked));
        assert_eq!(tx.take(), [ev(0x05, &[0x14, 0])], "then audio");
        t.on_bytes(&ack(0x05), 1020, &mut tx);
        outputs(&mut t);
        // The sink reports its start-up 0: answered with ours, not taken.
        t.on_bytes(&ev(0x26, &[0]), 1100, &mut tx);
        assert_eq!(tx.take(), [ev(0x26, &[raw])]);
        t.on_bytes(&ack(0x26), 1110, &mut tx);
        assert!(!outputs(&mut t).iter().any(|o| matches!(o, Output::SinkVolume { .. })));
        assert_eq!(t.volume(), Some(16));
        // Our query's answer with another value: ours again.
        t.on_bytes(&link_ev(2, HEADSET), 1200, &mut tx);
        assert_eq!(tx.take(), [ev(0x25, &[])]);
        t.on_bytes(&ack(0x25), 1210, &mut tx);
        t.on_bytes(&ev(0x25, &[5]), 1220, &mut tx);
        assert_eq!(tx.take(), [ev(0x26, &[raw])]);
        t.on_bytes(&ack(0x26), 1230, &mut tx);
        assert!(outputs(&mut t).contains(&Output::AbsVolume(Some(5))));
        // Later, the user turns the headset down: taken as the volume.
        t.on_bytes(&ev(0x26, &[40]), 1000 + VOLUME_SYNC_MS, &mut tx);
        assert!(tx.take().is_empty());
        assert_eq!(outputs(&mut t), [Output::SinkVolume { raw: 40, local: volume_from_sink(40) }]);
        assert_eq!(t.volume(), Some(volume_from_sink(40)));
        // A change on the player goes out at once.
        t.set_volume(20, 5000, &mut tx).unwrap();
        assert_eq!(tx.take(), [ev(0x26, &[volume_to_sink(20)])]);
    }

    #[test]
    fn switch_device_disconnects_first() {
        let other = Addr([9; 6]);
        let (mut t, mut tx) = started(Config::default(), &[HEADSET], Some(HEADSET));
        t.on_bytes(&ack(0x03), 60, &mut tx);
        let mut p = std::vec![0, 0];
        p.extend_from_slice(&HEADSET.0);
        t.on_bytes(&ev(0x02, &p), 100, &mut tx);
        assert_eq!(t.state(), State::Connected);
        t.connect(other, 200, &mut tx).unwrap();
        assert_eq!(tx.take(), [ev(0x04, &[0])]);
        t.on_bytes(&ack(0x04), 210, &mut tx);
        p[0] = 1;
        t.on_bytes(&ev(0x02, &p), 300, &mut tx);
        assert_eq!(tx.take(), [ev(0x03, &other.0)]);
        assert_eq!(t.state(), State::Connecting);
    }

    #[test]
    fn link_down_power_cycles_bounded() {
        let cfg = Config { max_restarts: 1, ..Config::default() };
        let (mut t, mut tx) = started(cfg, &[], None);
        t.refresh_paired(0, &mut tx).unwrap();
        assert_eq!(tx.take().len(), 1);
        t.tick(500, &mut tx); // resend
        assert_eq!(tx.take().len(), 1);
        t.tick(1000, &mut tx); // give up
        assert_eq!(t.state(), State::Off);
        let o = outputs(&mut t);
        assert!(o.contains(&Output::Enable(false)));
        assert!(o.contains(&Output::LinkDown { restarting: true }));
        t.tick(1500, &mut tx);
        assert_eq!(t.state(), State::Starting);
        assert!(outputs(&mut t).contains(&Output::Enable(true)));
        // Module never answers: probe, resend, give up for good.
        t.tick(6500, &mut tx);
        t.tick(7000, &mut tx);
        t.tick(7500, &mut tx);
        assert_eq!(t.state(), State::Off);
        assert!(outputs(&mut t).contains(&Output::LinkDown { restarting: false }));
        t.tick(100_000, &mut tx);
        assert_eq!(t.state(), State::Off);
    }

    #[test]
    fn module_restart_reruns_setup() {
        let (mut t, mut tx) = started(Config::default(), &[HEADSET], Some(HEADSET));
        t.on_bytes(&ack(0x03), 60, &mut tx);
        let mut p = std::vec![0, 0];
        p.extend_from_slice(&HEADSET.0);
        t.on_bytes(&ev(0x02, &p), 100, &mut tx);
        outputs(&mut t);
        t.on_bytes(&ev(0x23, &[]), 1000, &mut tx);
        assert_eq!(t.state(), State::Starting);
        assert_eq!(tx.take(), [ev(0x06, &[2])]);
        assert!(outputs(&mut t).contains(&Output::Disconnected));
    }

    #[test]
    fn forget_all_on_first_start() {
        let cfg = Config { forget_all_on_start: true, ..Config::default() };
        let mut t = Transmitter::new(cfg);
        let mut tx = Tx::default();
        t.power_on(0, &mut tx);
        t.on_bytes(&ev(0x23, &[]), 10, &mut tx);
        t.on_bytes(&ack(0x06), 20, &mut tx);
        t.on_bytes(&ack(0x30), 30, &mut tx);
        assert_eq!(tx.take().last().unwrap(), &ev(0x2B, &[0xFF]));
        t.on_bytes(&ack(0x2B), 40, &mut tx);
        assert_eq!(t.state(), State::Idle);
    }

    /// Frames as sent, restricted ones allowed (the shutdown test).
    #[derive(Default)]
    struct AnyTx(Vec<Vec<u8>>);
    impl Transport for AnyTx {
        fn send(&mut self, f: &[u8]) {
            assert_eq!(checksum(&f[1..]), 0, "{f:02X?}");
            self.0.push(f.to_vec());
        }
    }

    #[test]
    fn shutdown_only_when_up_then_completion() {
        let mut t = Transmitter::new(Config::default());
        let mut tx = AnyTx::default();
        // Off or starting: refused, nothing sent.
        assert_eq!(t.shutdown(0, &mut tx), Err(Refused));
        t.power_on(0, &mut tx);
        assert_eq!(t.shutdown(1, &mut tx), Err(Refused));
        assert!(tx.0.is_empty());
        // Up (paired list empty → Idle after the wait).
        let (mut t, _) = started(Config::default(), &[], None);
        assert!(t.module_up());
        outputs(&mut t);
        let mut tx = AnyTx::default();
        t.shutdown(20_000, &mut tx).unwrap();
        assert_eq!(tx.0, [std::vec![0xAA, 0x00, 0x01, 0x24, 0xDB]]);
        assert!(t.shutting_down());
        assert_eq!(t.shutdown(20_001, &mut tx), Err(Refused));
        // Requests while shutting down do not reach the module.
        t.set_audio_active(true, 20_002, &mut tx);
        assert_eq!(tx.0.len(), 1);
        // The vendor's tail of the dump: ack 24, event 29 01, event 24.
        t.on_bytes(&ack(0x24), 20_010, &mut tx);
        t.on_bytes(&ev(0x29, &[0x01]), 20_020, &mut tx);
        assert_eq!(t.state(), State::Idle);
        t.on_bytes(&ev(0x24, &[]), 20_030, &mut tx);
        assert_eq!(t.state(), State::Off);
        assert!(!t.shutting_down());
        assert_eq!(outputs(&mut t), [Output::Enable(false), Output::State(State::Off)]);
        // No restart is scheduled.
        t.tick(60_000, &mut tx);
        assert_eq!(t.state(), State::Off);
        assert_eq!(tx.0.len(), 1);
    }

    #[test]
    fn shutdown_without_ack_powers_off_no_restart() {
        let (mut t, _) = started(Config::default(), &[], None);
        outputs(&mut t);
        let mut tx = AnyTx::default();
        t.shutdown(20_000, &mut tx).unwrap();
        // A ready event (module restart) is ignored while shutting down.
        t.on_bytes(&ev(0x23, &[]), 20_100, &mut tx);
        t.tick(20_499, &mut tx);
        assert_eq!(t.state(), State::Idle);
        // No resend: one frame, then power-off after the ack timeout.
        t.tick(20_500, &mut tx);
        assert_eq!(tx.0.len(), 1);
        assert_eq!(t.state(), State::Off);
        assert_eq!(outputs(&mut t), [Output::Enable(false), Output::State(State::Off)]);
        t.tick(90_000, &mut tx);
        assert_eq!(t.state(), State::Off);
        assert!(t.next_output().is_none());
    }

    /// Fuzz the whole state machine with random module traffic, garbage
    /// and API calls: no panic, and (checked in `Tx::send`) no restricted
    /// command and no malformed frame ever leaves it.
    #[test]
    fn fuzz_state_machine() {
        let mut x: u64 = 0xDEAD_BEEF_CAFE_F00D;
        let mut rnd = move || {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            x
        };
        let ids = [
            0x00, 0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x20, 0x21, 0x22, 0x23, 0x24, 0x25, 0x26, 0x29, 0x2A,
            0x2C,
        ];
        for run in 0..200 {
            let mut t = Transmitter::new(Config {
                forget_all_on_start: run % 3 == 0,
                auto_reconnect: run % 2 == 0,
                ..Config::default()
            });
            t.set_last_device(Some(HEADSET));
            let mut tx = Tx::default();
            let mut now = 0u64;
            for _ in 0..400 {
                now += rnd() % 700;
                let r = rnd();
                match r % 16 {
                    0 => t.power_on(now, &mut tx),
                    1 => t.power_off(),
                    2 => {
                        let _ = t.start_inquiry(now, &mut tx);
                    }
                    3 => {
                        let _ = t.connect(
                            if r & 0x100 != 0 { HEADSET } else { Addr([r as u8; 6]) },
                            now,
                            &mut tx,
                        );
                    }
                    4 => {
                        let _ = t.disconnect(now, &mut tx);
                    }
                    5 => t.set_audio_active(r & 0x100 != 0, now, &mut tx),
                    6 => {
                        let _ = t.send_metadata(
                            &TrackInfo { length_s: 1, fields: ["a", "b", "c", "d"] },
                            now,
                            &mut tx,
                        );
                    }
                    7 => {
                        let _ = t.forget(None, now, &mut tx);
                    }
                    8 => {
                        let junk: Vec<u8> = (0..rnd() % 12).map(|_| rnd() as u8).collect();
                        t.on_bytes(&junk, now, &mut tx);
                    }
                    9..=11 => {
                        // Acknowledge whatever is pending, sometimes.
                        if let Some(id) = t.link.pending() {
                            t.on_bytes(&ack(id), now, &mut tx);
                        }
                    }
                    _ => {
                        let id = ids[(r >> 8) as usize % ids.len()];
                        let mut p: Vec<u8> = (0..rnd() % 20).map(|_| rnd() as u8).collect();
                        if id == 0x02 && p.len() >= 8 {
                            p[0] %= 6;
                        }
                        t.on_bytes(&ev(id, &p), now, &mut tx);
                    }
                }
                t.tick(now, &mut tx);
                while t.next_output().is_some() {}
            }
        }
    }
}
