//! Recording, a mode of the card task (WP AU-13 / UI-5b; 04 §5.2,
//! 05 §7.5).
//!
//! ```text
//! ADC ─DMA ch6 (reload)─► capture ring 16 KiB ─► Session::feed ─► write-behind ring ─► card
//!                         (arena)                 (chain, encoder)  (arena, ~37 KiB)
//! ```
//!
//! * No playback while recording: the card task runs [`run`] instead of
//!   its idle loop (the UI and the developer command stop the music
//!   first). The FM radio keeps playing through its analogue bypass.
//! * Two futures joined: capture (wait for a finished half of the DMA
//!   ring, process it in place, encode into the write-behind ring) and the
//!   writer (whole sectors to the card, the header every 10 s). While the
//!   card is slow the capture goes on; a block the ring cannot take is
//!   dropped whole, counted and marked (`orpheus_rec::session`).
//! * Buffers: all in the decoder arena (idle while recording): the
//!   capture ring, the encoder's block buffer, one encoded block, the
//!   write-behind ring. The session itself (≈ 500 B) is written into the
//!   arena too, so nothing large is on the stack or in the task's future.
//! * The state the UI and `rec status` read lives in SRAM3 after the
//!   radio's ([`SHARED_ADDR`], 02 §1), written by the card task, read by
//!   any task (atomics; the file name under a critical section).
//! * Files: `/Recordings/REC-YYYYMMDD-HHMMSS.ext` (the volume's write
//!   prefix is `/Recordings` for the length of the recording), or for the
//!   developer command under `/.orpheus/test/rec/` (CLAUDE.md rule 5).
//! * Developer commands ([`test_command`], from the USB task): `rec start
//!   <mic|line|fm> <wav|adpcm|flac> [secs] [sub-folder]`, `rec stop`,
//!   `rec status`.
//!
//! Not built: monitoring the microphone in the headphones (the setting is
//! there; the radio is heard through its bypass anyway), pre-record,
//! pre-allocation, split on mark.

use core::cell::RefCell;
use core::sync::atomic::{AtomicU8, AtomicU32, Ordering::Relaxed};

use atj2157_hal::adc::{self, Input};
use atj2157_hal::dma::{self, Channel as DmaCh};
use critical_section::Mutex;
use embassy_futures::join::join;
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::signal::Signal;
use embassy_time::{Instant, Timer};
use orpheus_apps::services::{FileRef, RecEnd, RecPhase, RecSettings};
use orpheus_fs::{DirEntry, File, Locator, OpenMode, Volume};
use orpheus_rec::chain::Chain;
use orpheus_rec::encoder::{BUF_SAMPLES, MAX_BLOCK_BYTES, MAX_HEADER};
use orpheus_rec::name::{self, Path, Stamp};
use orpheus_rec::session::{Config, Session, State, StopReason};
use orpheus_rec::{Format, Params, Source};
use yoton_bsp::card::Card;

use crate::audio::reply;

macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

type Vol = Volume<'static, Card>;

/// The ADC's DMA channel (01-hardware §6 plan).
const DMA_CH: usize = dma::RING_CH;
/// Capture ring: two halves.
const CAP_BYTES: usize = 16 * 1024;
/// Preferred card write and the latency bound.
const CHUNK: usize = 8 * 1024;
const MAX_WAIT_MS: u32 = 1000;
/// The meters stop when the app has not looked for this long.
const KEEPALIVE_MS: u32 = 3000;
/// The developer command's folder.
const TEST_DIR: &str = "/.orpheus/test/rec";

// ------------------------------------------------------------- shared

/// What the UI and `rec status` read (SRAM3, 02 §1).
#[repr(C)]
pub struct Shared {
    phase: AtomicU8,
    /// [`RecEnd`] code + 1 (0 none).
    ended: AtomicU8,
    /// Bit 0 clip, 1 voice activation waiting, 2 developer recording.
    flags: AtomicU8,
    /// Source, format, channels: `src | fmt << 2 | ch << 4`.
    kind: AtomicU8,
    /// Bars and holds (dBFS, `i8` each): bar L, bar R, hold L, hold R.
    meters: AtomicU32,
    elapsed_ms: AtomicU32,
    rate: AtomicU32,
    written: AtomicU32,
    produced: AtomicU32,
    dropouts: AtomicU32,
    dropped_frames: AtomicU32,
    lost_frames: AtomicU32,
    /// Write-behind ring fill and highest fill (bytes).
    ring: AtomicU32,
    ring_peak: AtomicU32,
    /// Recording time left (s), marks, AGC gain (tenths of a dB, as i32),
    /// the station.
    left_s: AtomicU32,
    marks: AtomicU32,
    agc_db10: AtomicU32,
    fm_khz: AtomicU32,
    /// When the UI last looked (ms, low 32 bits).
    keepalive_ms: AtomicU32,
    /// `/Recordings`' first cluster (0: none yet).
    rec_dir: AtomicU32,
    /// Changes (the UI turns them into events).
    seq: AtomicU32,
    /// The file name (UTF-8, length first).
    name: Mutex<RefCell<[u8; 40]>>,
}

