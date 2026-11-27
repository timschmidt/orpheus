//! Sound for the browser: the current track decoded by the firmware's
//! codecs (`orpheus-codecs`) and run through its DSP chain
//! (`orpheus-dsp`, set from the Sound settings as the firmware does), as
//! interleaved stereo `f32` for the page's AudioWorklet.
//!
//! The simulator's player decides what plays and where (queue, position,
//! pause, seek, A–B, next track); the engine follows it: a new track, a
//! pause, or a position that jumps away from what was decoded restarts the
//! decoder there (and tells the page to drop what it buffered). A pause
//! with Settings › Sound › Fade on pause on fades out over 60 ms first, as
//! the firmware does (`Chain::begin_pause`). The decode
//! loop is an `async` block polled on demand: it owns the file, the codec
//! arena and the decoder that borrows them, without `unsafe`.

use std::cell::RefCell;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::task::{Context, Poll, Waker};

use orpheus_audio::probe::Probe;
use orpheus_audio::{
    ByteSource, ContainerInfo, DecodeError, PcmBlock, SeekSource, SeekStep, SliceSource, Step,
};
use orpheus_dsp::{Chain, OutWidth};
use orpheus_sim::media::NowAudio;

/// The Sound settings the chain takes (the firmware's `media::Sound`).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sound {
    pub eq: [i8; 5],
    pub bass: i8,
    pub balance: i8,
    pub crossfeed: u8,
    pub channels: u8,
    pub replaygain: u8,
}

impl Sound {
    /// From the settings, as the firmware's UI task publishes them.
    #[must_use]
    pub fn from_settings(get: impl Fn(u16) -> i16) -> Sound {
        let eq = orpheus_apps::settings::eq_gains(get(0x0101), |id| get(id.0));
        Sound {
            eq,
            bass: get(0x0103) as i8,
            balance: get(0x0102) as i8,
            crossfeed: get(0x0104) as u8,
            channels: get(0x0105) as u8,
            replaygain: get(0x0106) as u8,
        }
    }

    /// Set a chain (the firmware's `media::apply_sound`).
    pub fn apply(&self, chain: &mut Chain) {
        use orpheus_dsp::eq::{EQ5_FREQS, EQ5_Q100};
        use orpheus_dsp::{Band, ChannelMode, CrossfeedParams, RgMode, RgSettings};
        let bands: [Band; 5] =
            core::array::from_fn(|i| Band::peaking(EQ5_FREQS[i], EQ5_Q100, i16::from(self.eq[i]) * 100));
        chain.set_eq(&bands);
        chain.set_bass(i16::from(self.bass) * 100);
        chain.set_balance(self.balance.saturating_mul(10));
        chain.set_crossfeed(match self.crossfeed {
            1 => Some(CrossfeedParams::DEFAULT),
            2 => Some(CrossfeedParams::CHU_MOY),
            3 => Some(CrossfeedParams::MEIER),
            _ => None,
        });
        chain.set_channel_mode(match self.channels {
            1 => ChannelMode::Mono,
            2 => ChannelMode::Swap,
            _ => ChannelMode::Stereo,
        });
        let mode = match self.replaygain {
            1 => RgMode::Track,
            2 | 3 => RgMode::Album,
            _ => RgMode::Off,
        };
        chain.set_replaygain(RgSettings { mode, ..RgSettings::default() });
    }
}

/// The player's volume step (0–30) as a linear gain: the firmware's DAC
/// code `0xBF − (8 + 4 × (30 − v))` at 0.375 dB per code (01-hardware §9),
/// so −3 dB at 30, 1.5 dB per step, 0 muted.
#[must_use]
pub fn volume_gain(v: u8) -> f32 {
    if v == 0 {
        return 0.0;
    }
    let codes = 8 + 4 * u32::from(30u8.saturating_sub(v.min(30)));
    10f32.powf(-(codes as f32 * 0.375) / 20.0)
}

