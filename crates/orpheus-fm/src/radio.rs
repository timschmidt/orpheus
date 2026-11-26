//! The radio service (05 §7.3) as a time-driven state machine, the same
//! in the firmware's `fm` task and in the simulator.
//!
//! The owner calls [`Radio::request`] with what the user or a developer
//! command wants, and [`Radio::poll`] with the time (ms) whenever the
//! deadline it returned has passed or a request arrived; `poll` talks to
//! the chip and the analogue route through a [`Port`] and returns the next
//! deadline (`None`: nothing to do until a request).
//!
//! Start (the stock radio engine's order, fm-i2c-imu.md §2.6): soft reset,
//! 10 ms, identify and load the init table muted, 500 ms (VERBATIM), the
//! amplifier gain to 0, the FM bypass (pre-charge, 100 ms, connect,
//! 100 ms, release), tune, tuner volume, unmute, then the amplifier gain
//! ramps up one step per 5 ms (the vendor's ramp). Stop: tuner muted, gain
//! ramped down, bypass off, gain back to the music value 7, tuner in
//! standby: the tuner is powered only while the radio plays.
//!
//! Seek uses the chip's hardware seek, stopping at the band edge; the
//! region's limits, the wrap-around, Wide's two hardware bands and the
//! 96 MHz reference harmonic are handled here. Auto-scan walks the region
//! with hardware seeks, keeps the 30 strongest stations, sorted strongest
//! first, and tunes to the first.

use crate::chip::{Analog, Port, Setup, Status, Tuner};
use crate::rds::Rds;
use crate::region::{Region, is_spur};
use crate::volume::{self, PA_MUSIC};

/// Stations an auto-scan keeps.
pub const SCAN_MAX: usize = 30;

/// Wait after the soft reset (vendor).
pub const RESET_MS: u64 = 10;
/// Wait after loading the init table (VERBATIM, vendor `FMb0` 0x1006D4).
pub const SETTLE_MS: u64 = 500;
/// Waits inside the bypass switch (vendor `AUD_DEV` A2:0x15840092).
pub const ROUTE_MS: u64 = 100;
pub const TUNE_POLL_MS: u64 = 10;
/// Give up waiting for a tune (the vendor polls 255 × 10 ms).
pub const TUNE_LIMIT_MS: u64 = 2550;
pub const SEEK_POLL_MS: u64 = 20;
/// One hardware seek (one band) at most.
pub const SEEK_LIMIT_MS: u64 = 8000;
/// A whole seek (with wraps) at most.
pub const SEEK_TOTAL_MS: u64 = 20_000;
/// Status (and RDS) read period while playing: twice per RDS group.
pub const LISTEN_MS: u64 = 40;
/// Amplifier gain ramp: one step per 5 ms (vendor `FMENGINE` 0x1A03175E).
pub const RAMP_MS: u64 = 5;
/// Consecutive failed status reads before the radio gives up.
pub const FAIL_LIMIT: u8 = 5;

#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum Phase {
    /// Tuner in standby, bypass off.
    #[default]
    Off,
    Starting,
    Playing,
    Seeking,
    Scanning,
    Stopping,
    /// The tuner did not answer (start) or stopped answering.
    Failed,
}

impl Phase {
    /// The tuner is on and heard (or about to be).
    #[must_use]
    pub const fn is_on(self) -> bool {
        matches!(self, Phase::Starting | Phase::Playing | Phase::Seeking | Phase::Scanning)
    }
}

/// Settings › Radio.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    pub region: Region,
    pub mono: bool,
    /// Seek sensitivity: 0 low, 1 medium, 2 high.
    pub sensitivity: u8,
    /// Decode RDS (if the chip has it).
    pub rds: bool,
    pub softmute: bool,
}

impl Config {
    pub const DEFAULT: Config =
        Config { region: Region::Europe, mono: false, sensitivity: 1, rds: true, softmute: true };

    /// Seek threshold (05h `SEEKTH`): medium is the vendor's 8.
    #[must_use]
    pub const fn threshold(&self) -> u8 {
        match self.sensitivity {
            0 => 12,
            2 => 5,
            _ => 8,
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Config::DEFAULT
    }
}

/// What the owner asks for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Req {
    On,
    Off,
    /// Tune to a frequency (snapped to the region); starts the radio if off.
    Tune(u32),
    /// One region step up (`true`) or down.
    Step(bool),
    /// Seek to the next station up (`true`) or down.
    Seek(bool),
    /// Stop a seek where it is, or an auto-scan (back where it started).
    Cancel,
    Mute(bool),
    Scan,
    /// The player's volume, 0–30.
    Volume(u8),
    Config(Config),
}

/// A station an auto-scan found (4 bytes: 30 of them are kept).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Station {
    /// Frequency in 10 kHz.
    pub khz10: u16,
    pub rssi: u8,
}

impl Station {
    #[must_use]
    pub const fn new(khz: u32, rssi: u8) -> Station {
        Station { khz10: (khz / 10) as u16, rssi }
    }

    #[must_use]
    pub const fn khz(&self) -> u32 {
        self.khz10 as u32 * 10
    }
}

