//! The simulator's recorder (WP UI-5b): the same session the firmware's
//! card task runs (`orpheus_rec::session`) fed by a fake ADC (tones,
//! noise, speech-like bursts), writing real WAV/FLAC files through
//! `orpheus-fs` onto the simulated card's `/Recordings`, in simulated
//! time. `set:recstall=MS` makes the card answer nothing for a while (the
//! write-behind ring fills, blocks drop and are counted).

use orpheus_apps::services::{DirId, RecEnd, RecInfo, RecPhase, RecSettings, RecSource, rec_event};
use orpheus_fs::host::block_on;
use orpheus_fs::{File, OpenMode};
use orpheus_rec::chain::Chain;
use orpheus_rec::encoder::{BUF_SAMPLES, MAX_BLOCK_BYTES, MAX_HEADER};
use orpheus_rec::fake::{FakeAdc, Signal};
use orpheus_rec::name::{self, Stamp};
use orpheus_rec::session::{Config, Session, State, StopReason};

use crate::media::Media;

/// The write-behind ring (the firmware's is about this size).
pub const RING: usize = 40 * 1024;
/// The recordings folder.
pub const FOLDER: &str = name::FOLDER;

pub struct SimRecorder {
    pub phase: RecPhase,
    /// The fake input (`set:recsignal=tone|noise|speech|silence`).
    pub signal: Signal,
    /// `set:recstall=MS`: no card writes for that long from now.
    pub stall_until: u64,
    pub line_in: bool,
    /// `set:rectuner=0`: no tuner (FM recordings end at once).
    pub tuner: bool,
    settings: Option<RecSettings>,
    adc: Option<FakeAdc>,
    chain: Option<Chain>,
    session: Option<Box<Session<Vec<i16>, Vec<u8>>>>,
    ring: Vec<u8>,
    file: Option<File>,
    path: String,
    pub name: String,
    pub ended: Option<RecEnd>,
    /// Requests and results, in order (behaviour tests).
    pub log: Vec<String>,
    /// Events for the screens (`rec_event`).
    pub events: Vec<u32>,
    now: u64,
    /// Sub-block time not yet captured (ms × rate).
    debt: u64,
    card_free: u64,
    fm_khz: u32,
    /// Size and length of the last file closed.
    pub last_file: Option<(String, u64)>,
    /// The clock at the start (an automatic split names the next file
    /// from it, with a suffix).
    when: Option<Stamp>,
}

impl Default for SimRecorder {
    fn default() -> Self {
        SimRecorder {
            phase: RecPhase::Off,
            signal: Signal::Bursts { hz: 220, amp: 14_000, on_ms: 700, off_ms: 500 },
            stall_until: 0,
            line_in: false,
            tuner: true,
            settings: None,
            adc: None,
            chain: None,
            session: None,
            ring: vec![0; RING],
            file: None,
            path: String::new(),
            name: String::new(),
            ended: None,
            log: Vec::new(),
            events: Vec::new(),
            now: 0,
            debt: 0,
            card_free: 0,
            fm_khz: 0,
            last_file: None,
            when: None,
        }
    }
}

impl SimRecorder {
    #[must_use]
    pub fn info(&self, settings: &RecSettings) -> RecInfo {
        let s = self.settings.as_ref().unwrap_or(settings);
        let mut i = RecInfo {
            phase: self.phase,
            source: s.source,
            format: s.params.format,
            rate: s.params.rate,
            channels: s.params.channels,
            auto_gain: s.chain.agc,
            gain_db: s.gain_db,
            fm_khz: if s.source == RecSource::Fm { self.fm_khz } else { 0 },
            ended: self.ended,
            bar: [-60; 2],
            hold: [-60; 2],
            ..RecInfo::default()
        };
        let levels = match (&self.session, &self.chain) {
            (Some(ss), _) => Some(ss.status().levels),
            (None, Some(c)) => Some(c.levels()),
            _ => None,
        };
        if let Some(l) = levels {
            for c in 0..2 {
                i.bar[c] = i8::try_from(l.bar[c]).unwrap_or(-60);
                i.hold[c] = i8::try_from(l.hold[c]).unwrap_or(-60);
            }
            i.clip = l.clip;
        }
        let bps = u64::from(s.params.bytes_per_second().max(1));
        match &self.session {
            Some(ss) => {
                let st = ss.status();
                i.elapsed_ms = u32::try_from(ss.elapsed_ms()).unwrap_or(u32::MAX);
                i.drops = st.dropouts;
                i.marks = u16::try_from(st.marks).unwrap_or(u16::MAX);
                if s.chain.agc {
                    i.gain_db = i8::try_from(st.agc_db10 / 10).unwrap_or(0);
                }
                i.left_s = Some(u32::try_from(self.card_free.saturating_sub(st.produced) / bps).unwrap_or(0));
            }
            None => i.left_s = (self.card_free > 0).then(|| u32::try_from(self.card_free / bps).unwrap_or(0)),
        }
        i
    }

