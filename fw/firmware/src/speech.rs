//! The speech voice (TTS-3; 04 §8 "Output", 05 §7.14 "For the
//! firmware"): the screen reader's [`SpeechService`] on the player.
//!
//! ```text
//! UI task ── say / earcon / stop ──► utterance queue (1 KiB)
//!   speech task (thread mode): orpheus-tts Speaker at 16 kHz, earcons
//!     ──► sample ring (16 kHz mono, 128 ms) ──► card task (audio/speech.rs):
//!          over music: resampled, mixed after the DSP chain, the music lowered
//!          alone: the DAC at 16 kHz
//! ```
//!
//! * The speech is rendered in thread mode, below the audio task: a speech
//!   that cannot keep up (Battery saver, a long draw) leaves gaps, never an
//!   underrun of the music or a hang. It renders a batch of [`BATCH`]
//!   samples straight into the ring, then yields and wakes the card task
//!   once, and renders again once [`REFILL`] samples of the ring are free:
//!   about sixteen wake-ups a second while speaking. Its per-sample and
//!   per-tick code is the `speech-render` cluster (`hot.toml`), so a batch
//!   reloads at most those lines (TTS-3b).
//! * A stop ([`SpeechService::stop`], any key) drops the queue and marks the
//!   ring's head ([`FLUSH_TO`], then [`GEN`]): the card task fades what it
//!   still has of the stopped speech over 3 ms and drops the rest (alone,
//!   it also takes back what it had queued for the DAC); the speech task
//!   silences the speaker. What follows the mark is the next utterance.
//! * While speech sounds the clock floor is Mid (`orpheus_power::governor::
//!   with_speech`): raised at once in Automatic, never in Battery saver.
//! * `ui speech` (developer channel) reports the last utterance, the
//!   queue, whether it sounds, the gains and the counters ([`status_reply`]).

use core::cell::RefCell;
use core::sync::atomic::{AtomicBool, AtomicI8, AtomicU8, AtomicU32, Ordering};

use critical_section::Mutex;
use embassy_futures::select::select;
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::signal::Signal;
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::Timer;
use orpheus_tts::mix::{self, Gains, MusicMode};
use orpheus_tts::{Config as TtsConfig, Options, Speaker, Speed, Stop, Voice};
use orpheus_ui::a11y::{Config, Earcon, SpeechService, Utterance, UtteranceQueue};
use orpheus_ui::text::StrBuf;

/// The sample ring (16 kHz): 128 ms.
pub const RING: usize = 2048;
/// The unit a batch is counted in (16 ms of speech): a step renders only
/// when this much of the ring is free.
pub const CHUNK: usize = 256;
/// Samples one step renders before it yields (64 ms of speech; ≈ 4 ms of
/// the CPU at Mid, the code in the cache). TTS-3b, 2026-11-28: one chunk
/// a step, with a wake-up of the card task after each, was 62 wake-ups a
/// second; each reloaded the render's lines that the card task's, the UI's
/// and the input tick's code had evicted in between (6,400 line loads/s
/// while speaking alone, more than real time: the owner's "choppy").
pub const BATCH: usize = 1024;
/// The speech task renders again once this much of the ring is free
/// (64 ms): one batch.
pub const REFILL: usize = RING / 2;
const _: () = assert!(BATCH % CHUNK == 0 && BATCH <= RING && REFILL >= CHUNK);

/// The ring's samples: written by the speech task at `HEAD`, read by the
/// card task at its tail (`RingSource`).
static mut SAMPLES: [i16; RING] = [0; RING];
/// Samples written (free-running).
static HEAD: AtomicU32 = AtomicU32::new(0);
/// Samples taken by the card task (free-running).
static TAIL: AtomicU32 = AtomicU32::new(0);
/// Stops so far, and the head at the last one.
static GEN: AtomicU32 = AtomicU32::new(0);
static FLUSH_TO: AtomicU32 = AtomicU32::new(0);
/// Speech is queued or being rendered (set by `say`, cleared by the speech
/// task when nothing is left).
static SOUNDING: AtomicBool = AtomicBool::new(false);
/// The settings the speech task renders the next utterance with, and the
/// card task's gains: rate (quarters), voice, pitch, volume | music << 4.
static RATE_Q: AtomicU8 = AtomicU8::new(4);
static VOICE: AtomicU8 = AtomicU8::new(0);
static PITCH: AtomicI8 = AtomicI8::new(0);
static GAINS: AtomicU8 = AtomicU8::new(0);