/// What the screens and the developer channel see.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Info {
    pub phase: Phase,
    pub khz: u32,
    /// 0–127.
    pub rssi: u8,
    pub stereo: bool,
    pub muted: bool,
    /// Auto-scan: progress 0–1000 and stations so far.
    pub progress: u16,
    pub found: u8,
    /// The chip id read at start (0 before).
    pub chip: u16,
    /// Counts changes of everything above except RSSI noise.
    pub seq: u16,
    /// Counts changes of the RDS name or text.
    pub rds_seq: u16,
    /// Counts finished auto-scans.
    pub scan_seq: u16,
    /// Failed bus transfers since start.
    pub errors: u16,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Then {
    Listen,
    Seek,
    Scan,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Step {
    Idle,
    Reset,
    Load,
    Precharge,
    Connect,
    Release,
    /// A tune was sent; waiting for `STC`.
    Tune(Then),
    /// A hardware seek runs.
    Seek,
    Listen,
    /// Muting before the ramp down.
    Stop,
    Ramp,
}

/// Changes the owner asked for, applied at the next poll that can.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
struct Want {
    tune: Option<u32>,
    seek: Option<bool>,
    scan: bool,
    cancel: bool,
    volume: bool,
    mute: bool,
    config: Option<Config>,
}

/// The radio service.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Radio {
    tuner: Tuner,
    cfg: Config,
    phase: Phase,
    step: Step,
    /// Next action time.
    at: u64,
    /// Deadline of the current tune or hardware seek.
    limit: u64,
    listen_at: u64,
    want: Want,
    khz: u32,
    rssi: u8,
    stereo: bool,
    muted: bool,
    volume: u8,
    /// Amplifier gain now (as last sent).
    pa: u8,
    routed: bool,
    failed: bool,
    chip: u16,
    last: Status,
    rds: Rds,
    seek_up: bool,
    seek_origin: u32,
    jumps: u8,
    seek_end: u64,
    scan: [Station; SCAN_MAX],
    scan_n: u8,
    scan_last: u32,
    scan_from: u32,
    fails: u8,
    seq: u16,
    rds_seq: u16,
    scan_seq: u16,
    errors: u16,
}

impl Default for Radio {
    fn default() -> Self {
        Radio::new()
    }
}

impl Radio {
    #[must_use]
    pub const fn new() -> Radio {
        Radio {
            tuner: Tuner::new(),
            cfg: Config::DEFAULT,
            phase: Phase::Off,
            step: Step::Idle,
            at: 0,
            limit: 0,
            listen_at: 0,
            want: Want {
                tune: None,
                seek: None,
                scan: false,
                cancel: false,
                volume: false,
                mute: false,
                config: None,
            },
            khz: 87_500,
            rssi: 0,
            stereo: false,
            muted: false,
            volume: 12,
            pa: PA_MUSIC,
            routed: false,
            failed: false,
            chip: 0,
            last: Status { r0a: 0, r0b: 0, blocks: [0; 4], with_blocks: false },
            rds: Rds::new(),
            seek_up: true,
            seek_origin: 0,
            jumps: 0,
            seek_end: 0,
            scan: [Station { khz10: 0, rssi: 0 }; SCAN_MAX],
            scan_n: 0,
            scan_last: 0,
            scan_from: 0,
            fails: 0,
            seq: 0,
            rds_seq: 0,
            scan_seq: 0,
            errors: 0,
        }
    }

    /// At boot: put a tuner left running (by a reset of the player alone)
    /// into standby. One 2-byte write.
    pub fn park<P: Port>(&mut self, p: &mut P) -> bool {
        self.tuner.standby(p)
    }

    #[must_use]
    pub fn info(&self) -> Info {
        let progress = if self.phase == Phase::Scanning && self.scan_last > 0 {
            let r = self.cfg.region;
            let span = (r.high_khz() - r.low_khz()).max(1);
            (self.scan_last.saturating_sub(r.low_khz()) * 1000 / span).min(1000) as u16
        } else {
            0
        };
        Info {
            phase: self.phase,
            khz: self.khz,
            rssi: self.rssi,
            stereo: self.stereo,
            muted: self.muted,
            progress,
            found: self.scan_n,
            chip: self.chip,
            seq: self.seq,
            rds_seq: self.rds_seq,
            scan_seq: self.scan_seq,
            errors: self.errors,
        }
    }

    #[must_use]
    pub const fn phase(&self) -> Phase {
        self.phase
    }

    #[must_use]
    pub const fn config(&self) -> Config {
        self.cfg
    }

    #[must_use]
    pub const fn rds(&self) -> &Rds {
        &self.rds
    }

    #[must_use]
    pub const fn tuner(&self) -> &Tuner {
        &self.tuner
    }

    /// The last status read (0Ah–0Fh).
    #[must_use]
    pub const fn last_status(&self) -> Status {
        self.last
    }

    /// The amplifier gain as last set.
    #[must_use]
    pub const fn gain(&self) -> u8 {
        self.pa
    }

    /// Playing and listening: no start, tune, seek, scan or stop under way
    /// (developer replies wait for this).
    #[must_use]
    pub fn settled(&self) -> bool {
        self.phase == Phase::Playing && self.step == Step::Listen
    }

    /// The user's volume (0–30) and mute.
    #[must_use]
    pub const fn volume(&self) -> u8 {
        self.volume
    }

    /// The last auto-scan's stations, strongest first.
    #[must_use]
    pub fn scan_results(&self) -> &[Station] {
        self.scan.get(..usize::from(self.scan_n)).unwrap_or(&[])
    }

    fn changed(&mut self) {
        self.seq = self.seq.wrapping_add(1);
    }