/// What the decode loop and the engine share.
#[derive(Default)]
struct Shared {
    /// Decoded, processed frames (stereo, interleaved) not yet taken.
    pcm: Vec<f32>,
    /// Samples wanted in `pcm` before the loop yields.
    want: usize,
    /// Output rate (after the chain's decimation).
    rate: u32,
    /// Frames handed out since the start position.
    frames: u64,
    /// Where decoding started (ms into the track).
    start_ms: u32,
    done: bool,
    error: Option<&'static str>,
    sound: Sound,
    sound_gen: u32,
    /// Paused with Fade on pause: the loop fades out, then ends.
    pausing: bool,
    /// Codec and stream, for the developer panel.
    desc: String,
    arena: usize,
}

type Job = Pin<Box<dyn Future<Output = ()>>>;

/// The audio output.
pub struct Engine {
    sh: Rc<RefCell<Shared>>,
    job: Option<Job>,
    /// Path and queue place of the loaded track.
    cur: Option<(String, usize)>,
    /// The page should drop what it has buffered.
    flush: bool,
    sound: Sound,
    sound_gen: u32,
    /// Settings › Sound › Fade on pause.
    fade: bool,
    /// Tracks started and errors seen (developer panel, tests).
    pub started: u32,
    pub last_error: Option<&'static str>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

/// Lead the engine may have over the player's position before a
/// position change counts as a jump (it decodes ahead to fill the page's
/// buffer).
const LEAD_MS: i64 = 4000;
const SLACK_MS: i64 = 700;

impl Engine {
    #[must_use]
    pub fn new() -> Engine {
        Engine {
            sh: Rc::new(RefCell::new(Shared::default())),
            job: None,
            cur: None,
            flush: false,
            sound: Sound::default(),
            sound_gen: 0,
            fade: true,
            started: 0,
            last_error: None,
        }
    }

    /// New Sound settings (applied at the next block).
    pub fn set_sound(&mut self, s: Sound) {
        if s != self.sound {
            self.sound = s;
            self.sound_gen += 1;
            let mut sh = self.sh.borrow_mut();
            sh.sound = s;
            sh.sound_gen = self.sound_gen;
        }
    }

    /// Settings › Sound › Fade on pause (on by default).
    pub fn set_fade_on_pause(&mut self, on: bool) {
        self.fade = on;
    }

    /// The position (ms) of the next frame the engine hands out.
    fn decoded_ms(&self) -> i64 {
        let sh = self.sh.borrow();
        let rate = u64::from(sh.rate.max(1));
        i64::from(sh.start_ms) + i64::try_from(sh.frames * 1000 / rate).unwrap_or(0)
    }

    /// Follow the player. `read` gives a file's bytes.
    pub fn sync(&mut self, now: Option<NowAudio>, read: impl FnOnce(&str) -> Vec<u8>) {
        let Some(now) = now.filter(|n| n.playing) else {
            if self.job.is_some() && !self.sh.borrow().pausing {
                // Paused or stopped: silence now, or after a 60 ms fade
                // (the loop ends then).
                self.flush = true;
                if self.fade {
                    let mut sh = self.sh.borrow_mut();
                    sh.pausing = true;
                    sh.pcm.clear();
                } else {
                    self.job = None;
                }
            }
            self.cur = None;
            return;
        };
        let key = (now.path.clone(), now.index);
        let same = self.cur.as_ref() == Some(&key);
        if same && self.job.is_some() {
            let d = i64::from(now.position_ms) - self.decoded_ms();
            let done = self.sh.borrow().done;
            // Within the lead, or past the end of a finished track.
            if (-LEAD_MS - SLACK_MS..=SLACK_MS).contains(&d) || (done && d > SLACK_MS) {
                return;
            }
        }
        // Another track, a resume, or a jump: start decoding there. A
        // track that finished plays out what the page still has.
        let finished = self.job.is_none() || self.sh.borrow().done;
        if !finished || same {
            self.flush = true;
        }
        let data = read(&now.path);
        self.start(data, now.path.clone(), now.position_ms);
        self.cur = Some(key);
    }

    fn start(&mut self, data: Vec<u8>, path: String, start_ms: u32) {
        let sh = Rc::new(RefCell::new(Shared {
            start_ms,
            sound: self.sound,
            sound_gen: self.sound_gen,
            ..Shared::default()
        }));
        self.sh = Rc::clone(&sh);
        self.started += 1;
        self.job = Some(Box::pin(async move {
            let r = decode(&data, &path, start_ms, &sh).await;
            let mut s = sh.borrow_mut();
            s.done = true;
            if let Err(e) = r {
                s.error = Some(e);
            }
        }));
        // Open and seek now: the rate is known before the first pull.
        let _ = self.pull(0);
    }

