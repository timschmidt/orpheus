//! Playback path (docs/plan/04-audio.md §2, WP AU-1 device part).
//!
//! ```text
//! card ─► stream ring (16 KiB) ─► decoder (arena 72 KiB) ─► PCM ring (12 KiB, SRAM3) ─► DMA ch5 ─► DAC
//! ```
//!
//! One task on the RT executor reads, decodes and fills the PCM ring; the
//! DAC DMA runs in reload mode over the ring and its half/complete
//! interrupts say which half is free. The same task owns the mounted card
//! and the playback service (`yoton_player::Player`, WP AU-5): the UI's
//! player requests become player events, the decode loop feeds it what
//! happens (opened, position, reader at end, decoder ended or failed,
//! ticks) and carries out its actions (open, seek, pause, resume, stop).
//! Developer commands (`ls PATH`, `play PATH`, `stop`, …) bypass the
//! player.
//!
//! While idle, the task also builds directory indexes in the background
//! (`build_slot`, AU-12) on the end of the decoder arena.
//!
//! Not yet: gapless hand-over (the player's `Prefetch` is not carried out;
//! the next track opens after the current one has played out), the fades
//! of 04 §2.2 around pause and seek, periodic resume saves while playing
//! (a card write between blocks could underrun; the state is saved at
//! pause, stop, track change, card hand-over and power-off instead).

use core::future::poll_fn;
use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use core::task::Poll;

use atj2157_hal::dma::{self, Channel as DmaCh};
use atj2157_hal::interrupt::Level;
use atj2157_hal::{dac, time};
use embassy_futures::select::{Either, Either3, select, select3};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::waitqueue::AtomicWaker;
use embassy_time::Timer;
use static_cell::ConstStaticCell;
use yoton_audio::{
    ByteSource, ContainerInfo, OpenError, PcmBlock, Probe, SeekSource, SeekStep, SourceError, Step, probe,
};
use yoton_bsp::card::Card;
use yoton_fs::{CacheSlot, DirEntry, File, MountOptions, OpenMode, Volume};
use yoton_index::Progress;
use yoton_index::card::{CardIo, Handles, Mode};
use yoton_player::image::{IMAGE_LEN, decode_queue, encode_queue, queue_file_len};
use yoton_player::{Command, Event, TrackRef};
use yoton_proto::hash::crc32_update;
use yoton_ui::event::Source;

use crate::media::{self, Flow, OpenReq, PlayReq, Req, Server, Target};

// ---------------------------------------------------------------- commands

pub const CMD_LEN: usize = 120;

#[derive(Clone, Copy)]
pub struct Cmd {
    pub len: usize,
    pub text: [u8; CMD_LEN],
}

pub static CMDS: Channel<CriticalSectionRawMutex, Cmd, 2> = Channel::new();

/// Text reply of the last command (read through the developer channel).
pub static REPLY: critical_section::Mutex<core::cell::RefCell<([u8; 1024], usize)>> =
    critical_section::Mutex::new(core::cell::RefCell::new(([0; 1024], 0)));

fn reply(f: impl FnOnce(&mut Writer<'_>)) {
    critical_section::with(|cs| {
        let mut r = REPLY.borrow(cs).borrow_mut();
        let (buf, n) = &mut *r;
        let mut w = Writer { buf, n: 0 };
        f(&mut w);
        *n = w.n;
    });
}

pub struct Writer<'a> {
    buf: &'a mut [u8; 1024],
    n: usize,
}

impl Writer<'_> {
    pub fn s(&mut self, s: &[u8]) -> &mut Self {
        for &b in s {
            if self.n < self.buf.len() {
                self.buf[self.n] = b;
                self.n += 1;
            }
        }
        self
    }
    pub fn u(&mut self, mut v: u64) -> &mut Self {
        let mut d = [0u8; 20];
        let mut i = d.len();
        loop {
            i -= 1;
            d[i] = b'0' + (v % 10) as u8;
            v /= 10;
            if v == 0 {
                break;
            }
        }
        let tmp = d;
        self.s(&tmp[i..])
    }
}

// ------------------------------------------------------------- statistics

pub static FRAMES: AtomicU32 = AtomicU32::new(0);
pub static UNDERRUNS: AtomicU32 = AtomicU32::new(0);
pub static CRC: AtomicU32 = AtomicU32::new(0);
pub static CORRUPT: AtomicU32 = AtomicU32::new(0);
pub static RATE: AtomicU32 = AtomicU32::new(0);
pub static DECODE_US: AtomicU32 = AtomicU32::new(0);
pub static PLAY_US: AtomicU32 = AtomicU32::new(0);
pub static PLAYING: AtomicBool = AtomicBool::new(false);
/// Where the playback loop is (status diagnostics): 1 card read, 2 media
/// request, 3 waiting for the DAC, 4 decoding, 5 DSP, 0 elsewhere.
pub static WHERE: AtomicU32 = AtomicU32::new(0);
/// Clock floor for the current track (set at its start from the codec).
static CODEC_FLOOR: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// Apply the clock floor: the codec's, raised to Mid while the DSP chain
/// works; the level is raised here at once (from the RT task) and the
/// governor never goes below it.
fn dsp_clock_floor(dsp_active: bool) {
    use atj2157_hal::clock::{Level, current, set_level};
    let floor = CODEC_FLOOR.load(Ordering::Relaxed).max(u8::from(dsp_active));
    crate::CLOCK_FLOOR.store(floor, Ordering::Relaxed);
    let have = match current() {
        Level::Low | Level::Base => 0,
        Level::Mid => 1,
        _ => 2,
    };
    if have < floor {
        set_level(if floor >= 2 { Level::High } else { Level::Mid });
    }
}

fn sound_active() -> bool {
    DSP_ON.load(Ordering::Relaxed)
        && critical_section::with(|cs| *media::SOUND.borrow(cs).borrow() != media::Sound::default())
}

/// Underruns after which a track is abandoned as undecodable in time.
const MAX_UNDERRUNS: u32 = 8;

/// Output CRC for HIL comparisons (test command `crc on|off`). Off by
/// default: the bitwise CRC32 per frame cost a large share of 48 MHz.
pub static CRC_ON: AtomicBool = AtomicBool::new(false);

/// DSP chain on/off (test command `dsp on|off`).
pub static DSP_ON: AtomicBool = AtomicBool::new(true);

/// Breadcrumb that survives a watchdog reset: the last mailbox argument
/// word (unused by the tests).
fn mark(v: u32) {
    WHERE.store(v, Ordering::Relaxed);
    // SAFETY: persistent-area word, written only here.
    unsafe { core::ptr::write_volatile(0x0013_7FEC as *mut u32, 0xB00D_0000 | v) };
}
static WRITTEN: AtomicU32 = AtomicU32::new(0);
/// Live RT-side timing (µs, cumulative since start): decode, card reads,
/// media requests served from inside playback, PCM ring waits.
pub static T_DECODE: AtomicU32 = AtomicU32::new(0);
pub static T_FILL: AtomicU32 = AtomicU32::new(0);
pub static T_SERVE: AtomicU32 = AtomicU32::new(0);
/// Developer `stop`: ends the current file at once (also inside a PCM
/// wait).
static STOP: AtomicBool = AtomicBool::new(false);