/// The card task is woken (speech to take, a stop) and wakes the speech
/// task when room comes free.
pub(crate) static RT_WAKER: AtomicWaker = AtomicWaker::new();
pub(crate) static RT_WANTED: AtomicBool = AtomicBool::new(false);
static ROOM_ASKED: AtomicBool = AtomicBool::new(false);
static ROOM: Signal<CriticalSectionRawMutex, ()> = Signal::new();
/// Work for the speech task (an utterance, a stop).
static WAKE: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// The utterances between the UI task and the speech task (both in thread
/// mode; the lock is for the type system).
static QUEUE: Mutex<RefCell<UtteranceQueue<1024>>> = Mutex::new(RefCell::new(UtteranceQueue::new()));
/// The last text said (`ui speech`), cut at 64 bytes.
static LAST: Mutex<RefCell<StrBuf<64>>> = Mutex::new(RefCell::new(StrBuf::new()));

/// Counters for `ui speech`: texts, earcons, stops, utterances dropped
/// (the queue full), samples rendered, frames the card task padded with
/// silence while speech was late (alone).
static TEXTS: AtomicU32 = AtomicU32::new(0);
static EARCONS: AtomicU32 = AtomicU32::new(0);
static STOPS: AtomicU32 = AtomicU32::new(0);
static RENDERED: AtomicU32 = AtomicU32::new(0);
pub(crate) static LATE: AtomicU32 = AtomicU32::new(0);
/// Speech goes out alone (the DAC at 16 kHz; set by the card task).
pub(crate) static ALONE: AtomicBool = AtomicBool::new(false);
/// The music's gain under the speech now (Q16; set by the card task).
pub(crate) static DUCK: AtomicU32 = AtomicU32::new(mix::Q16_ONE);

/// Speech sounds or waits: the clock floor (`main.rs` governor), Music
/// while speaking › Paused (the UI task), the card task's mixing.
#[inline]
pub fn speaking() -> bool {
    SOUNDING.load(Ordering::Relaxed) || HEAD.load(Ordering::Relaxed) != TAIL.load(Ordering::Relaxed)
}

/// The card task's gains (Settings › Accessibility › Speech volume, Music
/// while speaking).
pub(crate) fn gains() -> Gains {
    let g = GAINS.load(Ordering::Relaxed);
    Gains::of(g & 0xF, MusicMode::from_index(g >> 4))
}

/// The ring's reading side (the card task's only; `audio/speech.rs`).
pub(crate) struct RingSource {
    tail: u32,
    seen: u32,
    /// The head at the stop whose speech is still being faded.
    flush_to: Option<u32>,
}

impl RingSource {
    pub(crate) const fn new() -> RingSource {
        RingSource { tail: 0, seen: 0, flush_to: None }
    }

    /// Samples ready.
    pub(crate) fn ready(&self) -> u32 {
        HEAD.load(Ordering::Acquire).wrapping_sub(self.tail)
    }

    fn publish(&self) {
        TAIL.store(self.tail, Ordering::Release);
        // Room for the speech task: woken once enough is free.
        let free = RING as u32 - HEAD.load(Ordering::Relaxed).wrapping_sub(self.tail).min(RING as u32);
        if free as usize >= REFILL && ROOM_ASKED.swap(false, Ordering::AcqRel) {
            ROOM.signal(());
        }
    }
}

impl mix::Source for RingSource {
    fn next(&mut self) -> Option<i16> {
        let end = self.flush_to.unwrap_or_else(|| HEAD.load(Ordering::Acquire));
        if self.tail == end {
            return None;
        }
        // SAFETY: the speech task writes only at and after `HEAD`, which it
        // publishes after the samples (Release); this reads below it.
        let v = unsafe {
            core::ptr::read_volatile((&raw const SAMPLES).cast::<i16>().add(self.tail as usize % RING))
        };
        self.tail = self.tail.wrapping_add(1);
        if self.tail.wrapping_sub(end) == 0 || self.tail % 64 == 0 {
            self.publish();
        }
        Some(v)
    }