    /// Ask for something; `poll` carries it out.
    pub fn request(&mut self, r: Req, now: u64) {
        let region = self.cfg.region;
        match r {
            Req::On => self.start(now),
            Req::Off => self.stop(now),
            Req::Tune(k) => {
                let k = region.snap(k);
                if self.phase.is_on() && self.step != Step::Reset && self.step != Step::Load {
                    self.want.tune = Some(k);
                    self.want.seek = None;
                    self.want.scan = false;
                } else {
                    self.khz = k;
                }
                self.changed();
                if !self.phase.is_on() {
                    self.start(now);
                }
            }
            Req::Step(up) => {
                let base = self.want.tune.unwrap_or(self.khz);
                self.request(Req::Tune(region.step(base, up)), now);
                return;
            }
            Req::Seek(up) if self.phase.is_on() => {
                self.want.seek = Some(up);
                self.want.tune = None;
            }
            Req::Scan if self.phase.is_on() => {
                self.want.scan = true;
                self.want.tune = None;
                self.want.seek = None;
            }
            Req::Seek(_) | Req::Scan => {}
            Req::Cancel => {
                if matches!(self.phase, Phase::Seeking | Phase::Scanning) {
                    self.want.cancel = true;
                }
                self.want.seek = None;
                self.want.scan = false;
            }
            Req::Mute(m) => {
                if m != self.muted {
                    self.muted = m;
                    self.want.mute = true;
                    self.changed();
                }
            }
            Req::Volume(v) => {
                let v = v.min(30);
                if v != self.volume {
                    self.volume = v;
                    self.want.volume = true;
                }
            }
            Req::Config(c) => {
                if c != self.cfg {
                    if self.phase.is_on() {
                        self.want.config = Some(c);
                    } else {
                        self.cfg = c;
                        self.khz = c.region.snap(self.khz);
                    }
                    self.changed();
                }
            }
        }
        // Act on it at the next poll, but never cut the start-up's or the
        // ramp's waits short.
        if matches!(self.step, Step::Listen | Step::Tune(_) | Step::Seek) {
            self.at = now;
        }
    }

    fn start(&mut self, now: u64) {
        if self.phase.is_on() {
            return;
        }
        if self.phase == Phase::Stopping {
            // Still ramping down: finish that first, then start.
            self.want.tune = Some(self.khz);
            return;
        }
        self.phase = Phase::Starting;
        self.step = Step::Reset;
        self.failed = false;
        self.fails = 0;
        self.chip = 0;
        self.want = Want::default();
        self.rds.reset();
        self.rssi = 0;
        self.stereo = false;
        self.khz = self.cfg.region.snap(self.khz);
        self.at = now;
        self.changed();
    }

    fn stop(&mut self, now: u64) {
        if !self.phase.is_on() {
            return;
        }
        self.phase = Phase::Stopping;
        self.step = Step::Stop;
        self.want = Want::default();
        self.at = now;
        self.changed();
    }

    fn fail<P: Port>(&mut self, p: &mut P, now: u64) {
        self.errors = self.errors.wrapping_add(1);
        self.failed = true;
        if self.routed {
            self.phase = Phase::Stopping;
            self.step = Step::Stop;
            self.at = now;
        } else {
            let _ = self.tuner.standby(p);
            self.phase = Phase::Failed;
            self.step = Step::Idle;
            if self.pa != PA_MUSIC {
                self.pa = PA_MUSIC;
                p.analog(Analog::Gain(PA_MUSIC));
            }
        }
        self.changed();
    }

    fn setup(&self) -> Setup {
        let (tv, _) = volume::for_ui(self.volume);
        Setup {
            region: self.cfg.region,
            khz: self.khz,
            mono: self.cfg.mono,
            rds: self.cfg.rds,
            softmute: self.cfg.softmute,
            threshold: self.cfg.threshold(),
            volume: tv,
        }
    }

    /// The gain the amplifier should be at now.
    fn pa_target(&self) -> u8 {
        if matches!(self.phase, Phase::Playing | Phase::Seeking | Phase::Scanning) && self.step != Step::Stop
        {
            volume::for_ui(self.volume).1
        } else {
            0
        }
    }

    /// The tuner's audio: muted by the user, at volume 0 or while scanning.
    fn audible(&self) -> bool {
        !self.muted && self.volume > 0 && self.phase != Phase::Scanning
    }

    fn tune_now<P: Port>(&mut self, p: &mut P, khz: u32, then: Then, now: u64) {
        self.khz = khz;
        self.rds.reset();
        self.rds_seq = self.rds_seq.wrapping_add(1);
        if !self.tuner.tune(p, khz) {
            self.errors = self.errors.wrapping_add(1);
        }
        self.step = Step::Tune(then);
        self.limit = now + TUNE_LIMIT_MS;
        self.at = now + TUNE_POLL_MS;
        self.changed();
    }

    fn seek_now<P: Port>(&mut self, p: &mut P, now: u64) {
        if !self.tuner.seek(p, self.seek_up) {
            self.errors = self.errors.wrapping_add(1);
        }
        self.step = Step::Seek;
        self.limit = now + SEEK_LIMIT_MS;
        self.at = now + SEEK_POLL_MS;
    }

    fn listen(&mut self, now: u64) {
        self.step = Step::Listen;
        self.listen_at = now;
        self.at = now;
    }