    /// Up to `max_frames` frames of stereo PCM at [`Engine::rate`].
    pub fn pull(&mut self, max_frames: usize) -> Vec<f32> {
        let Some(job) = self.job.as_mut() else { return Vec::new() };
        self.sh.borrow_mut().want = max_frames * 2;
        if !self.sh.borrow().done {
            let mut cx = Context::from_waker(Waker::noop());
            // The loop yields once `want` samples are ready, or ends.
            let _ = job.as_mut().poll(&mut cx);
        }
        let mut sh = self.sh.borrow_mut();
        if let Some(e) = sh.error.take() {
            self.last_error = Some(e);
        }
        let n = sh.pcm.len().min(max_frames * 2) & !1;
        let out: Vec<f32> = sh.pcm.drain(..n).collect();
        sh.frames += (n / 2) as u64;
        out
    }

    /// The output rate of what [`Engine::pull`] returns (0: nothing loaded).
    #[must_use]
    pub fn rate(&self) -> u32 {
        self.sh.borrow().rate
    }

    /// Whether the page should drop its buffer (cleared by reading).
    pub fn take_flush(&mut self) -> bool {
        std::mem::take(&mut self.flush)
    }

    /// Codec, rate, arena bytes and position, for the developer panel.
    #[must_use]
    pub fn describe(&self) -> (String, usize, i64) {
        let sh = self.sh.borrow();
        (sh.desc.clone(), sh.arena, self.decoded_ms())
    }