// ------------------------------------------------------------ stream ring

/// 16 KiB: RAM does not allow `yoton_audio::MAX_PEEK` (28 KiB) plus a
/// refill unit next to the resident audio path. Ogg pages up to ~15.5 KiB
/// play; a stream that needs more ends with an error (open, STATUS).
const RING: usize = 16 * 1024;
#[repr(C, align(4))]
struct RingBuf([u8; RING]);
static RING_BUF: ConstStaticCell<RingBuf> = ConstStaticCell::new(RingBuf([0; RING]));

struct Ring {
    buf: &'static mut [u8; RING],
    rd: u64,
    wr: u64,
    len: Option<u64>,
    eof: bool,
    /// A decoder seek moved the read position: the next `fill` reads the
    /// file from `wr`.
    reseek: bool,
}

impl ByteSource for Ring {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])> {
        if self.available() < n {
            return None;
        }
        let start = (self.rd % RING as u64) as usize;
        let first = n.min(RING - start);
        Some((&self.buf[start..start + first], &self.buf[..n - first]))
    }
    fn available(&self) -> usize {
        (self.wr - self.rd) as usize
    }
    fn consume(&mut self, n: usize) {
        self.rd += n.min(self.available()) as u64;
    }
    fn position(&self) -> u64 {
        self.rd
    }
    fn len(&self) -> Option<u64> {
        self.len
    }
    fn at_end(&self) -> bool {
        self.eof
    }
}

/// A seek flushes the ring; the playback loop refills it from the new
/// position (`Ring::fill`).
impl SeekSource for Ring {
    fn seek(&mut self, pos: u64) -> Result<(), SourceError> {
        let len = self.len.ok_or(SourceError::NotSeekable)?;
        if pos > len {
            return Err(SourceError::OutOfRange);
        }
        self.rd = pos;
        self.wr = pos;
        self.eof = pos >= len;
        self.reseek = true;
        Ok(())
    }
}

impl Ring {
    fn reset(&mut self, len: u64) {
        self.rd = 0;
        self.wr = 0;
        self.len = Some(len);
        self.eof = false;
        self.reseek = false;
    }

    /// Read from the file into free space (sector-sized pieces where it can).
    async fn fill(&mut self, vol: &mut Volume<'static, Card>, f: &mut File) -> bool {
        if self.reseek {
            self.reseek = false;
            f.set_position(self.wr);
        }
        let mut any = false;
        while !self.eof && RING - self.available() >= 512 {
            let start = (self.wr % RING as u64) as usize;
            let space = (RING - self.available()).min(RING - start).min(16384);
            match vol.read(f, &mut self.buf[start..start + space]).await {
                Ok(0) => self.eof = true,
                Ok(n) => {
                    self.wr += n as u64;
                    any = true;
                }
                Err(_) => {
                    self.eof = true;
                }
            }
        }
        any
    }
}

// ----------------------------------------------------------------- PCM out

/// PCM ring in SRAM3 (0x304000, 12 KiB = 3072 stereo frames).
const PCM_ADDR: u32 = 0x0030_4000;
const PCM_FRAMES: usize = 3072;
const HALF: usize = PCM_FRAMES / 2;
const DAC_CH: usize = 5;

static PLAYED: AtomicU32 = AtomicU32::new(0);
static PCM_WAKER: AtomicWaker = AtomicWaker::new();

fn on_dac_dma() {
    let what = dma::clear(DAC_CH);
    let mut n = 0;
    if what & 2 != 0 {
        n += 1;
    }
    if what & 1 != 0 {
        n += 1;
    }
    PLAYED.fetch_add(n, Ordering::Release);
    PCM_WAKER.wake();
}

struct PcmOut {
    written: u32,
    fill: usize,
    running: bool,
    crc: u32,
}

impl PcmOut {
    fn new() -> Self {
        Self { written: 0, fill: 0, running: false, crc: 0xFFFF_FFFF }
    }

    fn slot(&self) -> *mut u32 {
        let half = (self.written % 2) as usize;
        (PCM_ADDR as *mut u32).wrapping_add(half * HALF + self.fill)
    }

    async fn push(&mut self, frame: u32) {
        // SAFETY: inside the PCM ring in SRAM3; the DMA reads the other half.
        unsafe { core::ptr::write_volatile(self.slot(), frame) };
        if CRC_ON.load(Ordering::Relaxed) {
            self.crc = crc32_update(self.crc, &frame.to_le_bytes());
        }
        self.fill += 1;
        if self.fill == HALF {
            self.fill = 0;
            self.written += 1;
            if !self.running && self.written == 2 {
                self.start();
            }
            // Wait while both halves hold unplayed data.
            WRITTEN.store(self.written, Ordering::Relaxed);
            if self.running {
                mark(3);
                core::future::poll_fn(|cx| {
                    PCM_WAKER.register(cx.waker());
                    let played = PLAYED.load(Ordering::Acquire);
                    // Signed: after an underrun `played` is ahead of
                    // `written`, which must count as room (it is accounted
                    // for below), not as a wrapped huge backlog.
                    if (self.written.wrapping_sub(played) as i32) < 2 || STOP.load(Ordering::Relaxed) {
                        core::task::Poll::Ready(())
                    } else {
                        core::task::Poll::Pending
                    }
                })
                .await;
                let played = PLAYED.load(Ordering::Acquire);
                if played > self.written {
                    UNDERRUNS.fetch_add(played - self.written, Ordering::Relaxed);
                    // Re-sync one half ahead of the DMA: the next push then
                    // waits for a DMA interrupt, so a decoder that cannot
                    // keep up still yields (otherwise the RT executor never
                    // returns, thread mode starves and the watchdog fires).
                    self.written = played + 1;
                }
            }
        }
    }

    fn start(&mut self) {
        PLAYED.store(0, Ordering::Release);
        // SAFETY: channel 5 belongs to this module.
        let ch = unsafe { DmaCh::steal(DAC_CH) };
        ch.start(dac::DMA_MODE_S16 | dma::mode::RELOAD, PCM_ADDR, dac::FIFO0_ADDR, (PCM_FRAMES * 4) as u32);
        self.running = true;
    }

    async fn finish(&mut self) {
        // Pad the current half with silence and let it play out.
        while self.fill != 0 {
            self.push(0).await;
        }
        for _ in 0..2 {
            for _ in 0..HALF {
                self.push(0).await;
            }
        }
        self.stop();
    }

    fn stop(&mut self) {
        // SAFETY: channel 5 belongs to this module.
        unsafe { DmaCh::steal(DAC_CH) }.abort();
        self.running = false;
        self.written = 0;
        self.fill = 0;
    }
}

// ------------------------------------------------------------------ player