    /// Do what is due; returns the next deadline (`None` = idle).
    pub fn poll<P: Port>(&mut self, p: &mut P, now: u64) -> Option<u64> {
        if self.step == Step::Idle {
            return None;
        }
        if now < self.at {
            return Some(self.at);
        }
        if matches!(self.step, Step::Listen | Step::Tune(_) | Step::Seek) {
            self.apply_wants(p, now);
        }
        match self.step {
            Step::Idle => return None,
            Step::Reset => {
                if self.tuner.soft_reset(p) {
                    self.step = Step::Load;
                    self.at = now + RESET_MS;
                } else {
                    self.fail(p, now);
                }
            }
            Step::Load => {
                let setup = self.setup();
                match self.tuner.identify(p) {
                    Some(id) if self.tuner.load(p, id, &setup) => {
                        self.chip = id;
                        self.step = Step::Precharge;
                        self.at = now + SETTLE_MS;
                        self.changed();
                    }
                    _ => self.fail(p, now),
                }
            }
            Step::Precharge => {
                self.pa = 0;
                p.analog(Analog::Gain(0));
                p.analog(Analog::Precharge);
                self.routed = true;
                self.step = Step::Connect;
                self.at = now + ROUTE_MS;
            }
            Step::Connect => {
                p.analog(Analog::Connect);
                self.step = Step::Release;
                self.at = now + ROUTE_MS;
            }
            Step::Release => {
                p.analog(Analog::Release);
                let k = self.want.tune.take().unwrap_or(self.khz);
                self.tune_now(p, k, Then::Listen, now);
            }
            Step::Tune(then) => match self.tuner.status(p, false) {
                Some(st) if st.stc() || now >= self.limit => {
                    self.last = st;
                    self.rssi = st.rssi();
                    self.stereo = st.stereo();
                    self.tuned(p, then, now);
                }
                Some(_) => self.at = now + TUNE_POLL_MS,
                None => self.bus_error(p, now, TUNE_POLL_MS),
            },
            Step::Seek => match self.tuner.status(p, false) {
                Some(st) if st.stc() => {
                    self.last = st;
                    self.seek_stopped(p, st, now);
                }
                Some(st) if now >= self.limit || now >= self.seek_end => {
                    // The chip never finished: stop where it is.
                    let _ = self.tuner.stop_seek(p);
                    let k = self.tuner.khz_of_chan(self.khz, st.chan());
                    if self.phase == Phase::Scanning {
                        self.finish_scan(p, now);
                    } else {
                        let k = if self.cfg.region.contains(k) { k } else { self.seek_origin };
                        self.phase = Phase::Playing;
                        self.tune_now(p, k, Then::Listen, now);
                    }
                }
                Some(_) => self.at = now + SEEK_POLL_MS,
                None => self.bus_error(p, now, SEEK_POLL_MS),
            },
            Step::Listen => self.listen_poll(p, now),
            Step::Stop => {
                let _ = self.tuner.set_mute(p, true);
                self.step = Step::Ramp;
                self.at = now;
                if !self.routed {
                    self.pa = 0;
                }
                return self.poll(p, now);
            }
            Step::Ramp => {
                if self.pa > 0 {
                    self.pa -= 1;
                    p.analog(Analog::Gain(self.pa));
                    self.at = now + RAMP_MS;
                } else {
                    if self.routed {
                        p.analog(Analog::Disconnect);
                        self.routed = false;
                    }
                    self.pa = PA_MUSIC;
                    p.analog(Analog::Gain(PA_MUSIC));
                    let _ = self.tuner.standby(p);
                    self.phase = if self.failed { Phase::Failed } else { Phase::Off };
                    self.step = Step::Idle;
                    self.rssi = 0;
                    self.stereo = false;
                    self.rds.reset();
                    self.rds_seq = self.rds_seq.wrapping_add(1);
                    self.changed();
                    // Started again while stopping.
                    if let Some(k) = self.want.tune.take()
                        && !self.failed
                    {
                        self.khz = k;
                        self.start(now);
                        return Some(now);
                    }
                    return None;
                }
            }
        }
        if self.step == Step::Idle { None } else { Some(self.at) }
    }

    fn bus_error<P: Port>(&mut self, p: &mut P, now: u64, retry: u64) {
        self.errors = self.errors.wrapping_add(1);
        self.fails += 1;
        if self.fails >= FAIL_LIMIT {
            self.fail(p, now);
        } else {
            self.at = now + retry;
        }
    }

    fn apply_wants<P: Port>(&mut self, p: &mut P, now: u64) {
        // Any write of 02h without `SEEK` stops a running hardware seek:
        // settings wait for its end (a cancel or a new tune ends it).
        let seeking = self.step == Step::Seek;
        if !seeking && let Some(c) = self.want.config.take() {
            let old = self.cfg;
            self.cfg = c;
            if c.mono != old.mono {
                let _ = self.tuner.set_mono(p, c.mono);
            }
            if c.sensitivity != old.sensitivity {
                self.want.volume = true;
            }
            if c.region != old.region {
                let k = c.region.snap(self.want.tune.unwrap_or(self.khz));
                let _ = self.tuner.set_region(p, c.region, k);
                self.want.tune = Some(k);
                self.want.seek = None;
                self.want.scan = false;
                if self.phase == Phase::Scanning {
                    self.scan_n = 0;
                    self.phase = Phase::Playing;
                }
            }
        }
        if core::mem::take(&mut self.want.cancel) {
            match self.phase {
                Phase::Seeking => {
                    let _ = self.tuner.stop_seek(p);
                    self.phase = Phase::Playing;
                    let k = match self.tuner.status(p, false) {
                        Some(st) => self.tuner.khz_of_chan(self.khz, st.chan()),
                        None => self.seek_origin,
                    };
                    let k =
                        if self.cfg.region.contains(k) { self.cfg.region.snap(k) } else { self.seek_origin };
                    self.tune_now(p, k, Then::Listen, now);
                }
                Phase::Scanning => {
                    let _ = self.tuner.stop_seek(p);
                    self.phase = Phase::Playing;
                    self.scan_n = 0;
                    let _ = self.tuner.set_mute(p, !self.audible());
                    let k = self.scan_from;
                    self.tune_now(p, k, Then::Listen, now);
                }
                _ => {}
            }
        }
        if self.step != Step::Seek {
            if core::mem::take(&mut self.want.volume) {
                let (tv, _) = volume::for_ui(self.volume);
                if !self.tuner.set_volume(p, tv, self.cfg.threshold()) {
                    self.errors = self.errors.wrapping_add(1);
                }
                self.want.mute = true;
            }
            if core::mem::take(&mut self.want.mute) {
                let _ = self.tuner.set_mute(p, !self.audible());
            }
        }
        if let Some(k) = self.want.tune.take() {
            if self.phase == Phase::Scanning {
                self.scan_n = 0;
            }
            self.phase = Phase::Playing;
            let _ = self.tuner.set_mute(p, !self.audible());
            self.tune_now(p, k, Then::Listen, now);
        }
        if let Some(up) = self.want.seek.take() {
            self.phase = Phase::Seeking;
            self.seek_up = up;
            self.seek_origin = self.khz;
            self.jumps = 0;
            self.seek_end = now + SEEK_TOTAL_MS;
            self.rds.reset();
            self.rds_seq = self.rds_seq.wrapping_add(1);
            self.changed();
            self.seek_now(p, now);
        }
        if core::mem::take(&mut self.want.scan) {
            self.phase = Phase::Scanning;
            self.scan_n = 0;
            self.scan_last = 0;
            self.scan_from = self.khz;
            self.jumps = 0;
            self.seek_up = true;
            self.seek_end = now + SEEK_TOTAL_MS * 4;
            let _ = self.tuner.set_mute(p, true);
            let low = self.cfg.region.low_khz();
            self.tune_now(p, low, Then::Scan, now);
        }
    }