/// SRAM3 after the radio's RAM, below the card I/O counter.
pub const SHARED_ADDR: usize = (crate::fm::FM_END + 7) & !7;
const _: () = assert!(SHARED_ADDR + core::mem::size_of::<Shared>() <= yoton_bsp::card::IO_ADDR);
const _: () = assert!(SHARED_ADDR % core::mem::align_of::<Shared>() == 0);

/// Once, from `audio::init_chain` before any task runs.
#[inline(never)]
pub fn init() {
    let s = Shared {
        phase: AtomicU8::new(0),
        ended: AtomicU8::new(0),
        flags: AtomicU8::new(0),
        kind: AtomicU8::new(0),
        meters: AtomicU32::new(0xC4C4_C4C4),
        elapsed_ms: AtomicU32::new(0),
        rate: AtomicU32::new(0),
        written: AtomicU32::new(0),
        produced: AtomicU32::new(0),
        dropouts: AtomicU32::new(0),
        dropped_frames: AtomicU32::new(0),
        lost_frames: AtomicU32::new(0),
        ring: AtomicU32::new(0),
        ring_peak: AtomicU32::new(0),
        left_s: AtomicU32::new(u32::MAX),
        marks: AtomicU32::new(0),
        agc_db10: AtomicU32::new(0),
        fm_khz: AtomicU32::new(0),
        keepalive_ms: AtomicU32::new(0),
        rec_dir: AtomicU32::new(0),
        seq: AtomicU32::new(0),
        name: Mutex::new(RefCell::new([0; 40])),
    };
    // SAFETY: `SHARED_ADDR` is plain SRAM3 (enabled in crt0, inside the
    // MPU's RAM sub-region), aligned and below the I/O counter (asserted
    // above), used by nothing else; written once before any task runs.
    unsafe { core::ptr::write(SHARED_ADDR as *mut Shared, s) };
}

fn shared() -> &'static Shared {
    // SAFETY: initialised by `init` before any task runs; only atomics and
    // a critical-section mutex inside, so shared references are fine.
    unsafe { &*(SHARED_ADDR as *const Shared) }
}

fn now_ms() -> u64 {
    Instant::now().as_millis()
}

const fn phase_code(p: RecPhase) -> u8 {
    match p {
        RecPhase::Off => 0,
        RecPhase::Armed => 1,
        RecPhase::Recording => 2,
        RecPhase::Paused => 3,
        RecPhase::Waiting => 4,
        RecPhase::Saving => 5,
    }
}

const fn phase_of(c: u8) -> RecPhase {
    match c {
        1 => RecPhase::Armed,
        2 => RecPhase::Recording,
        3 => RecPhase::Paused,
        4 => RecPhase::Waiting,
        5 => RecPhase::Saving,
        _ => RecPhase::Off,
    }
}

fn set_phase(p: RecPhase) {
    let s = shared();
    if s.phase.swap(phase_code(p), Relaxed) != phase_code(p) {
        s.seq.fetch_add(1, Relaxed);
    }
}

fn set_ended(e: RecEnd) {
    let c = match e {
        RecEnd::Saved => 1,
        RecEnd::CardFull => 2,
        RecEnd::CardError => 3,
        RecEnd::NoCard => 4,
        RecEnd::NoSource => 5,
    };
    shared().ended.store(c, Relaxed);
    shared().seq.fetch_add(1, Relaxed);
}

/// What the UI shows (`services::Fw`).
pub struct View {
    pub phase: RecPhase,
    pub ended: Option<RecEnd>,
    pub clip: bool,
    pub meters: [i8; 4],
    pub elapsed_ms: u32,
    pub dropouts: u32,
    pub marks: u32,
    pub left_s: Option<u32>,
    pub agc_db10: i32,
    pub fm_khz: u32,
    pub seq: u32,
}

/// The Recorder app is looking: the meters keep running (they stop
/// [`KEEPALIVE_MS`] after the last call).
pub fn keepalive() {
    shared().keepalive_ms.store(now_ms() as u32, Relaxed);
}

/// The state for the UI.
pub fn view() -> View {
    let s = shared();
    let m = s.meters.load(Relaxed).to_le_bytes();
    let left = s.left_s.load(Relaxed);
    View {
        phase: phase_of(s.phase.load(Relaxed)),
        ended: match s.ended.load(Relaxed) {
            1 => Some(RecEnd::Saved),
            2 => Some(RecEnd::CardFull),
            3 => Some(RecEnd::CardError),
            4 => Some(RecEnd::NoCard),
            5 => Some(RecEnd::NoSource),
            _ => None,
        },
        clip: s.flags.load(Relaxed) & 1 != 0,
        meters: [m[0] as i8, m[1] as i8, m[2] as i8, m[3] as i8],
        elapsed_ms: s.elapsed_ms.load(Relaxed),
        dropouts: s.dropouts.load(Relaxed),
        marks: s.marks.load(Relaxed),
        left_s: (left != u32::MAX).then_some(left),
        agc_db10: s.agc_db10.load(Relaxed) as i32,
        fm_khz: s.fm_khz.load(Relaxed),
        seq: s.seq.load(Relaxed),
    }
}

