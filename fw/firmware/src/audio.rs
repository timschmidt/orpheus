//! Playback path (docs/plan/04-audio.md §2, WP AU-1 device part).
//!
//! ```text
//! card ─► stream ring (32 KiB) ─► decoder (arena 72 KiB) ─► PCM ring (12 KiB, SRAM3) ─► DMA ch5 ─► DAC
//! ```
//!
//! One task on the RT executor reads, decodes and fills the PCM ring; the
//! DAC DMA runs in reload mode over the ring and its half/complete
//! interrupts say which half is free. Commands arrive from the developer
//! channel (`ls PATH`, `play PATH`, `stop`) until the Music app exists.

use core::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use atj2157_hal::dma::{self, Channel as DmaCh};
use atj2157_hal::interrupt::Level;
use atj2157_hal::{dac, time};
use embassy_futures::select::{Either, select};
use embassy_sync::blocking_mutex::raw::CriticalSectionRawMutex;
use embassy_sync::channel::Channel;
use embassy_sync::waitqueue::AtomicWaker;
use static_cell::ConstStaticCell;
use yoton_audio::{ByteSource, ContainerInfo, OpenError, PcmBlock, Probe, Step, probe};
use yoton_bsp::card::Card;
use yoton_fs::{CacheSlot, DirEntry, File, MountOptions, OpenMode, Volume};
use yoton_proto::hash::crc32_update;
use yoton_ui::event::Source;

use crate::media::{self, Req, Server};

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
/// Live RT-side timing (µs, cumulative since start): decode, card reads,
/// media requests served from inside playback, PCM ring waits.
pub static T_DECODE: AtomicU32 = AtomicU32::new(0);
pub static T_FILL: AtomicU32 = AtomicU32::new(0);
pub static T_SERVE: AtomicU32 = AtomicU32::new(0);
static STOP: AtomicBool = AtomicBool::new(false);
/// Pause request from the UI (the DAC DMA stops; play resumes from the
/// next decoded frame).
pub static PAUSED: AtomicBool = AtomicBool::new(false);

// ------------------------------------------------------------ stream ring

const RING: usize = 32 * 1024;
#[repr(C, align(4))]
struct RingBuf([u8; RING]);
static RING_BUF: ConstStaticCell<RingBuf> = ConstStaticCell::new(RingBuf([0; RING]));

struct Ring {
    buf: &'static mut [u8; RING],
    rd: u64,
    wr: u64,
    len: Option<u64>,
    eof: bool,
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

impl Ring {
    fn reset(&mut self, len: u64) {
        self.rd = 0;
        self.wr = 0;
        self.len = Some(len);
        self.eof = false;
    }