    /// A tune finished (`STC`, or its time ran out).
    fn tuned<P: Port>(&mut self, p: &mut P, then: Then, now: u64) {
        match then {
            Then::Listen => {
                if self.phase == Phase::Starting {
                    self.phase = Phase::Playing;
                    let (tv, _) = volume::for_ui(self.volume);
                    let _ = self.tuner.set_volume(p, tv, self.cfg.threshold());
                }
                if self.phase == Phase::Seeking {
                    self.phase = Phase::Playing;
                }
                let _ = self.tuner.set_mute(p, !self.audible());
                self.changed();
                self.listen(now);
            }
            Then::Seek => self.seek_now(p, now),
            Then::Scan => {
                self.scan_last = self.khz;
                self.seek_now(p, now);
            }
        }
    }

    /// A hardware seek stopped (`STC`).
    fn seek_stopped<P: Port>(&mut self, p: &mut P, st: Status, now: u64) {
        let region = self.cfg.region;
        let k = self.tuner.khz_of_chan(self.khz, st.chan());
        let found = !st.seek_failed() && region.contains(k);
        self.rssi = st.rssi();
        self.stereo = st.stereo();
        if self.phase == Phase::Scanning {
            if found && k > self.scan_last {
                self.khz = k;
                self.tuner.follow(k);
                self.scan_last = k;
                if !is_spur(k) {
                    self.record(Station::new(k, st.rssi()));
                }
                self.changed();
                if now >= self.seek_end {
                    self.finish_scan(p, now);
                } else {
                    self.seek_now(p, now);
                }
            } else if region.segments() > 1
                && region.band(self.khz) == crate::region::BAND_65
                && self.jumps == 0
            {
                // Wide: 65–76 MHz done, on to 76–108 MHz.
                self.jumps = 1;
                self.scan_last = 75_900;
                self.tune_now(p, 76_000, Then::Seek, now);
            } else {
                self.finish_scan(p, now);
            }
            return;
        }
        if found && !is_spur(k) {
            self.phase = Phase::Playing;
            self.khz = k;
            self.tuner.follow(k);
            let _ = self.tuner.set_mute(p, !self.audible());
            self.changed();
            self.listen(now);
        } else if found {
            // The reference harmonic: seek on from it.
            self.khz = k;
            self.tuner.follow(k);
            self.seek_now(p, now);
        } else {
            self.jumps += 1;
            if self.jumps > region.segments() || now >= self.seek_end {
                // Nothing anywhere: back where the seek started.
                self.phase = Phase::Playing;
                let o = self.seek_origin;
                self.tune_now(p, o, Then::Listen, now);
            } else {
                // The band edge, or a station outside the region: go on in
                // the next segment from where this hardware seek started
                // (`khz` is not moved by a seek until it stops on a station).
                let next = region.segment_after(self.khz, self.seek_up);
                self.tune_now(p, next, Then::Seek, now);
            }
        }
    }

    fn record(&mut self, s: Station) {
        let n = usize::from(self.scan_n);
        if n < SCAN_MAX {
            if let Some(slot) = self.scan.get_mut(n) {
                *slot = s;
                self.scan_n += 1;
            }
            return;
        }
        // Full: replace the weakest if this one is stronger.
        if let Some(w) = self.scan.iter_mut().min_by_key(|x| x.rssi)
            && s.rssi > w.rssi
        {
            *w = s;
        }
    }

    fn finish_scan<P: Port>(&mut self, p: &mut P, now: u64) {
        // Strongest first (insertion sort; equal strength by frequency).
        let n = usize::from(self.scan_n).min(SCAN_MAX);
        let list = self.scan.get_mut(..n).unwrap_or(&mut []);
        for i in 1..list.len() {
            let mut j = i;
            while j > 0 {
                let (a, b) = (list.get(j - 1).copied(), list.get(j).copied());
                match (a, b) {
                    (Some(a), Some(b)) if (b.rssi, u16::MAX - b.khz10) > (a.rssi, u16::MAX - a.khz10) => {
                        list.swap(j - 1, j);
                        j -= 1;
                    }
                    _ => break,
                }
            }
        }
        self.scan_seq = self.scan_seq.wrapping_add(1);
        self.phase = Phase::Playing;
        let k = self.scan.first().filter(|_| n > 0).map_or(self.scan_from, Station::khz);
        self.tune_now(p, k, Then::Listen, now);
    }