/// Copy the file name (the part after the last `/`) into `out`.
pub fn file_name(out: &mut orpheus_ui::text::StrBuf<40>) {
    critical_section::with(|cs| {
        let b = shared().name.borrow(cs).borrow();
        let n = usize::from(b[0]).min(39);
        let s = core::str::from_utf8(b.get(1..1 + n).unwrap_or(&[])).unwrap_or("");
        out.clear();
        out.push_str(s.rsplit('/').next().unwrap_or(s));
    });
}

fn set_name(p: &str) {
    critical_section::with(|cs| {
        let mut b = shared().name.borrow(cs).borrow_mut();
        // The tail if it is long (the name matters, not the folder).
        let mut start = p.len().saturating_sub(39);
        while !p.is_char_boundary(start) {
            start += 1;
        }
        let t = &p.as_bytes()[start..];
        b[0] = t.len() as u8;
        b[1..1 + t.len()].copy_from_slice(t);
    });
}

/// A file is open (auto-off waits; the UI shows the red dot).
#[must_use]
pub fn active() -> bool {
    phase_of(shared().phase.load(Relaxed)).is_recording()
}

/// `/Recordings`' first cluster (0: not there).
#[must_use]
pub fn rec_dir() -> u32 {
    shared().rec_dir.load(Relaxed)
}

// ------------------------------------------------------------ requests

/// A sub-folder name under [`TEST_DIR`] (developer command).
#[derive(Clone, Copy)]
pub struct Sub {
    len: u8,
    b: [u8; 24],
}

/// Requests to the card task.
#[derive(Clone, Copy)]
pub enum Cmd {
    /// Capture for the meters with these settings (the app is shown).
    Arm(RecSettings),
    /// Record: settings, the station when it is FM, the developer
    /// command's limit (s, 0 none) and folder.
    Start {
        s: RecSettings,
        fm_khz: u32,
        dev: Option<(u32, Option<Sub>)>,
    },
    Disarm,
    Stop,
    Pause,
    Resume,
    Mark,
    /// The manual input gain changed (dB from the source's default).
    Gain(i8),
}

pub static REQ: Channel<CriticalSectionRawMutex, Cmd, 2> = Channel::new();
/// Wakes the writer: bytes arrived or the capture ended.
static WRITER: Signal<CriticalSectionRawMutex, ()> = Signal::new();

/// Send a request (the UI: never blocks; a full queue drops it).
pub fn request(c: Cmd) -> bool {
    REQ.try_send(c).is_ok()
}

/// Power-off: stop and save a recording (at most 3 s).
pub async fn stop_for_power_off() {
    if phase_of(shared().phase.load(Relaxed)) == RecPhase::Off {
        return;
    }
    let _ = REQ.try_send(Cmd::Stop);
    let _ = REQ.try_send(Cmd::Disarm);
    for _ in 0..300 {
        if phase_of(shared().phase.load(Relaxed)) == RecPhase::Off {
            break;
        }
        Timer::after_millis(10).await;
    }
}

// ------------------------------------------------------------- capture

const fn input_of(s: Source) -> Input {
    match s {
        // RECORD_MIC_INPUT_MODE = 1 in the vendor's CONFIG.TXT.
        Source::Mic => Input::Mic { differential: true },
        Source::Line => Input::Line,
        Source::Fm => Input::Fm,
    }
}

/// The ADC running into the capture ring by DMA.
struct Capture {
    ch: DmaCh,
    ring: *mut i16,
    base: u32,
    taken: u32,
    channels: usize,
}

impl Capture {
    /// Start the converter, wait for it to settle, start the DMA.
    async fn start(s: &RecSettings, ring: *mut u8) -> Capture {
        let input = input_of(s.source);
        let (analog, _) = input.default_gain();
        let digital = adc::digital_for(input, if s.chain.agc { 0 } else { s.gain_db });
        adc::start(input, s.params.rate, analog, digital);
        Timer::after_millis(adc::SETTLE_MS).await;
        // SAFETY: channel 6 is the ADC's (01-hardware §6); only this
        // module uses it, one `Capture` at a time.
        let ch = unsafe { DmaCh::steal(DMA_CH) };
        ch.enable_ring_irq();
        let base = dma::events(DMA_CH);
        ch.start(adc::DMA_MODE, adc::FIFO_ADDR, ring as u32, CAP_BYTES as u32);
        Capture { ch, ring: ring.cast(), base, taken: 0, channels: usize::from(input.channels()) }
    }