    fn sounding(&self) -> bool {
        SOUNDING.load(Ordering::Relaxed) || self.ready() != 0
    }

    fn stopped(&mut self) -> bool {
        let g = GEN.load(Ordering::Acquire);
        if g == self.seen {
            return false;
        }
        self.seen = g;
        self.flush_to = Some(FLUSH_TO.load(Ordering::Relaxed));
        true
    }

    fn drop_stopped(&mut self) {
        if let Some(f) = self.flush_to.take() {
            // Never back: the mark is at or after what was taken.
            if f.wrapping_sub(self.tail) as i32 > 0 {
                self.tail = f;
            }
        }
        self.publish();
    }
}

/// What the UI task's reader speaks through (`ui.rs`).
pub struct FwSpeech;

impl SpeechService for FwSpeech {
    fn stop(&mut self) {
        let busy = speaking();
        critical_section::with(|cs| QUEUE.borrow(cs).borrow_mut().clear());
        FLUSH_TO.store(HEAD.load(Ordering::Relaxed), Ordering::Relaxed);
        GEN.fetch_add(1, Ordering::Release);
        if busy {
            STOPS.fetch_add(1, Ordering::Relaxed);
        }
        wake_rt();
        WAKE.signal(());
    }

    fn say(&mut self, text: &str) {
        critical_section::with(|cs| {
            QUEUE.borrow(cs).borrow_mut().push_text(text);
            let mut l = LAST.borrow(cs).borrow_mut();
            l.clear();
            l.push_str(text);
        });
        TEXTS.fetch_add(1, Ordering::Relaxed);
        started();
    }

    fn earcon(&mut self, e: Earcon) {
        critical_section::with(|cs| QUEUE.borrow(cs).borrow_mut().push_earcon(e));
        EARCONS.fetch_add(1, Ordering::Relaxed);
        started();
    }

    fn configure(&mut self, cfg: &Config) {
        RATE_Q.store(cfg.rate.clamp(1, 40), Ordering::Relaxed);
        VOICE.store(cfg.voice, Ordering::Relaxed);
        PITCH.store(cfg.pitch, Ordering::Relaxed);
        GAINS.store((cfg.volume & 0xF) | (cfg.music.min(15) << 4), Ordering::Relaxed);
    }
}

/// Something was queued: the speech task works, the clock goes to the
/// speech floor (Automatic only).
fn started() {
    SOUNDING.store(true, Ordering::Relaxed);
    crate::raise_to_floor(orpheus_power::governor::SPEECH_FLOOR);
    WAKE.signal(());
}

fn wake_rt() {
    RT_WANTED.store(true, Ordering::Release);
    RT_WAKER.wake();
}

/// Queue depth in bytes and whether it is empty.
fn queued() -> (bool, u32) {
    critical_section::with(|cs| {
        let q = QUEUE.borrow(cs).borrow();
        (q.is_empty(), q.dropped)
    })
}

/// Developer `ui speech`: `speech sounding=0|1 alone=0|1 queued=0|1
/// ring=N vol=V music=M duck=Q16 texts=N earcons=N stops=N dropped=N
/// late=N rendered=N said=TEXT` (the text last, cut at 64 bytes).
pub fn status_reply() {
    use Ordering::Relaxed;
    let (empty, dropped) = queued();
    let mut said = StrBuf::<64>::new();
    critical_section::with(|cs| said.push_str(LAST.borrow(cs).borrow().as_str()));
    let g = GAINS.load(Relaxed);
    let ring = HEAD.load(Relaxed).wrapping_sub(TAIL.load(Relaxed));
    crate::audio::reply(|r| {
        r.s(b"speech sounding=")
            .u(u64::from(speaking()))
            .s(b" alone=")
            .u(u64::from(ALONE.load(Relaxed)))
            .s(b" queued=")
            .u(u64::from(!empty))
            .s(b" ring=")
            .u(u64::from(ring))
            .s(b" vol=")
            .u(u64::from(g & 0xF))
            .s(b" music=")
            .u(u64::from(g >> 4))
            .s(b" duck=")
            .u(u64::from(DUCK.load(Relaxed)));
        for (k, v) in [
            (b" texts=".as_slice(), TEXTS.load(Relaxed)),
            (b" earcons=", EARCONS.load(Relaxed)),
            (b" stops=", STOPS.load(Relaxed)),
            (b" dropped=", dropped),
            (b" late=", LATE.load(Relaxed)),
            (b" rendered=", RENDERED.load(Relaxed)),
        ] {
            r.s(k).u(u64::from(v));
        }
        r.s(b" said=").s(said.as_str().as_bytes());
    });
}