    fn listen_poll<P: Port>(&mut self, p: &mut P, now: u64) {
        let target = self.pa_target();
        let mut next = u64::MAX;
        if self.pa != target {
            self.pa = if self.pa < target { self.pa + 1 } else { self.pa - 1 };
            p.analog(Analog::Gain(self.pa));
            if self.pa != target {
                next = now + RAMP_MS;
            }
        }
        if now >= self.listen_at {
            let blocks = self.cfg.rds && self.tuner.variant().rds();
            match self.tuner.status(p, blocks) {
                Some(st) => {
                    self.fails = 0;
                    self.last = st;
                    let rssi = st.rssi();
                    if st.stereo() != self.stereo || rssi.abs_diff(self.rssi) >= 4 {
                        self.changed();
                    }
                    self.rssi = rssi;
                    self.stereo = st.stereo();
                    if st.rds_ready() {
                        let (a, b) = st.bler();
                        if self.rds.feed(st.blocks, a, b) != 0 {
                            self.rds_seq = self.rds_seq.wrapping_add(1);
                        }
                    }
                }
                None => {
                    self.errors = self.errors.wrapping_add(1);
                    self.fails += 1;
                    if self.fails >= FAIL_LIMIT {
                        self.fail(p, now);
                        return;
                    }
                }
            }
            self.listen_at = now + LISTEN_MS;
        }
        self.at = next.min(self.listen_at);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::fake::FakeChip;
    use std::vec;
    use std::vec::Vec;

    /// Run the radio until it is idle or `ms` passed; returns the time.
    fn run(r: &mut Radio, chip: &mut FakeChip, from: u64, ms: u64) -> u64 {
        let mut now = from;
        let end = from + ms;
        while now < end {
            match r.poll(chip, now) {
                Some(t) => now = t.max(now + 1).min(end),
                None => break,
            }
        }
        now
    }

    fn started(region: Region, khz: u32) -> (Radio, FakeChip, u64) {
        let mut r = Radio::new();
        let mut chip = FakeChip::new();
        r.request(Req::Config(Config { region, ..Config::DEFAULT }), 0);
        r.request(Req::Tune(khz), 0);
        let t = run(&mut r, &mut chip, 0, 2000);
        assert_eq!(r.phase(), Phase::Playing);
        (r, chip, t)
    }

    #[test]
    fn start_follows_the_vendor_order() {
        let mut r = Radio::new();
        let mut chip = FakeChip::new();
        assert_eq!(r.poll(&mut chip, 0), None, "idle when off");
        r.request(Req::Volume(30), 0);
        r.request(Req::Tune(101_900), 0);
        assert_eq!(r.phase(), Phase::Starting);
        let t = run(&mut r, &mut chip, 0, 2000);
        assert_eq!(r.phase(), Phase::Playing);
        assert_eq!(r.info().khz, 101_900);
        assert_eq!(chip.khz(), 101_900);
        assert!(chip.enabled() && chip.audible());
        assert_eq!(r.info().chip, 0x5804);
        // The analogue sequence: gain 0, bypass, then the ramp up to 7.
        let a = &chip.analog;
        assert_eq!(&a[..4], &[Analog::Gain(0), Analog::Precharge, Analog::Connect, Analog::Release]);
        assert_eq!(a[4..].to_vec(), (1..=7).map(Analog::Gain).collect::<Vec<_>>());
        assert!(chip.routed());
        // Reset, then the table (one transfer), then tune.
        assert_eq!(chip.writes[0], vec![0x00, 0x02]);
        assert_eq!(chip.writes[1].len(), 104);
        // The 500 ms settle and both 100 ms route waits passed.
        assert!(t >= RESET_MS + SETTLE_MS + 2 * ROUTE_MS);
        assert_eq!(chip.volume(), 15);
    }

    #[test]
    fn every_write_after_the_table_keeps_the_tuner_enabled() {
        use crate::chip::r02;
        let (mut r, mut chip, t) = started(Region::Europe, 101_900);
        // Exercise the writes of a session: volume, mute, mono, step, seek.
        r.request(Req::Volume(20), t);
        r.request(Req::Mute(true), t);
        let t = run(&mut r, &mut chip, t, 100);
        r.request(Req::Mute(false), t);
        r.request(Req::Config(Config { mono: true, ..Config::DEFAULT }), t);
        let t = run(&mut r, &mut chip, t, 100);
        r.request(Req::Step(true), t);
        let t = run(&mut r, &mut chip, t, 200);
        r.request(Req::Seek(true), t);
        run(&mut r, &mut chip, t, 2000);
        let table = chip.writes.iter().position(|w| w.len() == 104).unwrap();
        assert_eq!(table, 1, "soft reset, then the table");
        assert_eq!(chip.writes[0], [0x00, 0x02], "the reset write is the only one without ENABLE");
        for (i, w) in chip.writes.iter().enumerate().skip(table) {
            let r02v = u16::from_be_bytes([w[0], w[1]]);
            assert_ne!(r02v & r02::ENABLE, 0, "write {i} ({w:02X?}) clears ENABLE");
            assert_ne!(r02v & r02::DHIZ, 0, "write {i} puts the outputs in high-Z");
        }
        // Playing: 02h has DHIZ, DMUTE (audible), RDS and ENABLE.
        let shadow = r.tuner().regs()[0];
        assert_eq!(shadow & (r02::DHIZ | r02::DMUTE | r02::ENABLE), r02::DHIZ | r02::DMUTE | r02::ENABLE);
        assert_ne!(shadow & r02::RDS_EN, 0);
        assert!(chip.enabled() && chip.audible());
        assert_eq!(r.info().chip, 0x5804);
    }

    #[test]
    fn the_chip_id_comes_from_register_00_when_0eh_has_none() {
        let mut r = Radio::new();
        let mut chip = FakeChip::new();
        chip.seq_id = 0;
        r.request(Req::Tune(94_700), 0);
        run(&mut r, &mut chip, 0, 2000);
        assert_eq!(r.info().chip, 0x5804);
        assert_eq!(chip.writes[1].len(), 104, "table A, not the fallback table");
        assert_eq!(r.phase(), Phase::Playing);
    }

    #[test]
    fn stop_ramps_down_restores_music_gain_and_parks_the_tuner() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        chip.analog.clear();
        r.request(Req::Off, t);
        assert_eq!(r.phase(), Phase::Stopping);
        run(&mut r, &mut chip, t, 500);
        assert_eq!(r.phase(), Phase::Off);
        assert!(!chip.enabled(), "tuner off when the radio is not playing");
        assert!(!chip.routed());
        let gains: Vec<_> = chip
            .analog
            .iter()
            .filter_map(|a| if let Analog::Gain(g) = a { Some(*g) } else { None })
            .collect();
        assert_eq!(gains.last(), Some(&PA_MUSIC), "music gain restored");
        assert!(gains.contains(&0), "ramped to 0 before the bypass went off");
        let off = chip.analog.iter().position(|a| *a == Analog::Disconnect).unwrap();
        assert_eq!(chip.analog[off - 1], Analog::Gain(0));
        assert_eq!(r.poll(&mut chip, t + 1000), None);
    }