/// Largest measured need: Opus stereo 59,488 B, Vorbis q10 55,140 B.
const ARENA: usize = 64 * 1024;
/// Arena bytes kept free of the decoder for file I/O while it plays (the
/// text window, the player state image).
const SPARE: usize = 2048;
#[repr(C, align(8))]
struct ArenaBuf([u8; ARENA]);
static mut ARENA_BUF: ArenaBuf = ArenaBuf([0; ARENA]);

/// The card reader's transfer buffer: the first 16 KiB of the decoder
/// arena. Only while the card is with the USB task, when this task is
/// parked in `CARD_BACK.receive()` and does not touch the arena.
///
/// # Safety
/// Call only between receiving the card from `CARD_OUT` and sending it
/// back on `CARD_BACK`, and drop the borrow before sending it back.
pub unsafe fn usb_buffer() -> &'static mut [u8; 16384] {
    // SAFETY: see above; the arena is 72 KiB, 8-byte aligned.
    unsafe { &mut *((&raw mut ARENA_BUF.0).cast::<[u8; 16384]>()) }
}
static mut CACHE: [CacheSlot; 8] = [CacheSlot::EMPTY; 8];

/// The card travels between this task (mounted) and the USB task (card
/// reader): released on request, handed back when the host lets go.
pub static CARD_OUT: Channel<CriticalSectionRawMutex, Card, 1> = Channel::new();
pub static CARD_BACK: Channel<CriticalSectionRawMutex, Card, 1> = Channel::new();

async fn mount(card: Card) -> Option<Volume<'static, Card>> {
    let opts = MountOptions { read_only: false, write_prefix: Some("/.yoton"), partition: None };
    // SAFETY: exactly one Volume exists at a time; the previous one was
    // unmounted (consumed) before this borrow is created.
    let cache = unsafe { &mut *(&raw mut CACHE) };
    Volume::mount(card, cache, opts).await.ok()
}

fn ext_of(path: &str) -> Option<&[u8]> {
    path.rsplit_once('.').map(|(_, e)| e.as_bytes())
}

/// Where a file starts, and whether the player drives it (`track`: the
/// file's identity; `None` for the developer `play PATH`).
#[derive(Clone, Copy)]
struct Start {
    frame: u64,
    paused: bool,
    track: Option<TrackRef>,
}

/// How a file's playback ended.
enum End {
    /// Open this next (the player's answer to the end, a skip, a failure).
    Open(OpenReq),
    /// Stopped, or the queue ended.
    Idle,
    /// A request for the idle loop is waiting (`Server::pending` or
    /// `pending_play`: card hand-over, power-off, a new queue).
    Pending,
}

/// The playback loop's next step after the player answered.
fn after(f: Flow) -> End {
    f.open.map_or(End::Idle, End::Open)
}

async fn play_path(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    path: &str,
    srv: &mut Server,
) -> Result<End, &'static [u8]> {
    let f = vol.open(path, OpenMode::Read).await.map_err(|_| &b"open failed"[..])?;
    play_file(vol, ring, arena, f, ext_of(path), srv, Start { frame: 0, paused: false, track: None }).await
}

fn codec_name(ext: Option<&[u8]>) -> &'static str {
    let mut e = [0u8; 4];
    if let Some(x) = ext {
        for (d, s) in e.iter_mut().zip(x) {
            *d = s.to_ascii_lowercase();
        }
    }
    match &e {
        b"mp3\0" => "MP3",
        b"mp2\0" => "MP2",
        b"flac" => "FLAC",
        b"wav\0" => "WAV",
        b"ogg\0" | b"oga\0" => "Vorbis",
        b"opus" => "Opus",
        b"spx\0" => "Speex",
        b"aif\0" | b"aiff" => "AIFF",
        _ => "?",
    }
}

/// Serve at most one media request from inside playback. False: a request
/// for the idle loop arrived (card hand-over, power-off); the file ends.
async fn serve_one(vol: &mut Volume<'static, Card>, srv: &mut Server, scratch: &mut [u8]) -> bool {
    match media::REQS.try_receive() {
        Ok(r) => srv.serve(vol, r, scratch, false).await,
        Err(_) => true,
    }
}

