//! Playback path (docs/plan/04-audio.md §2, WP AU-1 device part).
//!
//! ```text
//! card ─► stream ring (16 KiB) ─► decoder (arena 72 KiB) ─► PCM ring (12 KiB, SRAM3) ─► DMA ch5 ─► DAC
//! ```
//!
//! One task on the RT executor reads, decodes and fills the PCM ring; the
//! DAC DMA runs in reload mode over the ring and its half/complete
//! interrupts say which half is free. The same task owns the mounted card
//! and the playback service (`orpheus_player::Player`, WP AU-5): the UI's
//! player requests become player events, the decode loop feeds it what
//! happens (opened, position, reader at end, decoder ended or failed,
//! ticks) and carries out its actions (open, seek, pause, resume, stop).
//! Developer commands (`ls PATH`, `play PATH`, `stop`, …) bypass the
//! player.
//!
//! While idle, the task also builds directory indexes in the background
//! (`build_slot`, AU-12) and the music library (`crate::library::slot`)
//! on the end of the decoder arena.
//!
//! Gapless (04 §2.2): when the reader has read the current file to its
//! end the player asks for the next one (`Prefetch`): it is opened and the
//! reader goes on into it in the same stream ring
//! (`orpheus_audio::StreamRing`). When the decoder ends, the player answers
//! `Open { gapless }`: the PCM ring and the DAC keep running (no drain, no
//! silence), the ring hands the next stream over and the next decoder is
//! opened on its bytes in the same arena (`play_item`); the decoders trim
//! their own encoder delay and padding, so the join is sample-exact. A
//! different sample rate drains the PCM ring and restarts the DAC instead.
//! The clock is at its top level for the hand-over; the track's album art
//! (cache only) and library place follow between the next ring refills.
//! A track change the user makes (Next, a jump, a new queue) still stops
//! the output and starts fresh.
//!
//! Not yet: the fades of 04 §2.2 around pause and seek, periodic resume
//! saves while playing (a card write between blocks could underrun; the
//! state is saved at pause, stop, track change, card hand-over and
//! power-off instead; a gapless track change does not save).

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
use orpheus_audio::ring::{Space, Target as Part};
use orpheus_audio::{
    ByteSource, ContainerInfo, OpenError, PcmBlock, Probe, SeekSource, SeekStep, SourceError, Step,
    StreamRing, probe,
};
use orpheus_fs::{CacheSlot, DirEntry, File, MountOptions, OpenMode, Volume};
use orpheus_index::Progress;
use orpheus_index::card::{CardIo, Handles, Mode};
use orpheus_player::image::{IMAGE_LEN, decode_queue, encode_queue, queue_file_len};
use orpheus_player::{Command, Event, TrackRef};
use orpheus_proto::hash::crc32_update;
use orpheus_ui::event::Source;
use static_cell::ConstStaticCell;
use yoton_bsp::card::Card;

use crate::media::{self, Flow, OpenReq, PlayReq, Req, Server, Target};

// ---------------------------------------------------------------- commands

/// A byte string in a paged `static` (a plain literal would land in the
/// resident mergeable-string section).
macro_rules! t {
    ($s:literal) => {{
        static S: [u8; $s.len()] = *$s;
        let s: &'static [u8] = &S;
        s
    }};
}

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