    #[test]
    fn missing_tuner_fails_without_touching_the_route() {
        let mut r = Radio::new();
        let mut chip = FakeChip::new();
        chip.absent = true;
        r.request(Req::On, 0);
        run(&mut r, &mut chip, 0, 2000);
        assert_eq!(r.phase(), Phase::Failed);
        assert!(chip.analog.is_empty());
        // Retried by asking again.
        chip.absent = false;
        r.request(Req::On, 3000);
        run(&mut r, &mut chip, 3000, 2000);
        assert_eq!(r.phase(), Phase::Playing);
    }

    #[test]
    fn a_tuner_that_stops_answering_ends_the_radio() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        chip.absent = true;
        run(&mut r, &mut chip, t, 2000);
        assert_eq!(r.phase(), Phase::Failed);
        assert!(!chip.routed());
        assert_eq!(chip.gain(), PA_MUSIC);
    }

    #[test]
    fn steps_wrap_at_the_region_limits() {
        let (mut r, mut chip, t) = started(Region::Europe, 108_000);
        r.request(Req::Step(true), t);
        let t = run(&mut r, &mut chip, t, 300);
        assert_eq!(r.info().khz, 87_500);
        assert_eq!(chip.khz(), 87_500);
        r.request(Req::Step(false), t);
        run(&mut r, &mut chip, t, 300);
        assert_eq!(chip.khz(), 108_000);
    }

    #[test]
    fn seek_stops_on_stations_skips_the_harmonic_and_wraps() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        r.request(Req::Seek(true), t);
        let t = run(&mut r, &mut chip, t, 3000);
        assert_eq!(r.phase(), Phase::Playing);
        assert_eq!(r.info().khz, 99_900, "96.0 (the 24 MHz harmonic) skipped");
        // Up from the top station wraps to the first.
        r.request(Req::Tune(104_300), t);
        let t = run(&mut r, &mut chip, t, 300);
        r.request(Req::Seek(true), t);
        let t = run(&mut r, &mut chip, t, 5000);
        assert_eq!(r.info().khz, 88_300, "106.7 is below the threshold; wrapped");
        r.request(Req::Seek(false), t);
        run(&mut r, &mut chip, t, 5000);
        assert_eq!(r.info().khz, 104_300, "down wraps too");
    }

    #[test]
    fn high_sensitivity_finds_weak_stations() {
        let (mut r, mut chip, t) = started(Region::Europe, 104_300);
        r.request(Req::Config(Config { sensitivity: 2, ..Config::DEFAULT }), t);
        r.request(Req::Seek(true), t);
        run(&mut r, &mut chip, t, 3000);
        assert_eq!(r.info().khz, 106_700);
    }

    #[test]
    fn japan_limits_and_wide_crosses_bands() {
        let (mut r, mut chip, t) = started(Region::Japan, 90_000);
        r.request(Req::Seek(true), t);
        let t = run(&mut r, &mut chip, t, 6000);
        // 91.5 and 94.7 are in Japan's range.
        assert_eq!(r.info().khz, 91_500);
        r.request(Req::Tune(94_700), t);
        let t = run(&mut r, &mut chip, t, 300);
        r.request(Req::Seek(true), t);
        run(&mut r, &mut chip, t, 8000);
        assert_eq!(r.info().khz, 77_100, "99.9 is outside 76–95: wrapped to 76");
        let (mut r, mut chip, t) = started(Region::Wide, 104_300);
        r.request(Req::Seek(true), t);
        let t = run(&mut r, &mut chip, t, 8000);
        assert_eq!(r.info().khz, 68_300, "past 108 into the 65–76 band");
        r.request(Req::Seek(true), t);
        let t = run(&mut r, &mut chip, t, 8000);
        assert_eq!(r.info().khz, 77_100, "on into 76–108");
        r.request(Req::Seek(false), t);
        run(&mut r, &mut chip, t, 8000);
        assert_eq!(r.info().khz, 68_300, "and back down");
    }

    #[test]
    fn seek_with_nothing_to_find_returns_to_the_origin() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        chip.stations.clear();
        r.request(Req::Seek(true), t);
        run(&mut r, &mut chip, t, 20_000);
        assert_eq!(r.phase(), Phase::Playing);
        assert_eq!(r.info().khz, 94_700);
    }

    #[test]
    fn cancel_stops_a_seek_where_it_is() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        chip.seek_reads = 50;
        r.request(Req::Seek(true), t);
        let t = run(&mut r, &mut chip, t, 100);
        assert_eq!(r.phase(), Phase::Seeking);
        r.request(Req::Cancel, t);
        run(&mut r, &mut chip, t, 300);
        assert_eq!(r.phase(), Phase::Playing);
        assert!(Region::Europe.contains(r.info().khz));
        assert_eq!(chip.khz(), r.info().khz);
    }

    #[test]
    fn scan_keeps_strongest_first_and_tunes_to_the_first() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        r.request(Req::Scan, t);
        let mut now = t;
        let mut saw_progress = false;
        while r.phase() == Phase::Scanning || now == t {
            now = run(&mut r, &mut chip, now, 50);
            saw_progress |= r.info().progress > 0;
            assert!(now < t + 30_000);
        }
        assert!(saw_progress);
        let got: Vec<u32> = r.scan_results().iter().map(Station::khz).collect();
        // Strongest first; 96.0 (harmonic) and 106.7 (weak) left out.
        assert_eq!(got, [101_900, 94_700, 91_500, 99_900, 88_300, 104_300]);
        assert_eq!(r.info().scan_seq, 1);
        run(&mut r, &mut chip, now, 300);
        assert_eq!(chip.khz(), 101_900);
        assert!(chip.audible(), "unmuted after the scan");
        // The wide region finds the 65–76 MHz station too.
        let (mut r, mut chip, t) = started(Region::Wide, 94_700);
        r.request(Req::Scan, t);
        run(&mut r, &mut chip, t, 30_000);
        assert!(r.scan_results().iter().any(|s| s.khz() == 68_300));
        assert!(r.scan_results().iter().any(|s| s.khz() == 77_100));
    }

    #[test]
    fn scan_cancel_returns_to_the_old_frequency() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        r.request(Req::Scan, t);
        let t = run(&mut r, &mut chip, t, 120);
        assert_eq!(r.phase(), Phase::Scanning);
        r.request(Req::Cancel, t);
        run(&mut r, &mut chip, t, 300);
        assert_eq!(r.phase(), Phase::Playing);
        assert_eq!(chip.khz(), 94_700);
        assert!(r.scan_results().is_empty());
        assert_eq!(r.info().scan_seq, 0);
    }

    #[test]
    fn volume_pairs_mute_and_mono() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        r.request(Req::Volume(15), t);
        let t = run(&mut r, &mut chip, t, 100);
        assert_eq!((chip.volume(), chip.gain()), volume::for_ui(15));
        r.request(Req::Volume(0), t);
        let t = run(&mut r, &mut chip, t, 100);
        assert!(!chip.audible(), "volume 0 mutes the tuner");
        assert_eq!(chip.gain(), 0);
        r.request(Req::Volume(20), t);
        r.request(Req::Mute(true), t);
        let t = run(&mut r, &mut chip, t, 100);
        assert!(!chip.audible());
        assert!(r.info().muted);
        r.request(Req::Mute(false), t);
        let t = run(&mut r, &mut chip, t, 100);
        assert!(chip.audible());
        r.request(Req::Config(Config { mono: true, ..Config::DEFAULT }), t);
        let t = run(&mut r, &mut chip, t, 100);
        assert!(chip.mono());
        assert!(!r.info().stereo, "forced mono: no stereo indicator");
        let _ = t;
    }

    /// VER-13 (Set from RDS): a station's clock time reaches the radio's
    /// decoder once two minutes' groups agree, and counts as an RDS change.
    #[test]
    fn rds_clock_time_arrives() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        let utc = 1_795_595_400u64; // 2026-11-25 08:30:00
        chip.clock = Some(utc);
        let t = run(&mut r, &mut chip, t, 2000);
        assert_eq!(r.rds().clock(), None, "one minute's group only");
        let seq = r.info().rds_seq;
        chip.clock = Some(utc + 60);
        run(&mut r, &mut chip, t, 2000);
        assert_eq!(r.rds().clock(), Some(utc + 60));
        assert_ne!(r.info().rds_seq, seq);
    }

    #[test]
    fn rds_name_and_text_arrive() {
        let (mut r, mut chip, t) = started(Region::Europe, 101_900);
        run(&mut r, &mut chip, t, 3000);
        assert_eq!(r.rds().ps(), b"PARADISE");
        assert_eq!(r.rds().rt(), b"Radio Paradise - eclectic music");
        assert!(r.info().rds_seq > 1, "the name and text arrived as changes");
        assert!(r.info().stereo);
        assert!(r.info().rssi >= 55);
        // Tuning elsewhere forgets it.
        r.request(Req::Tune(88_300), t + 3000);
        run(&mut r, &mut chip, t + 3000, 2000);
        assert_eq!(r.rds().ps(), b"");
        assert!(!r.info().stereo);
    }

    #[test]
    fn region_change_while_playing_snaps_and_retunes() {
        let (mut r, mut chip, t) = started(Region::Europe, 88_000);
        r.request(Req::Config(Config { region: Region::UnitedStates, ..Config::DEFAULT }), t);
        run(&mut r, &mut chip, t, 300);
        assert_eq!(r.info().khz, 88_100, "88.0 is half way: rounded up");
        assert_eq!(chip.khz(), 88_100);
        assert!(!chip.de_50us());
    }

    #[test]
    fn tune_while_stopping_starts_again() {
        let (mut r, mut chip, t) = started(Region::Europe, 94_700);
        r.request(Req::Off, t);
        r.request(Req::Tune(99_900), t + 1);
        run(&mut r, &mut chip, t + 1, 3000);
        assert_eq!(r.phase(), Phase::Playing);
        assert_eq!(chip.khz(), 99_900);
    }

    #[test]
    fn park_writes_standby() {
        let mut r = Radio::new();
        let mut chip = FakeChip::new();
        chip.force_enabled();
        assert!(r.park(&mut chip));
        assert!(!chip.enabled());
        assert_eq!(chip.writes.last().unwrap().len(), 2);
    }
}