/// Seek the decoder to `frame`, refilling the ring from the new position.
/// Returns the frame it landed on.
async fn seek_to(
    dec: &mut yoton_codecs::AnyDecoder<'_>,
    ring: &mut Ring,
    vol: &mut Volume<'static, Card>,
    f: &mut File,
    frame: u64,
) -> Option<u64> {
    mark(9);
    // Bisection takes tens of steps; the bound only stops a broken stream.
    for _ in 0..4096 {
        match dec.seek(ring, frame) {
            Ok(SeekStep::Done(at)) => return Some(at),
            Ok(SeekStep::NeedData(_)) => {
                if (ring.eof && !ring.reseek) || (!ring.fill(vol, f).await && !ring.eof) {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// A decoder failure: the player moves on (driven), or the developer
/// command gets `why`.
fn failed(srv: &mut Server, driven: bool, why: &'static [u8]) -> Result<End, &'static [u8]> {
    if driven { Ok(after(srv.feed(Event::DecoderFailed))) } else { Err(why) }
}

#[allow(clippy::too_many_lines)]
async fn play_file(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    mut f: File,
    ext: Option<&[u8]>,
    srv: &mut Server,
    st: Start,
) -> Result<End, &'static [u8]> {
    mark(10);
    ring.reset(f.size());
    ring.fill(vol, &mut f).await;
    // Probe, skipping leading tags.
    let format = loop {
        match probe(ring, ext) {
            Probe::Found(fm) => break fm,
            Probe::Skip(n) => {
                if (n as usize) <= ring.available() {
                    ring.consume(n as usize);
                } else {
                    let target = ring.position() + n;
                    f.set_position(target);
                    ring.reset(f.size());
                    ring.rd = target;
                    ring.wr = target;
                    ring.fill(vol, &mut f).await;
                }
            }
            Probe::NeedData(_) if !ring.eof => {
                ring.fill(vol, &mut f).await;
            }
            _ => return Err(b"unknown format"),
        }
    };
    let hint = ContainerInfo { format, gapless: None, stream_len: None };
    let plan = loop {
        match yoton_codecs::inspect(format, ring, &hint) {
            Ok(p) => break p,
            Err(OpenError::NeedData(_)) if !ring.eof => {
                ring.fill(vol, &mut f).await;
            }
            Err(_) => return Err(b"unsupported stream"),
        }
    };
    if plan.arena + SPARE > arena.len() {
        return Err(b"arena too small");
    }
    // The decoder's part of the arena, and the rest for file reads and
    // writes while it plays (text window, player state).
    let (dec_mem, spare) = arena.split_at_mut(plan.arena);
    let (mut dec, info) =
        yoton_codecs::open(format, ring, dec_mem, &hint).map_err(|_| &b"open decoder failed"[..])?;
    if info.channels == 0 || info.channels > 2 {
        return Err(b"channels not supported");
    }
    // DSP chain: sets the DAC rate (88.2-192 kHz sources are decimated).
    // SAFETY: initialised at the start of the audio task, which is the only
    // user.
    let chain = unsafe { (*(&raw mut CHAIN)).assume_init_mut() };
    mark(6);
    let out_rate = chain
        .set_format(yoton_dsp::Format {
            rate: info.rate,
            channels: info.channels,
            kind: info.sample,
            bits: info.bits,
        })
        .map_err(|_| &b"rate not supported"[..])?;
    let mut sound_gen = media::SOUND_GEN.load(Ordering::Acquire);
    // Vorbis and Opus need High at the moment (paged decoders).
    let heavy = matches!(codec_name(ext), "Vorbis" | "Opus");
    CODEC_FLOOR.store(if heavy { 2 } else { 0 }, Ordering::Relaxed);
    dsp_clock_floor(sound_active());
    // Start inside the track (resume, bookmark): before any output.
    let mut pos = 0u64;
    if st.frame > 0 {
        match seek_to(&mut dec, ring, vol, &mut f, st.frame).await {
            Some(at) => pos = at,
            None => {
                CODEC_FLOOR.store(0, Ordering::Relaxed);
                dsp_clock_floor(false);
                return Err(b"seek failed");
            }
        }
    }
    mark(7);
    media::apply_sound(chain);
    mark(8);
    let dsp = DSP_ON.load(Ordering::Relaxed);
    dac::set_rate(out_rate);
    RATE.store(out_rate, Ordering::Relaxed);
    let stereo = info.channels == 2;
    let ch = if stereo { 2 } else { 1 };
    let mut out = PcmOut::new();
    let rate = u64::from(info.rate);
    let ms_of = move |frame: u64| (frame * 1000 / rate.max(1)) as u32;
    let size = f.size();
    media::player_update(|p| {
        p.source = if st.paused { Source::Paused } else { Source::Playing };
        p.position_ms = ms_of(pos);
        p.duration_ms = info.frames.map_or(0, ms_of);
        p.codec = codec_name(ext);
        p.rate = info.rate;
        p.channels = info.channels;
        p.bits = info.bits;
        p.size = size;
    });
    let driven = st.track.is_some();
    if let Some(track) = st.track {
        srv.feed(Event::Opened { track, rate: info.rate, frames: info.frames });
        // A track change: save while the DAC is not running yet.
        save_store(vol, srv, spare).await;
    }
    PLAYING.store(true, Ordering::Relaxed);
    STOP.store(false, Ordering::Relaxed);
    let t0 = time::now_us();
    let mut decode_us = 0u64;
    let mut frames = 0u32;
    let mut flow = Flow::default();
    let mut paused = st.paused;
    // The pause has been carried out (DAC stopped, state saved).
    let mut held = false;
    let mut reader_end = false;
    let mut last_tick = t0;
    // The decoder reached the end of the stream.
    let mut natural = false;
    let end = 'play: loop {
        // Player requests: one per block; while paused, wait for one.
        let got = if paused {
            match select3(media::PLAY.receive(), media::REQS.receive(), Timer::after_millis(200)).await {
                Either3::First(r) => Some(Ok(r)),
                Either3::Second(r) => Some(Err(r)),
                Either3::Third(()) => None,
            }
        } else {
            media::PLAY.try_receive().ok().map(Ok)
        };
        match got {
            Some(Ok(r)) if driven && !matches!(r, PlayReq::PlayDir { .. } | PlayReq::Shutdown) => {
                flow.merge(srv.play_req(r));
            }
            Some(Ok(r)) => {
                // A new queue, power-off, or any player request during
                // `play PATH`: for the idle loop.
                srv.pending_play = Some(r);
                break 'play Ok(End::Pending);
            }
            Some(Err(r)) => {
                // A card hand-over ends the file (for the idle loop).
                let go_on = srv.serve(vol, r, spare, false).await;
                if !go_on {
                    break 'play Ok(End::Pending);
                }
            }
            None => {}
        }
        if STOP.swap(false, Ordering::Relaxed) {
            if driven {
                srv.play_req(PlayReq::Stop);
            }
            break 'play Ok(End::Idle);
        }
        if !flow.is_empty() {
            let fl = core::mem::take(&mut flow);
            if let Some(o) = fl.open {
                break 'play Ok(End::Open(o));
            }
            if fl.stop {
                break 'play Ok(End::Idle);
            }
            if let Some(target) = fl.seek {
                // Stop the DAC for the seek: the bisection reads the card for
                // a while, which is not an underrun (and must not count
                // towards the too-slow abort).
                out.stop();
                match seek_to(&mut dec, ring, vol, &mut f, target).await {
                    Some(at) => {
                        pos = at;
                        reader_end = false;
                        media::player_update(|p| p.position_ms = ms_of(pos));
                    }
                    // The decoder's state is unknown after a failed seek.
                    None => break 'play failed(srv, driven, b"seek failed"),
                }
            }
            if let Some(p) = fl.pause {
                paused = p;
            }
        }
        if paused {
            if !held {
                held = true;
                out.stop();
                media::player_update(|p| p.source = Source::Paused);
                save_store(vol, srv, spare).await;
            }
            continue;
        }
        if held {
            held = false;
            last_tick = time::now_us();
            media::player_update(|p| p.source = Source::Playing);
        }
        if UNDERRUNS.load(Ordering::Relaxed) >= MAX_UNDERRUNS {
            // The decoder cannot keep up (Opus on paged code): stop the
            // track rather than starve the rest of the system.
            break 'play failed(srv, driven, b"decoder too slow for this player");
        }
        let d0 = time::now_us();
        mark(4);
        let step = dec.decode(ring);
        let dt = time::now_us() - d0;
        decode_us += dt;
        T_DECODE.fetch_add(dt as u32, Ordering::Relaxed);
        match step {
            Ok(Step::Pcm(block)) => {
                let g = media::SOUND_GEN.load(Ordering::Acquire);
                if g != sound_gen {
                    sound_gen = g;
                    dsp_clock_floor(sound_active());
                    mark(7);
                    media::apply_sound(chain);
                }
                // The DSP works on a copy: the decoder's output buffer may
                // be decoder state (processing it in place corrupted the MP3
                // decoder on the player, 2026-11-24).
                let piece = SCRATCH_FRAMES * ch;
                let samples = match block {
                    PcmBlock::I16(s) => {
                        for part in s.chunks(piece) {
                            let sc = scratch16(part.len());
                            sc.copy_from_slice(part);
                            mark(5);
                            let n = if dsp { chain.process(&mut PcmBlock::I16(&mut *sc)) } else { sc.len() };
                            mark(0);
                            for fr in sc[..n.min(sc.len())].chunks_exact(ch) {
                                let l = fr[0] as u16;
                                let r = if stereo { fr[1] as u16 } else { l };
                                out.push(u32::from(l) | u32::from(r) << 16).await;
                                frames += 1;
                            }
                        }
                        s.len()
                    }
                    PcmBlock::I32(s) => {
                        for part in s.chunks(piece) {
                            let sc = scratch32(part.len());
                            sc.copy_from_slice(part);
                            mark(5);
                            let n = if dsp { chain.process(&mut PcmBlock::I32(&mut *sc)) } else { sc.len() };
                            mark(0);
                            for fr in sc[..n.min(sc.len())].chunks_exact(ch) {
                                let l = (fr[0] >> 16) as u16;
                                let r = if stereo { (fr[1] >> 16) as u16 } else { l };
                                out.push(u32::from(l) | u32::from(r) << 16).await;
                                frames += 1;
                            }
                        }
                        s.len()
                    }
                };
                // Source frames: the player's time line (the DSP may
                // decimate).
                pos += (samples / ch) as u64;
                FRAMES.store(frames, Ordering::Relaxed);
                media::player_update(|p| p.position_ms = ms_of(pos));
                if driven {
                    flow.merge(srv.feed(Event::Position { frame: pos }));
                    let now = time::now_us();
                    if now - last_tick >= 1_000_000 {
                        let ms = ((now - last_tick) / 1000) as u32;
                        last_tick = now;
                        flow.merge(srv.feed(Event::Tick { ms }));
                    }
                }
                if ring.available() < RING / 2 {
                    let a = time::now_us();
                    mark(1);
                    ring.fill(vol, &mut f).await;
                    let b = time::now_us();
                    mark(2);
                    let go_on = serve_one(vol, srv, spare).await;
                    mark(0);
                    T_FILL.fetch_add((b - a) as u32, Ordering::Relaxed);
                    T_SERVE.fetch_add((time::now_us() - b) as u32, Ordering::Relaxed);
                    if !go_on {
                        break 'play Ok(End::Pending);
                    }
                }
                if driven && ring.eof && !reader_end {
                    reader_end = true;
                    // The player answers with `Prefetch` (not carried out).
                    flow.merge(srv.feed(Event::ReaderAtEnd));
                }
            }
            Ok(Step::NeedData(_)) => {
                if ring.eof {
                    natural = true;
                    break 'play Ok(End::Idle);
                }
                if !ring.fill(vol, &mut f).await {
                    // The ring is full and the decoder still wants more:
                    // it can never be satisfied; end the track instead of
                    // spinning.
                    CORRUPT.fetch_add(1, Ordering::Relaxed);
                    break 'play failed(srv, driven, b"stream needs a larger ring");
                }
                if !serve_one(vol, srv, spare).await {
                    break 'play Ok(End::Pending);
                }
            }
            Ok(Step::EndOfStream) => {
                natural = true;
                break 'play Ok(End::Idle);
            }
            Err(_) => {
                CORRUPT.fetch_add(1, Ordering::Relaxed);
                if CORRUPT.load(Ordering::Relaxed) > 10 {
                    break 'play failed(srv, driven, b"too many corrupt frames");
                }
            }
        }
    };
    // At the natural end play out what is queued, then ask the player what
    // comes next.
    let end = if natural {
        out.finish().await;
        if driven {
            let fl = srv.feed(Event::DecoderEnded);
            if fl.open.is_none() && !fl.stop {
                srv.feed(Event::Drained);
            }
            Ok(after(fl))
        } else {
            end
        }
    } else {
        if out.running {
            out.stop();
        }
        end
    };
    CRC.store(!out.crc, Ordering::Relaxed);
    DECODE_US.store(decode_us as u32, Ordering::Relaxed);
    PLAY_US.store((time::now_us() - t0) as u32, Ordering::Relaxed);
    PLAYING.store(false, Ordering::Relaxed);
    CODEC_FLOOR.store(0, Ordering::Relaxed);
    dsp_clock_floor(false);
    end
}

async fn ls(vol: &mut Volume<'static, Card>, path: &str) {
    let Ok(mut cur) = vol.open_dir(path).await else {
        reply(|w| {
            w.s(b"no such directory");
        });
        return;
    };
    let mut e = DirEntry::new();
    let mut out = [0u8; 1024];
    let mut n = 0;
    let mut count = 0;
    while let Ok(true) = vol.read_dir(&mut cur, &mut e).await {
        count += 1;
        for &b in e.name().as_bytes().iter().chain(if e.is_dir() { &b"/\n"[..] } else { &b"\n"[..] }) {
            if n < out.len() {
                out[n] = b;
                n += 1;
            }
        }
    }
    reply(|w| {
        w.s(&out[..n]).s(b"(").u(count).s(b" entries)");
    });
}

/// Hand the card to the USB task and wait for it to come back; `None` if
/// the remount failed.
async fn release(vol: Volume<'static, Card>) -> Option<Volume<'static, Card>> {
    crate::state::CARD_READY.store(false, Ordering::Relaxed);
    media::player_update(|p| p.source = Source::Idle);
    let (card, _) = vol.unmount().await;
    CARD_OUT.send(card).await;
    let card = CARD_BACK.receive().await;
    let v = mount(card).await;
    if v.is_some() {
        crate::state::CARD_READY.store(true, Ordering::Relaxed);
    } else {
        reply(|w| {
            w.s(b"card: remount failed");
        });
    }
    v
}

/// Frames the DSP processes per piece, and the piece buffer: one buffer
/// seen as `i16` or `i32` samples (a block is one or the other).
const SCRATCH_FRAMES: usize = 256;
#[repr(C, align(4))]
struct Scratch([i32; SCRATCH_FRAMES * 2]);
static mut SCRATCH: Scratch = Scratch([0; SCRATCH_FRAMES * 2]);

/// The first `n` (≤ 512) `i16` samples of the scratch buffer.
fn scratch16(n: usize) -> &'static mut [i16] {
    // SAFETY: the audio task is the buffer's only user and holds at most
    // one view at a time (each is dropped before the next piece); 1024
    // `i16` fit in 512 aligned `i32`.
    unsafe { core::slice::from_raw_parts_mut((&raw mut SCRATCH.0).cast::<i16>(), n.min(SCRATCH_FRAMES * 4)) }
}

/// The first `n` (≤ 512) `i32` samples of the scratch buffer.
fn scratch32(n: usize) -> &'static mut [i32] {
    // SAFETY: as in `scratch16`.
    unsafe { &mut (&mut (*(&raw mut SCRATCH)).0)[..n.min(SCRATCH_FRAMES * 2)] }
}

/// DSP chain between decoder and DAC (AU-6), ~4 KiB; written once at the
/// start of the audio task, used only by it.
static mut CHAIN: core::mem::MaybeUninit<yoton_dsp::Chain> = core::mem::MaybeUninit::uninit();

/// Build the DSP chain. Called from `main` before the executors start: a
/// `Chain` is 4 KiB and built by value, which inside the audio task's
/// poll function would add to the stack frame the RT interrupt pushes on
/// top of whatever thread mode is using.
#[inline(never)]
pub fn init_chain() {
    // SAFETY: before any task runs; afterwards only the audio task uses it.
    unsafe { (*(&raw mut CHAIN)).write(yoton_dsp::Chain::new(yoton_dsp::OutWidth::Bits16, false)) };
}

static SERVER: ConstStaticCell<Server> = ConstStaticCell::new(Server::new());

// ------------------------------------------------------------ the player

/// Play what the player asked for, then whatever it asks for next, until
/// it stops (or a request for the idle loop arrives).
async fn run_player(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    first: OpenReq,
) {
    let mut next = Some(first);
    while let Some(o) = next.take() {
        for c in [&FRAMES, &UNDERRUNS, &CORRUPT] {
            c.store(0, Ordering::Relaxed);
        }
        STOP.store(false, Ordering::Relaxed);
        next = match play_item(vol, ring, arena, srv, o).await {
            End::Open(o) => Some(o),
            End::Idle | End::Pending => None,
        };
    }
    STOP.store(false, Ordering::Relaxed);
    media::player_update(|p| p.source = Source::Idle);
    save_store(vol, srv, arena).await;
}

/// Open and play one file for the player; a file that cannot be opened or
/// decoded is reported as `OpenFailed`.
async fn play_item(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    o: OpenReq,
) -> End {
    let Some((f, track, ext, n)) = open_target(vol, srv, o.target).await else {
        return after(srv.feed(Event::OpenFailed));
    };
    let ext = (n > 0).then_some(&ext[..n]);
    match play_file(
        vol,
        ring,
        arena,
        f,
        ext,
        srv,
        Start { frame: o.frame, paused: o.paused, track: Some(track) },
    )
    .await
    {
        Ok(end) => end,
        Err(_) => after(srv.feed(Event::OpenFailed)),
    }
}

/// Open the file of `t` (a track must still carry its name: the entry may
/// have been replaced) and publish its title. Returns the file, its
/// identity and its extension.
async fn open_target(
    vol: &mut Volume<'static, Card>,
    srv: &Server,
    t: Target,
) -> Option<(File, TrackRef, [u8; 8], usize)> {
    let l = match t {
        Target::Item(i) => *srv.queue[..srv.qlen as usize].get(i as usize)?,
        Target::Track(t) => t.locator(),
    };
    let mut e = DirEntry::new();
    vol.open_locator(l, &mut e).await.ok()?;
    if let Target::Track(t) = t
        && (t.serial != vol.serial() || !t.matches(e.name()))
    {
        return None;
    }
    let f = vol.open_entry(&e).ok()?;
    let name = e.name();
    let track = TrackRef::new(vol.serial(), l, name);
    let mut ext = [0u8; 8];
    let mut n = 0;
    if let Some((_, x)) = name.rsplit_once('.') {
        n = x.len().min(ext.len());
        ext[..n].copy_from_slice(&x.as_bytes()[..n]);
    }
    let title = media::Name::title(name);
    media::player_update(|p| {
        p.title = title;
        p.file = yoton_apps::services::FileRef { dir: l.dir, index: l.index };
    });
    Some((f, track, ext, n))
}

/// Open a file for writing under `/.yoton/`, creating the folder if needed.
async fn create(vol: &mut Volume<'static, Card>, path: &str, mode: OpenMode) -> Option<File> {
    match vol.open(path, mode).await {
        Ok(f) => Some(f),
        Err(_) => {
            let _ = vol.create_dir("/.yoton").await;
            vol.open(path, mode).await.ok()
        }
    }
}

/// Write the changed part of the player store to `/.yoton/player.bin`
/// (in place: each value has its own CRC, the resume slots alternate) and
/// flush the volume. Called only while no audio is being output (pause,
/// stop, track change, before a card hand-over or power-off): a card write
/// can take longer than the PCM ring holds.
async fn save_store(vol: &mut Volume<'static, Card>, srv: &mut Server, scratch: &mut [u8]) {
    let Some(range) = srv.store.dirty_range() else { return };
    let Some(img) = scratch.first_chunk_mut::<IMAGE_LEN>() else { return };
    let Some(pl) = srv.player.as_ref() else { return };
    srv.store.image(pl.bookmarks(), img);
    mark(11);
    let Some(mut f) = create(vol, media::PLAYER_FILE, OpenMode::Create).await else { return };
    let range = if f.size() < IMAGE_LEN as u64 { 0..IMAGE_LEN } else { range };
    f.set_position(range.start as u64);
    let wrote = vol.write(&mut f, &img[range]).await.is_ok();
    let ok = vol.close(f).await.is_ok() && wrote && vol.flush().await.is_ok();
    if ok {
        srv.store.clear_dirty();
    }
    mark(0);
}

/// Write the queue to `/.yoton/queue.bin` (from the idle decoder arena).
async fn save_queue(vol: &mut Volume<'static, Card>, arena: &mut [u8], srv: &Server) {
    let Some(n) = encode_queue(vol.serial(), &srv.queue[..srv.qlen as usize], arena) else { return };
    let Some(mut f) = create(vol, media::QUEUE_FILE, OpenMode::Truncate).await else { return };
    let _ = vol.write(&mut f, &arena[..n]).await;
    let _ = vol.close(f).await;
    let _ = vol.flush().await;
}

/// After mounting: the queue and the player store from the card (read
/// through the idle decoder arena), the player, the titles it shows.
async fn load_state(vol: &mut Volume<'static, Card>, arena: &mut [u8], srv: &mut Server) {
    if let Ok(mut f) = vol.open(media::QUEUE_FILE, OpenMode::Read).await {
        let want = queue_file_len(media::QUEUE).min(arena.len());
        let n = vol.read_exact_or_eof(&mut f, &mut arena[..want]).await.unwrap_or(0);
        if let Some(h) = decode_queue(&arena[..n], &mut srv.queue)
            && h.serial == vol.serial()
        {
            srv.qlen = h.len;
            srv.queue_id = h.id;
        }
    }
    let mut n = 0;
    if let Ok(mut f) = vol.open(media::PLAYER_FILE, OpenMode::Read).await {
        n = vol.read_exact_or_eof(&mut f, &mut arena[..IMAGE_LEN]).await.unwrap_or(0);
    }
    srv.init_player(&arena[..n]);
    srv.load_titles(vol).await;
}

/// Before the card goes to the USB task: the player stops (the card is
/// gone; Resume continues later) and its state is saved.
async fn hand_over(vol: &mut Volume<'static, Card>, srv: &mut Server, scratch: &mut [u8]) {
    srv.play_req(PlayReq::Cmd(Command::Stop));
    save_store(vol, srv, scratch).await;
    srv.card_leaving(vol).await;
}

// ------------------------------------------------------------ index build

/// Something for the idle loop is waiting.
fn work_waiting() -> bool {
    !CMDS.is_empty() || !media::REQS.is_empty() || !media::PLAY.is_empty()
}

/// Background index building while the card task is idle (AU-12): steps
/// of `media::BUILD_BUDGET` entries or rows on the end of the idle arena,
/// until a request arrives or the build ends, then the build is suspended
/// (its state stays in the server; playback may take the arena). Each step
/// spins the card (the RT executor does not yield inside it), so after
/// each one this task sleeps as long as the step took (at least 2 ms):
/// thread mode (UI, USB) keeps at least half of the CPU.
async fn build_slot(vol: &mut Volume<'static, Card>, srv: &mut Server, arena: &mut [u8]) {
    let mode = Mode::Write { alt: false };
    media::with_io(vol, mode, |io| srv.clean_orphan(io)).await;
    let mut h = Handles::new();
    // In this future, not on the stack (840 bytes).
    let mut e = DirEntry::new();
    let Some(b) = srv.resume_build(media::index_scratch(arena)) else { return };
    let mut b = Some(b);
    loop {
        if work_waiting() {
            break;
        }
        let t0 = time::now_us();
        let p = poll_fn(|cx| {
            let io = CardIo::new(vol, cx, &mut h, mode);
            Poll::Ready(b.as_mut().and_then(|b| srv.step_build(&io, b, &mut e)))
        })
        .await;
        match p {
            Some(Progress::Done(ix)) => {
                poll_fn(|cx| {
                    let io = CardIo::new(vol, cx, &mut h, mode);
                    let _ = io.close_all();
                    srv.build_done(&io, ix, &mut e);
                    let _ = io.close_all();
                    Poll::Ready(())
                })
                .await;
                return;
            }
            Some(_) => {}
            None => {
                poll_fn(|cx| {
                    let _ = CardIo::new(vol, cx, &mut h, mode).close_all();
                    Poll::Ready(())
                })
                .await;
                srv.abandon_build();
                return;
            }
        }
        let dt = time::now_us() - t0;
        let _ = select(
            Timer::after_micros(dt.clamp(2_000, 100_000)),
            select3(CMDS.ready_to_receive(), media::REQS.ready_to_receive(), media::PLAY.ready_to_receive()),
        )
        .await;
    }
    poll_fn(|cx| {
        let io = CardIo::new(vol, cx, &mut h, mode);
        if let Some(b) = b.take() {
            srv.suspend_build(&io, b);
        }
        let _ = io.close_all();
        Poll::Ready(())
    })
    .await;
}

/// A player request from the idle loop.
async fn player_request(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    r: PlayReq,
) {
    let fl = match r {
        PlayReq::Shutdown => {
            // The resume record with the position, then the volume: before
            // `SHUT_DOWN` lets the power go.
            srv.feed(Event::Shutdown);
            save_store(vol, srv, arena).await;
            let _ = vol.flush().await;
            media::SHUT_DOWN.store(true, Ordering::Release);
            return;
        }
        PlayReq::PlayDir { entry, start, shuffle } => {
            let s = srv.build_queue(vol, entry, start).await;
            save_queue(vol, arena, srv).await;
            srv.start_queue(s, shuffle, time::now_us() as u32)
        }
        r => srv.play_req(r),
    };
    match fl.open {
        Some(o) => run_player(vol, ring, arena, srv, o).await,
        None => save_store(vol, srv, arena).await,
    }
}

// ------------------------------------------------------------ upload

/// Length of the uploaded chunk waiting in the arena (0 = free). The chunk
/// itself goes to the first 4 KiB of the decoder arena, which is idle while
/// the card task runs the upload loop (and only then is `UPLOADING` set).
static PUT_LEN: AtomicU32 = AtomicU32::new(0);
static UPLOADING: AtomicBool = AtomicBool::new(false);
static PUT_SIGNAL: embassy_sync::signal::Signal<CriticalSectionRawMutex, ()> =
    embassy_sync::signal::Signal::new();

/// From the developer channel: queue a chunk for the open upload.
pub fn put_chunk(data: &[u8]) -> bool {
    if data.len() > 4096 || !UPLOADING.load(Ordering::Acquire) || PUT_LEN.load(Ordering::Acquire) != 0 {
        return false;
    }
    // SAFETY: while UPLOADING is set the card task does not touch the arena
    // except to copy a chunk out after PUT_LEN says one is there; PUT_LEN 0
    // means it is done with the previous one.
    unsafe { (&mut (*(&raw mut ARENA_BUF)).0)[..data.len()].copy_from_slice(data) };
    PUT_LEN.store(data.len() as u32, Ordering::Release);
    PUT_SIGNAL.signal(());
    true
}

/// Upload loop (`fcreate PATH`): appends chunks until `fclose` arrives as
/// a command; writes only where the volume allows (`/.yoton/`).
async fn upload(vol: &mut Volume<'static, Card>, arena: &mut [u8], path: &str) {
    let mut f = match vol.open(path, OpenMode::Truncate).await {
        Ok(f) => f,
        Err(_) => {
            reply(|w| {
                w.s(b"fcreate failed");
            });
            return;
        }
    };
    let mut total = 0u64;
    PUT_LEN.store(0, Ordering::Release);
    UPLOADING.store(true, Ordering::Release);
    reply(|w| {
        w.s(b"put 0");
    });
    loop {
        match select(PUT_SIGNAL.wait(), CMDS.receive()).await {
            Either::First(()) => {
                let n = PUT_LEN.load(Ordering::Acquire) as usize;
                if n > 0 {
                    let r = vol.write(&mut f, &arena[..n]).await;
                    PUT_LEN.store(0, Ordering::Release);
                    if r.is_err() {
                        UPLOADING.store(false, Ordering::Release);
                        reply(|w| {
                            w.s(b"put write failed");
                        });
                        return;
                    }
                    total += n as u64;
                    reply(|w| {
                        w.s(b"put ").u(total);
                    });
                }
            }
            Either::Second(c) => {
                if &c.text[..c.len] == b"fclose" {
                    UPLOADING.store(false, Ordering::Release);
                    let ok = vol.close(f).await.is_ok() && vol.flush().await.is_ok();
                    reply(|w| {
                        w.s(if ok { b"closed " } else { b"close failed " }).u(total);
                    });
                    return;
                }
            }
        }
    }
}

/// Write the playback counters as the reply (answers at once, from any
/// task).
pub fn status_reply() {
    reply(|w| {
        w.s(if PLAYING.load(Ordering::Relaxed) { b"playing" } else { b"idle" })
            .s(b" frames=")
            .u(u64::from(FRAMES.load(Ordering::Relaxed)))
            .s(b" rate=")
            .u(u64::from(RATE.load(Ordering::Relaxed)))
            .s(b" underruns=")
            .u(u64::from(UNDERRUNS.load(Ordering::Relaxed)))
            .s(b" corrupt=")
            .u(u64::from(CORRUPT.load(Ordering::Relaxed)))
            .s(b" where=")
            .u(u64::from(WHERE.load(Ordering::Relaxed)))
            .s(b" written=")
            .u(u64::from(WRITTEN.load(Ordering::Relaxed)))
            .s(b" played=")
            .u(u64::from(PLAYED.load(Ordering::Relaxed)));
    });
}

/// Reply for card commands while the card is with the USB card reader.
pub fn busy_reply() {
    reply(|w| {
        w.s(b"card: in use by USB");
    });
}

/// Stop the current track (from another task).
pub fn request_stop() {
    STOP.store(true, Ordering::Relaxed);
    PCM_WAKER.wake();
}

#[embassy_executor::task]
pub async fn audio() {
    // SAFETY: DMA channel 5 is the DAC's.
    unsafe { DmaCh::steal(DAC_CH) }.enable_irq_with(on_dac_dma, Level::Deadline);
    dac::power_up(44100, dac::VOL_0DB - 40);
    let card = match Card::open().await {
        Ok(c) => c,
        Err(_) => {
            reply(|w| {
                w.s(b"card: init failed");
            });
            return;
        }
    };
    let mut vol = match mount(card).await {
        Some(v) => v,
        None => {
            reply(|w| {
                w.s(b"card: mount failed");
            });
            return;
        }
    };
    crate::state::CARD_READY.store(true, Ordering::Relaxed);
    reply(|w| {
        w.s(b"card: mounted");
    });
    let mut ring = Ring { buf: &mut RING_BUF.take().0, rd: 0, wr: 0, len: None, eof: true, reseek: false };
    // SAFETY: this task is the arena's owner; the USB task borrows it only
    // while this task waits for the card (`usb_buffer`).
    let arena: &mut [u8] = unsafe { &mut (*(&raw mut ARENA_BUF)).0 };
    let srv = SERVER.take();
    load_state(&mut vol, arena, srv).await;
    loop {
        // A request left over from playback, else wait for one.
        let r = if let Some(p) = srv.pending_play.take() {
            Either3::Third(p)
        } else if let Some(r) = srv.pending.take() {
            Either3::Second(r)
        } else {
            if srv.wants_build() && !work_waiting() {
                build_slot(&mut vol, srv, arena).await;
            }
            select3(CMDS.receive(), media::REQS.receive(), media::PLAY.receive()).await
        };
        let cmd = match r {
            Either3::First(c) => c,
            Either3::Second(r) => {
                if srv.serve(&mut vol, r, arena, true).await {
                    continue;
                }
                if let Some(Req::Release) = srv.pending.take() {
                    hand_over(&mut vol, srv, arena).await;
                    match release(vol).await {
                        Some(v) => vol = v,
                        None => return,
                    }
                }
                continue;
            }
            Either3::Third(p) => {
                player_request(&mut vol, &mut ring, arena, srv, p).await;
                continue;
            }
        };
        let text = core::str::from_utf8(&cmd.text[..cmd.len]).unwrap_or("");
        if text == "release" {
            hand_over(&mut vol, srv, arena).await;
            match release(vol).await {
                Some(v) => vol = v,
                None => return,
            }
        } else if let Some(p) = text.strip_prefix("ls ") {
            ls(&mut vol, p.trim()).await;
        } else if let Some(p) = text.strip_prefix("play ") {
            for s in [&FRAMES, &UNDERRUNS, &CORRUPT] {
                s.store(0, Ordering::Relaxed);
            }
            let r =
                select(play_path(&mut vol, &mut ring, arena, p.trim(), srv), core::future::pending::<()>())
                    .await;
            match r {
                Either::First(Ok(_)) => reply(|w| {
                    w.s(b"played frames=")
                        .u(u64::from(FRAMES.load(Ordering::Relaxed)))
                        .s(b" rate=")
                        .u(u64::from(RATE.load(Ordering::Relaxed)))
                        .s(b" underruns=")
                        .u(u64::from(UNDERRUNS.load(Ordering::Relaxed)))
                        .s(b" corrupt=")
                        .u(u64::from(CORRUPT.load(Ordering::Relaxed)))
                        .s(b" decode_us=")
                        .u(u64::from(DECODE_US.load(Ordering::Relaxed)))
                        .s(b" play_us=")
                        .u(u64::from(PLAY_US.load(Ordering::Relaxed)))
                        .s(b" crc=")
                        .u(u64::from(CRC.load(Ordering::Relaxed)));
                }),
                Either::First(Err(e)) => reply(|w| {
                    w.s(b"error: ").s(e);
                }),
                Either::Second(()) => {}
            }
        } else if text == "dspt" {
            // Test: the real chain on synthetic blocks, no decoder.
            // SAFETY: initialised in main; this task is its only user.
            let chain = unsafe { (*(&raw mut CHAIN)).assume_init_mut() };
            mark(50);
            let r = chain.set_format(yoton_dsp::Format {
                rate: 44_100,
                channels: 2,
                kind: yoton_audio::SampleKind::I16,
                bits: 16,
            });
            mark(51);
            media::apply_sound(chain);
            mark(52);
            // SAFETY: i16 has no invalid bit patterns; the arena is idle.
            let (pre, buf, _) = unsafe { arena[..2304 * 2].align_to_mut::<i16>() };
            let ok = pre.is_empty() && r.is_ok();
            let t0 = time::now_us();
            let mut blocks = 0u64;
            let mut total = 0usize;
            for k in 0..20u32 {
                for (i, v) in buf.iter_mut().enumerate() {
                    *v = ((((i as u32 + k * 7) % 200) as i32 - 100) * 150) as i16;
                }
                mark(60 + k);
                total += chain.process(&mut PcmBlock::I16(buf));
                blocks += 1;
            }
            let us = time::now_us() - t0;
            reply(|w| {
                w.s(if ok { b"dspt ok" } else { b"dspt setup failed" })
                    .s(b" blocks=")
                    .u(blocks)
                    .s(b" samples=")
                    .u(total as u64)
                    .s(b" us=")
                    .u(us);
            });
        } else if let Some(p) = text.strip_prefix("fcreate ") {
            let mut pb = [0u8; CMD_LEN];
            let p = p.trim();
            pb[..p.len()].copy_from_slice(p.as_bytes());
            let path = core::str::from_utf8(&pb[..p.len()]).unwrap_or("");
            // Parents one by one (only `/.yoton/…` is writable).
            for (i, _) in path.match_indices('/').skip(1) {
                let _ = vol.create_dir(&path[..i]).await;
            }
            upload(&mut vol, arena, path).await;
        } else if text == "fp" {
            // Test: f32 work through paged helpers inside the RT executor.
            let t0 = time::now_us();
            let mut acc = 0f32;
            for i in 0..2000u32 {
                mark(40);
                acc += yoton_dsp::math::exp2(core::hint::black_box(i as f32 * 0.001));
                acc += yoton_dsp::math::log2(core::hint::black_box(1.0 + i as f32));
            }
            let us = time::now_us() - t0;
            reply(|w| {
                w.s(b"fp ok us=").u(us).s(b" acc=").u(acc as u64);
            });
        } else if let Some(v) = text.strip_prefix("vol ") {
            let v: u8 = v.trim().parse().unwrap_or(0);
            dac::set_volume(v, v);
            reply(|w| {
                w.s(b"ok");
            });
        } else {
            reply(|w| {
                w.s(b"commands: ls PATH, play PATH, vol N");
            });
        }
    }
}
