//! A fake Bluetooth transmitter module for the simulator (WP UI-6a):
//! start-up, an inquiry whose devices appear over time (one answers
//! without a name, which resolves after the inquiry; one never gives
//! one), connections that succeed or fail by device, the paired list,
//! forget. Timings are short versions of the module's (06 §4.2).

use orpheus_apps::services::{BT_DEVICES, BtAddr, BtDevice, BtKind, BtList, BtPhase, BtRequest, bt_event};
use orpheus_ui::event::BtState;

/// A device in radio range.
struct World {
    addr: [u8; 6],
    cod: [u8; 3],
    name: &'static str,
    /// The inquiry result carries the name (else it resolves later).
    named: bool,
    /// When it answers the inquiry, ms after the start.
    at: u64,
    /// A connection succeeds.
    connects: bool,
}

const WORLD: [World; 5] = [
    // Headphones (minor 0x06), named at once.
    World {
        addr: [0x11, 0x22, 0x33, 0x44, 0x55, 0x66],
        cod: [0x18, 0x04, 0x24],
        name: "Sport Buds",
        named: true,
        at: 300,
        connects: true,
    },
    // A loudspeaker (minor 0x05) without a name in its result.
    World {
        addr: [0x20, 0x21, 0x22, 0x23, 0x24, 0x25],
        cod: [0x14, 0x04, 0x24],
        name: "Kitchen Speaker",
        named: false,
        at: 900,
        connects: true,
    },
    // A phone (major 0x02).
    World {
        addr: [0x30, 0x31, 0x32, 0x33, 0x34, 0x35],
        cod: [0x0C, 0x02, 0x5A],
        name: "Pixel 8",
        named: true,
        at: 1500,
        connects: false,
    },
    // Car audio (minor 0x08): connection fails.
    World {
        addr: [0x40, 0x41, 0x42, 0x43, 0x44, 0x45],
        cod: [0x20, 0x04, 0x24],
        name: "Car Kit",
        named: true,
        at: 2200,
        connects: false,
    },
    // Something that never gives a name.
    World {
        addr: [0x56, 0x34, 0x12, 0x4C, 0xE0, 0x00],
        cod: [0x00, 0x1F, 0x00],
        name: "",
        named: false,
        at: 2700,
        connects: false,
    },
];

/// Paired in the module before the simulation starts.
const PAIRED_AT_START: ([u8; 6], &str) = ([0x50, 0x51, 0x52, 0x53, 0x54, 0x55], "Studio Headphones");

pub const START_MS: u64 = 1200;
pub const INQUIRY_MS: u64 = 5000;
pub const NAME_MS: u64 = 600;
pub const CONNECT_MS: u64 = 2000;

pub struct FakeBt {
    pub phase: BtPhase,
    /// Time in the current phase.
    t: u64,
    pub found: Vec<BtDevice>,
    pub paired: Vec<BtDevice>,
    /// The device connected to, or being connected to.
    target: Option<BtAddr>,
    /// Connection asked for during an inquiry (it stops first).
    after_inquiry: Option<BtAddr>,
    /// Names still to resolve after the inquiry, and when.
    names_at: Option<u64>,
    last: Option<BtAddr>,
    pub reconnect: bool,
    /// Audio is playing (Streaming while connected).
    pub audio: bool,
    events: Vec<u32>,
    /// Requests the screens made, in order (behaviour tests).
    pub log: Vec<BtRequest>,
}

impl Default for FakeBt {
    fn default() -> Self {
        let (a, n) = PAIRED_AT_START;
        FakeBt {
            phase: BtPhase::Off,
            t: 0,
            found: Vec::new(),
            paired: vec![BtDevice::new(BtAddr(a), BtKind::Unknown, n.as_bytes(), false)],
            target: None,
            after_inquiry: None,
            names_at: None,
            last: None,
            reconnect: true,
            audio: false,
            events: Vec::new(),
            log: Vec::new(),
        }
    }
}