    /// The next finished half and the frames lost before it (the DMA
    /// lapped this reader).
    async fn next(&mut self) -> (&mut [i16], u64) {
        let mut e = dma::events(DMA_CH).wrapping_sub(self.base);
        if e == self.taken {
            e = self.ch.wait_events(self.base.wrapping_add(self.taken)).await.wrapping_sub(self.base);
        }
        let half = CAP_BYTES / 4;
        let mut lost = 0;
        if e.wrapping_sub(self.taken) >= 2 {
            let skip = e.wrapping_sub(self.taken) - 1;
            lost = u64::from(skip) * (half / self.channels) as u64;
            self.taken = e - 1;
        }
        let at = (self.taken % 2) as usize * half;
        self.taken = self.taken.wrapping_add(1);
        // SAFETY: the half the DMA finished; it writes the other one now
        // (a lap is detected at the next call and counted, the data then
        // is what it is).
        let s = unsafe { core::slice::from_raw_parts_mut(self.ring.add(at), half) };
        (s, lost)
    }

    fn stop(self) {
        self.ch.abort();
        self.ch.disable_irq();
        adc::stop();
    }
}

fn publish_levels(l: &orpheus_rec::meter::Levels) {
    let b = |v: i16| v.clamp(-60, 0) as i8 as u8;
    let m = u32::from_le_bytes([b(l.bar[0]), b(l.bar[1]), b(l.hold[0]), b(l.hold[1])]);
    let s = shared();
    s.meters.store(m, Relaxed);
    let f = s.flags.load(Relaxed);
    s.flags.store((f & !1) | u8::from(l.clip), Relaxed);
}

fn publish_kind(s: &RecSettings) {
    let src = match s.source {
        Source::Mic => 0u8,
        Source::Line => 1,
        Source::Fm => 2,
    };
    let fmt = match s.params.format {
        Format::Wav => 0u8,
        Format::Adpcm => 1,
        Format::Flac => 2,
    };
    shared().kind.store(src | fmt << 2 | s.params.channels << 4, Relaxed);
    shared().rate.store(s.params.rate, Relaxed);
}

fn looked_at_lately() -> bool {
    (now_ms() as u32).wrapping_sub(shared().keepalive_ms.load(Relaxed)) < KEEPALIVE_MS
}

/// Meters only, until a request that is not for the meters (returned).
async fn meter(s: RecSettings, ring: *mut u8) -> Cmd {
    publish_kind(&s);
    let mut cap = Capture::start(&s, ring).await;
    let mut chain = Chain::new(s.chain, s.params.rate, s.params.channels);
    set_phase(RecPhase::Armed);
    let next = loop {
        match select3(cap.next(), REQ.receive(), Timer::after_millis(500)).await {
            Either3::First((half, _)) => {
                chain.process(half);
                publish_levels(&chain.levels());
            }
            Either3::Second(Cmd::Arm(n)) if n.source == s.source && n.params.rate == s.params.rate => {
                chain.set_config(n.chain);
                publish_kind(&n);
            }
            Either3::Second(Cmd::Gain(g)) => {
                let input = input_of(s.source);
                adc::set_gain(input.default_gain().0, adc::digital_for(input, g));
            }
            Either3::Second(c @ (Cmd::Arm(_) | Cmd::Start { .. } | Cmd::Disarm)) => break c,
            Either3::Second(_) => {}
            Either3::Third(()) => {}
        }
        if !looked_at_lately() {
            break Cmd::Disarm;
        }
    };
    cap.stop();
    next
}

// ----------------------------------------------------------- recording

/// The arena, cut up for a recording.
struct Bufs {
    cap: *mut u8,
    enc: &'static mut [i16],
    scratch: &'static mut [u8],
    wb: *mut u8,
    wb_len: usize,
    session: *mut Session<&'static mut [i16], &'static mut [u8]>,
}

/// Cut `arena` (the card task's, idle while recording). `None` if too small.
fn carve(arena: &mut [u8]) -> Option<Bufs> {
    type S = Session<&'static mut [i16], &'static mut [u8]>;
    let base = arena.as_mut_ptr();
    let len = arena.len();
    let align = |o: usize, a: usize| (o + a - 1) & !(a - 1);
    let cap = align(base.align_offset(4), 4);
    let enc = align(cap + CAP_BYTES, 4);
    let scratch = enc + BUF_SAMPLES * 2;
    let sess = align(scratch + MAX_BLOCK_BYTES, core::mem::align_of::<S>().max(8));
    let wb = align(sess + core::mem::size_of::<S>(), 4);
    let wb_len = len.checked_sub(wb)? / 512 * 512;
    if wb_len < 4 * CHUNK {
        return None;
    }
    // SAFETY: disjoint, aligned pieces of `arena`, which the card task
    // does not touch otherwise while recording; 'static because the arena
    // is a static (the pieces are dropped before `run` returns).
    unsafe {
        Some(Bufs {
            cap: base.add(cap),
            enc: core::slice::from_raw_parts_mut(base.add(enc).cast::<i16>(), BUF_SAMPLES),
            scratch: core::slice::from_raw_parts_mut(base.add(scratch), MAX_BLOCK_BYTES),
            wb: base.add(wb),
            wb_len,
            session: base.add(sess).cast(),
        })
    }
}

fn stamp() -> Option<Stamp> {
    if !atj2157_hal::rtc_clock::running() {
        return None;
    }
    let n = atj2157_hal::rtc_clock::now();
    (n.year >= 2024).then_some(Stamp {
        year: n.year,
        month: n.month,
        day: n.day,
        hour: n.hour,
        minute: n.minute,
        second: n.second,
    })
}

