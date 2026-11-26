//! The simulator's radio (WP UI-5a): the firmware's own radio service
//! (`orpheus_fm::Radio`, the same state machine the `fm` task runs) over
//! the register-level fake tuner (`orpheus_fm::fake`: stations with RSSI,
//! stereo, RDS name and text, the 96 MHz reference harmonic), driven by
//! simulated time. Presets, the earphone (antenna) state and the requests
//! the screens made are kept here.

use orpheus_apps::services::{PresetOp, Presets, RadioInfo, RadioPhase, RadioRequest, radio_event};
use orpheus_fm::fake::FakeChip;
use orpheus_fm::radio::{Config, Phase, Radio, Req};
use orpheus_fm::rds;

pub struct SimRadio {
    pub radio: Radio,
    pub chip: FakeChip,
    pub presets: Presets,
    /// Earphones plugged in (`set:antenna=0` takes them out).
    pub antenna: bool,
    /// Requests the screens made, in order (behaviour tests).
    pub log: Vec<RadioRequest>,
    now: u64,
    seen: orpheus_fm::Info,
    ps: String,
    rt: String,
    title: String,
    events: Vec<u32>,
    /// Milliseconds of the on-air clock (`chip.clock`) not yet a second.
    clock_ms: u64,
}

impl Default for SimRadio {
    fn default() -> Self {
        SimRadio {
            radio: Radio::new(),
            chip: FakeChip::new(),
            presets: Presets::new(),
            antenna: true,
            log: Vec::new(),
            now: 0,
            seen: orpheus_fm::Info::default(),
            ps: String::new(),
            rt: String::new(),
            title: String::new(),
            events: Vec::new(),
            clock_ms: 0,
        }
    }
}

fn text(raw: &[u8]) -> String {
    let mut b = [0u8; 256];
    let n = rds::to_utf8(raw, &mut b);
    String::from_utf8_lossy(&b[..n]).into_owned()
}

impl SimRadio {
    #[must_use]
    pub fn info(&self) -> RadioInfo {
        let i = self.radio.info();
        let phase = match i.phase {
            Phase::Off | Phase::Stopping => RadioPhase::Off,
            Phase::Starting => RadioPhase::Starting,
            Phase::Playing => RadioPhase::Playing,
            Phase::Seeking => RadioPhase::Seeking,
            Phase::Scanning => RadioPhase::Scanning,
            Phase::Failed => RadioPhase::NotResponding,
        };
        RadioInfo {
            phase,
            khz: i.khz,
            rssi: i.rssi,
            stereo: i.stereo,
            muted: i.muted,
            antenna: self.antenna,
            progress: i.progress,
            found: i.found,
        }
    }

    #[must_use]
    pub fn ps(&self) -> &str {
        &self.ps
    }

    #[must_use]
    pub fn rt(&self) -> &str {
        &self.rt
    }

    /// "FM 101.90 · NAME" for the mini-player.
    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
    }

    pub fn request(&mut self, r: RadioRequest) {
        self.log.push(r);
        let req = match r {
            RadioRequest::Start => Req::On,
            RadioRequest::Stop => Req::Off,
            RadioRequest::Tune(k) => Req::Tune(k),
            RadioRequest::Step { up } => Req::Step(up),
            RadioRequest::Seek { up } => Req::Seek(up),
            RadioRequest::Cancel => Req::Cancel,
            RadioRequest::Mute(m) => Req::Mute(m),
            RadioRequest::Scan => Req::Scan,
        };
        self.radio.request(req, self.now);
        self.advance(0);
    }

    /// Settings › Radio changed.
    pub fn config(&mut self, c: Config) {
        self.radio.request(Req::Config(c), self.now);
    }

    pub fn volume(&mut self, v: u8) {
        self.radio.request(Req::Volume(v), self.now);
    }

    pub fn preset_op(&mut self, op: PresetOp) -> bool {
        let ok = match op {
            PresetOp::Save(khz) => {
                let name: Vec<u8> =
                    if self.radio.info().khz == khz { self.radio.rds().ps().to_vec() } else { Vec::new() };
                self.presets.save(khz, &name).is_some()
            }
            PresetOp::Remove(i) => self.presets.remove(i),
            PresetOp::MoveUp(i) => i > 0 && self.presets.swap(i, i - 1),
            PresetOp::MoveDown(i) => self.presets.swap(i, i + 1),
        };
        if ok {
            self.events.push(radio_event::PRESETS);
        }
        ok
    }

    /// Let `ms` pass: the radio polls the fake chip at its own deadlines.
    pub fn advance(&mut self, ms: u64) {
        // The stations' clock time on air (`set:rdsclock=UNIX`).
        if let Some(c) = self.chip.clock.as_mut() {
            self.clock_ms += ms;
            *c += self.clock_ms / 1000;
            self.clock_ms %= 1000;
        }
        let end = self.now + ms;
        let mut guard = 0u32;
        while let Some(t) = self.radio.poll(&mut self.chip, self.now) {
            if t > end || guard > 100_000 {
                break;
            }
            self.now = t.max(self.now);
            guard += 1;
        }
        self.now = end;
        self.refresh();
    }

    fn refresh(&mut self) {
        let i = self.radio.info();
        let o = self.seen;
        if i.rds_seq != o.rds_seq {
            self.ps = text(self.radio.rds().ps());
            self.rt = text(self.radio.rds().rt());
            // A preset without a name takes the station's.
            if let Some(p) = self.presets.find(i.khz)
                && !self.radio.rds().ps().is_empty()
                && self.presets.get(p).is_some_and(|x| x.name().is_empty())
            {
                self.presets.set_name(p, self.radio.rds().ps());
            }
            self.events.push(radio_event::RDS);
        }
        if i.scan_seq != o.scan_seq {
            let found: Vec<u32> =
                self.radio.scan_results().iter().map(orpheus_fm::radio::Station::khz).collect();
            if !found.is_empty() {
                self.presets.replace(found.into_iter());
            }
            self.events.push(radio_event::SCANNED);
        }
        if i.seq != o.seq || i.rds_seq != o.rds_seq {
            self.title = if i.phase == Phase::Off || i.phase == Phase::Failed {
                String::new()
            } else {
                let mut b = orpheus_ui::text::StrBuf::<16>::new();
                orpheus_apps::services::push_mhz(i.khz, &mut b);
                if self.ps.is_empty() {
                    format!("FM {}", b.as_str())
                } else {
                    format!("FM {} · {}", b.as_str(), self.ps)
                }
            };
            self.events.push(radio_event::STATE);
        }
        self.seen = i;
    }

    pub fn take_events(&mut self) -> Vec<u32> {
        core::mem::take(&mut self.events)
    }

    /// Script helper: presets as an auto-scan would leave them.
    pub fn demo_presets(&mut self) {
        self.presets.replace([101_900, 94_700, 91_500, 99_900, 88_300, 104_300].into_iter());
        self.presets.set_name(0, b"PARADISE");
        self.presets.set_name(1, b"ROCK FM");
        self.presets.set_name(2, b"KLASSIK");
        self.presets.set_name(3, b"RADIO 1");
        self.events.push(radio_event::PRESETS);
    }

    /// Script helper: the radio playing at `khz`, started and settled.
    pub fn play_at(&mut self, khz: u32) {
        self.radio.request(Req::Tune(khz), self.now);
        self.advance(3000);
    }

    /// A fresh radio with `cfg` (setting values at start).
    pub fn configure(&mut self, cfg: Config) {
        self.radio.request(Req::Config(cfg), self.now);
    }
}