fn world(a: BtAddr) -> Option<&'static World> {
    WORLD.iter().find(|w| w.addr == a.0)
}

impl FakeBt {
    #[must_use]
    pub fn state(&self) -> BtState {
        match self.phase() {
            BtPhase::Off | BtPhase::NotResponding => BtState::Off,
            BtPhase::Connected => BtState::Connected,
            BtPhase::Streaming => BtState::Streaming,
            _ => BtState::On,
        }
    }

    #[must_use]
    pub fn phase(&self) -> BtPhase {
        if self.phase == BtPhase::Connected && self.audio { BtPhase::Streaming } else { self.phase }
    }

    fn lookup(&self, a: BtAddr) -> Option<&BtDevice> {
        self.paired.iter().chain(&self.found).find(|d| d.addr == a)
    }

    /// The connected (or connecting) device's name, else the last one's.
    #[must_use]
    pub fn device_name(&self) -> &str {
        let who = if matches!(self.phase, BtPhase::Connecting | BtPhase::Connected | BtPhase::Streaming) {
            self.target
        } else {
            self.last
        };
        who.and_then(|a| self.lookup(a)).map_or("", BtDevice::name)
    }

    fn list(&self, l: BtList) -> &Vec<BtDevice> {
        match l {
            BtList::Found => &self.found,
            BtList::Paired => &self.paired,
        }
    }

    #[must_use]
    pub fn count(&self, l: BtList) -> usize {
        self.list(l).len()
    }

    #[must_use]
    pub fn entry(&self, l: BtList, i: usize) -> Option<BtDevice> {
        let mut d = *self.list(l).get(i)?;
        d.connected = self.phase().is_connected() && self.target == Some(d.addr);
        Some(d)
    }

    fn set_phase(&mut self, p: BtPhase) {
        if self.phase != p {
            self.phase = p;
            self.t = 0;
            self.events.push(bt_event::PHASE);
        }
    }