/// Make the folders of `dir` (each step; existing ones are fine).
async fn make_dirs(vol: &mut Vol, dir: &str) {
    for (i, _) in dir.match_indices('/').skip(1) {
        let _ = vol.create_dir(&dir[..i]).await;
    }
    let _ = vol.create_dir(dir).await;
}

/// Create the file: a free name in `dir`.
async fn create(vol: &mut Vol, dir: &str, s: &RecSettings, fm_khz: u32) -> Option<(File, Path)> {
    make_dirs(vol, dir).await;
    let when = stamp();
    let fm = (s.source == Source::Fm).then_some(fm_khz);
    for k in 0..9999u32 {
        let (counter, suffix) = if when.is_some() { (0, k) } else { (k + 1, 0) };
        let p = name::path(dir, when, counter, fm, suffix, s.params.format.ext());
        if vol.exists(p.as_str()).await == Ok(false)
            && let Ok(f) = vol.open(p.as_str(), OpenMode::CreateNew).await
        {
            return Some((f, p));
        }
        if when.is_some() && k > 50 {
            break;
        }
    }
    None
}

/// The clock into the file system's time stamps.
fn fs_time(vol: &mut Vol) {
    if let Some(t) = stamp() {
        vol.set_time(orpheus_fs::DateTime {
            year: t.year,
            month: t.month,
            day: t.day,
            hour: t.hour,
            minute: t.minute,
            second: t.second,
            centis: 0,
        });
    }
}

/// Publish the session's state.
fn publish<B: AsRef<[i16]> + AsMut<[i16]>, S: AsRef<[u8]> + AsMut<[u8]>>(
    ss: &Session<B, S>,
    card_free: u64,
    bps: u32,
) {
    let st = ss.status();
    let s = shared();
    publish_levels(&st.levels);
    s.elapsed_ms.store(ss.elapsed_ms().min(u64::from(u32::MAX)) as u32, Relaxed);
    s.written.store(st.written.min(u64::from(u32::MAX)) as u32, Relaxed);
    s.produced.store(st.produced.min(u64::from(u32::MAX)) as u32, Relaxed);
    if s.dropouts.swap(st.dropouts, Relaxed) != st.dropouts {
        s.seq.fetch_add(1, Relaxed);
    }
    s.dropped_frames.store(st.dropped_frames.min(u64::from(u32::MAX)) as u32, Relaxed);
    s.lost_frames.store(st.lost_frames.min(u64::from(u32::MAX)) as u32, Relaxed);
    s.ring.store(st.ring_fill, Relaxed);
    s.ring_peak.store(st.ring_peak, Relaxed);
    s.marks.store(st.marks, Relaxed);
    s.agc_db10.store(i32::from(st.agc_db10) as u32, Relaxed);
    s.left_s.store(
        (card_free.saturating_sub(st.produced) / u64::from(bps.max(1))).min(u64::from(u32::MAX - 1)) as u32,
        Relaxed,
    );
    let f = s.flags.load(Relaxed);
    s.flags.store((f & !2) | (u8::from(st.waiting) << 1), Relaxed);
    let phase = match st.state {
        State::Paused => RecPhase::Paused,
        State::Closing | State::Done => RecPhase::Saving,
        State::Recording if st.waiting => RecPhase::Waiting,
        State::Recording => RecPhase::Recording,
    };
    set_phase(phase);
}