    /// Whether a track is being decoded.
    #[must_use]
    pub fn active(&self) -> bool {
        self.job.is_some() && !self.sh.borrow().done
    }
}

/// Yield to the engine once.
#[derive(Default)]
struct Yield(bool);

impl Future for Yield {
    type Output = ();
    fn poll(mut self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<()> {
        if self.0 {
            Poll::Ready(())
        } else {
            self.0 = true;
            Poll::Pending
        }
    }
}

/// Probe (skipping leading tags), inspect, open, set up the chain, seek,
/// then decode block by block into `sh.pcm`, yielding when enough is ready.
async fn decode(
    data: &[u8],
    path: &str,
    start_ms: u32,
    sh: &Rc<RefCell<Shared>>,
) -> Result<(), &'static str> {
    if let Some(kar) = orpheus_midi::extension(path) {
        return decode_midi(data, kar, start_ms, sh).await;
    }
    let ext = path.rsplit_once('.').map(|(_, e)| e.as_bytes());
    let mut src = SliceSource::new(data);
    let mut format = None;
    for _ in 0..1000 {
        match orpheus_audio::probe(&src, ext) {
            Probe::Found(f) => {
                format = Some(f);
                break;
            }
            Probe::Skip(n) => {
                let to = src.position().saturating_add(n);
                src.seek(to).map_err(|_| "bad tag")?;
                src.refill();
            }
            Probe::NeedData(_) if !src.at_end() => src.refill(),
            Probe::NeedData(_) | Probe::Unknown => break,
        }
    }
    let format = format.ok_or("not a sound file")?;
    let hint = ContainerInfo { format, ..ContainerInfo::default() };
    let plan = loop {
        match orpheus_codecs::inspect(format, &src, &hint) {
            Ok(p) => break p,
            Err(orpheus_audio::OpenError::NeedData(_)) if !src.at_end() => src.refill(),
            Err(_) => return Err("format not supported"),
        }
    };
    let mut arena = vec![0u8; plan.arena];
    let (mut dec, info) =
        orpheus_codecs::open(format, &mut src, &mut arena, &hint).map_err(|_| "decoder did not open")?;
    if !(1..=2).contains(&info.channels) {
        return Err("more than two channels");
    }
    let mut chain = Box::new(Chain::new(OutWidth::Bits16, false));
    let rate = chain
        .set_format(orpheus_dsp::Format {
            rate: info.rate,
            channels: info.channels,
            kind: info.sample,
            bits: info.bits,
        })
        .map_err(|_| "sample rate not supported")?;
    let mut sgen = {
        let mut s = sh.borrow_mut();
        s.rate = rate;
        s.desc = format!("{:?} {} Hz {} ch", info.codec, info.rate, info.channels);
        s.arena = plan.arena;
        s.sound.apply(&mut chain);
        s.sound_gen
    };
    if start_ms > 0 {
        let frame = u64::from(start_ms) * u64::from(info.rate) / 1000;
        for _ in 0..10_000 {
            match dec.seek(&mut src, frame) {
                Ok(SeekStep::Done(at)) => {
                    let mut s = sh.borrow_mut();
                    s.start_ms = u32::try_from(at * 1000 / u64::from(info.rate.max(1))).unwrap_or(start_ms);
                    break;
                }
                Ok(SeekStep::NeedData(_)) if !src.at_end() => src.refill(),
                _ => return Err("seek failed"),
            }
        }
    }
    let ch = usize::from(info.channels);
    let mut corrupt = 0;
    let mut fading = false;
    loop {
        while {
            let s = sh.borrow();
            s.pcm.len() >= s.want
        } {
            Yield::default().await;
        }
        {
            let s = sh.borrow();
            if s.sound_gen != sgen {
                sgen = s.sound_gen;
                s.sound.apply(&mut chain);
            }
            if s.pausing && !fading {
                fading = chain.begin_pause(true);
            }
        }
        if fading && chain.fade_done() {
            return Ok(());
        }
        match dec.decode(&mut src) {
            Ok(Step::Pcm(mut b)) => {
                corrupt = 0;
                let n = chain.process(&mut b);
                let mut s = sh.borrow_mut();
                push(&mut s.pcm, &b, n, ch);
            }
            Ok(Step::NeedData(_)) if !src.at_end() => src.refill(),
            Ok(Step::NeedData(_) | Step::EndOfStream) => return Ok(()),
            Err(DecodeError::Corrupt) => {
                corrupt += 1;
                if corrupt >= 10 {
                    return Err("too many corrupt frames");
                }
            }
            Err(_) => return Err("decode error"),
        }
    }
}

/// The synthesiser's output rate for MIDI (the player's DAC rate).
const MIDI_RATE: u32 = 48_000;

/// MIDI files (`.mid`, `.kar`; MIDI-1): rendered by the firmware's player
/// and synthesiser (`orpheus-midi`, `orpheus-synth`) at 48 kHz, then the
/// same DSP chain as decoded tracks.
async fn decode_midi(
    data: &[u8],
    kar: bool,
    start_ms: u32,
    sh: &Rc<RefCell<Shared>>,
) -> Result<(), &'static str> {
    use orpheus_midi::{MidiPlayer, Options, SliceSource};
    let mut src = SliceSource::new(data);
    let mut player = Box::new(MidiPlayer::new(Options { kar, ..Options::new(MIDI_RATE) }));
    player.open_from(&mut src, kar).map_err(|_| "not a MIDI file")?;
    let mut chain = Box::new(Chain::new(OutWidth::Bits16, false));
    let rate = chain
        .set_format(orpheus_dsp::Format {
            rate: MIDI_RATE,
            channels: 2,
            kind: orpheus_audio::SampleKind::I16,
            bits: 16,
        })
        .map_err(|_| "sample rate not supported")?;
    let mut sgen = {
        let mut s = sh.borrow_mut();
        s.rate = rate;
        s.desc = format!("MIDI {MIDI_RATE} Hz 2 ch");
        s.arena = core::mem::size_of::<MidiPlayer>();
        s.sound.apply(&mut chain);
        s.sound_gen
    };
    if start_ms > 0 {
        let at = player
            .seek_from(&mut src, u64::from(start_ms) * u64::from(MIDI_RATE) / 1000)
            .map_err(|_| "seek failed")?;
        sh.borrow_mut().start_ms = u32::try_from(at * 1000 / u64::from(MIDI_RATE)).unwrap_or(start_ms);
    }
    let mut buf = vec![0i16; 2 * 1152];
    let mut fading = false;
    loop {
        while {
            let s = sh.borrow();
            s.pcm.len() >= s.want
        } {
            Yield::default().await;
        }
        {
            let s = sh.borrow();
            if s.sound_gen != sgen {
                sgen = s.sound_gen;
                s.sound.apply(&mut chain);
            }
            if s.pausing && !fading {
                fading = chain.begin_pause(true);
            }
        }
        if fading && chain.fade_done() {
            return Ok(());
        }
        let frames = player.render_from(&mut src, &mut buf).map_err(|_| "MIDI error")?;
        if frames == 0 {
            return Ok(());
        }
        let mut b = PcmBlock::I16(&mut buf[..frames * 2]);
        let n = chain.process(&mut b);
        push(&mut sh.borrow_mut().pcm, &b, n, 2);
    }
}