pub(crate) fn reply(f: impl FnOnce(&mut Writer<'_>)) {
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

/// Clock floor High while the card task decodes a picture (no track is
/// loaded then; `play_file` sets the codec's floor when a track starts).
pub(crate) fn boost(on: bool) {
    CODEC_FLOOR.store(if on { 2 } else { 0 }, Ordering::Relaxed);
    dsp_clock_floor(false);
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
pub(crate) fn mark(v: u32) {
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
/// This track's decoder calls (`status` `dec:`): count, µs in them (sum,
/// largest) and pager line loads taken inside them — the decoder's own
/// paging, apart from the playback loop's (AU-8, Opus on the player).
static DEC_N: AtomicU32 = AtomicU32::new(0);
static DEC_US: AtomicU32 = AtomicU32::new(0);
static DEC_MAX_US: AtomicU32 = AtomicU32::new(0);
static DEC_LOADS: AtomicU32 = AtomicU32::new(0);
/// The last gapless track change (`status`, µs and counts): PCM frames
/// queued when the decoder ended (the headroom), time from there to the
/// next track's first block, pager line loads in that time, underruns of
/// the new track after its first two halves (the join's), time of the
/// library lookup and of the album-art lookup that follow, underruns after
/// them, time of the read-ahead open. Plus the start time and pager count
/// of the join in progress.
static JOIN: [AtomicU32; 13] = [const { AtomicU32::new(0) }; 13];
const J_HEAD: usize = 0;
const J_OPEN_US: usize = 1;
const J_LOADS: usize = 2;
const J_UNDER: usize = 3;
const J_LIB_US: usize = 4;
const J_ART_US: usize = 5;
const J_UNDER_META: usize = 6;
const J_PREFETCH_US: usize = 7;
const J_T0: usize = 8;
const J_L0: usize = 9;
const J_PREP_US: usize = 10;
const J_PREP_LOADS: usize = 11;
/// Joins since start-up (which one `status` shows).
const J_SEQ: usize = 12;
/// Not measured yet (`status` prints `-`).
const J_NONE: u32 = u32::MAX;

fn jset(i: usize, v: u32) {
    JOIN[i].store(v, Ordering::Relaxed);
}

fn jget(i: usize) -> u32 {
    JOIN[i].load(Ordering::Relaxed)
}

fn now32() -> u32 {
    time::now_us() as u32
}

fn pager_loads() -> u32 {
    atj2157_hal::pager::stats().loads as u32
}

/// Card work besides the current file's refill (UI requests, the
/// read-ahead open and reads, a gapless track's library place and art)
/// waits until the DAC has this many frames ahead of it (≈ 52 ms at
/// 44.1 kHz; `PcmOut::headroom`): each piece runs while nothing feeds the
/// DAC. After [`CHORE_WAIT_US`] a waiting piece runs anyway (a decoder
/// that never gets ahead must not starve the UI).
const ROOMY: i32 = (HALF + HALF / 2) as i32;
const CHORE_WAIT_US: u64 = 300_000;
/// A gapless track's library place and art are looked for after this much
/// of it has played (its first seconds stay free of card work).
const META_AFTER_MS: u64 = 2000;
/// The player is asked for the next track (`ReaderAtEnd`) when this much of
/// the current file is still to be read (≈ 2 s of FLAC, 11 s of MP3 at
/// 192 kbit/s; a shorter file at once, which for a queue's first track is
/// before the DAC starts): its open and preparation then have time to find
/// moments the DAC has room, well before the join. Its bytes still come
/// after the current file's.
const READ_AHEAD_LEAD: u64 = 256 * 1024;

/// Underruns of the current track by cause (`orpheus_audio::Ledger`,
/// reset with [`UNDERRUNS`]): every operation that keeps the writer from
/// the DAC is noted with its length; halves the writer finds missed go to
/// the longest one noted since its last half boundary, else to the decoder.
/// The causes add up to [`UNDERRUNS`] exactly.
static LEDGER: critical_section::Mutex<core::cell::RefCell<orpheus_audio::Ledger<8>>> =
    critical_section::Mutex::new(core::cell::RefCell::new(orpheus_audio::Ledger::new()));
const UB_JOIN: usize = 0;
const UB_FILL: usize = 1;
const UB_SERVE: usize = 2;
const UB_PREFETCH: usize = 3;
const UB_PREPARE: usize = 4;
const UB_LIB: usize = 5;
const UB_ART: usize = 6;
const UB_DECODER: usize = 7;

/// An operation of `cause` that started at `t0` (µs) has ended.
fn note(cause: usize, t0: u32) {
    let us = now32().wrapping_sub(t0);
    critical_section::with(|cs| LEDGER.borrow(cs).borrow_mut().note(cause, us));
}

/// Developer `stop`: ends the current file at once (also inside a PCM
/// wait).
static STOP: AtomicBool = AtomicBool::new(false);

// ------------------------------------------------------------ stream ring

/// 16 KiB: RAM does not allow `orpheus_audio::MAX_PEEK` (28 KiB) plus a
/// refill unit next to the resident audio path. Ogg pages up to ~15.5 KiB
/// play; a stream that needs more ends with an error (open, STATUS).
const RING: usize = 16 * 1024;
#[repr(C, align(4))]
struct RingBuf([u8; RING]);
static RING_BUF: ConstStaticCell<RingBuf> = ConstStaticCell::new(RingBuf([0; RING]));

/// The stream ring over the current file, and the next file read ahead
/// after it (gapless); also the picture decoders' source while no track is
/// loaded (`pictures`).
///
/// The files are kept here, not moved through the playback loop's frames:
/// a `File` is 64 bytes and the RT task's stack is what limits it.
pub(crate) struct Ring {
    s: StreamRing<'static>,
    /// The file played (`play_file`; the pictures pass their own).
    file: Option<File>,
    /// The queue item read ahead after it (`Action::Prefetch`), and its
    /// file.
    next: Option<Next>,
    next_file: Option<File>,
    /// The item handed over at the last gapless change, until its title is
    /// shown (`play_file`'s chores: not while the next decoder opens).
    shown: Option<Next>,
    /// Its stream as prepared (taken by `play_file`).
    prepared: Option<Prepared>,
}

pub(crate) struct Next {
    item: u32,
    track: TrackRef,
    ext: [u8; 8],
    ext_len: u8,
    title: media::Name,
    /// Probed and inspected while the current file still plays
    /// (`prepare`): `Some(Ok)` ready, `Some(Err)` gave up (the hand-over
    /// does it the ordinary way).
    prep: Option<Result<Prepared, ()>>,
}

/// A stream probed and inspected ahead: its format, the decoder's plan, and
/// where its codec data starts (after leading tags).
#[derive(Clone, Copy)]
pub(crate) struct Prepared {
    format: orpheus_audio::Format,
    plan: orpheus_audio::Plan,
    start: u64,
}

impl ByteSource for Ring {
    fn peek(&self, n: usize) -> Option<(&[u8], &[u8])> {
        self.s.peek(n)
    }
    fn available(&self) -> usize {
        self.s.available()
    }
    fn consume(&mut self, n: usize) {
        self.s.consume(n);
    }
    fn position(&self) -> u64 {
        self.s.position()
    }
    fn len(&self) -> Option<u64> {
        self.s.len()
    }
    fn at_end(&self) -> bool {
        self.s.at_end()
    }
}

/// A seek flushes the ring (and what was read of the next file); the
/// playback loop refills it from the new position (`Ring::fill`).
impl SeekSource for Ring {
    fn seek(&mut self, pos: u64) -> Result<(), SourceError> {
        self.s.seek(pos)
    }
}

impl Ring {
    /// A new current file of `len` bytes; a file read ahead is forgotten.
    pub(crate) fn reset(&mut self, len: u64) {
        self.s.reset(Some(len));
        self.cancel_next();
        self.shown = None;
        self.prepared = None;
    }

    /// The current file has been read to its end.
    pub(crate) fn eof(&self) -> bool {
        self.s.eof()
    }

    /// A seek moved the read position and nothing was read there yet.
    pub(crate) fn reseek(&self) -> bool {
        self.s.reseek_pending()
    }

    /// Forget the file read ahead.
    fn cancel_next(&mut self) {
        self.s.cancel_next();
        self.next = None;
        self.next_file = None;
    }

    /// The decoder of the current file ended and the player opens `t`:
    /// if that is the file read ahead, it becomes the current one (its
    /// bytes stay in the ring); its title waits in `shown` (published once
    /// the next decoder runs: the UI's repaint then does not compete with
    /// the open for the code cache). Returns its identity and extension.
    #[inline(never)]
    fn hand_over(&mut self, t: Target) -> Option<(TrackRef, [u8; 8], u8)> {
        let Target::Item(i) = t else { return None };
        if self.next.as_ref().is_none_or(|n| n.item != i) || self.next_file.is_none() || !self.s.hand_over() {
            return None;
        }
        self.file = self.next_file.take();
        self.shown = self.next.take();
        let n = self.shown.as_mut()?;
        self.prepared = n.prep.take().and_then(Result::ok);
        Some((n.track, n.ext, n.ext_len))
    }

    /// Publish the title kept by [`Ring::hand_over`].
    #[inline(never)]
    fn show_handed(&mut self) {
        if let Some(n) = self.shown.take() {
            publish_track(n.track, &n.title);
        }
    }

    /// Read into free space (sector-sized pieces where it can): the
    /// current file (`f`, else the ring's own) to its end, then the file
    /// read ahead.
    pub(crate) async fn fill(&mut self, vol: &mut Volume<'static, Card>, f: Option<&mut File>) -> bool {
        self.fill_in(vol, f, true).await
    }

    /// [`Ring::fill`]; the file read ahead only if `next` (held back while
    /// the DAC is short of data near the current file's end).
    async fn fill_in(
        &mut self,
        vol: &mut Volume<'static, Card>,
        mut f: Option<&mut File>,
        next: bool,
    ) -> bool {
        let mut any = false;
        loop {
            let Ring { s, file: own, next_file, .. } = self;
            let Some(Space { target, seek, buf }) = s.space_in(512, 16384, next) else { break };
            let file = match target {
                Part::Current => match (f.as_deref_mut(), own) {
                    (Some(f), _) | (None, Some(f)) => f,
                    (None, None) => break,
                },
                Part::Next => match next_file {
                    Some(n) => n,
                    None => break,
                },
            };
            if let Some(p) = seek {
                file.set_position(p);
            }
            match vol.read(file, buf).await {
                Ok(0) | Err(_) => s.end(target),
                Ok(n) => {
                    s.wrote(target, n);
                    any = true;
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
                let lost = played.saturating_sub(self.written);
                critical_section::with(|cs| {
                    let mut l = LEDGER.borrow(cs).borrow_mut();
                    if lost > 0 { l.missed(lost, UB_DECODER) } else { l.kept_up() }
                });
                if played > self.written {
                    UNDERRUNS.fetch_add(played - self.written, Ordering::Relaxed);
                    let now = (time::now_us() / 1000) as u32;
                    if now.wrapping_sub(LAST_SERVE_MS.load(Ordering::Relaxed)) > 500 {
                        SLOW_UNDERRUNS.fetch_add(played - self.written, Ordering::Relaxed);
                    }
                    // Re-sync one half ahead of the DMA: the next push then
                    // waits for a DMA interrupt, so a decoder that cannot
                    // keep up still yields (otherwise the RT executor never
                    // returns, thread mode starves and the watchdog fires).
                    self.written = played + 1;
                }
            }
        }
    }

    /// Frames the DAC has ahead of it: the written halves it has not
    /// finished, less what it has played of the current one (its DMA
    /// position), plus the half being filled. `i32::MAX` while it is not
    /// running (nothing has a deadline then); ≤ 0: it is playing stale data.
    fn headroom(&self) -> i32 {
        if !self.running {
            return i32::MAX;
        }
        const BYTES: u32 = (PCM_FRAMES * 4) as u32;
        // SAFETY: channel 5 belongs to this module; a register read.
        let rem = unsafe { DmaCh::steal(DAC_CH) }.remaining().min(BYTES);
        let at = ((BYTES - rem) / 4) as usize % HALF;
        let halves = self.written.wrapping_sub(PLAYED.load(Ordering::Acquire)) as i32;
        halves * HALF as i32 + self.fill as i32 - at as i32
    }

    fn start(&mut self) {
        PLAYED.store(0, Ordering::Release);
        // SAFETY: channel 5 belongs to this module.
        let ch = unsafe { DmaCh::steal(DAC_CH) };
        ch.start(dac::DMA_MODE_S16 | dma::mode::RELOAD, PCM_ADDR, dac::FIFO0_ADDR, (PCM_FRAMES * 4) as u32);
        self.running = true;
    }

    /// Pad the current half with silence, let everything play out, stop.
    async fn finish(&mut self) {
        if !self.running && self.written == 0 && self.fill == 0 {
            return;
        }
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

/// Largest measured need: Opus stereo 60,008 B (59,488 B + the 520-B PVQ
/// row, AU-8), Vorbis q10 55,140 B.
const ARENA: usize = 64 * 1024;
/// Arena bytes kept free of the decoder for file I/O while it plays (the
/// text window, the player state image).
const SPARE: usize = 2048;
/// The largest decoder, the spare bytes and the music library's build
/// state (the arena's last bytes, `crate::library::STATE`) fit.
const _: () = assert!(60_008 + SPARE + crate::library::STATE <= ARENA);
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
/// The path walk's directory cache (`/.orpheus/library`, the art cache…:
/// a walk from the root of a large card takes tens of sector reads).
static mut WALKS: orpheus_fs::WalkCache = orpheus_fs::WalkCache::EMPTY;

/// The card travels between this task (mounted) and the USB task (card
/// reader): released on request, handed back when the host lets go.
pub static CARD_OUT: Channel<CriticalSectionRawMutex, Card, 1> = Channel::new();
pub static CARD_BACK: Channel<CriticalSectionRawMutex, Card, 1> = Channel::new();

async fn mount(card: Card) -> Option<Volume<'static, Card>> {
    let opts = MountOptions { read_only: false, write_prefix: Some("/.orpheus"), partition: None };
    // SAFETY: exactly one Volume exists at a time; the previous one was
    // unmounted (consumed) before this borrow is created.
    let cache = unsafe { &mut *(&raw mut CACHE) };
    let mut vol = Volume::mount(card, cache, opts).await.ok()?;
    // SAFETY: as `CACHE`: lent to the one volume that exists.
    vol.set_walk_cache(unsafe { &mut *(&raw mut WALKS) });
    // The project was renamed (orpheus → Orpheus, 2026-11-24): its card
    // folder moves from `/.orpheus` to `/.orpheus` once, keeping the queue,
    // resume point, indexes and caches (owner's decision). Only this one
    // directory rename runs without the write prefix.
    if vol.exists("/.orpheus").await == Ok(true) && vol.exists("/.orpheus").await == Ok(false) {
        vol.set_write_prefix(None);
        let ok = vol.rename("/.orpheus", "/.orpheus").await.is_ok();
        vol.set_write_prefix(Some("/.orpheus"));
        let _ = vol.flush().await;
        atj2157_hal::log::line(if ok {
            b"card: /.orpheus moved to /.orpheus"
        } else {
            b"card: moving /.orpheus failed"
        });
    }
    Some(vol)
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
    /// The previous track's decoder ended and the ring holds this file
    /// already (handed over): the DAC is still running with the previous
    /// track's last samples.
    gapless: bool,
    /// The player has not been told that the previous track ended
    /// (`OpenReq::early`; the queue item started): told after this one's
    /// first block.
    early: Option<u32>,
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
    /// A gapless hand-over cannot go on (another sample rate): open the
    /// same item again the ordinary way.
    Again,
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
    ring.file = Some(vol.open(path, OpenMode::Read).await.map_err(|_| &b"open failed"[..])?);
    let mut out = PcmOut::new();
    let st = Start { frame: 0, paused: false, track: None, gapless: false, early: None };
    play_file(vol, ring, arena, ext_of(path), srv, &mut out, st).await
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
        Ok(r) => {
            let done = srv.serve(vol, r, scratch, false).await;
            LAST_SERVE_MS.store((time::now_us() / 1000) as u32, Ordering::Relaxed);
            done
        }
        Err(_) => true,
    }
}

/// When the card task last served a media request while playing (ms):
/// underruns right after one are caused by the request, not by a decoder
/// that cannot keep up, and do not count towards the too-slow abort.
static LAST_SERVE_MS: AtomicU32 = AtomicU32::new(0);
/// Underruns that count towards [`MAX_UNDERRUNS`].
static SLOW_UNDERRUNS: AtomicU32 = AtomicU32::new(0);

/// Seek the decoder to `frame`, refilling the ring (its own file) from the
/// new position. Returns the frame it landed on.
async fn seek_to(
    dec: &mut orpheus_codecs::AnyDecoder<'_>,
    ring: &mut Ring,
    vol: &mut Volume<'static, Card>,
    frame: u64,
) -> Option<u64> {
    mark(9);
    // Bisection takes tens of steps; the bound only stops a broken stream.
    for _ in 0..4096 {
        match dec.seek(ring, frame) {
            Ok(SeekStep::Done(at)) => return Some(at),
            Ok(SeekStep::NeedData(_)) => {
                if (ring.eof() && !ring.reseek()) || (!ring.fill(vol, None).await && !ring.eof()) {
                    return None;
                }
            }
            Err(_) => return None,
        }
    }
    None
}

/// The current track's decoder ended and the next one is handed over:
/// the join's statistics start, and its underruns are no decoder's.
fn handed_over(out: &PcmOut) {
    // A new join: what it has not measured yet shows `-`, not the last
    // join's numbers (the read-ahead and preparation are this join's).
    for i in [J_OPEN_US, J_LOADS, J_UNDER, J_LIB_US, J_ART_US, J_UNDER_META] {
        jset(i, J_NONE);
    }
    jset(J_SEQ, jget(J_SEQ).wrapping_add(1));
    jset(J_HEAD, out.headroom().max(0) as u32);
    jset(J_T0, now32());
    jset(J_L0, pager_loads());
    // Underruns while the next decoder opens are not the decoder's (no
    // too-slow abort).
    LAST_SERVE_MS.store((time::now_us() / 1000) as u32, Ordering::Relaxed);
}

/// A decoder failure: the player moves on (driven), or the developer
/// command gets `why`.
fn failed(srv: &mut Server, driven: bool, why: &'static [u8]) -> Result<End, &'static [u8]> {
    if driven { Ok(after(srv.feed(Event::DecoderFailed))) } else { Err(why) }
}

/// Read ahead into queue item `item` after the current file (the player's
/// `Prefetch`): open it now; the ring reads it once the current file is
/// read to its end. A file that cannot be opened is simply not read ahead
/// (the open at the track change fails and the player moves on).
async fn prefetch(vol: &mut Volume<'static, Card>, ring: &mut Ring, srv: &Server, item: u32) {
    ring.cancel_next();
    // For the next join.
    jset(J_PREP_US, J_NONE);
    jset(J_PREP_LOADS, J_NONE);
    let mut title = media::Name::EMPTY;
    let got = open_target(vol, srv, Target::Item(item), Some(&mut title), &mut ring.next_file).await;
    if let (Some((track, ext, ext_len)), Some(f)) = (got, ring.next_file.as_ref()) {
        ring.s.begin_next(Some(f.size()));
        ring.next = Some(Next { item, track, ext, ext_len, title, prep: None });
    }
}

/// What the head of a file shows (`inspect_head`).
enum HeadStep {
    Ready(Prepared),
    /// A leading tag ends beyond the window: read from this offset.
    SkipTo(u64),
    Fail,
}

/// Probe (skipping leading tags) and inspect a window of a file's head.
#[inline(never)]
fn inspect_head(src: &mut orpheus_audio::WindowSource<'_>, ext: Option<&[u8]>) -> HeadStep {
    for _ in 0..16 {
        match probe(&*src, ext) {
            Probe::Found(format) => {
                let hint = ContainerInfo { format, gapless: None, stream_len: None };
                let start = src.position();
                return match orpheus_codecs::inspect(format, &*src, &hint) {
                    Ok(plan) => HeadStep::Ready(Prepared { format, plan, start }),
                    Err(_) => HeadStep::Fail,
                };
            }
            Probe::Skip(k) if (k as usize) <= src.available() => src.consume(k as usize),
            Probe::Skip(k) => return HeadStep::SkipTo(src.position() + k),
            _ => return HeadStep::Fail,
        }
    }
    HeadStep::Fail
}

/// Probe and inspect the file read ahead from its own head (read into
/// `buf`, free arena memory; a copy of its handle, so the ring's reading is
/// untouched), while the current file plays: at the hand-over only the
/// decoder opens. Up to 8 KiB per read; a stream that needs more is left
/// to the hand-over.
async fn prepare(vol: &mut Volume<'static, Card>, ring: &mut Ring, buf: &mut [u8]) {
    let Some(mut f) = ring.next_file.clone() else { return };
    let Some(n) = ring.next.as_ref().filter(|n| n.prep.is_none()) else { return };
    let (ext, ext_len) = (n.ext, usize::from(n.ext_len));
    let ext = ext.get(..ext_len).filter(|e| !e.is_empty());
    let len = f.size();
    let n = buf.len().min(8192);
    let Some(buf) = buf.get_mut(..n) else { return };
    let mut off = 0u64;
    let mut r = Err(());
    for _ in 0..4 {
        f.set_position(off);
        let got = vol.read_exact_or_eof(&mut f, buf).await.unwrap_or(0);
        let mut w = orpheus_audio::WindowSource::new(buf.get(..got).unwrap_or_default(), off, Some(len));
        match inspect_head(&mut w, ext) {
            HeadStep::Ready(p) => {
                r = Ok(p);
                break;
            }
            HeadStep::SkipTo(at) if at < len => off = at,
            _ => break,
        }
    }
    if let Some(n) = ring.next.as_mut() {
        n.prep = Some(r);
    }
}

/// Play the ring's file (`Ring::file`).
#[allow(clippy::too_many_lines)]
async fn play_file(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    ext: Option<&[u8]>,
    srv: &mut Server,
    out: &mut PcmOut,
    st: Start,
) -> Result<End, &'static [u8]> {
    mark(10);
    let size = ring.file.as_ref().map_or(0, File::size);
    if !st.gapless {
        ring.reset(size);
        out.crc = 0xFFFF_FFFF;
    }
    // A gapless start has the file's head in the ring already: no card
    // read before the decoder runs unless the probe asks for more.
    let prepared = if st.gapless { ring.prepared.take() } else { None };
    if let Some(p) = prepared {
        // To the codec data, past leading tags (as the preparation found).
        let skip = p.start.saturating_sub(ring.position());
        if skip <= ring.available() as u64 {
            ring.consume(skip as usize);
        } else {
            ring.s.skip_to(p.start);
        }
    }
    if !st.gapless || ring.available() < 4096 {
        ring.fill(vol, None).await;
    }
    // Probe, skipping leading tags (done already for a prepared stream).
    let format = if let Some(p) = prepared {
        p.format
    } else {
        loop {
            match probe(ring, ext) {
                Probe::Found(fm) => break fm,
                Probe::Skip(n) => {
                    if (n as usize) <= ring.available() {
                        ring.consume(n as usize);
                    } else {
                        ring.s.skip_to(ring.position() + n);
                        ring.fill(vol, None).await;
                    }
                }
                Probe::NeedData(_) if !ring.eof() => {
                    ring.fill(vol, None).await;
                }
                _ => return Err(b"unknown format"),
            }
        }
    };
    let hint = ContainerInfo { format, gapless: None, stream_len: None };
    let plan = if let Some(p) = prepared {
        p.plan
    } else {
        loop {
            match orpheus_codecs::inspect(format, ring, &hint) {
                Ok(p) => break p,
                Err(OpenError::NeedData(_)) if !ring.eof() => {
                    ring.fill(vol, None).await;
                }
                Err(_) => return Err(b"unsupported stream"),
            }
        }
    };
    if plan.arena + SPARE > arena.len() {
        return Err(b"arena too small");
    }
    // SAFETY: initialised at the start of the audio task, which is the only
    // user.
    let chain = unsafe { (*(&raw mut CHAIN)).assume_init_mut() };
    if st.gapless && plan.info.rate != chain.format().rate {
        // Another sample rate: the previous track plays out and this one
        // opens again, fresh (`play_item`).
        return Ok(End::Again);
    }
    // The decoder's part of the arena, and the rest for file reads and
    // writes while it plays (text window, player state).
    let (dec_mem, spare) = arena.split_at_mut(plan.arena);
    let (mut dec, info) =
        orpheus_codecs::open(format, ring, dec_mem, &hint).map_err(|_| &b"open decoder failed"[..])?;
    if info.channels == 0 || info.channels > 2 {
        return Err(b"channels not supported");
    }
    // DSP chain: sets the DAC rate (88.2-192 kHz sources are decimated).
    // The same format as the previous track keeps its state (no reset at a
    // gapless join).
    mark(6);
    let out_rate = chain
        .set_format(orpheus_dsp::Format {
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
        match seek_to(&mut dec, ring, vol, st.frame).await {
            Some(at) => pos = at,
            None => {
                CODEC_FLOOR.store(0, Ordering::Relaxed);
                dsp_clock_floor(false);
                return Err(b"seek failed");
            }
        }
    }
    if !st.gapless {
        mark(7);
        media::apply_sound(chain);
    }
    mark(8);
    let dsp = DSP_ON.load(Ordering::Relaxed);
    if !out.running {
        dac::set_rate(out_rate);
    }
    RATE.store(out_rate, Ordering::Relaxed);
    let stereo = info.channels == 2;
    let ch = if stereo { 2 } else { 1 };
    let rate = u64::from(info.rate);
    let ms_of = move |frame: u64| (frame * 1000 / rate.max(1)) as u32;
    let (codec, start_ms) = (codec_name(ext), ms_of(pos));
    let show = move |p: &mut media::PlayerSnap| {
        p.source = if st.paused { Source::Paused } else { Source::Playing };
        p.position_ms = start_ms;
        p.duration_ms = info.frames.map_or(0, ms_of);
        p.codec = codec;
        p.rate = info.rate;
        p.channels = info.channels;
        p.bits = info.bits;
        p.size = size;
    };
    // After a gapless change the UI hears of the new track once its
    // decoder runs (chores below).
    if !st.gapless {
        media::player_update(show);
    }
    let driven = st.track.is_some();
    if let Some(track) = st.track.filter(|_| st.early.is_none()) {
        srv.feed(Event::Opened { track, rate: info.rate, frames: info.frames });
        // A track change: save while the DAC is not running yet (at a
        // gapless one it runs: the next safe moment saves).
        if !out.running {
            save_store(vol, srv, spare).await;
        }
    }
    // Chores (see `ROOMY`): the read-ahead open, a waiting request, the
    // gapless track's library place (1: start, 2: steps) and art (3).
    let mut serve_due = false;
    let mut pf_due: Option<u32> = None;
    let mut meta: u8 = if st.gapless { 1 } else { 0 };
    let mut lib_find: Option<orpheus_library::FindStep> = None;
    // The folder of the track before (a gapless track in the same folder
    // keeps its art: an album's cover).
    let mut prev_dir = u32::MAX;
    let mut chore_since = 0u64;
    // A gapless join being measured: 1 until the first block, 2 until two
    // halves are out.
    let mut join: u8 = if st.gapless { 1 } else { 0 };
    let mut early = st.early;
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
            Some(Ok(PlayReq::Edit(e))) if driven => flow.merge(srv.edit(e, vol.serial())),
            Some(Ok(r))
                if driven
                    && !matches!(
                        r,
                        PlayReq::PlayDir { .. }
                            | PlayReq::PlayLib { .. }
                            | PlayReq::ShuffleAll
                            | PlayReq::Shutdown
                    ) =>
            {
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
            if fl.cancel {
                // The next track changed: whatever was read ahead goes, and
                // the reader's end is reported again (a new `Prefetch`).
                ring.cancel_next();
                pf_due = None;
                reader_end = false;
            }
            if let Some(item) = fl.prefetch() {
                pf_due = Some(item);
            }
            if let Some(target) = fl.seek {
                // Stop the DAC for the seek: the bisection reads the card for
                // a while, which is not an underrun (and must not count
                // towards the too-slow abort).
                out.stop();
                match seek_to(&mut dec, ring, vol, target).await {
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
        if SLOW_UNDERRUNS.load(Ordering::Relaxed) >= MAX_UNDERRUNS {
            // The decoder cannot keep up (Opus on paged code): stop the
            // track rather than starve the rest of the system.
            break 'play failed(srv, driven, b"decoder too slow for this player");
        }
        let d0 = time::now_us();
        let l0 = pager_loads();
        mark(4);
        let step = dec.decode(ring);
        let dt = time::now_us() - d0;
        decode_us += dt;
        T_DECODE.fetch_add(dt as u32, Ordering::Relaxed);
        DEC_N.fetch_add(1, Ordering::Relaxed);
        DEC_US.fetch_add(dt as u32, Ordering::Relaxed);
        DEC_MAX_US.fetch_max(dt as u32, Ordering::Relaxed);
        DEC_LOADS.fetch_add(pager_loads().wrapping_sub(l0), Ordering::Relaxed);
        match step {
            Ok(Step::Pcm(block)) => {
                if join == 1 {
                    // The hand-over is over when the new decoder has a
                    // block: noted before its first pushes see the result.
                    jset(J_OPEN_US, now32().wrapping_sub(jget(J_T0)));
                    jset(J_LOADS, pager_loads().wrapping_sub(jget(J_L0)));
                    note(UB_JOIN, jget(J_T0));
                    join = 2;
                }
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
                if let (Some(item), Some(track)) = (early.take(), st.track) {
                    // Now the player hears that the previous track ended
                    // (it answers with this item) and that this one opened.
                    let mut fl = srv.feed(Event::DecoderEnded);
                    if fl.open.is_some_and(|o| o.target.is_item(item)) {
                        srv.feed(Event::Opened { track, rate: info.rate, frames: info.frames });
                        fl.open = None;
                    }
                    flow.merge(fl);
                }
                if join == 2 && frames >= 2 * HALF as u32 {
                    // Underruns are seen at half boundaries: by now the
                    // join's are counted. The UI hears of the new track now
                    // (no card work: the title came with the read-ahead).
                    jset(J_UNDER, UNDERRUNS.load(Ordering::Relaxed));
                    join = 0;
                    prev_dir = critical_section::with(|cs| media::PLAYER.borrow(cs).borrow().file.dir);
                    ring.show_handed();
                    media::player_update(show);
                }
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
                    let (a, head) = (now32(), out.headroom());
                    mark(1);
                    // The next file's head only with room to spare: near a
                    // file's end the decoder must stay ahead of the DAC.
                    ring.fill_in(vol, None, head >= ROOMY).await;
                    mark(0);
                    note(UB_FILL, a);
                    T_FILL.fetch_add(now32().wrapping_sub(a), Ordering::Relaxed);
                    // A request may be waiting: served as a chore.
                    serve_due = true;
                }
                // The gapless track's library place and art once its first
                // seconds have played.
                let meta_now = meta != 0 && join == 0 && u64::from(frames) * 1000 >= META_AFTER_MS * rate;
                // Requests wait while the current file's last bytes play
                // (≤ 0.7 s: the hand-over comes first).
                let serve_now = serve_due && !(ring.eof() && ring.next.is_some());
                // The file read ahead, probed and inspected before the end.
                let prep_now = ring.next.as_ref().is_some_and(|n| n.prep.is_none());
                if pf_due.is_some() || serve_now || meta_now || prep_now {
                    let now = time::now_us();
                    if chore_since == 0 {
                        chore_since = now;
                    }
                    let head = out.headroom();
                    if head >= ROOMY || now - chore_since > CHORE_WAIT_US {
                        chore_since = 0;
                        mark(2);
                        let t0 = now32();
                        // The read-ahead first: the current file's last
                        // bytes may last only ~0.15 s (FLAC).
                        if let Some(item) = pf_due.take() {
                            prefetch(vol, ring, srv, item).await;
                            jset(J_PREFETCH_US, now32().wrapping_sub(t0));
                            note(UB_PREFETCH, t0);
                        } else if prep_now {
                            let l0 = pager_loads();
                            prepare(vol, ring, spare).await;
                            jset(J_PREP_US, now32().wrapping_sub(t0));
                            jset(J_PREP_LOADS, pager_loads().wrapping_sub(l0));
                            note(UB_PREPARE, t0);
                        } else if serve_now {
                            serve_due = false;
                            let go_on = serve_one(vol, srv, spare).await;
                            T_SERVE.fetch_add(now32().wrapping_sub(t0), Ordering::Relaxed);
                            note(UB_SERVE, t0);
                            if !go_on {
                                break 'play Ok(End::Pending);
                            }
                        } else if meta == 1 {
                            // A folder that missed before, or no library:
                            // nothing to look for.
                            lib_find = st.track.and_then(|t| crate::library::find_begin(srv, t.locator()));
                            meta = if lib_find.is_some() { 2 } else { 3 };
                            jset(J_LIB_US, 0);
                        } else if meta == 2 {
                            // One probe per chore.
                            let done = match (lib_find.as_mut(), st.track) {
                                (Some(f), Some(t)) => {
                                    crate::library::find_more(vol, srv, f, t.locator()).await
                                }
                                _ => true,
                            };
                            if done {
                                meta = 3;
                            }
                            jset(J_LIB_US, jget(J_LIB_US).max(now32().wrapping_sub(t0)));
                            note(UB_LIB, t0);
                        } else {
                            meta = 0;
                            // An album's next track keeps the cover shown.
                            if let Some(t) = st.track.filter(|t| t.locator().dir != prev_dir) {
                                crate::pictures::track_art_cached(vol, srv, t.locator()).await;
                            }
                            jset(J_ART_US, now32().wrapping_sub(t0));
                            note(UB_ART, t0);
                            jset(J_UNDER_META, UNDERRUNS.load(Ordering::Relaxed));
                        }
                        mark(0);
                        LAST_SERVE_MS.store((time::now_us() / 1000) as u32, Ordering::Relaxed);
                    }
                }
                if driven && !reader_end && ring.s.file_left().is_some_and(|l| l <= READ_AHEAD_LEAD) {
                    reader_end = true;
                    // The player answers with `Prefetch`: the next file is
                    // read after this one (gapless).
                    flow.merge(srv.feed(Event::ReaderAtEnd));
                }
            }
            Ok(Step::NeedData(_)) => {
                if ring.eof() {
                    natural = true;
                    break 'play Ok(End::Idle);
                }
                let (a, head) = (now32(), out.headroom());
                let filled = ring.fill_in(vol, None, head >= ROOMY).await;
                note(UB_FILL, a);
                if !filled {
                    // The ring is full and the decoder still wants more:
                    // it can never be satisfied; end the track instead of
                    // spinning.
                    CORRUPT.fetch_add(1, Ordering::Relaxed);
                    break 'play failed(srv, driven, b"stream needs a larger ring");
                }
                // A request may be waiting: served as a chore.
                serve_due = true;
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
    if let (Some(item), Some(track)) = (early, st.track) {
        // Ended before its first block (a request for the idle loop, a
        // stop): the player still hears of the change.
        if srv.feed(Event::DecoderEnded).open.is_some_and(|o| o.target.is_item(item)) {
            srv.feed(Event::Opened { track, rate: info.rate, frames: info.frames });
        }
    }
    if join != 0 {
        // A gapless track shorter than two halves: show it anyway.
        ring.show_handed();
        media::player_update(show);
    }
    // At the natural end ask the player what comes next: the file read
    // ahead is handed over with the DAC running (gapless), anything else
    // first plays out what is queued.
    let mut gapless = false;
    // The item read ahead is next (the player's answer to `DecoderEnded`
    // is known): start it before telling the player, so its decoder opens
    // first (the player's code pages in after the new track's first block).
    let early = natural
        && driven
        && ring
            .next
            .as_ref()
            .is_some_and(|n| srv.player.as_ref().and_then(|p| p.gapless_next()) == Some(n.item));
    let end = if early {
        gapless = true;
        handed_over(out);
        let item = ring.next.as_ref().map_or(0, |n| n.item);
        Ok(End::Open(OpenReq {
            target: Target::Item(item),
            frame: 0,
            paused: false,
            gapless: true,
            early: true,
        }))
    } else if natural && driven {
        let fl = srv.feed(Event::DecoderEnded);
        match fl.open {
            Some(o)
                if o.gapless && !o.paused && ring.next.as_ref().is_some_and(|n| o.target.is_item(n.item)) =>
            {
                gapless = true;
                handed_over(out);
                Ok(End::Open(o))
            }
            Some(mut o) => {
                out.finish().await;
                o.gapless = false;
                Ok(End::Open(o))
            }
            None => {
                out.finish().await;
                if !fl.stop {
                    srv.feed(Event::Drained);
                }
                Ok(End::Idle)
            }
        }
    } else if natural {
        out.finish().await;
        end
    } else {
        if out.running {
            out.stop();
        }
        end
    };
    CRC.store(!out.crc, Ordering::Relaxed);
    DECODE_US.store(decode_us as u32, Ordering::Relaxed);
    PLAY_US.store((time::now_us() - t0) as u32, Ordering::Relaxed);
    if gapless {
        // The top clock level for the hand-over (Vorbis sets up its
        // codebooks while the PCM ring plays out).
        CODEC_FLOOR.store(2, Ordering::Relaxed);
        dsp_clock_floor(false);
    } else {
        PLAYING.store(false, Ordering::Relaxed);
        CODEC_FLOOR.store(0, Ordering::Relaxed);
        dsp_clock_floor(false);
    }
    end
}

/// Developer `playdir PATH [START]`: the folder's own entry (`None`: the
/// root) and the start file, as Files → OK passes them. `PATH` may also
/// name a file (its folder, from that file); `START` is a file name in the
/// folder.
async fn resolve_dir(
    vol: &mut Volume<'static, Card>,
    arg: &str,
) -> Option<(Option<orpheus_apps::services::FileRef>, Option<orpheus_apps::services::FileRef>)> {
    let fref = |l: orpheus_fs::Locator| orpheus_apps::services::FileRef { dir: l.dir, index: l.index };
    let mut e = DirEntry::new();
    let (dir, name) = if arg.is_empty() || arg == "/" {
        ("/", None)
    } else if vol.stat(arg, &mut e).await.is_ok() {
        if e.is_dir() {
            (arg, None)
        } else {
            let (d, n) = arg.rsplit_once('/')?;
            (if d.is_empty() { "/" } else { d }, Some(n))
        }
    } else {
        let (d, n) = arg.rsplit_once(' ')?;
        (d.trim_end(), Some(n))
    };
    let entry = if dir == "/" {
        None
    } else {
        vol.stat(dir, &mut e).await.ok()?;
        if !e.is_dir() {
            return None;
        }
        Some(fref(e.locator()))
    };
    let start = match name {
        None => None,
        Some(n) => {
            let mut cur = vol.open_dir(dir).await.ok()?;
            loop {
                if !vol.read_dir(&mut cur, &mut e).await.ok()? {
                    return None;
                }
                if e.name() == n {
                    break Some(fref(e.locator()));
                }
            }
        }
    };
    Some((entry, start))
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
static mut CHAIN: core::mem::MaybeUninit<orpheus_dsp::Chain> = core::mem::MaybeUninit::uninit();

/// Build the DSP chain. Called from `main` before the executors start: a
/// `Chain` is 4 KiB and built by value, which inside the audio task's
/// poll function would add to the stack frame the RT interrupt pushes on
/// top of whatever thread mode is using.
#[inline(never)]
pub fn init_chain() {
    // SAFETY: before any task runs; afterwards only the audio task uses it.
    unsafe { (*(&raw mut CHAIN)).write(orpheus_dsp::Chain::new(orpheus_dsp::OutWidth::Bits16, false)) };
}

static SERVER: ConstStaticCell<Server> = ConstStaticCell::new(Server::new());

// ------------------------------------------------------------ the player

/// Play what the player asked for, then whatever it asks for next, until
/// it stops (or a request for the idle loop arrives). The PCM output lives
/// across the tracks: a gapless change keeps it running.
async fn run_player(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    first: OpenReq,
) {
    let mut out = PcmOut::new();
    let mut next = Some(first);
    while let Some(o) = next.take() {
        for c in [&FRAMES, &UNDERRUNS, &SLOW_UNDERRUNS, &CORRUPT, &DEC_N, &DEC_US, &DEC_MAX_US, &DEC_LOADS] {
            c.store(0, Ordering::Relaxed);
        }
        critical_section::with(|cs| LEDGER.borrow(cs).borrow_mut().reset());
        STOP.store(false, Ordering::Relaxed);
        next = match play_item(vol, ring, arena, srv, &mut out, o).await {
            End::Open(o) => Some(o),
            End::Idle | End::Pending | End::Again => None,
        };
    }
    if out.running {
        out.stop();
    }
    ring.cancel_next();
    ring.file = None;
    ring.shown = None;
    PLAYING.store(false, Ordering::Relaxed);
    CODEC_FLOOR.store(0, Ordering::Relaxed);
    dsp_clock_floor(false);
    STOP.store(false, Ordering::Relaxed);
    media::player_update(|p| p.source = Source::Idle);
    save_store(vol, srv, arena).await;
}

/// Open and play one file for the player; a file that cannot be opened or
/// decoded is reported as `OpenFailed`. A gapless open takes the file read
/// ahead (its bytes are in the ring, the DAC still plays the previous
/// track's end); its album art and library place follow while it plays.
async fn play_item(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    out: &mut PcmOut,
    mut o: OpenReq,
) -> End {
    loop {
        let handed = if o.gapless { ring.hand_over(o.target) } else { None };
        let gapless = handed.is_some();
        let (track, ext, n) = match handed {
            Some(h) => h,
            None => {
                // The previous track's end plays out (after a gapless
                // hand-over that could not go on, too).
                out.finish().await;
                let Some(h) = open_target(vol, srv, o.target, None, &mut ring.file).await else {
                    return after(srv.feed(Event::OpenFailed));
                };
                // Its place in the music library ("Go to album / artist").
                crate::library::find_now(vol, srv, h.0.locator()).await;
                // Album art while the arena is still free (AU-14).
                crate::pictures::track_art(vol, ring, arena, srv, h.0.locator()).await;
                h
            }
        };
        let ext = ext.get(..usize::from(n)).filter(|e| !e.is_empty());
        let st = if gapless {
            let early = if let (true, Target::Item(i)) = (o.early, o.target) { Some(i) } else { None };
            Start { frame: 0, paused: false, track: Some(track), gapless, early }
        } else {
            Start { frame: o.frame, paused: o.paused, track: Some(track), gapless, early: None }
        };
        match play_file(vol, ring, arena, ext, srv, out, st).await {
            // Another rate, or the file read ahead cannot be played: open
            // it again after the previous track has played out (a broken
            // file then fails as usual).
            Ok(End::Again) | Err(_) if gapless => {
                if o.early {
                    // The player learns of the end first (it opens this item).
                    o.early = false;
                    srv.feed(Event::DecoderEnded);
                }
                o.gapless = false;
            }
            Ok(end) => return end,
            Err(_) => return after(srv.feed(Event::OpenFailed)),
        }
    }
}

/// Open the file of `t` into `file` (a track must still carry its name:
/// the entry may have been replaced); its title goes to `keep` (a file
/// read ahead), else it is published. Returns its identity and extension.
async fn open_target(
    vol: &mut Volume<'static, Card>,
    srv: &Server,
    t: Target,
    keep: Option<&mut media::Name>,
    file: &mut Option<File>,
) -> Option<(TrackRef, [u8; 8], u8)> {
    *file = None;
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
    *file = Some(vol.open_entry(&e).ok()?);
    let name = e.name();
    let track = TrackRef::new(vol.serial(), l, name);
    let mut ext = [0u8; 8];
    let mut n = 0;
    if let Some((_, x)) = name.rsplit_once('.') {
        n = x.len().min(ext.len());
        ext[..n].copy_from_slice(&x.as_bytes()[..n]);
    }
    match keep {
        Some(k) => *k = media::Name::title(name),
        None => publish_track(track, &media::Name::title(name)),
    }
    Some((track, ext, n as u8))
}

/// The track now playing, for the UI.
fn publish_track(t: TrackRef, title: &media::Name) {
    let l = t.locator();
    media::player_update(|p| {
        p.title = *title;
        p.file = orpheus_apps::services::FileRef { dir: l.dir, index: l.index };
    });
}

/// Open a file for writing under `/.orpheus/`, creating the folder if needed.
async fn create(vol: &mut Volume<'static, Card>, path: &str, mode: OpenMode) -> Option<File> {
    match vol.open(path, mode).await {
        Ok(f) => Some(f),
        Err(_) => {
            let _ = vol.create_dir("/.orpheus").await;
            vol.open(path, mode).await.ok()
        }
    }
}

/// Write the changed part of the player store to `/.orpheus/player.bin`
/// (in place: each value has its own CRC, the resume slots alternate) and
/// flush the volume. Called only while no audio is being output (pause,
/// stop, track change, before a card hand-over or power-off): a card write
/// can take longer than the PCM ring holds.
async fn save_store(vol: &mut Volume<'static, Card>, srv: &mut Server, scratch: &mut [u8]) {
    // An edited queue first: the resume record names its identity.
    if srv.queue_dirty && save_queue(vol, scratch, srv).await {
        srv.queue_dirty = false;
    }
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

/// Write the queue to `/.orpheus/queue.bin` (encoded in `arena`, free
/// decoder-arena memory: 2,068 bytes for 256 items). True if written.
async fn save_queue(vol: &mut Volume<'static, Card>, arena: &mut [u8], srv: &Server) -> bool {
    let Some(n) = encode_queue(vol.serial(), &srv.queue[..srv.qlen as usize], arena) else { return false };
    mark(12);
    let Some(mut f) = create(vol, media::QUEUE_FILE, OpenMode::Truncate).await else { return false };
    let wrote = vol.write(&mut f, &arena[..n]).await.is_ok();
    let ok = vol.close(f).await.is_ok() && wrote && vol.flush().await.is_ok();
    mark(0);
    ok
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
    crate::library::mounted(vol, srv).await;
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
pub(crate) fn work_waiting() -> bool {
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
            srv.queue_dirty = !save_queue(vol, arena, srv).await;
            srv.start_queue(s, shuffle, time::now_us() as u32)
        }
        PlayReq::ShuffleAll => {
            let seed = time::now_us() as u32;
            // A sample of the library; without one, a walk of the card.
            if !crate::library::sample(vol, srv, seed).await {
                srv.build_all_queue(vol, seed).await;
            }
            srv.queue_dirty = !save_queue(vol, arena, srv).await;
            srv.start_queue(None, true, seed)
        }
        // Without a library (gone with the card) the old queue stays.
        PlayReq::PlayLib { .. } if srv.lib.lib.is_none() => Flow::default(),
        PlayReq::PlayLib { list, start, shuffle } => {
            let s = crate::library::queue(vol, srv, list, start).await;
            srv.queue_dirty = !save_queue(vol, arena, srv).await;
            srv.start_queue(s, shuffle, time::now_us() as u32)
        }
        PlayReq::Edit(e) => srv.edit(e, vol.serial()),
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
/// a command; writes only where the volume allows (`/.orpheus/`).
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
            .u(u64::from(PLAYED.load(Ordering::Relaxed)))
            .s(b" walk=")
            .u(u64::from(media::Server::walk_progress().0))
            .s(b"/")
            .u(u64::from(media::Server::walk_progress().1 & 0xFF_FFFF))
            .s(b" d")
            .u(u64::from(media::Server::walk_progress().1 >> 24));
        // The queue and the track (HIL tests need no key navigation).
        let (pos, len, shuffled, title) = critical_section::with(|cs| {
            let p = media::PLAYER.borrow(cs).borrow();
            (p.queue_pos, p.queue_len, p.shuffled, p.title)
        });
        let t = title.as_str();
        let mut n = t.len().min(24);
        while !t.is_char_boundary(n) {
            n -= 1;
        }
        w.s(t!(b" q="))
            .u(u64::from(pos) + u64::from(len > 0))
            .s(b"/")
            .u(u64::from(len))
            .s(t!(b" shuf="))
            .u(u64::from(shuffled))
            .s(t!(b" title="))
            .s(&t.as_bytes()[..n]);
        // The last gapless join (`-`: not measured yet).
        w.s(t!(b" join#")).u(u64::from(jget(J_SEQ))).s(b":");
        for (label, i) in [
            (t!(b" head="), J_HEAD),
            (t!(b" open_us="), J_OPEN_US),
            (t!(b" loads="), J_LOADS),
            (t!(b" u="), J_UNDER),
            (t!(b" lib_us="), J_LIB_US),
            (t!(b" art_us="), J_ART_US),
            (t!(b" u_meta="), J_UNDER_META),
            (t!(b" pf_us="), J_PREFETCH_US),
            (t!(b" prep_us="), J_PREP_US),
            (t!(b" prep_loads="), J_PREP_LOADS),
        ] {
            w.s(label);
            match jget(i) {
                J_NONE => w.s(b"-"),
                v => w.u(u64::from(v)),
            };
        }
        // This track's decoder calls: count, µs (sum, largest), pager line
        // loads inside them.
        w.s(t!(b" dec: n="))
            .u(u64::from(DEC_N.load(Ordering::Relaxed)))
            .s(t!(b" us="))
            .u(u64::from(DEC_US.load(Ordering::Relaxed)))
            .s(t!(b" max_us="))
            .u(u64::from(DEC_MAX_US.load(Ordering::Relaxed)))
            .s(t!(b" loads="))
            .u(u64::from(DEC_LOADS.load(Ordering::Relaxed)));
        // This track's underruns by cause (they add up to `underruns`).
        let ub = critical_section::with(|cs| LEDGER.borrow(cs).borrow().by());
        w.s(t!(b" ub: join="))
            .u(u64::from(ub[UB_JOIN]))
            .s(t!(b" fill="))
            .u(u64::from(ub[UB_FILL]))
            .s(t!(b" serve="))
            .u(u64::from(ub[UB_SERVE]))
            .s(t!(b" pf="))
            .u(u64::from(ub[UB_PREFETCH]))
            .s(t!(b" prep="))
            .u(u64::from(ub[UB_PREPARE]))
            .s(t!(b" lib="))
            .u(u64::from(ub[UB_LIB]))
            .s(t!(b" art="))
            .u(u64::from(ub[UB_ART]))
            .s(t!(b" dec="))
            .u(u64::from(ub[UB_DECODER]));
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
    atj2157_hal::log::line(b"audio: dac up");
    let card = match Card::open().await {
        Ok(c) => c,
        Err(_) => {
            reply(|w| {
                w.s(b"card: init failed");
            });
            return;
        }
    };
    atj2157_hal::log::line(b"audio: card open");
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
    let mut ring = Ring {
        s: StreamRing::new(&mut RING_BUF.take().0),
        file: None,
        next: None,
        next_file: None,
        shown: None,
        prepared: None,
    };
    // SAFETY: this task is the arena's owner; the USB task borrows it only
    // while this task waits for the card (`usb_buffer`).
    let full: &'static mut [u8] = unsafe { &mut (*(&raw mut ARENA_BUF)).0 };
    // Its last bytes hold the music library's build state (never lent out).
    let (arena, lib_state) = full.split_at_mut(ARENA - crate::library::STATE);
    crate::library::init(lib_state);
    let srv = SERVER.take();
    atj2157_hal::log::line(b"audio: mounted, loading state");
    load_state(&mut vol, arena, srv).await;
    atj2157_hal::log::line(b"audio: state loaded");
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
            // The music library builds when it is due and nothing else waits.
            if crate::library::wait_ms(srv) == Some(0) && !srv.wants_build() && !work_waiting() {
                crate::library::slot(&mut vol, srv, arena).await;
            }
            let next = select3(CMDS.receive(), media::REQS.receive(), media::PLAY.receive());
            match crate::library::wait_ms(srv) {
                Some(ms) => match select(next, Timer::after_millis(ms.max(1))).await {
                    Either::First(r) => r,
                    Either::Second(()) => continue,
                },
                None => next.await,
            }
        };
        let cmd = match r {
            Either3::First(c) => c,
            Either3::Second(Req::Picture(j)) => {
                // No track loaded: the whole arena for the picture.
                crate::pictures::serve_idle(&mut vol, &mut ring, arena, srv, &j).await;
                continue;
            }
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
                    crate::library::card_back(&mut vol, srv).await;
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
            crate::library::card_back(&mut vol, srv).await;
        } else if let Some(arg) = text.strip_prefix("playdir ") {
            // The player's queue on a folder, as Files → OK (in folder
            // order: shuffle off, for repeatable tests).
            match resolve_dir(&mut vol, arg.trim()).await {
                Some((entry, start)) => {
                    reply(|w| {
                        w.s(t!(b"playdir ok"));
                    });
                    srv.pending_play = Some(PlayReq::PlayDir { entry, start, shuffle: false });
                }
                None => reply(|w| {
                    w.s(t!(b"playdir: no such folder or file"));
                }),
            }
        } else if let Some(p) = text.strip_prefix("ls ") {
            ls(&mut vol, p.trim()).await;
        } else if let Some(p) = text.strip_prefix("play ") {
            for s in [&FRAMES, &UNDERRUNS, &SLOW_UNDERRUNS, &CORRUPT, &DEC_N, &DEC_US, &DEC_MAX_US, &DEC_LOADS] {
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
            let r = chain.set_format(orpheus_dsp::Format {
                rate: 44_100,
                channels: 2,
                kind: orpheus_audio::SampleKind::I16,
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
            // Parents one by one (only `/.orpheus/…` is writable).
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
                acc += orpheus_dsp::math::exp2(core::hint::black_box(i as f32 * 0.001));
                acc += orpheus_dsp::math::log2(core::hint::black_box(1.0 + i as f32));
            }
            let us = time::now_us() - t0;
            reply(|w| {
                w.s(b"fp ok us=").u(us).s(b" acc=").u(acc as u64);
            });
        } else if let Some(g) = text.strip_prefix("pa ") {
            // Amplifier gain field PA_CTL[14:12] (the vendor's music value is 7).
            let g: u32 = g.trim().parse().unwrap_or(0);
            dac::set_pa_gain(g);
            reply(|w| {
                w.s(b"pa ").u(u64::from(dac::regs()[2]));
            });
        } else if let Some(v) = text.strip_prefix("vol ") {
            let v: u8 = v.trim().parse().unwrap_or(0);
            dac::set_volume(v, v);
            reply(|w| {
                w.s(b"ok");
            });
        } else {
            reply(|w| {
                w.s(t!(b"commands: ls PATH, play PATH, playdir PATH [START], vol N"));
            });
        }
    }
}