/// One file, from creation to close. Returns why it stopped and the next
/// request that is not about this recording (if one came).
async fn record(
    vol: &mut Vol,
    b: &mut Bufs,
    s: RecSettings,
    fm_khz: u32,
    dev: Option<(u32, Option<Sub>)>,
) -> (StopReason, Option<Cmd>) {
    // The folder and the write prefix.
    let mut dir = Path::of(if dev.is_some() { TEST_DIR } else { name::FOLDER });
    if let Some((_, Some(sub))) = dev {
        dir.push("/");
        dir.push(core::str::from_utf8(&sub.b[..usize::from(sub.len)]).unwrap_or(""));
    }
    if dev.is_none() {
        vol.set_write_prefix(Some(name::FOLDER));
    }
    fs_time(vol);
    let created = create(vol, dir.as_str(), &s, fm_khz).await;
    if dev.is_none() {
        let mut e = DirEntry::new();
        if vol.stat(name::FOLDER, &mut e).await.is_ok() {
            shared().rec_dir.store(e.first_cluster(), Relaxed);
        }
    }
    let Some((mut file, path)) = created else {
        vol.set_write_prefix(Some("/.orpheus"));
        set_ended(RecEnd::NoCard);
        reply(|w| {
            w.s(t!(b"rec: cannot create the file"));
        });
        return (StopReason::CardError, None);
    };
    set_name(path.as_str());
    let card_free = vol.free_bytes().await.unwrap_or(0);
    let cfg = Config {
        params: s.params,
        chain: s.chain,
        limit_ms: dev.and_then(|(secs, _)| (secs > 0).then_some(u64::from(secs) * 1000)),
        split_ms: (s.split_min > 0).then_some(u64::from(s.split_min) * 60_000),
        ring_bytes: b.wb_len,
        chunk: CHUNK,
        max_wait_ms: MAX_WAIT_MS,
    };
    // SAFETY: `session` is an aligned slot in the arena for exactly this
    // type (`carve`); written here, used only by this call, and never
    // dropped (it holds no resources).
    let ss: &mut Session<&'static mut [i16], &'static mut [u8]> = unsafe {
        let enc: &'static mut [i16] = &mut *core::ptr::from_mut(b.enc);
        let scratch: &'static mut [u8] = &mut *core::ptr::from_mut(b.scratch);
        b.session.write(Session::new(cfg, card_free, now_ms(), enc, scratch));
        &mut *b.session
    };
    let wb = b.wb;
    let mut sink = |at: usize, bytes: &[u8]| {
        // SAFETY: `at..at + len` is a free region of the write-behind ring
        // (`WriteBehind::put_at`), never the one a card write is reading.
        unsafe { core::ptr::copy_nonoverlapping(bytes.as_ptr(), wb.add(at), bytes.len()) };
    };
    ss.begin(now_ms(), &mut sink);
    shared().ended.store(0, Relaxed);
    let bps = s.params.bytes_per_second();
    publish_kind(&s);
    shared().fm_khz.store(if s.source == Source::Fm { fm_khz } else { 0 }, Relaxed);
    let f = shared().flags.load(Relaxed);
    shared().flags.store((f & !4) | (u8::from(dev.is_some()) << 2), Relaxed);
    publish(ss, card_free, bps);
    reply(|w| {
        w.s(t!(b"rec started ")).s(path.as_str().as_bytes());
    });
    crate::CLOCK_FLOOR.fetch_max(1, Relaxed);
    let cell = RefCell::new(ss);
    let mut pending: Option<Cmd> = None;
    let mut why = StopReason::User;
    let capture = async {
        let mut cap = Capture::start(&s, b.cap).await;
        let frames_per_half = (CAP_BYTES / 4 / usize::from(s.params.channels)) as u64;
        loop {
            match select(cap.next(), REQ.receive()).await {
                Either::First((half, lost)) => {
                    let mut ss = cell.borrow_mut();
                    if lost > 0 {
                        ss.lost(lost.min(frames_per_half * 8));
                    }
                    ss.feed(half, now_ms(), &mut sink);
                    publish(&ss, card_free, bps);
                    if let Some(r) = ss.due_stop() {
                        why = r;
                        ss.finish(r, now_ms(), &mut sink);
                        break;
                    }
                }
                Either::Second(c) => {
                    let mut ss = cell.borrow_mut();
                    match c {
                        Cmd::Pause => ss.pause(),
                        Cmd::Resume => ss.resume(),
                        Cmd::Mark => ss.mark(),
                        Cmd::Gain(g) => {
                            let input = input_of(s.source);
                            adc::set_gain(input.default_gain().0, adc::digital_for(input, g));
                        }
                        Cmd::Arm(n) => ss.set_chain(n.chain),
                        Cmd::Start { .. } => {}
                        Cmd::Stop | Cmd::Disarm => {
                            if matches!(c, Cmd::Disarm) {
                                pending = Some(Cmd::Disarm);
                            }
                            ss.finish(StopReason::User, now_ms(), &mut sink);
                            break;
                        }
                    }
                    publish(&ss, card_free, bps);
                }
            }
            WRITER.signal(());
        }
        cap.stop();
        WRITER.signal(());
    };
    let writer = async {
        let mut h = [0u8; MAX_HEADER];
        loop {
            let t = now_ms();
            let w = cell.borrow_mut().next_write(t);
            if let Some(w) = w {
                // SAFETY: the ring piece handed out for this write; the
                // producer does not touch it until `written`.
                let bytes = unsafe { core::slice::from_raw_parts(wb.add(w.at), w.len) };
                file.set_position(w.file_at);
                let ok = vol.write(&mut file, bytes).await.is_ok();
                cell.borrow_mut().written(ok, now_ms());
                continue;
            }
            let (state, drained, due) = {
                let ss = cell.borrow();
                (ss.state(), ss.drained(), ss.header_due(t))
            };
            if state == State::Done || (state == State::Closing && drained) {
                break;
            }
            if due {
                let n = cell.borrow_mut().header(&mut h, t);
                let end = file.position();
                file.set_position(0);
                let _ = vol.write(&mut file, &h[..n]).await;
                file.set_position(end);
                let _ = vol.flush_file(&mut file).await;
                continue;
            }
            let _ = select(WRITER.wait(), Timer::after_millis(100)).await;
        }
    };
    join(capture, writer).await;
    let ss = cell.into_inner();
    // `Done` before the final header: the writes kept failing.
    let failed = ss.state() == State::Done;
    let mut h = [0u8; MAX_HEADER];
    let n = if failed { 0 } else { ss.final_header(&mut h) };
    if n > 0 {
        file.set_position(0);
        let _ = vol.write(&mut file, &h[..n]).await;
    }
    let size = file.size();
    let closed = vol.close(file).await.is_ok() && vol.flush().await.is_ok();
    vol.set_write_prefix(Some("/.orpheus"));
    crate::CLOCK_FLOOR.store(0, Relaxed);
    let st = ss.status();
    let why = if failed || !closed { StopReason::CardError } else { why };
    set_ended(match why {
        StopReason::CardFull => RecEnd::CardFull,
        StopReason::CardError => RecEnd::CardError,
        _ => RecEnd::Saved,
    });
    reply(|w| {
        w.s(t!(b"rec saved "))
            .s(path.as_str().as_bytes())
            .s(t!(b" bytes="))
            .u(size)
            .s(t!(b" frames="))
            .u(st.frames)
            .s(t!(b" drops="))
            .u(u64::from(st.dropouts))
            .s(t!(b" dropped_frames="))
            .u(st.dropped_frames)
            .s(t!(b" lost_frames="))
            .u(st.lost_frames)
            .s(t!(b" ring_peak="))
            .u(u64::from(st.ring_peak))
            .s(t!(b" why="))
            .s(match why {
                StopReason::User => t!(b"stop"),
                StopReason::TimeLimit => t!(b"time"),
                StopReason::Split => t!(b"split"),
                StopReason::CardFull => t!(b"card-full"),
                StopReason::CardError => t!(b"card-error"),
            });
    });
    (why, pending)
}