// ------------------------------------------------------------ the task

enum Playing {
    Idle,
    Speech,
    Tone { hz: (u16, u16), level: u8, at: u32, total: u32 },
}

struct State {
    playing: Playing,
    seen: u32,
    applied: Option<(u8, u8, i8)>,
}

/// What the task waits for next.
enum Next {
    /// Nothing to do: an utterance or a stop.
    Work,
    /// The ring is full: room (or 50 ms).
    Room,
    /// A chunk was rendered: let the other tasks run.
    Yield,
}

static mut SPEAKER: core::mem::MaybeUninit<Speaker> = core::mem::MaybeUninit::uninit();

fn voice(v: u8, pitch: i8) -> Voice {
    let v = Voice::ALL.get(usize::from(v)).copied().unwrap_or(Voice::NEUTRAL);
    v.with_pitch(f32::from(pitch))
}

/// Once, from `main` before the executors run: the speaker (3.3 KB) built
/// in its static. Its frame (7,640 B: the speaker built, then moved) is in
/// the start-up context, whose worst case (the audio chain's set-up) is
/// deeper; in the speech task it was the thread context's deepest path
/// (PG-2, `xtask stack` with the feature on, 2026-11-27).
#[inline(never)]
pub fn init() {
    let cfg =
        TtsConfig { rate: mix::RATE, channels: 1, voice: voice(0, 0), speed: Speed::NORMAL, seed: 0x5EED };
    // SAFETY: called once, before the speech task (the only user) runs.
    unsafe { (*(&raw mut SPEAKER)).write(Speaker::new(cfg, Options::default())) };
}

/// The speaker [`init`] built.
fn speaker() -> &'static mut Speaker {
    // SAFETY: `init` ran in `main` before the task was spawned; the speech
    // task is the only user and takes it once.
    unsafe { (*(&raw mut SPEAKER)).assume_init_mut() }
}

/// The settings `configure` left, from the next utterance on.
fn apply(sp: &mut Speaker, st: &mut State) {
    let want = (RATE_Q.load(Ordering::Relaxed), VOICE.load(Ordering::Relaxed), PITCH.load(Ordering::Relaxed));
    if st.applied != Some(want) {
        sp.set_speed(Speed::from_quarters(want.0.clamp(1, 40)));
        sp.set_voice(voice(want.1, want.2));
        st.applied = Some(want);
    }
}

/// A stop: the speaker's own fade is not heard (the card task fades what
/// it has); it is rendered away.
#[inline(never)]
fn silence(sp: &mut Speaker) {
    sp.stop(Stop::Now);
    let mut scratch = [0i16; 64];
    for _ in 0..16 {
        if sp.is_idle() {
            break;
        }
        sp.render(&mut scratch, |_, _| {});
    }
}

/// The next utterance from the queue, if any.
#[inline(never)]
fn next_utterance(sp: &mut Speaker, st: &mut State) -> bool {
    let mut text = StrBuf::<255>::new();
    let u = critical_section::with(|cs| QUEUE.borrow(cs).borrow_mut().pop(&mut text));
    match u {
        Some(Utterance::Text) => {
            apply(sp, st);
            let mut rest = text.as_str().as_bytes();
            while !rest.is_empty() {
                let n = sp.feed(rest);
                if n == 0 {
                    break;
                }
                rest = &rest[n..];
            }
            sp.finish();
            st.playing = Playing::Speech;
            true
        }
        Some(Utterance::Earcon(e)) => {
            let t = e.tone();
            let total = u32::from(t.ms) * mix::RATE / 1000;
            st.playing = Playing::Tone { hz: (t.hz[0], t.hz[1]), level: t.level, at: 0, total };
            true
        }
        None => false,
    }
}