/// Append the first `n` samples of a processed block as stereo `f32`.
fn push(out: &mut Vec<f32>, b: &PcmBlock<'_>, n: usize, ch: usize) {
    let mut put = |it: &mut dyn Iterator<Item = f32>| {
        if ch == 2 {
            out.extend(it);
        } else {
            for x in it {
                out.push(x);
                out.push(x);
            }
        }
    };
    match b {
        PcmBlock::I16(s) => put(&mut s[..n.min(s.len())].iter().map(|&x| f32::from(x) / 32768.0)),
        PcmBlock::I32(s) => put(&mut s[..n.min(s.len())].iter().map(|&x| x as f32 / 2_147_483_648.0)),
    }
}

/// A track's length in milliseconds: from the stream's frame count when
/// the container states it, else by decoding it (results are kept).
#[must_use]
pub fn duration_ms(data: &[u8], path: &str) -> Option<u32> {
    thread_local! {
        static KNOWN: RefCell<std::collections::HashMap<(String, usize), Option<u32>>> = RefCell::default();
    }
    let key = (path.to_string(), data.len());
    if let Some(d) = KNOWN.with(|k| k.borrow().get(&key).copied()) {
        return d;
    }
    let d = measure(data, path);
    KNOWN.with(|k| k.borrow_mut().insert(key, d));
    d
}