/// Recording mode: from the first request until disarmed. Returns to the
/// card task's idle loop.
pub async fn run(vol: &mut Vol, arena: &mut [u8], first: Cmd) {
    let Some(mut b) = carve(arena) else {
        reply(|w| {
            w.s(t!(b"rec: no room"));
        });
        return;
    };
    let mut cmd = first;
    let mut armed: Option<RecSettings> = None;
    loop {
        cmd = match cmd {
            Cmd::Arm(s) => {
                armed = Some(s);
                meter(s, b.cap).await
            }
            Cmd::Start { s, fm_khz, dev } => {
                if s.source == Source::Fm && fm_khz == 0 {
                    set_ended(RecEnd::NoSource);
                    reply(|w| {
                        w.s(t!(b"rec: the radio is off (fm on first)"));
                    });
                } else if !adc::rate_supported(s.params.rate) {
                    set_ended(RecEnd::NoSource);
                } else {
                    let (mut why, mut next) = record(vol, &mut b, s, fm_khz, dev).await;
                    // Automatic split: the next file at once.
                    while why == StopReason::Split && next.is_none() {
                        (why, next) = record(vol, &mut b, s, fm_khz, dev).await;
                    }
                    if let Some(n) = next {
                        cmd = n;
                        continue;
                    }
                }
                match armed {
                    Some(a) if dev.is_none() && looked_at_lately() => Cmd::Arm(a),
                    _ => Cmd::Disarm,
                }
            }
            Cmd::Disarm => break,
            // Nothing to stop, pause or mark: wait for the next request.
            _ => {
                if !looked_at_lately() && armed.is_none() {
                    break;
                }
                REQ.receive().await
            }
        };
    }
    set_phase(RecPhase::Off);
    shared().meters.store(0xC4C4_C4C4, Relaxed);
}

// ------------------------------------------------------------- deleting

/// Delete a recording (Recorder › Recordings…, 05 §7.5): only a file in
/// `/Recordings`, with the write prefix moved there for the one remove.
pub async fn delete(vol: &mut Vol, f: FileRef) -> bool {
    let dir = rec_dir();
    if dir == 0 || f.dir != dir {
        return false;
    }
    let mut e = DirEntry::new();
    if vol.open_locator(Locator { dir: f.dir, index: f.index }, &mut e).await.is_err() || e.is_dir() {
        return false;
    }
    let mut p = Path::of(name::FOLDER);
    p.push("/");
    p.push(e.name());
    vol.set_write_prefix(Some(name::FOLDER));
    let ok = vol.remove(p.as_str()).await.is_ok() && vol.flush().await.is_ok();
    vol.set_write_prefix(Some("/.orpheus"));
    ok
}

/// At mount: where `/Recordings` is (the UI's list).
pub async fn find_dir(vol: &mut Vol) {
    let mut e = DirEntry::new();
    let c = if vol.stat(name::FOLDER, &mut e).await.is_ok() && e.is_dir() { e.first_cluster() } else { 0 };
    shared().rec_dir.store(c, Relaxed);
}

// ------------------------------------------------------ developer channel

fn parse_u32(s: &str) -> Option<u32> {
    if s.is_empty() || !s.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    s.parse().ok()
}