    fn signal_for(&self, s: &RecSettings) -> Signal {
        match s.source {
            // The radio: music-like, a chord with noise.
            RecSource::Fm => Signal::Bursts { hz: 330, amp: 18_000, on_ms: 2_000, off_ms: 100 },
            _ => self.signal,
        }
    }

    /// Capture for the meters.
    pub fn arm(&mut self, s: RecSettings, media: &mut Media) {
        if self.phase.is_recording() {
            // Live changes while recording: the chain only.
            if let Some(ss) = self.session.as_mut() {
                ss.set_chain(s.chain);
            }
            return;
        }
        self.card_free = block_on(media.volume().free_bytes()).unwrap_or(0);
        self.adc = Some(FakeAdc::new(s.params.rate, s.params.channels, self.signal_for(&s), 11));
        self.chain = Some(Chain::new(s.chain, s.params.rate, s.params.channels));
        self.settings = Some(s);
        self.phase = RecPhase::Armed;
        self.log.push(format!("arm {}", s.source.name()));
        self.events.push(rec_event::STATE);
    }

    pub fn disarm(&mut self) {
        if !self.phase.is_recording() {
            self.phase = RecPhase::Off;
            self.adc = None;
            self.chain = None;
            self.log.push("disarm".into());
            self.events.push(rec_event::STATE);
        }
    }

    fn end(&mut self, why: RecEnd) {
        self.ended = Some(why);
        self.log.push(format!("ended {why:?}"));
        self.events.push(rec_event::ENDED);
        self.events.push(rec_event::STATE);
    }

    /// Start a recording (`fm_khz`: the station, when recording FM).
    pub fn start(&mut self, s: RecSettings, media: &mut Media, when: Option<Stamp>, fm_khz: Option<u32>) {
        if self.phase.is_recording() {
            return;
        }
        self.ended = None;
        if s.source == RecSource::Fm && (!self.tuner || fm_khz.is_none()) {
            self.end(RecEnd::NoSource);
            return;
        }
        self.fm_khz = fm_khz.unwrap_or(0);
        self.when = when;
        let vol = media.volume();
        let _ = block_on(vol.create_dir(FOLDER));
        // The name: dated, or the first free counter; `-2`… if taken.
        let mut path = String::new();
        for suffix in 0..100 {
            let counter = if when.is_none() { suffix + 1 } else { 0 };
            let p = name::path(
                FOLDER,
                when,
                counter,
                fm_khz,
                if when.is_some() { suffix } else { 0 },
                s.params.format.ext(),
            );
            if !block_on(vol.exists(p.as_str())).unwrap_or(true) {
                path = p.as_str().to_string();
                break;
            }
        }
        let Ok(f) = (if path.is_empty() {
            Err(())
        } else {
            block_on(vol.open(&path, OpenMode::CreateNew)).map_err(|_| ())
        }) else {
            self.end(RecEnd::NoCard);
            return;
        };
        self.card_free = block_on(vol.free_bytes()).unwrap_or(0);
        let cfg = Config {
            params: s.params,
            chain: s.chain,
            limit_ms: None,
            split_ms: (s.split_min > 0).then(|| u64::from(s.split_min) * 60_000),
            ring_bytes: RING,
            chunk: 16 * 1024,
            max_wait_ms: 1_000,
        };
        let mut ss = Box::new(Session::new(
            cfg,
            self.card_free,
            self.now,
            vec![0; BUF_SAMPLES],
            vec![0; MAX_BLOCK_BYTES],
        ));
        let ring = &mut self.ring;
        ss.begin(self.now, &mut |at, b| ring[at..at + b.len()].copy_from_slice(b));
        self.session = Some(ss);
        self.file = Some(f);
        self.name = path.rsplit('/').next().unwrap_or("").to_string();
        self.path = path;
        // The capture restarts with the recording's settings (and the
        // script's current fake input).
        self.adc = Some(FakeAdc::new(s.params.rate, s.params.channels, self.signal_for(&s), 11));
        self.chain = None;
        self.settings = Some(s);
        self.phase = RecPhase::Recording;
        self.log.push(format!("start {}", self.path));
        self.events.push(rec_event::STATE);
    }