    /// Events since the last call (`bt_event` values).
    pub fn take_events(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.events)
    }

    pub fn power(&mut self, on: bool) {
        if on {
            if matches!(self.phase, BtPhase::Off | BtPhase::NotResponding) {
                self.set_phase(BtPhase::Starting);
            }
        } else {
            if self.phase.is_connected() {
                self.events.push(bt_event::DISCONNECTED);
            }
            self.target = None;
            self.after_inquiry = None;
            self.names_at = None;
            self.set_phase(BtPhase::Off);
        }
    }

    /// Up at once (scripts), optionally connected to Sport Buds.
    pub fn force(&mut self, phase: BtPhase) {
        self.target = None;
        if phase.is_connected() || phase == BtPhase::Connecting {
            let w = &WORLD[0];
            let a = BtAddr(w.addr);
            if !self.paired.iter().any(|d| d.addr == a) {
                self.paired.push(BtDevice::new(a, BtKind::from_cod(w.cod), w.name.as_bytes(), false));
            }
            self.target = Some(a);
            self.last = Some(a);
        }
        self.audio = phase == BtPhase::Streaming;
        self.phase = if phase == BtPhase::Streaming { BtPhase::Connected } else { phase };
        self.t = 0;
        self.events.push(bt_event::PHASE);
    }

    /// The first `n` devices of the world found, names resolved.
    pub fn force_found(&mut self, n: usize) {
        self.found = WORLD
            .iter()
            .take(n.min(BT_DEVICES))
            .map(|w| BtDevice::new(BtAddr(w.addr), BtKind::from_cod(w.cod), w.name.as_bytes(), false))
            .collect();
        self.events.push(bt_event::LISTS);
    }

    fn begin_connect(&mut self, a: BtAddr) {
        self.target = Some(a);
        self.set_phase(BtPhase::Connecting);
    }

    pub fn request(&mut self, r: BtRequest) {
        self.log.push(r);
        match r {
            BtRequest::Scan if self.phase == BtPhase::Ready => {
                self.found.clear();
                self.names_at = None;
                self.events.push(bt_event::LISTS);
                self.set_phase(BtPhase::Scanning);
            }
            BtRequest::StopScan if self.phase == BtPhase::Scanning => self.end_inquiry(),
            BtRequest::Connect(a) => match self.phase {
                BtPhase::Ready => self.begin_connect(a),
                BtPhase::Scanning => {
                    self.after_inquiry = Some(a);
                    // The stop is acknowledged quickly.
                    self.t = self.t.max(INQUIRY_MS - 100);
                }
                _ => {}
            },
            BtRequest::CancelConnect => {
                if self.phase == BtPhase::Scanning {
                    self.after_inquiry = None;
                } else if self.phase == BtPhase::Connecting {
                    self.target = None;
                    self.events.push(bt_event::CONNECT_FAILED);
                    self.set_phase(BtPhase::Ready);
                }
            }
            BtRequest::Disconnect if self.phase.is_connected() => {
                self.target = None;
                self.events.push(bt_event::DISCONNECTED);
                self.set_phase(BtPhase::Ready);
            }
            BtRequest::Forget(a) if self.phase.is_up() => {
                if self.target == Some(a) && self.phase.is_connected() {
                    self.target = None;
                    self.events.push(bt_event::DISCONNECTED);
                    self.set_phase(BtPhase::Ready);
                }
                self.paired.retain(|d| d.addr != a);
                if self.last == Some(a) {
                    self.last = None;
                }
                self.events.push(bt_event::LISTS);
            }
            _ => {}
        }
    }

    fn end_inquiry(&mut self) {
        self.set_phase(BtPhase::Ready);
        if let Some(a) = self.after_inquiry.take() {
            self.begin_connect(a);
        } else if self.found.iter().any(|d| d.name().is_empty()) {
            self.names_at = Some(NAME_MS);
        }
    }

    /// Time passes.
    pub fn advance(&mut self, ms: u64) {
        let before = self.t;
        self.t += ms;
        match self.phase {
            BtPhase::Starting if self.t >= START_MS => {
                self.set_phase(BtPhase::Ready);
                if self.reconnect
                    && let Some(a) = self.last
                    && self.paired.iter().any(|d| d.addr == a)
                {
                    self.begin_connect(a);
                }
            }
            BtPhase::Scanning => {
                for w in WORLD.iter().filter(|w| before < w.at && w.at <= self.t) {
                    if self.found.len() < BT_DEVICES {
                        let name = if w.named { w.name.as_bytes() } else { &[] };
                        self.found.push(BtDevice::new(BtAddr(w.addr), BtKind::from_cod(w.cod), name, false));
                        self.events.push(bt_event::LISTS);
                    }
                }
                if self.t >= INQUIRY_MS {
                    self.end_inquiry();
                }
            }
            BtPhase::Ready => {
                if let Some(at) = self.names_at
                    && self.t >= at
                {
                    self.names_at = None;
                    for d in &mut self.found {
                        if d.name().is_empty()
                            && let Some(w) = world(d.addr).filter(|w| !w.name.is_empty())
                        {
                            *d = BtDevice::new(d.addr, d.kind, w.name.as_bytes(), false);
                        }
                    }
                    self.events.push(bt_event::LISTS);
                }
            }
            BtPhase::Connecting if self.t >= CONNECT_MS => {
                let Some(a) = self.target else { return };
                let ok = world(a).map_or(a.0 == PAIRED_AT_START.0, |w| w.connects);
                if ok {
                    if !self.paired.iter().any(|d| d.addr == a)
                        && let Some(&d) = self.found.iter().find(|d| d.addr == a)
                    {
                        self.paired.push(d);
                    }
                    self.last = Some(a);
                    self.set_phase(BtPhase::Connected);
                    self.events.push(bt_event::LISTS);
                    self.events.push(bt_event::CONNECTED);
                } else {
                    self.target = None;
                    self.set_phase(BtPhase::Ready);
                    self.events.push(bt_event::CONNECT_FAILED);
                }
            }
            _ => {}
        }
    }
}