/// Up to `out.len()` samples of what plays; fewer at its end (`Idle`
/// then). Out of line: the renderer's sample loop, in `speech-render`
/// (`hot.toml`).
#[inline(never)]
fn render(sp: &mut Speaker, st: &mut State, out: &mut [i16]) -> usize {
    match st.playing {
        Playing::Idle => 0,
        Playing::Speech => {
            let n = sp.render(out, |_, _| {});
            if n < out.len() || sp.is_idle() {
                st.playing = Playing::Idle;
            }
            n
        }
        Playing::Tone { hz, level, at, total } => {
            let n = (out.len() as u32).min(total - at);
            for (k, s) in out.iter_mut().take(n as usize).enumerate() {
                *s = mix::glide(hz, level, at + k as u32, total, mix::RATE);
            }
            let at = at + n;
            st.playing = if at >= total { Playing::Idle } else { Playing::Tone { hz, level, at, total } };
            n as usize
        }
    }
}

/// One step of the speech task (see [`Next`]).
#[inline(never)]
fn step(sp: &mut Speaker, st: &mut State) -> Next {
    let g = GEN.load(Ordering::Acquire);
    if g != st.seen {
        st.seen = g;
        if matches!(st.playing, Playing::Speech) {
            silence(sp);
        }
        st.playing = Playing::Idle;
    }
    if matches!(st.playing, Playing::Idle) && !next_utterance(sp, st) {
        SOUNDING.store(false, Ordering::Relaxed);
        // An utterance queued between the pop and the store.
        if !queued().0 {
            SOUNDING.store(true, Ordering::Relaxed);
            return Next::Yield;
        }
        return Next::Work;
    }
    let head = HEAD.load(Ordering::Relaxed);
    let used = head.wrapping_sub(TAIL.load(Ordering::Acquire)) as usize;
    let room = RING.saturating_sub(used);
    if room < CHUNK {
        ROOM_ASKED.store(true, Ordering::Release);
        return Next::Room;
    }
    // One batch (TTS-3b): up to `BATCH` samples straight into the ring
    // (at most two pieces, at its wrap), then one wake-up of the card task.
    let want = batch_len(room);
    let at = head as usize % RING;
    let first = want.min(RING - at);
    let base = (&raw mut SAMPLES).cast::<i16>();
    // SAFETY: `at..at + first` and `0..want - first` lie inside the ring
    // and are slots from `HEAD` on, `want` ≤ the free room: the speech
    // task's alone. The card task reads only below `HEAD` and not these
    // until `HEAD` moves past them (Release, below).
    let mut n = render(sp, st, unsafe { core::slice::from_raw_parts_mut(base.add(at), first) });
    if n == first && want > first {
        n += render(sp, st, unsafe { core::slice::from_raw_parts_mut(base, want - first) });
    }
    HEAD.store(head.wrapping_add(n as u32), Ordering::Release);
    RENDERED.fetch_add(n as u32, Ordering::Relaxed);
    if n > 0 {
        wake_rt();
    }
    Next::Yield
}

/// How much one step renders with `room` samples of the ring free: all of
/// it up to [`BATCH`], whole chunks (TTS-3b).
#[inline]
const fn batch_len(room: usize) -> usize {
    let n = if room < BATCH { room } else { BATCH };
    n - n % CHUNK
}

/// The speech task (thread mode).
#[embassy_executor::task]
pub async fn speech() {
    let sp = speaker();
    let mut st = State { playing: Playing::Idle, seen: GEN.load(Ordering::Relaxed), applied: None };
    loop {
        match step(sp, &mut st) {
            Next::Work => WAKE.wait().await,
            Next::Room => {
                // Woken by the card task once `REFILL` is free, by a stop,
                // or after 50 ms (a ring nobody reads: the card with USB).
                let _ = select(select(ROOM.wait(), WAKE.wait()), Timer::after_millis(50)).await;
            }
            Next::Yield => embassy_futures::yield_now().await,
        }
    }
}