    pub fn pause(&mut self, on: bool) {
        if let Some(ss) = self.session.as_mut() {
            if on {
                ss.pause();
            } else {
                ss.resume();
            }
            self.log.push(if on { "pause".into() } else { "resume".into() });
            self.events.push(rec_event::STATE);
        }
    }

    pub fn mark(&mut self) {
        if let Some(ss) = self.session.as_mut() {
            ss.mark();
            self.log.push("mark".into());
        }
    }

    /// Issue the card writes that are due (none while the card stalls).
    fn pump(&mut self, media: &mut Media) {
        if self.now < self.stall_until {
            return;
        }
        let (Some(ss), Some(f)) = (self.session.as_mut(), self.file.as_mut()) else { return };
        let vol = media.volume();
        while let Some(w) = ss.next_write(self.now) {
            f.set_position(w.file_at);
            let ok = block_on(vol.write(f, &self.ring[w.at..w.at + w.len])).is_ok();
            ss.written(ok, self.now);
            if !ok {
                break;
            }
        }
        if ss.header_due(self.now) {
            let mut h = [0u8; MAX_HEADER];
            let n = ss.header(&mut h, self.now);
            let end = f.position();
            f.set_position(0);
            let _ = block_on(vol.write(f, &h[..n]));
            f.set_position(end);
        }
    }

    /// Stop: the rest written, the final header, the file closed.
    pub fn stop(&mut self, why: StopReason, media: &mut Media) {
        let Some(mut ss) = self.session.take() else { return };
        self.phase = RecPhase::Saving;
        let ring = &mut self.ring;
        ss.finish(why, self.now, &mut |at, b| ring[at..at + b.len()].copy_from_slice(b));
        self.session = Some(ss);
        self.stall_until = 0;
        self.pump(media);
        let (Some(mut ss), Some(mut f)) = (self.session.take(), self.file.take()) else { return };
        let vol = media.volume();
        if ss.state() != State::Done {
            let mut h = [0u8; MAX_HEADER];
            let n = ss.final_header(&mut h);
            f.set_position(0);
            let _ = block_on(vol.write(&mut f, &h[..n]));
        }
        let size = f.size();
        let _ = block_on(vol.close(f));
        let _ = block_on(vol.flush());
        media.refresh();
        media.learn(FOLDER);
        self.last_file = Some((self.path.clone(), size));
        self.log.push(format!("saved {} {size}", self.path));
        self.phase = if self.adc.is_some() { RecPhase::Armed } else { RecPhase::Off };
        // Back to meters only.
        if let Some(s) = self.settings {
            self.chain = Some(Chain::new(s.chain, s.params.rate, s.params.channels));
        }
        self.end(match why {
            StopReason::CardFull => RecEnd::CardFull,
            StopReason::CardError => RecEnd::CardError,
            _ => RecEnd::Saved,
        });
    }

    /// Simulated time passes: capture, process, write.
    pub fn advance(&mut self, ms: u64, media: &mut Media) {
        let Some(s) = self.settings else {
            self.now += ms;
            return;
        };
        if self.adc.is_none() {
            self.now += ms;
            return;
        }
        let ch = usize::from(s.params.channels);
        let block = (s.params.rate / 50) as usize;
        let mut buf = vec![0i16; block * ch];
        self.debt += ms * u64::from(s.params.rate);
        let per_block = block as u64 * 1000;
        while self.debt >= per_block {
            self.debt -= per_block;
            self.now += 20;
            if let Some(a) = self.adc.as_mut() {
                a.fill(&mut buf);
            }
            match (self.session.as_mut(), self.chain.as_mut()) {
                (Some(ss), _) => {
                    let ring = &mut self.ring;
                    ss.feed(&mut buf, self.now, &mut |at, b| ring[at..at + b.len()].copy_from_slice(b));
                }
                (None, Some(c)) => {
                    c.process(&mut buf);
                }
                _ => {}
            }
            self.pump(media);
            if let Some(ss) = self.session.as_ref() {
                let phase = if ss.state() == State::Paused {
                    RecPhase::Paused
                } else if ss.status().waiting {
                    RecPhase::Waiting
                } else {
                    RecPhase::Recording
                };
                if phase != self.phase {
                    self.phase = phase;
                    self.events.push(rec_event::STATE);
                }
                if let Some(why) = ss.due_stop() {
                    let split = why == StopReason::Split;
                    self.stop(why, media);
                    if split {
                        let when = self.when;
                        self.start(s, media, when, (self.fm_khz > 0).then_some(self.fm_khz));
                    }
                }
            }
        }
    }

    /// The recordings folder (made by the first recording).
    pub fn dir(media: &mut Media) -> Option<DirId> {
        media.learn(FOLDER);
        media.dir_id(FOLDER)
    }
}