    /// Read from the file into free space (sector-sized pieces where it can).
    async fn fill(&mut self, vol: &mut Volume<'static, Card>, f: &mut File) -> bool {
        let mut any = false;
        while !self.eof && RING - self.available() >= 4096 {
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
        self.crc = crc32_update(self.crc, &frame.to_le_bytes());
        self.fill += 1;
        if self.fill == HALF {
            self.fill = 0;
            self.written += 1;
            if !self.running && self.written == 2 {
                self.start();
            }
            // Wait while both halves hold unplayed data.
            if self.running {
                core::future::poll_fn(|cx| {
                    PCM_WAKER.register(cx.waker());
                    let played = PLAYED.load(Ordering::Acquire);
                    if self.written.wrapping_sub(played) < 2 || STOP.load(Ordering::Relaxed) {
                        core::task::Poll::Ready(())
                    } else {
                        core::task::Poll::Pending
                    }
                })
                .await;
                let played = PLAYED.load(Ordering::Acquire);
                if played > self.written {
                    UNDERRUNS.fetch_add(played - self.written, Ordering::Relaxed);
                    self.written = played;
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

const ARENA: usize = 72 * 1024;
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

async fn play_path(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    path: &str,
    srv: &mut Server,
) -> Result<(), &'static [u8]> {
    let f = vol.open(path, OpenMode::Read).await.map_err(|_| &b"open failed"[..])?;
    play_file(vol, ring, arena, f, ext_of(path), srv).await
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
        b"aif\0" | b"aiff" => "AIFF",
        _ => "?",
    }
}

/// Serve at most one media request from inside playback. Requests for the
/// player loop (skip, stop, new queue) end the current track.
async fn serve_one(vol: &mut Volume<'static, Card>, srv: &mut Server) {
    if let Ok(r) = media::REQS.try_receive()
        && !srv.serve(vol, r).await
    {
        STOP.store(true, Ordering::Relaxed);
    }
}

async fn play_file(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    mut f: File,
    ext: Option<&[u8]>,
    srv: &mut Server,
) -> Result<(), &'static [u8]> {
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
    if plan.arena > arena.len() {
        return Err(b"arena too small");
    }
    let (mut dec, info) = yoton_codecs::open(format, ring, &mut arena[..plan.arena], &hint)
        .map_err(|_| &b"open decoder failed"[..])?;
    if !dac::RATES.contains(&info.rate) || info.channels == 0 || info.channels > 2 {
        return Err(b"rate or channels not supported");
    }
    dac::set_rate(info.rate);
    RATE.store(info.rate, Ordering::Relaxed);
    let stereo = info.channels == 2;
    let mut out = PcmOut::new();
    let rate = u64::from(info.rate);
    media::player_update(|p| {
        p.source = Source::Playing;
        p.position_ms = 0;
        p.duration_ms = info.frames.map_or(0, |n| (n * 1000 / rate) as u32);
        p.codec = codec_name(ext);
        p.rate = info.rate;
        p.channels = info.channels;
        p.bits = info.bits;
        p.size = f.size();
    });
    PLAYING.store(true, Ordering::Relaxed);
    STOP.store(false, Ordering::Relaxed);
    let t0 = time::now_us();
    let mut decode_us = 0u64;
    let mut frames = 0u32;
    loop {
        if STOP.load(Ordering::Relaxed) {
            break;
        }
        if PAUSED.load(Ordering::Relaxed) {
            out.stop();
            media::player_update(|p| p.source = Source::Paused);
            while PAUSED.load(Ordering::Relaxed) && !STOP.load(Ordering::Relaxed) {
                if let Ok(r) = media::REQS.try_receive() {
                    if !srv.serve(vol, r).await {
                        STOP.store(true, Ordering::Relaxed);
                    }
                } else {
                    embassy_time::Timer::after_millis(20).await;
                }
            }
            media::player_update(|p| p.source = Source::Playing);
            continue;
        }
        let d0 = time::now_us();
        let step = dec.decode(ring);
        let dt = time::now_us() - d0;
        decode_us += dt;
        T_DECODE.fetch_add(dt as u32, Ordering::Relaxed);
        match step {
            Ok(Step::Pcm(block)) => {
                // Copy out first: the block borrows the decoder.
                match block {
                    PcmBlock::I16(s) => {
                        let ch = if stereo { 2 } else { 1 };
                        for fr in s.chunks_exact(ch) {
                            let l = fr[0] as u16;
                            let r = if stereo { fr[1] as u16 } else { l };
                            out.push(u32::from(l) | u32::from(r) << 16).await;
                            frames += 1;
                        }
                    }
                    PcmBlock::I32(s) => {
                        let ch = if stereo { 2 } else { 1 };
                        for fr in s.chunks_exact(ch) {
                            let l = (fr[0] >> 16) as u16;
                            let r = if stereo { (fr[1] >> 16) as u16 } else { l };
                            out.push(u32::from(l) | u32::from(r) << 16).await;
                            frames += 1;
                        }
                    }
                }
                FRAMES.store(frames, Ordering::Relaxed);
                media::player_update(|p| p.position_ms = (u64::from(frames) * 1000 / rate) as u32);
                if ring.available() < RING / 2 {
                    let a = time::now_us();
                    ring.fill(vol, &mut f).await;
                    let b = time::now_us();
                    serve_one(vol, srv).await;
                    T_FILL.fetch_add((b - a) as u32, Ordering::Relaxed);
                    T_SERVE.fetch_add((time::now_us() - b) as u32, Ordering::Relaxed);
                }
            }
            Ok(Step::NeedData(_)) => {
                if ring.eof {
                    break;
                }
                ring.fill(vol, &mut f).await;
                serve_one(vol, srv).await;
            }
            Ok(Step::EndOfStream) => break,
            Err(_) => {
                CORRUPT.fetch_add(1, Ordering::Relaxed);
                if CORRUPT.load(Ordering::Relaxed) > 10 {
                    break;
                }
            }
        }
    }
    out.finish().await;
    CRC.store(!out.crc, Ordering::Relaxed);
    DECODE_US.store(decode_us as u32, Ordering::Relaxed);
    PLAY_US.store((time::now_us() - t0) as u32, Ordering::Relaxed);
    PLAYING.store(false, Ordering::Relaxed);
    PAUSED.store(false, Ordering::Relaxed);
    Ok(())
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

static SERVER: ConstStaticCell<Server> = ConstStaticCell::new(Server::new());

/// Play the queue: `r` is the request that started it (new folder, skip,
/// jump). Returns when the queue ends or a stop is requested.
async fn run_queue(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    mut r: Req,
) {
    loop {
        match r {
            Req::PlayDir { entry, start } => srv.build_queue(vol, entry, start).await,
            Req::Skip(d) => {
                let p = i64::from(srv.qpos) + i64::from(d);
                if p < 0 || p >= i64::from(srv.qlen) {
                    break;
                }
                srv.qpos = p as u32;
            }
            Req::Jump(i) if i < srv.qlen => srv.qpos = i,
            _ => break,
        }
        let mut e = DirEntry::new();
        let Some(f) = srv.open_current(vol, &mut e).await else { break };
        let name = e.name();
        let ext = name.rsplit_once('.').map(|(_, x)| x.as_bytes());
        let mut ext_buf = [0u8; 8];
        let ext = ext.map(|x| {
            let n = x.len().min(8);
            ext_buf[..n].copy_from_slice(&x[..n]);
            &ext_buf[..n]
        });
        let title = media::Name::new(name.rsplit_once('.').map_or(name, |(t, _)| t));
        let (qpos, qlen, file) = (srv.qpos, srv.qlen, e.locator());
        media::player_update(|p| {
            p.title = title;
            p.queue_pos = qpos;
            p.queue_len = qlen;
            p.file = yoton_apps::services::FileRef { dir: file.dir, index: file.index };
        });
        for c in [&FRAMES, &UNDERRUNS, &CORRUPT] {
            c.store(0, Ordering::Relaxed);
        }
        STOP.store(false, Ordering::Relaxed);
        let _ = play_file(vol, ring, arena, f, ext, srv).await;
        r = match srv.pending.take() {
            Some(Req::Stop) => break,
            Some(p) => p,
            None => Req::Skip(1),
        };
    }
    STOP.store(false, Ordering::Relaxed);
    PAUSED.store(false, Ordering::Relaxed);
    media::player_update(|p| p.source = Source::Idle);
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
            .u(u64::from(CORRUPT.load(Ordering::Relaxed)));
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
    let mut ring = Ring { buf: &mut RING_BUF.take().0, rd: 0, wr: 0, len: None, eof: true };
    // SAFETY: this task is the arena's owner; the USB task borrows it only
    // while this task waits for the card (`usb_buffer`).
    let arena: &mut [u8] = unsafe { &mut (*(&raw mut ARENA_BUF)).0 };
    let srv = SERVER.take();
    loop {
        // A player request left over from playback, else wait for one.
        let r = match srv.pending.take() {
            Some(r) => Either::Second(r),
            None => select(CMDS.receive(), media::REQS.receive()).await,
        };
        let cmd = match r {
            Either::First(c) => c,
            Either::Second(r) => {
                if srv.serve(&mut vol, r).await {
                    continue;
                }
                let Some(r) = srv.pending.take() else { continue };
                run_queue(&mut vol, &mut ring, arena, srv, r).await;
                continue;
            }
        };
        let text = core::str::from_utf8(&cmd.text[..cmd.len]).unwrap_or("");
        if text == "release" {
            // Hand the card to the USB task, wait for it to come back.
            crate::state::CARD_READY.store(false, Ordering::Relaxed);
            let (card, _) = vol.unmount().await;
            CARD_OUT.send(card).await;
            let card = CARD_BACK.receive().await;
            match mount(card).await {
                Some(v) => {
                    vol = v;
                    crate::state::CARD_READY.store(true, Ordering::Relaxed);
                }
                None => {
                    reply(|w| {
                        w.s(b"card: remount failed");
                    });
                    return;
                }
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
                Either::First(Ok(())) => reply(|w| {
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