/// `rec start|stop|status` from the developer channel (USB task, main
/// executor). `start` goes to the card task (music stops first); `stop`
/// asks it; `status` answers at once.
pub fn test_command(rest: &[u8]) -> bool {
    let text = core::str::from_utf8(rest).unwrap_or("");
    let mut words = text.split(' ').filter(|w| !w.is_empty());
    match words.next() {
        Some("start") => {
            let (Some(src), Some(fmt)) =
                (words.next().and_then(Source::parse), words.next().and_then(Format::parse))
            else {
                reply(|w| {
                    w.s(t!(b"usage: rec start <mic|line|fm> <wav|adpcm|flac> [secs] [sub-folder]"));
                });
                return true;
            };
            let mut secs = 0;
            let mut sub = None;
            for w in words {
                if let Some(n) = parse_u32(w) {
                    secs = n;
                } else if w.len() <= 24
                    && w.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
                {
                    let mut b = [0u8; 24];
                    b[..w.len()].copy_from_slice(w.as_bytes());
                    sub = Some(Sub { len: w.len() as u8, b });
                } else {
                    reply(|w| {
                        w.s(t!(b"rec: the folder is a name of letters, digits, - and _"));
                    });
                    return true;
                }
            }
            let ch = src.channels();
            let s = RecSettings {
                source: src,
                params: Params { rate: 44_100, channels: ch, format: fmt, level: 1 },
                chain: orpheus_rec::chain::ChainConfig::default(),
                gain_db: 0,
                split_min: 0,
                monitor: false,
            };
            let fm_khz = crate::fm::with(|r| {
                let i = r.info();
                if matches!(i.phase, orpheus_fm::Phase::Playing) { i.khz } else { 0 }
            });
            // No playback while recording.
            crate::media::play(crate::media::PlayReq::Stop);
            crate::audio::request_stop();
            reply(|w| {
                w.s(t!(b"rec: starting"));
            });
            request(Cmd::Start { s, fm_khz, dev: Some((secs, sub)) })
        }
        Some("stop") => {
            if active() {
                reply(|w| {
                    w.s(t!(b"rec: stopping"));
                });
                request(Cmd::Stop)
            } else {
                reply(|w| {
                    w.s(t!(b"rec: not recording"));
                });
                true
            }
        }
        Some("status") => {
            status_reply();
            true
        }
        _ => {
            reply(|w| {
                w.s(t!(
                    b"rec start <mic|line|fm> <wav|adpcm|flac> [secs] [sub-folder] | rec stop | rec status"
                ));
            });
            true
        }
    }
}

/// `rec status`: state, level, written, drops, ring headroom.
pub fn status_reply() {
    let s = shared();
    let v = view();
    let kind = s.kind.load(Relaxed);
    let mut name = orpheus_ui::text::StrBuf::<40>::new();
    file_name(&mut name);
    let regs = adc::regs();
    reply(|w| {
        w.s(t!(b"rec "))
            .s(match v.phase {
                RecPhase::Off => t!(b"off"),
                RecPhase::Armed => t!(b"armed"),
                RecPhase::Recording => t!(b"recording"),
                RecPhase::Paused => t!(b"paused"),
                RecPhase::Waiting => t!(b"waiting"),
                RecPhase::Saving => t!(b"saving"),
            })
            .s(t!(b" src="))
            .s(match kind & 3 {
                1 => t!(b"line"),
                2 => t!(b"fm"),
                _ => t!(b"mic"),
            })
            .s(t!(b" fmt="))
            .s(match (kind >> 2) & 3 {
                1 => t!(b"adpcm"),
                2 => t!(b"flac"),
                _ => t!(b"wav"),
            })
            .s(t!(b" rate="))
            .u(u64::from(s.rate.load(Relaxed)))
            .s(t!(b" ch="))
            .u(u64::from(kind >> 4))
            .s(t!(b" ms="))
            .u(u64::from(v.elapsed_ms))
            .s(t!(b" level="))
            .i(i16::from(v.meters[0]))
            .s(b"/")
            .i(i16::from(v.meters[1]))
            .s(t!(b" peak="))
            .i(i16::from(v.meters[2]))
            .s(b"/")
            .i(i16::from(v.meters[3]))
            .s(t!(b" clip="))
            .u(u64::from(v.clip))
            .s(t!(b" written="))
            .u(u64::from(s.written.load(Relaxed)))
            .s(t!(b" produced="))
            .u(u64::from(s.produced.load(Relaxed)))
            .s(t!(b" drops="))
            .u(u64::from(v.dropouts))
            .s(t!(b" dropped_frames="))
            .u(u64::from(s.dropped_frames.load(Relaxed)))
            .s(t!(b" lost_frames="))
            .u(u64::from(s.lost_frames.load(Relaxed)))
            .s(t!(b" ring="))
            .u(u64::from(s.ring.load(Relaxed)))
            .s(t!(b" ring_peak="))
            .u(u64::from(s.ring_peak.load(Relaxed)))
            .s(t!(b" marks="))
            .u(u64::from(v.marks))
            .s(t!(b" fm="))
            .u(u64::from(v.fm_khz))
            .s(t!(b" file="))
            .s(name.as_str().as_bytes())
            .s(t!(b" adc="));
        for (i, r) in regs.iter().enumerate() {
            if i > 0 {
                w.s(b",");
            }
            w.h(*r);
        }
        w.s(t!(b" dma_events=")).u(u64::from(dma::events(DMA_CH)));
    });
}