fn measure(data: &[u8], path: &str) -> Option<u32> {
    let mut e = Engine::new();
    e.start(data.to_vec(), path.to_string(), 0);
    // Run the loop to its first block: it states the stream.
    let mut frames = 0u64;
    let mut rate = 0;
    loop {
        let got = e.pull(1 << 16);
        rate = rate.max(e.rate());
        frames += (got.len() / 2) as u64;
        if got.is_empty() && !e.active() {
            break;
        }
    }
    if e.last_error.is_some() && frames == 0 {
        return None;
    }
    u32::try_from(frames * 1000 / u64::from(rate.max(1))).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::demo;

    /// A demo file's bytes.
    fn file(path: &str) -> Vec<u8> {
        demo::committed().into_iter().find(|f| f.path == path).unwrap_or_else(|| panic!("{path}")).data
    }

    #[test]
    fn volume_curve() {
        assert_eq!(volume_gain(0), 0.0);
        assert!((20.0 * volume_gain(30).log10() + 3.0).abs() < 0.01);
        assert!((20.0 * (volume_gain(29) / volume_gain(30)).log10() + 1.5).abs() < 0.01);
    }

    /// Every audio file of the demo set decodes through the firmware's
    /// codecs to the length its excerpt was cut to (`sources.toml`), and
    /// the MIDI file plays through the synthesiser.
    #[test]
    fn every_demo_track_decodes_to_its_length() {
        let want = [
            (demo::ARIA, 30_000),
            (demo::VARIATION, 10_000),
            ("/Music/Beethoven/Symphony No. 5/1 Allegro con brio.ogg", 26_000),
            ("/Music/Caruso/Vesti la giubba (1907).opus", 64_000),
            ("/Music/Chopin/Nocturne Op. 9 No. 2.aiff", 8_000),
            ("/Music/Satie/Gymnopédie No. 1.wav", 10_000),
            ("/Music/Sousa/The Stars and Stripes Forever (1909).au", 20_000),
            ("/Music/Bulfinch/The Age of Fable/24 Orpheus and Eurydice.spx", 50_000),
        ];
        let audio: Vec<_> = demo::committed()
            .into_iter()
            .filter(|f| {
                orpheus_apps::browse::is_audio_name(&f.path) && orpheus_midi::extension(&f.path).is_none()
            })
            .collect();
        assert_eq!(audio.len(), want.len(), "{:?}", audio.iter().map(|f| &f.path).collect::<Vec<_>>());
        for (path, ms) in want {
            let d = duration_ms(&file(path), path).unwrap_or_else(|| panic!("{path} does not decode"));
            assert!(d.abs_diff(ms) < 150, "{path}: {d} ms, want {ms}");
        }
        let midi = file(demo::PRELUDE_MIDI);
        let mut e = Engine::new();
        e.sync(now(demo::PRELUDE_MIDI, 0, true), |_| midi.clone());
        let a = e.pull(48_000);
        assert_eq!(e.rate(), 48_000);
        assert_eq!(a.len(), 96_000, "{:?}", e.last_error);
        assert!(a.iter().any(|x| x.abs() > 0.01), "the prelude is not silent");
        // A seek restarts the synthesiser there.
        e.sync(now(demo::PRELUDE_MIDI, 30_000, true), |_| midi.clone());
        assert!(e.take_flush());
        let _ = e.pull(100);
        assert!((29_000..31_000).contains(&e.describe().2), "{}", e.describe().2);
    }

    fn now(path: &str, pos: u32, playing: bool) -> Option<NowAudio> {
        Some(NowAudio { path: path.into(), index: 0, position_ms: pos, duration_ms: 30_000, playing })
    }

    #[test]
    fn follows_the_player() {
        let path = demo::ARIA;
        let data = file(path);
        let mut e = Engine::new();
        e.sync(now(path, 5000, true), |_| data.clone());
        let a = e.pull(44_100);
        assert_eq!(e.rate(), 44_100);
        assert_eq!(a.len(), 88_200);
        assert!(a.iter().any(|x| x.abs() > 0.01));
        assert!(!e.take_flush());
        // Small drift: nothing happens.
        e.sync(now(path, 6150, true), |_| unreachable!());
        // Seek ahead: restart, flush.
        e.sync(now(path, 20_000, true), |_| data.clone());
        assert!(e.take_flush());
        let _ = e.pull(100);
        let (_, _, ms) = e.describe();
        assert!((19_500..20_500).contains(&ms), "{ms}");
        // Pause (Fade on pause off; `fade_on_pause` has it on): silence.
        e.set_fade_on_pause(false);
        e.sync(now(path, 20_000, false), |_| unreachable!());
        assert!(e.take_flush());
        assert!(e.pull(100).is_empty());
        assert_eq!(e.started, 2);
    }

    /// VER-13 (07 §5.12): the Sound settings' effect on the rendered
    /// sound, through `Sound::from_settings` (the settings → chain mapping
    /// the firmware's UI task also makes) and the real `orpheus-dsp` chain,
    /// one setting changed at a time against the defaults; and no other
    /// setting of the tree changes what the chain is given.
    mod settings_effects {
        use super::super::*;
        use orpheus_apps::settings::{self, Kind, SettingId, SettingsCache};

        /// Settings with one value changed.
        fn with(id: u16, v: i16) -> SettingsCache {
            let mut c = SettingsCache::new();
            c.set(SettingId(id), v);
            c
        }

        /// 4,410 stereo frames through a chain set from `c`: left 100 Hz
        /// plus 6 kHz, right 1 kHz (different channels so that balance,
        /// mono, swap and crossfeed show), ReplayGain tags on the track
        /// (−6 dB track, +3 dB album).
        fn render(c: &SettingsCache) -> Vec<i16> {
            let s = Sound::from_settings(|k| c.get(SettingId(k)));
            let mut chain = Chain::new(OutWidth::Bits16, false);
            chain
                .set_format(orpheus_dsp::Format {
                    rate: 44_100,
                    channels: 2,
                    kind: orpheus_audio::SampleKind::I16,
                    bits: 16,
                })
                .unwrap();
            s.apply(&mut chain);
            chain.set_track_gains(orpheus_dsp::RgTags {
                track_gain_mb: Some(-600),
                track_peak_ppm: None,
                album_gain_mb: Some(300),
                album_peak_ppm: None,
            });
            let mut pcm: Vec<i16> = (0..4410)
                .flat_map(|n| {
                    let t = n as f32 / 44_100.0;
                    let tau = core::f32::consts::TAU;
                    let l = 6000.0 * (tau * 100.0 * t).sin() + 3000.0 * (tau * 6000.0 * t).sin();
                    let r = 8000.0 * (tau * 1000.0 * t).sin();
                    [l as i16, r as i16]
                })
                .collect();
            let n = chain.process_i16(&mut pcm);
            pcm.truncate(n);
            pcm
        }

        fn default_sound() -> Vec<i16> {
            render(&SettingsCache::new())
        }

        /// Every non-default value of `id` changes the sound.
        fn changes_the_sound(id: u16, values: &[i16]) {
            let base = default_sound();
            for &v in values {
                assert_ne!(render(&with(id, v)), base, "{id:#06x}={v} does not change the sound");
            }
        }

        #[test]
        fn equaliser() {
            // Every preset but Flat and the two custom curves (flat until
            // their bands are set) changes the sound; a custom curve with
            // a band set does.
            changes_the_sound(0x0101, &[1, 2, 3, 4, 5, 6, 7, 8]);
            for (preset, band0) in [(9i16, 0x0D10u16), (10, 0x0D15)] {
                let mut c = with(0x0101, preset);
                assert_eq!(render(&c), default_sound(), "custom curve {preset} at 0 dB is flat");
                c.set(SettingId(band0), 6);
                assert_ne!(render(&c), default_sound(), "custom curve {preset} with a band raised");
            }
        }

        /// The custom curves' bands act only through their preset.
        #[test]
        fn custom_bands() {
            for band in 0x0D10..=0x0D19u16 {
                assert_eq!(render(&with(band, 6)), default_sound(), "{band:#06x} without its preset");
                let preset = if band < 0x0D15 { 9 } else { 10 };
                let mut c = with(0x0101, preset);
                let flat = render(&c);
                c.set(SettingId(band), 6);
                assert_ne!(render(&c), flat, "{band:#06x} with its preset");
                c.set(SettingId(band), -12);
                assert_ne!(render(&c), flat, "{band:#06x} at -12 dB");
            }
        }

        #[test]
        fn balance() {
            changes_the_sound(0x0102, &[-10, -3, 5, 10]);
            // Left: the right channel is quieter, and the other way round.
            let peak = |p: &[i16], ch: usize| p.iter().skip(ch).step_by(2).map(|x| x.unsigned_abs()).max();
            let l = render(&with(0x0102, -10));
            let r = render(&with(0x0102, 10));
            assert!(peak(&l, 1) < peak(&default_sound(), 1) && peak(&r, 0) < peak(&default_sound(), 0));
        }

        #[test]
        fn bass() {
            changes_the_sound(0x0103, &[-6, -1, 3, 6]);
        }

        #[test]
        fn crossfeed() {
            changes_the_sound(0x0104, &[1, 2, 3]);
            assert_ne!(render(&with(0x0104, 1)), render(&with(0x0104, 3)), "levels differ");
        }

        #[test]
        fn channel_mode() {
            changes_the_sound(0x0105, &[1, 2]);
            // After the matrix's ramp from stereo (the second half), within
            // the dither's step (it runs per channel).
            let near = |a: i16, b: i16| (i32::from(a) - i32::from(b)).abs() <= 2;
            let late = |p: Vec<i16>| p[p.len() / 2..].to_vec();
            let mono = late(render(&with(0x0105, 1)));
            assert!(mono.chunks(2).all(|p| near(p[0], p[1])), "mono");
            let base = late(default_sound());
            let swap = late(render(&with(0x0105, 2)));
            assert!(
                swap.chunks(2).zip(base.chunks(2)).all(|(s, b)| near(s[0], b[1]) && near(s[1], b[0])),
                "swap"
            );
        }

        /// Track −6 dB, album +3 dB. "Auto" is the album gain here (the
        /// mapping has no shuffle input: `2 | 3 => Album`).
        #[test]
        fn replaygain() {
            changes_the_sound(0x0106, &[1, 2, 3]);
            assert_ne!(render(&with(0x0106, 1)), render(&with(0x0106, 2)), "track and album gains differ");
            assert_eq!(render(&with(0x0106, 3)), render(&with(0x0106, 2)), "auto = album (no shuffle input)");
        }

        /// No other setting of the tree reaches the chain: each at a value
        /// other than its default gives the default sound settings (the
        /// "absent elsewhere" half for the audio context).
        #[test]
        fn nothing_else_reaches_the_chain() {
            let sound: [u16; 16] = [
                0x0101, 0x0102, 0x0103, 0x0104, 0x0105, 0x0106, 0x0D10, 0x0D11, 0x0D12, 0x0D13, 0x0D14,
                0x0D15, 0x0D16, 0x0D17, 0x0D18, 0x0D19,
            ];
            let base = Sound::from_settings(|k| SettingsCache::new().get(SettingId(k)));
            for d in settings::all() {
                if sound.contains(&d.id.0) || !d.stored() && d.pack.is_none() {
                    continue;
                }
                let v = match d.kind {
                    Kind::Toggle => 1 - d.default,
                    Kind::Choice(c) => (d.default + 1) % i16::try_from(c.len()).unwrap(),
                    Kind::Range { min, max, .. } => {
                        if d.default == max {
                            min
                        } else {
                            max
                        }
                    }
                    Kind::Language => settings::lang_code("ru"),
                    Kind::HomeApps => 1,
                    Kind::Action(_) | Kind::Info(_) => continue,
                };
                let c = with(d.id.0, v);
                assert_eq!(
                    Sound::from_settings(|k| c.get(SettingId(k))),
                    base,
                    "{:#06x} reaches the chain",
                    d.id.0
                );
            }
        }
    }

    /// Settings › Sound › Fade on pause (VER-13): on, a pause gives 60 ms
    /// more of the track fading to silence (the page drops what it had);
    /// off, nothing more.
    #[test]
    fn fade_on_pause() {
        let path = demo::VARIATION;
        let data = file(path);
        let tail = |fade: bool| {
            let mut e = Engine::new();
            e.set_fade_on_pause(fade);
            // From 0:05 (44.1 kHz, the variation at full tilt).
            e.sync(now(path, 5000, true), |_| data.clone());
            let _ = e.pull(22_050);
            e.sync(now(path, 5500, false), |_| unreachable!());
            assert!(e.take_flush());
            let mut out = Vec::new();
            for _ in 0..100 {
                let p = e.pull(1024);
                if p.is_empty() {
                    break;
                }
                out.extend(p);
            }
            out
        };
        assert!(tail(false).is_empty(), "off: silence at once");
        let on = tail(true);
        let frames = on.len() / 2;
        let fade = 44_100 * orpheus_dsp::PAUSE_FADE_MS as usize / 1000;
        assert!((fade..fade + 4096).contains(&frames), "{frames} frames");
        let peak = |s: &[f32]| s.iter().fold(0f32, |m, x| m.max(x.abs()));
        assert!(peak(&on[..441]) > 0.05, "starts at the music's level");
        assert!(peak(&on[2 * (fade - 50)..2 * fade]) < 0.01, "silent at 60 ms");
    }

    #[test]
    fn eq_changes_the_output() {
        let path = demo::VARIATION;
        let data = file(path);
        let run = |s: Sound| {
            let mut e = Engine::new();
            e.set_sound(s);
            e.sync(now(path, 0, true), |_| data.clone());
            e.pull(22_050)
        };
        let flat = run(Sound::default());
        let bass = run(Sound { bass: 6, ..Sound::default() });
        assert_ne!(flat, bass);
        let mono = run(Sound { channels: 1, ..Sound::default() });
        // Mono: both channels the same, up to the output's TPDF dither
        // (independent per channel, ±1 LSB), once the channel matrix has
        // ramped from stereo (`orpheus_dsp::matrix::MATRIX_RAMP_MS`, 30 ms).
        let settled = 2 * 44_100 * 40 / 1000;
        let worst = mono[settled..].chunks(2).map(|p| (p[0] - p[1]).abs()).fold(0f32, f32::max);
        assert!(worst <= 2.0 / 32768.0, "{worst}");
        let stereo_apart = flat.chunks(2).filter(|p| (p[0] - p[1]).abs() > 0.01).count();
        assert!(stereo_apart > 100, "the stereo source differs between channels ({stereo_apart})");
    }
}
