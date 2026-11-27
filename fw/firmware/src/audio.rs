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
//! Settings (VER-13): Fade on pause fades the output out over 60 ms before
//! the DAC stops and in again after (`Chain::begin_pause`/`end_pause`);
//! Gapless off makes every track change a fresh start (the player reads
//! nothing ahead); Resume at start-up opens the saved place paused once the
//! card is mounted; Key click mixes `orpheus_dsp::click` into the music, or
//! sounds it alone while nothing plays ([`click`]). `status` shows `fade=`,
//! `gapless=` and `clicks=`.
//!
//! Not yet: the fades of 04 §2.2 around seek, periodic resume
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
use orpheus_audio::chore::{self, Ask, Costs, DecPeak, Piece, Spent, Verdict};
use orpheus_audio::pcm::{self, Book, Fed, Feeder, Look, Reading, Sink};
use orpheus_audio::ring::{Space, Target as Part};
use orpheus_audio::synth::{SourceKind, Start as PlanStart, plan_start, source_of};
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

/// Speech over music and alone (TTS-3): feature `speech`.
#[cfg(feature = "speech")]
mod speech;
#[cfg(not(feature = "speech"))]
mod speech {
    //! Without feature `speech`: no speech to mix or to sound alone.

    #[inline(always)]
    pub(super) fn wanted() -> bool {
        false
    }
    pub(super) fn mix_i16(_s: &mut [i16], _n: usize, _ch: usize) {}
    pub(super) fn mix_i32(_s: &mut [i32], _n: usize, _ch: usize) {}
    #[inline(always)]
    pub(super) fn alone() -> bool {
        false
    }
    #[inline(always)]
    pub(super) fn alone_running() -> bool {
        false
    }
    #[inline(always)]
    pub(super) fn end_alone() {}
    pub(super) async fn wait() {
        core::future::pending::<()>().await;
    }
    pub(super) async fn serve() -> core::convert::Infallible {
        core::future::pending().await
    }
}
/// Synth sources: the engine in the pool, MIDI files, the bench (SYN-3).
mod synth;

pub const CMD_LEN: usize = 120;

#[derive(Clone, Copy)]
pub struct Cmd {
    pub len: usize,
    pub text: [u8; CMD_LEN],
}

pub static CMDS: Channel<CriticalSectionRawMutex, Cmd, 2> = Channel::new();
/// Test commands served while a track is paused (`ls`,
/// `orpheus_proto::devchan::served_while_paused`): a reboot reopens the
/// last track paused, and `CMDS` is read only between tracks.
pub static PAUSED_CMDS: Channel<CriticalSectionRawMutex, Cmd, 1> = Channel::new();

/// A command left in [`PAUSED_CMDS`] when the track ended goes to the idle
/// loop's queue.
#[inline(always)]
fn hand_paused_cmd_on() {
    if let Ok(c) = PAUSED_CMDS.try_receive() {
        let _ = CMDS.try_send(c);
    }
}

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
    /// A signed number.
    pub fn i(&mut self, v: i16) -> &mut Self {
        if v < 0 {
            self.s(b"-");
        }
        self.u(u64::from(v.unsigned_abs()))
    }
    /// `v` in hex, at least two digits (no table: a 16-byte one would be
    /// a resident constant).
    pub fn h(&mut self, v: u32) -> &mut Self {
        let mut started = false;
        for k in (0..8).rev() {
            let d = ((v >> (4 * k)) & 15) as u8;
            if d != 0 || started || k < 2 {
                started = true;
                let c = [if d < 10 { b'0' + d } else { b'a' + d - 10 }];
                self.s(&c);
            }
        }
        self
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
/// The timer or an alarm rings (UI-5d, set by the UI): the card task
/// sounds the alert while it is set (`alert_tone`).
pub static ALERT: AtomicBool = AtomicBool::new(false);
/// Settings › Sound › Key click (VER-13): the frame of the click the next
/// output frame gets (`orpheus_dsp::click`), `u32::MAX` while none sounds.
/// The UI sets it to 0 ([`click`]); the playback loop mixes it into the
/// music, the idle and paused loops sound it alone.
static CLICK_AT: AtomicU32 = AtomicU32::new(u32::MAX);
/// `status` (VER-13): the last pause's fade-out in ms (bits 0–11, `0xFFF`:
/// no pause yet), the last natural track change (bits 12–13: 0 none yet,
/// 1 gapless, 2 with the DAC stopped), key clicks sounded (bits 16–31).
static PLAY_STATS: AtomicU32 = AtomicU32::new(0xFFF);

#[inline(never)]
fn stats_set(mask: u32, v: u32) {
    let _ = PLAY_STATS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |s| Some((s & !mask) | (v & mask)));
}

/// A key click (Settings › Sound › Key click; the UI decides whether it
/// sounds). `wake`: nothing plays, the card task is woken to sound it.
pub fn click(wake: bool) {
    CLICK_AT.store(0, Ordering::Relaxed);
    if wake {
        let _ = media::try_request(media::Req::Alert);
    }
}

/// Mix the key click into a piece of output (`ch` channels at the DAC's
/// rate), if one sounds.
#[inline(never)]
fn click_mix<T>(s: &mut [T], n: usize, ch: usize, mix: fn(&mut [T], usize, u32, &mut u32)) {
    let n = n.min(s.len());
    let s = &mut s[..n];
    let (mut at, rate) = (CLICK_AT.load(Ordering::Relaxed), RATE.load(Ordering::Relaxed));
    if at == 0 {
        PLAY_STATS.fetch_add(1 << 16, Ordering::Relaxed);
    }
    mix(s, ch, rate, &mut at);
    let done = at >= orpheus_dsp::click::frames(rate);
    CLICK_AT.store(if done { u32::MAX } else { at }, Ordering::Relaxed);
}

/// The key click alone (nothing plays): the DAC started for it, the rest
/// of the click, silence to the end of the ring, stopped.
#[inline(never)]
async fn click_alone(out: &mut PcmOut) {
    let rate = match RATE.load(Ordering::Relaxed) {
        0 => 44_100,
        r => r,
    };
    if !out.running {
        dac::set_rate(rate);
    }
    let at = CLICK_AT.swap(u32::MAX, Ordering::Relaxed);
    if at == 0 {
        PLAY_STATS.fetch_add(1 << 16, Ordering::Relaxed);
    }
    for i in at..orpheus_dsp::click::frames(rate) {
        let s = u32::from(orpheus_dsp::click::sample(i, rate) as u16);
        out.push(s | s << 16).await;
    }
    out.finish().await;
}

/// Where the playback loop is (status diagnostics): 1 card read, 2 media
/// request, 3 waiting for the DAC, 4 decoding, 5 DSP, 0 elsewhere.
pub static WHERE: AtomicU32 = AtomicU32::new(0);
/// Clock floor for the current track (set at its start from the codec, by
/// the synth up to Max): 0 Base, 1 Mid, 2 High, 3 Turbo, 4 Max; carried
/// out by [`dsp_clock_floor`].
static CODEC_FLOOR: core::sync::atomic::AtomicU8 = core::sync::atomic::AtomicU8::new(0);

/// The DSP chain works (the last [`dsp_clock_floor`]'s argument).
static DSP_ACTIVE: AtomicBool = AtomicBool::new(false);
/// A picture decodes (the card task, [`boost`]).
static BOOST: AtomicBool = AtomicBool::new(false);
/// The loaded track is paused (`pause` until `resumed`): its floor is
/// released and the governor treats it as nothing playing
/// (`orpheus_power::governor::{audio_floor, floor_level}`; 2026-11-26).
pub static PAUSED: AtomicBool = AtomicBool::new(false);

/// Apply the clock floor (`CODEC_FLOOR`: 0 Base, 1 Mid, 2 High, 3 Turbo,
/// 4 Max), the codec's or the synth's, raised to Mid while the DSP chain
/// works, none while the track is paused, High while a picture decodes
/// (`orpheus_power::governor::audio_floor`); the level is raised here at
/// once (from the RT task; not in Battery saver, `crate::raise_to_floor`)
/// and the governor in Automatic never goes below it.
#[inline(never)]
fn dsp_clock_floor(dsp_active: bool) {
    DSP_ACTIVE.store(dsp_active, Ordering::Relaxed);
    let floor = orpheus_power::governor::audio_floor(
        CODEC_FLOOR.load(Ordering::Relaxed),
        dsp_active,
        PAUSED.load(Ordering::Relaxed),
        BOOST.load(Ordering::Relaxed),
    );
    crate::CLOCK_FLOOR.store(floor, Ordering::Relaxed);
    crate::raise_to_floor(floor);
}

/// The track paused (`true`) or playing again: its floor released, or
/// back at once.
fn set_paused(on: bool) {
    PAUSED.store(on, Ordering::Relaxed);
    dsp_clock_floor(DSP_ACTIVE.load(Ordering::Relaxed));
}

/// Clock floor High while the card task decodes a picture, whatever the
/// track does (before, the end of a picture also cleared a playing
/// track's codec floor).
pub(crate) fn boost(on: bool) {
    BOOST.store(on, Ordering::Relaxed);
    dsp_clock_floor(DSP_ACTIVE.load(Ordering::Relaxed));
}

fn sound_active() -> bool {
    DSP_ON.load(Ordering::Relaxed)
        && critical_section::with(|cs| !media::SOUND.borrow(cs).borrow().chain_default())
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
/// The last decoder call (µs; the `pcm: head` log line).
static LAST_DEC_US: AtomicU32 = AtomicU32::new(0);
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

/// Add `v` to a join statistic (`-` counts as 0).
fn jadd(i: usize, v: u32) {
    let was = jget(i);
    jset(i, if was == J_NONE { v } else { was.saturating_add(v).min(J_NONE - 1) });
}

fn now32() -> u32 {
    time::now_us() as u32
}

fn pager_loads() -> u32 {
    atj2157_hal::pager::stats().loads as u32
}

/// Card work besides the current file's refill (chores: UI requests,
/// listing pieces, the read-ahead open and head, a gapless track's library
/// place and art, the battery and step logs) waits until the DAC has this
/// many frames ahead of it (≈ 52 ms at 44.1 kHz; `PcmOut::headroom`), or
/// has waited `orpheus_audio::chore::WAIT_US`; either way only when the
/// headroom less a margin for the next decoder call covers the chore's
/// measured need, and a splittable chore stops at that budget
/// (`orpheus_audio::chore`: the rule and its host model; soak seed 1,
/// 2026-11-25: chores of 25–55 ms forced in at −93…−649 frames).
const ROOMY: i32 = (HALF + HALF / 2) as i32;

/// Chore kinds, each with its own measured need (`COSTS`): fixed ones,
/// the listing pieces (`media::JOB_KINDS`), the request kinds
/// (`media::REQ_KINDS`).
const K_PF: usize = 0;
const K_PREP: usize = 1;
const K_LIB: usize = 2;
const K_ART: usize = 3;
const K_DRAIN: usize = 4;
const K_SENSOR: usize = 5;
const K_JOB: usize = 6;
const K_SERVE: usize = K_JOB + media::JOB_KINDS;
const KINDS: usize = K_SERVE + media::REQ_KINDS;

/// Needs (µs) until measured: guesses on the high side (a kind runs only
/// at a headroom that covers its need, so too low a guess costs an
/// underrun, too high one a later answer; the first pieces measure it).
const SEED_US: [u32; KINDS] = [
    // read-ahead open, head (one 2-KiB read), library step, art, battery
    // log, step log
    12_000, 10_000, 6_000, 15_000, 20_000, 20_000,
    // listing pieces: count, index open, focus, rows, unsorted window, title
    // (count and rows as measured on the player, 2026-11-25: one directory
    // read up to 21 ms, row pieces 15–18 ms; the index file's lookup in
    // stretches of `media::FIND_SETS` entry sets, one to six sector reads
    // each, was one 52–60 ms piece)
    20_000, 15_000, 15_000, 16_000, 8_000, 8_000,
    // games (`crate::games::piece`: an open with a read, or with a write
    // into the sector cache; one sector written back with its FAT copy;
    // the sync and the clean flag)
    15_000,
    // Books (`crate::books::chore`, one step of `orpheus_books::shelf::Desk`
    // at a time, ≤ 16 sectors read and ≤ 12 written; never a folder made
    // while playing): writes (a sector written back, a job's entry, the
    // reading state), the library search (16 entries or a 512-byte head),
    // reads (rows, an open, the 2 KiB window, an index chunk), a
    // conversion piece (a 512-byte read, up to two writes), a layout
    // piece (a 512-byte read, or 512 bytes of index written)
    20_000, 20_000, 15_000, 15_000, 15_000,
    // requests: browse, want, text, queue-win, release, image, picture,
    // lib-list, rebuild, drain, sensor, alert, otp, delete, update, games
    // (its first piece, as `job:games`), books (a wake-up while playing)
    10_000, 15_000, 15_000, 2_000, 1_000, 1_000, 30_000, 20_000, 1_000, 15_000, 15_000, 1_000, 25_000, 30_000,
    30_000, 15_000, 1_000,
];

/// The chore kinds' measured needs (zeros: the seeds stand until measured;
/// in `.bss`, so no initialised RAM next to the stack).
static COSTS: critical_section::Mutex<core::cell::RefCell<Costs<KINDS>>> =
    critical_section::Mutex::new(core::cell::RefCell::new(Costs::new()));

fn seed_of(kind: usize) -> u32 {
    SEED_US.get(kind).copied().unwrap_or(0)
}

fn need_of(kind: usize) -> u32 {
    critical_section::with(|cs| COSTS.borrow(cs).borrow().need(kind, seed_of(kind)))
}

/// The chore piece running (its budget; [`piece_left`]); `None` outside
/// chores (idle, paused: nothing plays, no budget).
static PIECE: critical_section::Mutex<core::cell::Cell<Option<Piece>>> =
    critical_section::Mutex::new(core::cell::Cell::new(None));

/// Before a card operation of a splittable chore (listing pieces, Up next
/// titles, the library search, the read-ahead's head): may it start within
/// the piece's budget? The first one always does; outside a chore always.
pub(crate) fn piece_left() -> bool {
    let now = now32();
    critical_section::with(|cs| {
        let c = PIECE.borrow(cs);
        let Some(mut p) = c.get() else { return true };
        let ok = p.left(now);
        c.set(Some(p));
        ok
    })
}

/// A chore piece of `kind` starts with `budget` (µs).
#[inline(never)]
fn piece_begin(kind: usize, budget: u32) {
    let p = Piece::new(now32(), budget, need_of(kind)).with_io(yoton_bsp::card::io_count());
    critical_section::with(|cs| PIECE.borrow(cs).set(Some(p)));
}

/// The piece of `kind` is over: its time measured into the kind's need
/// (`Costs::observe_piece`: a piece that did no card operation, such as
/// `want` while a folder plays, does not lower it), an overrun (or a
/// starved run) logged.
#[inline(never)]
fn piece_end(kind: usize, starved: bool) {
    let now = now32();
    let io = yoton_bsp::card::io_count();
    let seed = seed_of(kind);
    let s = critical_section::with(|cs| {
        let p = PIECE.borrow(cs).take()?;
        let s = p.end(now);
        COSTS.borrow(cs).borrow_mut().observe_piece(kind, s.unit, seed, p.worked(io));
        Some(s)
    });
    if let Some(s) = s.filter(|s| s.over || starved) {
        chore_log(kind, s, starved);
    }
}

/// When the last `chore:` line was logged (ms, 0: never) and the lines
/// not logged since.
static CHORE_LOG_MS: AtomicU32 = AtomicU32::new(0);
static CHORE_QUIET: AtomicU32 = AtomicU32::new(0);

/// The `chore:` line's name of a kind: `pf`, `prep`, `lib`, `art`,
/// `drain`, `sensor`, `job:<piece>` (`media::JOB_NAMES`), `serve:<request>`
/// (`media::REQ_NAMES`).
fn kind_name(kind: usize) -> (&'static str, &'static str) {
    match kind {
        K_PF => ("pf", ""),
        K_PREP => ("prep", ""),
        K_LIB => ("lib", ""),
        K_ART => ("art", ""),
        K_DRAIN => ("drain", ""),
        K_SENSOR => ("sensor", ""),
        k if k < K_SERVE => ("job:", media::JOB_NAMES.get(k - K_JOB).copied().unwrap_or("")),
        k => ("serve:", media::REQ_NAMES.get(k - K_SERVE).copied().unwrap_or("")),
    }
}

/// An overrun this far past its budget (µs) is always logged: it ate more
/// than a fifth of the decoder margin (`chore::MARGIN_US`) and may have
/// emptied the DAC ring. Smaller ones (`job:rows 16/16 ms`, routine) keep
/// to one line a second and could hide a large one behind them (soak seed
/// 1, 2026-11-25: `ub=serve:55ms` with no `chore:` line for it).
const LOUD_OVER_US: u32 = chore::MARGIN_US / 5;

/// A chore piece took longer than its budget, or ran starved (its need fit
/// no headroom for `chore::STARVE_US`): `chore: serve:picture 31/22 ms
/// u31 +0` (took / budget, the longest unsplittable stretch, lines not
/// logged since the last). At most one line a second, except a starved
/// piece or one over by [`LOUD_OVER_US`] or more.
#[inline(never)]
fn chore_log(kind: usize, s: Spent, starved: bool) {
    let now = ((time::now_us() / 1000) as u32).max(1);
    let last = CHORE_LOG_MS.load(Ordering::Relaxed);
    let loud = starved || s.took >= s.budget.saturating_add(LOUD_OVER_US);
    if !loud && last != 0 && now.wrapping_sub(last) < 1000 {
        CHORE_QUIET.fetch_add(1, Ordering::Relaxed);
        return;
    }
    CHORE_LOG_MS.store(now, Ordering::Relaxed);
    let (name, sub) = kind_name(kind);
    let mut l = atj2157_hal::log::LineBuf::new();
    l.push_str(if starved { "chore: starved " } else { "chore: " })
        .push_str(name)
        .push_str(sub)
        .push_str(" ")
        .push_u32(s.took / 1000)
        .push_str("/")
        .push_u32(s.budget / 1000)
        .push_str(" ms u")
        .push_u32(s.unit / 1000)
        .push_str(" +")
        .push_u32(CHORE_QUIET.swap(0, Ordering::Relaxed));
    l.emit();
}

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
    note_as(cause, orpheus_audio::ledger::NO_DETAIL, t0);
}

/// [`note`] naming the exact path for the `pcm: head` line's `ub=`
/// (`ub_label`): a chore kind (`K_*`, `0..KINDS`: `serve:picture`,
/// `job:rows`, `drain`, …) or a player request taken in `head`
/// (`D_REQ + REQ_LABELS` index: `req:skip`, …). The counts stay by cause.
#[inline(never)]
fn note_as(cause: usize, detail: u8, t0: u32) {
    let us = now32().wrapping_sub(t0);
    critical_section::with(|cs| LEDGER.borrow(cs).borrow_mut().note_as(cause, detail, us));
}

/// Details from here on name a player request handled in `head`
/// ([`REQ_LABELS`]); below it, a chore kind.
const D_REQ: u8 = 0x40;
const _: () = assert!(KINDS <= D_REQ as usize);
/// Details from here on name a synth source's operation
/// (`orpheus_audio::synth::SynthOp`: `ub=synth:render`, `synth:rest`,
/// `synth:read`), noted under the decoder's cause.
const D_SYN: u8 = 0x60;
const _: () = assert!(D_REQ as usize + 9 <= D_SYN as usize);
/// The player requests `head` takes while a track plays, as the `pcm:`
/// line names them (`req:<label>`), NUL-separated in a paged static (plain
/// literals would land in the resident mergeable-string section and cost
/// stack: `__stack_bottom`).
static REQ_LABELS: [u8; 47] = *b"pause\0skip\0seek\0stop\0shuffle\0ab\0mark\0edit\0other";
static REQ_PREFIX: [u8; 4] = *b"req:";

/// Label `i` of [`REQ_LABELS`].
fn req_label(i: usize) -> Option<&'static str> {
    let b = REQ_LABELS.split(|&c| c == 0).nth(i)?;
    core::str::from_utf8(b).ok()
}

/// The detail of player request `r` ([`REQ_LABELS`]).
fn req_detail(r: &PlayReq) -> u8 {
    let i = match r {
        PlayReq::Cmd(Command::Toggle | Command::Play | Command::Pause) | PlayReq::Resume => 0,
        PlayReq::Cmd(Command::Next | Command::Prev | Command::JumpTo(_)) | PlayReq::JumpPos(_) => 1,
        PlayReq::Cmd(Command::SeekMs(_) | Command::SeekByMs(_)) => 2,
        PlayReq::Stop | PlayReq::Cmd(Command::Stop) => 3,
        PlayReq::Cmd(Command::Shuffle(_) | Command::Repeat(_)) => 4,
        PlayReq::SetA | PlayReq::SetB | PlayReq::Cmd(Command::AbPoint | Command::AbClear) => 5,
        PlayReq::Cmd(Command::AddBookmark | Command::GotoBookmark(_) | Command::DeleteBookmark(_)) => 6,
        PlayReq::Edit(_) => 7,
        _ => 8,
    };
    D_REQ + i
}

/// The `pcm: head` line's name of the ledger's candidate: its detail
/// (`note_as`), else its cause.
fn ub_label(cause: usize, detail: u8) -> (&'static str, &'static str) {
    if usize::from(detail) < KINDS {
        return kind_name(usize::from(detail));
    }
    if let Some(l) = detail.checked_sub(D_SYN).and_then(orpheus_audio::synth::synth_label) {
        return (core::str::from_utf8(&orpheus_audio::synth::SYNTH_PREFIX).unwrap_or(""), l);
    }
    if let Some(l) = detail.checked_sub(D_REQ).and_then(|i| req_label(usize::from(i))) {
        return (core::str::from_utf8(&REQ_PREFIX).unwrap_or(""), l);
    }
    let c = match cause {
        UB_JOIN => "join",
        UB_FILL => "fill",
        UB_SERVE => "serve",
        UB_PREFETCH => "pf",
        UB_PREPARE => "prep",
        UB_LIB => "lib",
        UB_ART => "art",
        _ => "dec",
    };
    (c, "")
}

/// Developer `stop`: ends the current file at once (also inside a PCM
/// wait).
static STOP: AtomicBool = AtomicBool::new(false);

// ------------------------------------------------------------ stream ring

/// 16 KiB: RAM does not allow `orpheus_audio::MAX_PEEK` (28 KiB) plus a
/// refill unit next to the resident audio path. Ogg pages up to ~15.5 KiB
/// play; a stream that needs more ends with an error (open, STATUS).
const RING: usize = 16 * 1024;

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
    /// `prepare`'s next read (offset) and the reads done (one per card
    /// operation, resumed in the next chore).
    prep_at: u32,
    prep_reads: u8,
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
const PCM_ADDR: u32 = orpheus_proto::mem::SRAM3_PCM_RING;
const PCM_FRAMES: usize = 3072;
const _: () = assert!(PCM_FRAMES * 4 == orpheus_proto::mem::SRAM3_PCM_RING_SIZE as usize);
const HALF: usize = PCM_FRAMES / 2;
const DAC_CH: usize = 5;

static PLAYED: AtomicU32 = AtomicU32::new(0);
/// The DAC's lowest headroom (frames) seen since the last read (VER-12
/// soak: STATS group 0 word 21; `i32::MAX`: not playing). Every look at the
/// DMA feeds it: before each block's pushes (where a stall leaves the writer
/// furthest behind), at half boundaries and at the loop's chore checks.
static HEAD_MIN: core::sync::atomic::AtomicI32 = core::sync::atomic::AtomicI32::new(i32::MAX);

/// Take the headroom low-water mark (reset to "none").
pub fn take_head_min() -> u32 {
    HEAD_MIN.swap(i32::MAX, Ordering::Relaxed) as u32
}
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

/// Where a look at the DMA was taken (the `pcm: head` log line's `by`).
#[derive(Clone, Copy)]
enum By {
    /// Before a block's pushes.
    Block,
    /// At a half boundary.
    Half,
    /// The playback loop's `headroom` (chores, refills, joins).
    Loop,
}

/// When the last `pcm: head` line was logged (ms, 0: never) and the looks
/// with negative headroom since that were not.
static HEAD_LOG_MS: AtomicU32 = AtomicU32::new(0);
static HEAD_QUIET: AtomicU32 = AtomicU32::new(0);

/// The DAC fetched frames the writer had not written (VER-12, 2026-11-25:
/// the soak saw -13 frames with no underrun counted). At most one line a
/// second: the headroom, the longest operation since the last half
/// boundary (the ledger's candidate: its exact path, `ub_label`, and
/// length; first, so the 96-byte line never cuts it off), where the look
/// was, the writer's half and fill, the DMA's half and frame, halves newly
/// counted, the last decoder call, the loop's breadcrumb ([`mark`]) and the
/// looks with negative headroom not logged since the last line:
/// `pcm: head -817 ub=serve:picture:55ms blk w=… f=… p=… at=… n=2 dec=6ms
/// mk=0 +0`.
#[inline(never)]
fn head_log(l: Look, r: Reading, b: &Book, by: By) {
    let now = ((time::now_us() / 1000) as u32).max(1);
    let last = HEAD_LOG_MS.load(Ordering::Relaxed);
    if last != 0 && now.wrapping_sub(last) < 1000 {
        HEAD_QUIET.fetch_add(1, Ordering::Relaxed);
        return;
    }
    HEAD_LOG_MS.store(now, Ordering::Relaxed);
    let worst = critical_section::with(|cs| LEDGER.borrow(cs).borrow().candidate());
    let mut s = atj2157_hal::log::LineBuf::new();
    s.push_str(if l.head < 0 { "pcm: head -" } else { "pcm: head " }).push_u32(l.head.unsigned_abs());
    match worst {
        Some((c, d, us)) => {
            let (a, b) = ub_label(c, d);
            s.push_str(" ub=").push_str(a).push_str(b).push_str(":").push_u32(us / 1000).push_str("ms");
        }
        None => {
            s.push_str(" ub=-");
        }
    }
    s.push_str(match by {
        By::Block => " blk w=",
        By::Half => " half w=",
        By::Loop => " loop w=",
    })
    .push_u32(b.written)
    .push_str(" f=")
    .push_u32(b.fill)
    .push_str(" p=")
    .push_u32(r.played)
    .push_str(" at=")
    .push_u32(r.at)
    .push_str(" n=")
    .push_u32(l.stale)
    .push_str(" dec=")
    .push_u32(LAST_DEC_US.load(Ordering::Relaxed) / 1000)
    .push_str("ms mk=")
    .push_u32(WHERE.load(Ordering::Relaxed))
    .push_str(" +")
    .push_u32(HEAD_QUIET.swap(0, Ordering::Relaxed));
    s.emit();
}

/// The PCM ring's writer; the accounting is `orpheus_audio::pcm::Book`
/// (host-tested).
struct PcmOut {
    book: Book,
    running: bool,
    crc: u32,
}

impl PcmOut {
    fn new() -> Self {
        Self { book: Book::new(HALF as u32), running: false, crc: 0xFFFF_FFFF }
    }

    fn slot(&self) -> *mut u32 {
        let half = (self.book.written % 2) as usize;
        (PCM_ADDR as *mut u32).wrapping_add(half * HALF + self.book.fill as usize)
    }

    /// The DMA's half count and position, from the same half.
    fn reading() -> Reading {
        const BYTES: u32 = (PCM_FRAMES * 4) as u32;
        pcm::read(
            || PLAYED.load(Ordering::Acquire),
            || {
                // SAFETY: channel 5 belongs to this module; a register read.
                let rem = unsafe { DmaCh::steal(DAC_CH) }.remaining().min(BYTES);
                ((BYTES - rem) / 4) % HALF as u32
            },
        )
    }

    /// Count what a look found (underruns: halves the DAC fetched stale
    /// frames from, each once). At a half boundary a clean look clears the
    /// ledger's candidate operation.
    fn account(&self, l: Look, r: Reading, by: By) -> i32 {
        HEAD_MIN.fetch_min(l.head, Ordering::Relaxed);
        if l.head < 0 {
            head_log(l, r, &self.book, by);
        }
        if l.stale > 0 {
            critical_section::with(|cs| LEDGER.borrow(cs).borrow_mut().missed(l.stale, UB_DECODER));
            UNDERRUNS.fetch_add(l.stale, Ordering::Relaxed);
        } else if matches!(by, By::Half) {
            critical_section::with(|cs| LEDGER.borrow(cs).borrow_mut().kept_up());
        }
        l.head
    }

    /// Write one frame. True when it finished a half while the DAC runs:
    /// the writer then waits for room ([`PcmOut::half`]) before the next.
    #[inline(always)]
    fn put(&mut self, frame: u32) -> bool {
        // SAFETY: inside the PCM ring in SRAM3; the DMA reads the other half.
        unsafe { core::ptr::write_volatile(self.slot(), frame) };
        if CRC_ON.load(Ordering::Relaxed) {
            self.crc = crc32_update(self.crc, &frame.to_le_bytes());
        }
        if !self.book.wrote() {
            return false;
        }
        self.wrote_half()
    }

    /// A half is written: the DAC starts after the second. True while it
    /// runs (the writer waits for room).
    #[inline(never)]
    fn wrote_half(&mut self) -> bool {
        if !self.running && self.book.written == 2 {
            self.start();
        }
        WRITTEN.store(self.book.written, Ordering::Relaxed);
        self.running
    }

    /// Wait while both halves hold unplayed data, then the half boundary's
    /// look ([`PcmOut::after_half`]).
    async fn half(&mut self) {
        mark(3);
        Room(&self.book).await;
        self.after_half();
    }

    /// After the wait for room at a half boundary.
    #[inline(never)]
    fn after_half(&mut self) {
        let r = Self::reading();
        // The too-slow abort counts whole halves the DMA finished past the
        // writer, as before VER-12's finer count.
        let lost = r.played.wrapping_sub(self.book.written) as i32;
        if lost > 0 {
            let now = (time::now_us() / 1000) as u32;
            if now.wrapping_sub(LAST_SERVE_MS.load(Ordering::Relaxed)) > 500 {
                SLOW_UNDERRUNS.fetch_add(lost as u32, Ordering::Relaxed);
            }
        }
        // Past the DMA the writer re-syncs one half ahead of it: the next
        // push then waits for a DMA interrupt, so a decoder that cannot
        // keep up still yields (otherwise the RT executor never returns,
        // thread mode starves and the watchdog fires).
        let l = self.book.boundary(r);
        self.account(l, r, By::Half);
    }

    /// One frame, waiting for room after a finished half (the silence of
    /// `finish` and the alert tone; a block goes through `Out`).
    async fn push(&mut self, frame: u32) {
        if self.put(frame) {
            self.half().await;
        }
    }

    /// Look at the DMA before a block's pushes: after the decoder or a chore
    /// the writer is furthest behind here (pushing only gains on the DAC).
    fn look(&mut self) {
        if self.running {
            let r = Self::reading();
            let l = self.book.look(r);
            self.account(l, r, By::Block);
        }
    }

    /// Frames the DAC has ahead of it: the written halves it has not
    /// finished, less what it has fetched of the current one (its DMA
    /// position), plus the half being filled (`orpheus_audio::pcm`).
    /// `i32::MAX` while it is not running (nothing has a deadline then);
    /// < 0: it is fetching stale frames (counted as underruns).
    fn headroom(&mut self) -> i32 {
        if !self.running {
            return i32::MAX;
        }
        let r = Self::reading();
        let l = self.book.look(r);
        self.account(l, r, By::Loop)
    }

    fn start(&mut self) {
        // What ran while the DAC was stopped (an unlimited chore at a track
        // start, after a seek or a pause) had no deadline: it cannot be the
        // cause of an underrun after this.
        critical_section::with(|cs| LEDGER.borrow(cs).borrow_mut().kept_up());
        PLAYED.store(0, Ordering::Release);
        // SAFETY: channel 5 belongs to this module.
        let ch = unsafe { DmaCh::steal(DAC_CH) };
        ch.start(dac::DMA_MODE_S16 | dma::mode::RELOAD, PCM_ADDR, dac::FIFO0_ADDR, (PCM_FRAMES * 4) as u32);
        self.running = true;
    }

    /// Pad the current half with silence, let everything play out, stop.
    async fn finish(&mut self) {
        if !self.running && self.book.written == 0 && self.book.fill == 0 {
            return;
        }
        while self.book.fill != 0 {
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
        self.book.reset();
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

/// The pool (02 §1.3): the stream ring, then the decoder arena, one
/// contiguous region (`repr(C)`: the ring's last byte is followed by the
/// arena's first). A codec uses the two apart; a synth source
/// (`synth`, SYN-3) borrows both together, less the library state at the
/// arena's end, while the ring is idle (`StreamRing::lend`).
#[repr(C, align(8))]
struct Pool {
    ring: [u8; RING],
    arena: [u8; ARENA],
}
static mut POOL: Pool = Pool { ring: [0; RING], arena: [0; ARENA] };

/// The card reader's transfer buffer: the first 16 KiB of the decoder
/// arena. Only while the card is with the USB task, when this task is
/// parked in `CARD_BACK.receive()` and does not touch the arena.
///
/// # Safety
/// Call only between receiving the card from `CARD_OUT` and sending it
/// back on `CARD_BACK`, and drop the borrow before sending it back.
pub unsafe fn usb_buffer() -> &'static mut [u8; 16384] {
    // SAFETY: see above; the arena is 64 KiB, 8-byte aligned.
    unsafe { &mut *((&raw mut POOL.arena).cast::<[u8; 16384]>()) }
}
/// USB Drive's extent table (USB-6): the 8 KiB after the transfer buffer.
const EXTENTS_AT: usize = 16384;
const EXTENTS_LEN: usize = 8192;
const _: () = assert!(EXTENTS_AT + EXTENTS_LEN + crate::library::STATE <= ARENA);

/// The extent area of the arena (the card task's view: inside its arena
/// slice, before the library state).
fn extent_area(arena: &mut [u8]) -> &mut [u8] {
    &mut arena[EXTENTS_AT..EXTENTS_AT + EXTENTS_LEN]
}

/// The extent table the card task filled for USB Drive. Only while the
/// card is with the USB task, as [`usb_buffer`].
///
/// # Safety
/// As [`usb_buffer`]; the borrow ends before the card goes back.
pub unsafe fn usb_extents() -> &'static mut [u8] {
    // SAFETY: see above; the bytes lie after `usb_buffer`'s 16 KiB and
    // before the library state, inside the 8-byte aligned arena.
    unsafe {
        core::slice::from_raw_parts_mut((&raw mut POOL.arena).cast::<u8>().add(EXTENTS_AT), EXTENTS_LEN)
    }
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
    publish_card(&vol);
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

/// Card info (Settings › Storage): size and file system now; the free
/// space is counted in the idle loop ([`count_free`]).
fn publish_card(vol: &Volume<'static, Card>) {
    use crate::state::{CARD_FREE_MIB, CARD_FS, CARD_MIB, FREE_UNKNOWN};
    let bytes = u64::from(vol.cluster_count()) * u64::from(vol.cluster_size());
    CARD_MIB.store(u32::try_from(bytes >> 20).unwrap_or(u32::MAX), Ordering::Relaxed);
    CARD_FS.store(
        match vol.kind() {
            orpheus_fs::FsKind::Fat32 => crate::state::CARD_FS_FAT32,
            orpheus_fs::FsKind::ExFat => crate::state::CARD_FS_EXFAT,
            _ => crate::state::CARD_FS_OTHER,
        },
        Ordering::Relaxed,
    );
    CARD_FREE_MIB.store(FREE_UNKNOWN, Ordering::Relaxed);
}

/// The card's free space for Card info: counted once after a mount (exFAT:
/// the allocation bitmap, about 240 KB for 238 GB; FAT32: FSINFO, else one
/// scan of the FAT), then kept by the volume as files grow or go.
async fn count_free(vol: &mut Volume<'static, Card>) {
    if let Ok(b) = vol.free_bytes().await {
        let mib = u32::try_from(b >> 20).unwrap_or(crate::state::FREE_UNKNOWN - 1);
        crate::state::CARD_FREE_MIB.store(mib, Ordering::Relaxed);
    }
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
    if let SourceKind::Midi { kar } = source_of(ext_of(path)) {
        return Apart(synth::play_midi(vol, ring, arena, srv, &mut out, st, kar)).await;
    }
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
        b"mid\0" | b"midi" | b"kar\0" | b"rmi\0" => "MIDI",
        _ => "?",
    }
}

/// Serve at most one media request from inside playback. False: a request
/// for the idle loop arrived (card hand-over, power-off); the file ends.
async fn serve_one(vol: &mut Volume<'static, Card>, srv: &mut Server, scratch: &mut [u8]) -> bool {
    match media::REQS.try_receive() {
        Ok(r) => {
            let (name, t0) = (r.name(), time::now_us());
            let (pl0, sd0) = (atj2157_hal::pager::stats().loads as u32, vol.device().stats());
            let done = srv.serve(vol, r, scratch, false).await;
            let took = time::now_us() - t0;
            if took >= 30_000 {
                // A request that blocks the playback loop this long can
                // empty the DAC ring (soak, 2026-11-25: 309 ms). Where the
                // time went: code lines paged in (`pl`), blocks read and
                // written (`r`, `w`) and the card's busy polls after writes
                // (`bp`, ~0.25 ms each) (2026-11-25: games pieces of 33–40
                // ms against a few sector I/Os on the host).
                let sd1 = vol.device().stats();
                let mut l = atj2157_hal::log::LineBuf::new();
                l.push_str("serve: ")
                    .push_str(name)
                    .push_str(" ")
                    .push_u32((took / 1000) as u32)
                    .push_str(" ms while playing pl=")
                    .push_u32((atj2157_hal::pager::stats().loads as u32).wrapping_sub(pl0))
                    .push_str(" r=")
                    .push_u32(sd1.blocks_read.wrapping_sub(sd0.blocks_read))
                    .push_str(" w=")
                    .push_u32(sd1.blocks_written.wrapping_sub(sd0.blocks_written))
                    .push_str(" bp=")
                    .push_u32(sd1.busy_polls.wrapping_sub(sd0.busy_polls));
                l.emit();
            }
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
fn handed_over(out: &mut PcmOut) {
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
        ring.next = Some(Next { item, track, ext, ext_len, title, prep: None, prep_at: 0, prep_reads: 0 });
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
/// decoder opens. Up to 8 KiB per read, at most 4 reads, as many as the
/// chore's budget allows (`piece_left`; the rest in the next chore); a
/// stream that needs more is left to the hand-over.
async fn prepare(vol: &mut Volume<'static, Card>, ring: &mut Ring, buf: &mut [u8]) {
    let Some(mut f) = ring.next_file.clone() else { return };
    let Some(n) = ring.next.as_ref().filter(|n| n.prep.is_none()) else { return };
    let (ext, ext_len) = (n.ext, usize::from(n.ext_len));
    let (mut off, mut reads) = (u64::from(n.prep_at), n.prep_reads);
    let ext = ext.get(..ext_len).filter(|e| !e.is_empty());
    let len = f.size();
    let n = buf.len().min(8192);
    let Some(buf) = buf.get_mut(..n) else { return };
    let mut r = None;
    while reads < 4 {
        if !piece_left() {
            break;
        }
        reads += 1;
        f.set_position(off);
        let got = vol.read_exact_or_eof(&mut f, buf).await.unwrap_or(0);
        let mut w = orpheus_audio::WindowSource::new(buf.get(..got).unwrap_or_default(), off, Some(len));
        match inspect_head(&mut w, ext) {
            HeadStep::Ready(p) => {
                r = Some(Ok(p));
                break;
            }
            HeadStep::SkipTo(at) if at < len => off = at,
            _ => {
                r = Some(Err(()));
                break;
            }
        }
    }
    if let Some(n) = ring.next.as_mut() {
        // Four reads, or a head beyond 4 GiB: left to the hand-over.
        n.prep = r.or((reads >= 4 || off > u64::from(u32::MAX)).then_some(Err(())));
        n.prep_at = u32::try_from(off).unwrap_or(u32::MAX);
        n.prep_reads = reads;
    }
}

// --------------------------------------------------------- the block path
//
// AU-PERF (2026-11-25): the code that runs for every block (decode, DSP,
// PCM ring, the looks at the DMA, the refill check, the chores check) is
// out of the large async functions: `play_block` (polled through `Apart`,
// so it is a function of its own) holds the awaits of one block and calls
// small non-async functions with stable names (`decode_timed`, the
// `Feeder` over `Out`, `after_block`, `chore_due`, `reader_check`); the rare
// paths (requests, pause, chores, logs) are futures of their own too
// (`head`, `pause`, `chores`). `hot.toml` packs the block path in the
// `play-block` cluster. Behaviour is that of the single loop before.

/// A future polled through a function of its own: its state machine is not
/// inlined into the caller's (whose poll then stays small), and its code is
/// one section `hot.toml` can pack (`<firmware::audio::Apart<…>>::poll`).
struct Apart<F>(F);

impl<F: Future> Future for Apart<F> {
    type Output = F::Output;

    #[inline(never)]
    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<F::Output> {
        // SAFETY: structural pinning of the only field: `Apart` never moves
        // it out and has no `Drop` of its own.
        unsafe { self.map_unchecked_mut(|s| &mut s.0) }.poll(cx)
    }
}

/// The PCM ring has room for a half (or a stop is asked for).
struct Room<'a>(&'a Book);

impl Future for Room<'_> {
    type Output = ();

    #[inline(never)]
    fn poll(self: core::pin::Pin<&mut Self>, cx: &mut core::task::Context<'_>) -> Poll<()> {
        PCM_WAKER.register(cx.waker());
        if !self.0.full(PLAYED.load(Ordering::Acquire)) || STOP.load(Ordering::Relaxed) {
            Poll::Ready(())
        } else {
            Poll::Pending
        }
    }
}

/// What the UI shows of a track once it plays (`media::PlayerSnap`).
#[derive(Clone, Copy)]
struct Shown {
    paused: bool,
    start_ms: u32,
    duration_ms: u32,
    codec: &'static str,
    rate: u32,
    channels: u8,
    bits: u8,
    size: u64,
    kbps: u16,
}

impl Shown {
    fn apply(&self, p: &mut media::PlayerSnap) {
        p.source = if self.paused { Source::Paused } else { Source::Playing };
        p.position_ms = self.start_ms;
        p.duration_ms = self.duration_ms;
        p.codec = self.codec;
        p.rate = self.rate;
        p.channels = self.channels;
        p.bits = self.bits;
        p.size = self.size;
        p.kbps = self.kbps;
    }
}

/// Milliseconds of `frame` at `rate`.
fn ms_at(rate: u64, frame: u64) -> u32 {
    (frame * 1000 / rate.max(1)) as u32
}

/// The playback loop's state for one file (`play_file`).
struct Blk {
    st: Start,
    info: orpheus_audio::StreamInfo,
    show: Shown,
    /// Source rate (the player's time line).
    rate: u64,
    ch: usize,
    stereo: bool,
    /// The DSP chain runs (`dsp on|off`).
    dsp: bool,
    /// The player drives this file (`st.track`).
    driven: bool,
    /// Source frames to the current position.
    pos: u64,
    /// Frames written to the PCM ring.
    frames: u32,
    flow: Flow,
    sound_gen: u32,
    decode_us: u64,
    paused: bool,
    /// The pause has been carried out (DAC stopped, state saved).
    held: bool,
    /// Fade on pause: the output fades out before the pause is carried out
    /// (`pause_fades`).
    fading: bool,
    reader_end: bool,
    last_tick: u64,
    /// The decoder reached the end of the stream.
    natural: bool,
    // Chores (see `ROOMY`): the read-ahead open, a waiting request, the
    // gapless track's library place (1: start, 2: steps) and art (3).
    serve_due: bool,
    pf_due: Option<u32>,
    meta: u8,
    lib_find: Option<orpheus_library::FindStep>,
    /// The folder of the track before (a gapless track in the same folder
    /// keeps its art: an album's cover).
    prev_dir: u32,
    chore_since: u64,
    /// The longest recent decoder call (`chore::margin_us`).
    dec: DecPeak,
    /// A gapless join being measured: 1 until the first block, 2 until two
    /// halves are out.
    join: u8,
    early: Option<u32>,
}

/// The chore to run now (`chore_due`): which, its kind (`COSTS`), the
/// piece's budget (µs), and whether it ran starved.
#[derive(Clone, Copy)]
struct Due {
    what: Chore,
    kind: usize,
    budget: u32,
    starved: bool,
}

/// The chores, in the order they are tried.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Chore {
    Prefetch,
    Prepare,
    Serve,
    Job,
    /// Books card work (`crate::books::chore`), after the listing pieces.
    Books,
    Meta,
}

/// What `play_block` found.
enum Went {
    /// On to the next block.
    On,
    /// A pause was asked for: carry it out (`pause`), then go on.
    Pause,
    /// The file's playback ends.
    End(Result<End, &'static [u8]>),
}

/// The decoder's next step, timed (the `status` `dec:` numbers and the
/// RT-side timing).
#[inline(never)]
fn decode_timed<'d>(
    dec: &'d mut orpheus_codecs::AnyDecoder<'_>,
    ring: &mut Ring,
) -> (Result<Step<'d>, orpheus_audio::DecodeError>, u64) {
    let d0 = time::now_us();
    let l0 = pager_loads();
    mark(4);
    let step = dec.decode(ring);
    let dt = time::now_us() - d0;
    T_DECODE.fetch_add(dt as u32, Ordering::Relaxed);
    DEC_N.fetch_add(1, Ordering::Relaxed);
    DEC_US.fetch_add(dt as u32, Ordering::Relaxed);
    DEC_MAX_US.fetch_max(dt as u32, Ordering::Relaxed);
    LAST_DEC_US.store(dt as u32, Ordering::Relaxed);
    DEC_LOADS.fetch_add(pager_loads().wrapping_sub(l0), Ordering::Relaxed);
    (step, dt)
}

/// The writer of one block's pieces (`orpheus_audio::pcm::Feeder`): the
/// DSP over a copy of each piece (the decoder's output buffer may be
/// decoder state: processing it in place corrupted the MP3 decoder on the
/// player, 2026-11-24), the look at the DMA, then the frames.
struct Out<'a> {
    out: &'a mut PcmOut,
    chain: &'a mut orpheus_dsp::Chain,
    dsp: bool,
    stereo: bool,
    frames: &'a mut u32,
}

impl Sink<i16> for Out<'_> {
    fn piece(&mut self, s: &mut [i16]) -> usize {
        mark(5);
        let n = if self.dsp { self.chain.process(&mut PcmBlock::I16(s)) } else { s.len() };
        mark(0);
        if CLICK_AT.load(Ordering::Relaxed) != u32::MAX {
            click_mix(s, n, 1 + usize::from(self.stereo), orpheus_dsp::click::mix_i16);
        }
        // TTS-3: the speech after the chain (which it bypasses).
        if speech::wanted() {
            speech::mix_i16(s, n, 1 + usize::from(self.stereo));
        }
        self.out.look();
        n
    }

    #[inline(always)]
    fn frame(&mut self, f: &[i16]) -> bool {
        let l = f.first().copied().unwrap_or(0) as u16;
        let r = if self.stereo { f.get(1).copied().unwrap_or(0) as u16 } else { l };
        *self.frames += 1;
        self.out.put(u32::from(l) | u32::from(r) << 16)
    }
}

impl Sink<i32> for Out<'_> {
    fn piece(&mut self, s: &mut [i32]) -> usize {
        mark(5);
        let n = if self.dsp { self.chain.process(&mut PcmBlock::I32(s)) } else { s.len() };
        mark(0);
        if CLICK_AT.load(Ordering::Relaxed) != u32::MAX {
            click_mix(s, n, 1 + usize::from(self.stereo), orpheus_dsp::click::mix_i32);
        }
        if speech::wanted() {
            speech::mix_i32(s, n, 1 + usize::from(self.stereo));
        }
        self.out.look();
        n
    }

    #[inline(always)]
    fn frame(&mut self, f: &[i32]) -> bool {
        let l = (f.first().copied().unwrap_or(0) >> 16) as u16;
        let r = if self.stereo { (f.get(1).copied().unwrap_or(0) >> 16) as u16 } else { l };
        *self.frames += 1;
        self.out.put(u32::from(l) | u32::from(r) << 16)
    }
}

/// Settings › Sound › Fade on pause (VER-13): a pause while the DAC plays
/// fades the output out over the next blocks first (`Chain::begin_pause`;
/// `play_block` carries the pause out once the fade is done, `pause` lets
/// the faded end play out). True while it fades: go on decoding.
#[inline(never)]
fn pause_fades(b: &mut Blk, out: &PcmOut, chain: &mut orpheus_dsp::Chain) -> bool {
    if !b.fading && !b.held && b.dsp && out.running && media::opts() & media::opt::FADE != 0 {
        b.fading = chain.begin_pause(true);
    }
    b.fading
}

/// Playing again after a pause (fading in with Fade on pause), or while
/// its fade-out ran (back up from where it is). Out of `play_block`: its
/// lines are the per-block code's (hot.toml `play-block`).
#[inline(never)]
fn resumed(b: &mut Blk, chain: &mut orpheus_dsp::Chain) {
    if b.held {
        b.held = false;
        set_paused(false);
        b.last_tick = time::now_us();
        chain.end_pause(media::opts() & media::opt::FADE != 0);
        media::player_update(|p| p.source = Source::Playing);
    } else {
        b.fading = false;
        chain.fade_in(orpheus_dsp::PAUSE_FADE_MS);
    }
}

/// A track starts fresh: the output at full level (a pause's fade-out may
/// have left it at silence).
#[inline(never)]
fn full_level(chain: &mut orpheus_dsp::Chain) {
    chain.end_pause(false);
}

/// A new block: the end of a gapless join's hand-over, a change of the
/// sound settings.
#[inline(never)]
fn begin_block(b: &mut Blk, chain: &mut orpheus_dsp::Chain) {
    // TTS-3: speech alone had the DAC (paused): the music's again.
    if speech::alone_running() {
        speech::end_alone();
    }
    if b.join == 1 {
        // The hand-over is over when the new decoder has a block: noted
        // before its first pushes see the result.
        jset(J_OPEN_US, now32().wrapping_sub(jget(J_T0)));
        jset(J_LOADS, pager_loads().wrapping_sub(jget(J_L0)));
        note(UB_JOIN, jget(J_T0));
        b.join = 2;
    }
    let g = media::SOUND_GEN.load(Ordering::Acquire);
    if g != b.sound_gen {
        b.sound_gen = g;
        dsp_clock_floor(sound_active());
        mark(7);
        media::apply_sound(chain);
    }
}

/// After a block's frames are written: the position (UI, player), the
/// player's events, the gapless join's end. Returns whether the stream
/// ring wants a refill.
#[inline(never)]
fn after_block(b: &mut Blk, ring: &mut Ring, srv: &mut Server, samples: usize) -> bool {
    // Source frames: the player's time line (the DSP may decimate).
    b.pos += (samples / b.ch) as u64;
    FRAMES.store(b.frames, Ordering::Relaxed);
    let (rate, pos) = (b.rate, b.pos);
    media::player_update(|p| p.position_ms = ms_at(rate, pos));
    if let (Some(item), Some(track)) = (b.early.take(), b.st.track) {
        // Now the player hears that the previous track ended (it answers
        // with this item) and that this one opened.
        let mut fl = srv.feed(Event::DecoderEnded);
        if fl.open.is_some_and(|o| o.target.is_item(item)) {
            let opened = srv.feed(Event::Opened { track, rate: b.info.rate, frames: b.info.frames });
            fl.open = None;
            // A long file's automatic bookmark: a seek.
            fl.merge(opened);
        }
        b.flow.merge(fl);
    }
    if b.join == 2 && b.frames >= 2 * HALF as u32 {
        // Underruns are seen at half boundaries: by now the join's are
        // counted. The UI hears of the new track now (no card work: the
        // title came with the read-ahead).
        jset(J_UNDER, UNDERRUNS.load(Ordering::Relaxed));
        b.join = 0;
        b.prev_dir = critical_section::with(|cs| media::PLAYER.borrow(cs).borrow().file.dir);
        ring.show_handed();
        let show = b.show;
        media::player_update(|p| show.apply(p));
    }
    if b.driven {
        b.flow.merge(srv.position(pos));
        let now = time::now_us();
        if now - b.last_tick >= 1_000_000 {
            let ms = ((now - b.last_tick) / 1000) as u32;
            b.last_tick = now;
            b.flow.merge(srv.feed(Event::Tick { ms }));
        }
    }
    ring.available() < RING / 2
}

/// The first chore due, if `chore::decide` lets it run now (`ROOMY`); the
/// others wait behind it. `None`: none now.
#[inline(never)]
fn chore_due(b: &mut Blk, ring: &Ring, srv: &Server, out: &mut PcmOut) -> Option<Due> {
    b.dec = b.dec.after(LAST_DEC_US.load(Ordering::Relaxed));
    // Requests and listing pieces wait while the current file's last bytes
    // play (≤ 0.7 s: the hand-over comes first).
    let tail = ring.eof() && ring.next.is_some();
    // A request may be waiting (looked at, not taken: its kind decides).
    let req = if b.serve_due && !tail { media::REQS.try_peek().ok().map(|r| r.kind()) } else { None };
    if b.serve_due && !tail && req.is_none() {
        b.serve_due = false;
    }
    let (what, kind) = if b.pf_due.is_some() {
        (Chore::Prefetch, K_PF)
    } else if ring.next.as_ref().is_some_and(|n| n.prep.is_none()) {
        // The file read ahead, probed and inspected before the end.
        (Chore::Prepare, K_PREP)
    } else if let Some(r) = req {
        (Chore::Serve, K_SERVE + r)
    } else if srv.has_job() && !tail {
        // A listing being made in pieces (Files while playing).
        (Chore::Job, K_JOB + srv.job_kind())
    } else if crate::books::wants() && !tail {
        (Chore::Books, K_JOB + media::JOB_BOOKS + crate::books::kind())
    } else if b.meta != 0 && b.join == 0 && u64::from(b.frames) * 1000 >= META_AFTER_MS * b.rate {
        // The gapless track's library place and art once its first seconds
        // have played.
        (Chore::Meta, if b.meta == 3 { K_ART } else { K_LIB })
    } else {
        b.chore_since = 0;
        return None;
    };
    let now = time::now_us();
    if b.chore_since == 0 {
        b.chore_since = now;
    }
    let a = Ask {
        head: out.headroom(),
        rate: RATE.load(Ordering::Relaxed),
        roomy: ROOMY,
        need_us: need_of(kind),
        waited_us: (now - b.chore_since).min(u64::from(u32::MAX)) as u32,
        dec_us: b.dec.0,
    };
    let v = chore::decide(&a);
    let budget = v.budget()?;
    b.chore_since = 0;
    Some(Due { what, kind, budget, starved: matches!(v, Verdict::Starved { .. }) })
}

/// The battery or step log (kind `kind`) may write now: never forced (no
/// wait), only roomy, only when its need fits, only when no chore waits.
/// Returns the budget.
#[inline(never)]
fn log_due(b: &Blk, out: &mut PcmOut, kind: usize) -> Option<u32> {
    if b.chore_since != 0 {
        return None;
    }
    let a = Ask {
        head: out.headroom(),
        rate: RATE.load(Ordering::Relaxed),
        roomy: ROOMY,
        need_us: need_of(kind),
        waited_us: 0,
        dec_us: b.dec.0,
    };
    chore::decide(&a).budget()
}

/// The current file's last bytes are near: the player hears of the
/// reader's end once (it answers with `Prefetch`: the next file is read
/// after this one, gapless).
#[inline(never)]
fn reader_check(b: &mut Blk, ring: &Ring, srv: &mut Server) {
    if b.driven && !b.reader_end && ring.s.file_left().is_some_and(|l| l <= READ_AHEAD_LEAD) {
        b.reader_end = true;
        b.flow.merge(srv.feed(Event::ReaderAtEnd));
    }
}

/// A request that arrived, a stop, the player's actions (`flow`). `Some`:
/// the file's playback ends.
#[allow(clippy::too_many_arguments)]
async fn head(
    b: &mut Blk,
    got: Option<Result<PlayReq, Req>>,
    dec: &mut orpheus_codecs::AnyDecoder<'_>,
    ring: &mut Ring,
    vol: &mut Volume<'static, Card>,
    srv: &mut Server,
    out: &mut PcmOut,
    spare: &mut [u8],
) -> Option<Result<End, &'static [u8]>> {
    match got {
        // The player's answer is CPU only (its code may page in): noted, so
        // an underrun after it names it (`ub=req:<label>`), not a chore.
        Some(Ok(PlayReq::Edit(e))) if b.driven => {
            let t0 = now32();
            b.flow.merge(srv.edit(e, vol.serial()));
            note_as(UB_SERVE, D_REQ + 7, t0);
        }
        Some(Ok(r))
            if b.driven
                && !matches!(
                    r,
                    PlayReq::PlayDir { .. }
                        | PlayReq::PlayLib { .. }
                        | PlayReq::ShuffleAll
                        | PlayReq::Shutdown
                ) =>
        {
            let (t0, d) = (now32(), req_detail(&r));
            b.flow.merge(srv.play_req(r));
            note_as(UB_SERVE, d, t0);
        }
        Some(Ok(r)) => {
            // A new queue, power-off, or any player request during
            // `play PATH`: for the idle loop.
            srv.pending_play = Some(r);
            return Some(Ok(End::Pending));
        }
        Some(Err(r)) => {
            // A card hand-over ends the file (for the idle loop).
            let go_on = srv.serve(vol, r, spare, false).await;
            if !go_on {
                return Some(Ok(End::Pending));
            }
        }
        None => {}
    }
    if STOP.swap(false, Ordering::Relaxed) {
        if b.driven {
            srv.play_req(PlayReq::Stop);
        }
        return Some(Ok(End::Idle));
    }
    if !b.flow.is_empty() {
        let fl = core::mem::take(&mut b.flow);
        if let Some(o) = fl.open {
            return Some(Ok(End::Open(o)));
        }
        if fl.stop {
            return Some(Ok(End::Idle));
        }
        if fl.cancel {
            // The next track changed: whatever was read ahead goes, and the
            // reader's end is reported again (a new `Prefetch`).
            ring.cancel_next();
            b.pf_due = None;
            b.reader_end = false;
        }
        if let Some(item) = fl.prefetch() {
            b.pf_due = Some(item);
        }
        if let Some(target) = fl.seek {
            // Stop the DAC for the seek: the bisection reads the card for a
            // while, which is not an underrun (and must not count towards
            // the too-slow abort).
            out.stop();
            match seek_to(dec, ring, vol, target).await {
                Some(at) => {
                    b.pos = at;
                    b.reader_end = false;
                    let (rate, pos) = (b.rate, b.pos);
                    media::player_update(|p| p.position_ms = ms_at(rate, pos));
                }
                // The decoder's state is unknown after a failed seek.
                None => return Some(failed(srv, b.driven, b"seek failed")),
            }
        }
        if let Some(p) = fl.pause {
            b.paused = p;
        }
    }
    None
}

/// Paused: the pause carried out once (DAC stopped, state saved); then the
/// battery log, the step counter's file and trace, and a listing in pieces
/// while the DAC is stopped; an alert sounds (the UI paused the music for
/// it).
async fn pause(
    b: &mut Blk,
    vol: &mut Volume<'static, Card>,
    srv: &mut Server,
    out: &mut PcmOut,
    spare: &mut [u8],
) {
    if !b.held {
        b.held = true;
        set_paused(true);
        // Faded out: the faded end plays out first.
        let faded = core::mem::take(&mut b.fading);
        if faded {
            out.finish().await;
        } else {
            out.stop();
        }
        stats_set(0xFFF, if faded { orpheus_dsp::PAUSE_FADE_MS } else { 0 });
        media::player_update(|p| p.source = Source::Paused);
        save_store(vol, srv, spare).await;
    }
    // TTS-3: the screen reader alone while paused (Music while speaking
    // › Paused, or a pause by hand).
    speech::alone();
    if CLICK_AT.load(Ordering::Relaxed) != u32::MAX {
        if speech::alone_running() {
            // A key while it speaks: the reader answers it.
            CLICK_AT.store(u32::MAX, Ordering::Relaxed);
        } else {
            click_alone(out).await;
        }
    }
    if crate::drain::due() {
        drain_flush_rows(vol, spare).await;
    }
    if crate::sensor::card_due() {
        sensor_flush_rows(vol, spare).await;
    }
    if ALERT.load(Ordering::Relaxed) {
        speech::end_alone();
        alert_tone(out).await;
    }
    while srv.has_job() && media::REQS.is_empty() && media::PLAY.is_empty() {
        srv.step(vol).await;
    }
    // Books card work while the DAC is stopped (no scratch: the decoder
    // is loaded; folders may be made).
    if crate::books::wants() {
        Apart(crate::books::run_paused(vol)).await;
    }
}

/// The paused loops' wait (codecs, synth): 200 ms, or speech to take (the
/// screen reader alone while paused, TTS-3), or an `ls` to answer
/// ([`PAUSED_CMDS`]).
pub(super) async fn paused_wait(vol: &mut Volume<'static, Card>) {
    if let Either3::Third(c) = select3(Timer::after_millis(200), speech::wait(), PAUSED_CMDS.receive()).await
    {
        let text = core::str::from_utf8(&c.text[..c.len]).unwrap_or("");
        if let Some(p) = text.strip_prefix("ls ") {
            Apart(ls(vol, p.trim())).await;
        }
    }
}

/// One chore (the first due, in this order). `Some`: the file's playback
/// ends (a request for the idle loop).
async fn chores(
    b: &mut Blk,
    due: Due,
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    srv: &mut Server,
    spare: &mut [u8],
) -> Option<Result<End, &'static [u8]>> {
    mark(2);
    let t0 = now32();
    piece_begin(due.kind, due.budget);
    let mut end = None;
    // The read-ahead first: the current file's last bytes may last only
    // ~0.15 s (FLAC).
    if due.what == Chore::Prefetch {
        if let Some(item) = b.pf_due.take() {
            prefetch(vol, ring, srv, item).await;
        }
        jset(J_PREFETCH_US, now32().wrapping_sub(t0));
        note_as(UB_PREFETCH, due.kind as u8, t0);
    } else if due.what == Chore::Prepare {
        let l0 = pager_loads();
        Apart(prepare(vol, ring, spare)).await;
        // The reads of all its pieces (each its own chore).
        jadd(J_PREP_US, now32().wrapping_sub(t0));
        jadd(J_PREP_LOADS, pager_loads().wrapping_sub(l0));
        note_as(UB_PREPARE, due.kind as u8, t0);
    } else if due.what == Chore::Serve {
        b.serve_due = false;
        let go_on = serve_one(vol, srv, spare).await;
        T_SERVE.fetch_add(now32().wrapping_sub(t0), Ordering::Relaxed);
        note_as(UB_SERVE, due.kind as u8, t0);
        if !go_on {
            end = Some(Ok(End::Pending));
        }
    } else if due.what == Chore::Job {
        srv.step(vol).await;
        T_SERVE.fetch_add(now32().wrapping_sub(t0), Ordering::Relaxed);
        note_as(UB_SERVE, due.kind as u8, t0);
    } else if due.what == Chore::Books {
        Apart(crate::books::chore(vol)).await;
        T_SERVE.fetch_add(now32().wrapping_sub(t0), Ordering::Relaxed);
        note_as(UB_SERVE, due.kind as u8, t0);
    } else if b.meta == 1 {
        // A folder that missed before, or no library: nothing to look for.
        b.lib_find = b.st.track.and_then(|t| crate::library::find_begin(srv, t.locator()));
        b.meta = if b.lib_find.is_some() { 2 } else { 3 };
        jset(J_LIB_US, 0);
    } else if b.meta == 2 {
        // One probe per chore.
        let done = match (b.lib_find.as_mut(), b.st.track) {
            (Some(f), Some(t)) => crate::library::find_more(vol, srv, f, t.locator()).await,
            _ => true,
        };
        if done {
            b.meta = 3;
        }
        jset(J_LIB_US, jget(J_LIB_US).max(now32().wrapping_sub(t0)));
        note_as(UB_LIB, due.kind as u8, t0);
    } else {
        b.meta = 0;
        // An album's next track keeps the cover shown.
        if let Some(t) = b.st.track.filter(|t| t.locator().dir != b.prev_dir) {
            crate::pictures::track_art_cached(vol, srv, t.locator()).await;
        }
        jset(J_ART_US, now32().wrapping_sub(t0));
        note_as(UB_ART, due.kind as u8, t0);
        jset(J_UNDER_META, UNDERRUNS.load(Ordering::Relaxed));
    }
    piece_end(due.kind, due.starved);
    mark(0);
    LAST_SERVE_MS.store((time::now_us() / 1000) as u32, Ordering::Relaxed);
    end
}

/// One pass of the playback loop while playing: a request (unless `polled`
/// is false: the paused loop took it), then one block of the decoder to
/// the PCM ring, the refill, the chores, the logs.
#[allow(clippy::too_many_arguments)]
async fn play_block(
    b: &mut Blk,
    polled: bool,
    dec: &mut orpheus_codecs::AnyDecoder<'_>,
    ring: &mut Ring,
    vol: &mut Volume<'static, Card>,
    srv: &mut Server,
    out: &mut PcmOut,
    chain: &mut orpheus_dsp::Chain,
    spare: &mut [u8],
) -> Went {
    if polled {
        // Player requests: one per block.
        let got = media::PLAY.try_receive().ok().map(Ok);
        if (got.is_some() || STOP.load(Ordering::Relaxed) || !b.flow.is_empty())
            && let Some(e) = Apart(head(b, got, dec, ring, vol, srv, out, spare)).await
        {
            return Went::End(e);
        }
        if b.paused && !pause_fades(b, out, chain) {
            return Went::Pause;
        }
    }
    if b.held || b.fading && !b.paused {
        resumed(b, chain);
    }
    if SLOW_UNDERRUNS.load(Ordering::Relaxed) >= MAX_UNDERRUNS {
        // The decoder cannot keep up (Opus on paged code): stop the track
        // rather than starve the rest of the system.
        return Went::End(failed(srv, b.driven, b"decoder too slow for this player"));
    }
    let (step, dt) = decode_timed(dec, ring);
    b.decode_us += dt;
    match step {
        Ok(Step::Pcm(block)) => {
            begin_block(b, chain);
            let (dsp, stereo, ch) = (b.dsp, b.stereo, b.ch);
            let mut feed = Feeder::new();
            let samples = match block {
                PcmBlock::I16(s) => {
                    let sc = scratch16(SCRATCH_FRAMES * ch);
                    while feed.run(s, sc, ch, &mut Out { out, chain, dsp, stereo, frames: &mut b.frames })
                        == Fed::Wait
                    {
                        out.half().await;
                    }
                    s.len()
                }
                PcmBlock::I32(s) => {
                    let sc = scratch32(SCRATCH_FRAMES * ch);
                    while feed.run(s, sc, ch, &mut Out { out, chain, dsp, stereo, frames: &mut b.frames })
                        == Fed::Wait
                    {
                        out.half().await;
                    }
                    s.len()
                }
            };
            if after_block(b, ring, srv, samples) {
                let (a, head) = (now32(), out.headroom());
                mark(1);
                // The next file's head only with room to spare: near a
                // file's end the decoder must stay ahead of the DAC.
                {
                    // A deadline: no card rests (yoton_bsp::card::Urgent).
                    let _urgent = yoton_bsp::card::Urgent::enter();
                    ring.fill_in(vol, None, head >= ROOMY).await;
                }
                mark(0);
                note(UB_FILL, a);
                T_FILL.fetch_add(now32().wrapping_sub(a), Ordering::Relaxed);
                // A request may be waiting: served as a chore.
                b.serve_due = true;
            }
            if let Some(due) = chore_due(b, ring, srv, out)
                && let Some(e) = Apart(chores(b, due, vol, ring, srv, spare)).await
            {
                return Went::End(e);
            }
            // The battery log (PWR-5): only with room to spare and its
            // need covered, never forced like the chores above.
            if crate::drain::due()
                && let Some(budget) = log_due(b, out, K_DRAIN)
            {
                let t0 = now32();
                piece_begin(K_DRAIN, budget);
                Apart(drain_flush_rows(vol, spare)).await;
                piece_end(K_DRAIN, false);
                note_as(UB_SERVE, K_DRAIN as u8, t0);
            }
            // The step counter's file and trace (SENS-1): the same rule.
            if crate::sensor::card_due()
                && let Some(budget) = log_due(b, out, K_SENSOR)
            {
                let t0 = now32();
                piece_begin(K_SENSOR, budget);
                Apart(sensor_flush_rows(vol, spare)).await;
                piece_end(K_SENSOR, false);
                note_as(UB_SERVE, K_SENSOR as u8, t0);
            }
            reader_check(b, ring, srv);
            if b.fading && b.paused && chain.fade_done() {
                return Went::Pause;
            }
            Went::On
        }
        Ok(Step::NeedData(_)) => {
            if ring.eof() {
                b.natural = true;
                return Went::End(Ok(End::Idle));
            }
            let (a, head) = (now32(), out.headroom());
            let filled = {
                let _urgent = yoton_bsp::card::Urgent::enter();
                ring.fill_in(vol, None, head >= ROOMY).await
            };
            note(UB_FILL, a);
            if !filled {
                // The ring is full and the decoder still wants more: it can
                // never be satisfied; end the track instead of spinning.
                CORRUPT.fetch_add(1, Ordering::Relaxed);
                return Went::End(failed(srv, b.driven, b"stream needs a larger ring"));
            }
            // A request may be waiting: served as a chore.
            b.serve_due = true;
            Went::On
        }
        Ok(Step::EndOfStream) => {
            b.natural = true;
            Went::End(Ok(End::Idle))
        }
        Err(_) => {
            CORRUPT.fetch_add(1, Ordering::Relaxed);
            if CORRUPT.load(Ordering::Relaxed) > 10 {
                return Went::End(failed(srv, b.driven, b"too many corrupt frames"));
            }
            Went::On
        }
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
    // The decoder stays where `open` returned it, in this future, and is
    // used by reference: moved out of the `Result` by value it took two
    // copies (976 B each) in this poll function's frame, above the playback
    // loop's deepest chains (VER-10).
    let mut opened = orpheus_codecs::open(format, ring, dec_mem, &hint);
    let Ok((dec, info)) = &mut opened else { return Err(b"open decoder failed") };
    let info: &orpheus_audio::StreamInfo = info;
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
    let sound_gen = media::SOUND_GEN.load(Ordering::Acquire);
    // Vorbis and Opus need High at the moment (paged decoders).
    let heavy = matches!(codec_name(ext), "Vorbis" | "Opus");
    CODEC_FLOOR.store(if heavy { 2 } else { 0 }, Ordering::Relaxed);
    dsp_clock_floor(sound_active());
    // Start inside the track (resume, bookmark): before any output.
    let mut pos = 0u64;
    if st.frame > 0 {
        match seek_to(dec, ring, vol, st.frame).await {
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
        full_level(chain);
    }
    mark(8);
    let dsp = DSP_ON.load(Ordering::Relaxed);
    if !out.running {
        dac::set_rate(out_rate);
    }
    RATE.store(out_rate, Ordering::Relaxed);
    let stereo = info.channels == 2;
    let rate = u64::from(info.rate);
    let duration_ms = info.frames.map_or(0, |f| ms_at(rate, f));
    let show = Shown {
        paused: st.paused,
        start_ms: ms_at(rate, pos),
        duration_ms,
        codec: codec_name(ext),
        rate: info.rate,
        channels: info.channels,
        bits: info.bits,
        size,
        // The decoder's average, else size over duration (a 48 kHz MP3
        // showed 0 kbit/s, 2026-11-24).
        kbps: orpheus_apps::services::average_kbps(info.bitrate, size, duration_ms),
    };
    // After a gapless change the UI hears of the new track once its
    // decoder runs (`after_block`).
    if !st.gapless {
        media::player_update(|p| show.apply(p));
    }
    let driven = st.track.is_some();
    // The player's answer to `Opened`: a long file's automatic bookmark
    // (a seek, carried out at the first block).
    let mut flow = Flow::default();
    if let Some(track) = st.track.filter(|_| st.early.is_none()) {
        flow = srv.feed(Event::Opened { track, rate: info.rate, frames: info.frames });
        // A track change: save while the DAC is not running yet (at a
        // gapless one it runs: the next safe moment saves).
        if !out.running {
            save_store(vol, srv, spare).await;
        }
    }
    PLAYING.store(true, Ordering::Relaxed);
    PAUSED.store(false, Ordering::Relaxed);
    STOP.store(false, Ordering::Relaxed);
    let t0 = time::now_us();
    let mut b = Blk {
        st,
        info: *info,
        show,
        rate,
        ch: if stereo { 2 } else { 1 },
        stereo,
        dsp,
        driven,
        pos,
        frames: 0,
        flow,
        sound_gen,
        decode_us: 0,
        paused: st.paused,
        held: false,
        fading: false,
        reader_end: false,
        last_tick: t0,
        natural: false,
        serve_due: false,
        pf_due: None,
        meta: if st.gapless { 1 } else { 0 },
        lib_find: None,
        prev_dir: u32::MAX,
        chore_since: 0,
        dec: DecPeak(0),
        join: if st.gapless { 1 } else { 0 },
        early: st.early,
    };
    let end = 'play: loop {
        // While paused, wait for a request (one per pass).
        let polled = if b.paused && !b.fading {
            let got = match select3(media::PLAY.receive(), media::REQS.receive(), paused_wait(vol)).await {
                Either3::First(r) => Some(Ok(r)),
                Either3::Second(r) => Some(Err(r)),
                Either3::Third(()) => None,
            };
            if let Some(e) = Apart(head(&mut b, got, dec, ring, vol, srv, out, spare)).await {
                break 'play e;
            }
            if b.paused {
                Apart(pause(&mut b, vol, srv, out, spare)).await;
                continue;
            }
            false
        } else {
            true
        };
        match Apart(play_block(&mut b, polled, dec, ring, vol, srv, out, chain, spare)).await {
            Went::On => {}
            Went::Pause => Apart(pause(&mut b, vol, srv, out, spare)).await,
            Went::End(e) => break 'play e,
        }
    };
    let Blk { early, join, natural, decode_us, show, .. } = b;
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
        media::player_update(|p| show.apply(p));
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
        stats_set(0x3000, 1 << 12);
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
                stats_set(0x3000, 1 << 12);
                handed_over(out);
                Ok(End::Open(o))
            }
            Some(mut o) => {
                // Gapless off (nothing read ahead), a pause, another rate.
                stats_set(0x3000, 2 << 12);
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
        PAUSED.store(false, Ordering::Relaxed);
        hand_paused_cmd_on();
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

/// Hand the card (the volume in `slot`) to the USB task and wait for it to
/// come back, remounted into `slot` (`None` there if the remount failed).
async fn release(slot: &mut Option<Volume<'static, Card>>) {
    let Some(vol) = slot.take() else { return };
    crate::state::CARD_READY.store(false, Ordering::Relaxed);
    media::player_update(|p| p.source = Source::Idle);
    let (card, _) = vol.unmount().await;
    CARD_OUT.send(card).await;
    // The USB task re-initialised the card before handing it back; the
    // screen reader speaks meanwhile (TTS-3).
    let card = match select(CARD_BACK.receive(), speech::serve()).await {
        Either::First(c) => c,
        Either::Second(never) => match never {},
    };
    *slot = Apart(mount(card)).await;
    if slot.is_some() {
        crate::state::CARD_READY.store(true, Ordering::Relaxed);
    } else {
        reply(|w| {
            w.s(b"card: remount failed");
        });
    }
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
    // The battery logger's, the step counter's and the radio's state in
    // SRAM3, one after the other, before any task runs (here, not in the
    // resident `main`).
    crate::drain::init();
    crate::sensor::init();
    crate::fm::init();
    crate::rec::init();
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
    PAUSED.store(false, Ordering::Relaxed);
    hand_paused_cmd_on();
    CODEC_FLOOR.store(0, Ordering::Relaxed);
    dsp_clock_floor(false);
    STOP.store(false, Ordering::Relaxed);
    media::player_update(|p| p.source = Source::Idle);
    Apart(save_store(vol, srv, arena)).await;
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
                Apart(out.finish()).await;
                let Some(h) = Apart(open_target(vol, srv, o.target, None, &mut ring.file)).await else {
                    return after(srv.feed(Event::OpenFailed));
                };
                // Its place in the music library ("Go to album / artist").
                Apart(crate::library::find_now(vol, srv, h.0.locator())).await;
                // Album art while the arena is still free (AU-14).
                Apart(crate::pictures::track_art(vol, ring, arena, srv, h.0.locator())).await;
                h
            }
        };
        let ext = ext.get(..usize::from(n)).filter(|e| !e.is_empty());
        let start = plan_start(gapless, source_of(ext));
        if gapless && start != PlanStart::Gapless {
            // A synth file read ahead: never gapless. The player hears of
            // the end first if it has not (it opens this item), then the
            // file opens fresh after the previous track has played out.
            if o.early {
                o.early = false;
                srv.feed(Event::DecoderEnded);
            }
            o.gapless = false;
            continue;
        }
        if let PlanStart::Synth { kar } = start {
            let st =
                Start { frame: o.frame, paused: o.paused, track: Some(track), gapless: false, early: None };
            return match Apart(synth::play_midi(vol, ring, arena, srv, out, st, kar)).await {
                Ok(end) => end,
                Err(_) => after(srv.feed(Event::OpenFailed)),
            };
        }
        let st = if gapless {
            let early = if let (true, Target::Item(i)) = (o.early, o.target) { Some(i) } else { None };
            Start { frame: 0, paused: false, track: Some(track), gapless, early }
        } else {
            Start { frame: o.frame, paused: o.paused, track: Some(track), gapless, early: None }
        };
        match Apart(play_file(vol, ring, arena, ext, srv, out, st)).await {
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
    srv.store.image(pl.bookmarks(), pl.auto_marks(), img);
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

// ------------------------------------------------------------ authenticator

/// The Authenticator's import file (OTP-2, `media::OtpJob`): read it into
/// the bytes the UI lent, or overwrite it with zeros and delete it.
pub(crate) async fn otp_file(vol: &mut Volume<'static, Card>, j: &media::OtpJob) {
    if !media::otp_start(j) {
        return;
    }
    let r = if j.delete { otp_delete(vol).await } else { otp_read(vol, j).await };
    media::otp_done(j, r);
}

async fn otp_read(vol: &mut Volume<'static, Card>, j: &media::OtpJob) -> i32 {
    let Ok(mut f) = vol.open(media::OTP_FILE, OpenMode::Read).await else { return media::OTP_NONE };
    if f.size() > u64::from(j.cap) {
        let _ = vol.close(f).await;
        return media::OTP_TOO_LARGE;
    }
    // SAFETY: the UI lent `buf..buf + cap` (part of its frame buffer) for
    // this job and neither draws nor reads it until `OTP_SLOT` is 0 again
    // (`media::otp_done`); `otp_start` made this task the only user.
    let dst = unsafe { core::slice::from_raw_parts_mut(j.buf as *mut u8, j.cap as usize) };
    let n = vol.read_exact_or_eof(&mut f, dst).await;
    let _ = vol.close(f).await;
    match n {
        Ok(n) => i32::try_from(n).unwrap_or(media::OTP_IO),
        Err(_) => media::OTP_IO,
    }
}

/// Overwrite the file with zeros, then delete it (the card's flash may
/// keep old copies; the app says so).
async fn otp_delete(vol: &mut Volume<'static, Card>) -> i32 {
    let Ok(mut f) = vol.open(media::OTP_FILE, OpenMode::ReadWrite).await else { return media::OTP_NONE };
    let zeros = [0u8; 64];
    let mut left = f.size();
    f.set_position(0);
    let mut ok = true;
    while left > 0 && ok {
        let n = left.min(64) as usize;
        ok = vol.write(&mut f, &zeros[..n]).await.is_ok();
        left -= n as u64;
    }
    ok &= vol.close(f).await.is_ok() && vol.flush().await.is_ok();
    ok &= vol.remove(media::OTP_FILE).await.is_ok() && vol.flush().await.is_ok();
    if ok { 0 } else { media::OTP_IO }
}

// ------------------------------------------------------------ battery log

/// Choose the battery log's file number if not done yet: one more than the
/// largest `drain-<n>` in `/.orpheus/test/` (created if missing). Reads
/// only, apart from creating the folder once (PWR-5, `crate::drain`).
pub(crate) async fn drain_prepare(vol: &mut Volume<'static, Card>) {
    if !crate::drain::needs_file() {
        return;
    }
    let dir = orpheus_power::drain::dir();
    let mut cur = match vol.open_dir(dir).await {
        Ok(c) => c,
        Err(_) => {
            // "/.orpheus", then "/.orpheus/test".
            let _ = vol.create_dir(dir.get(..9).unwrap_or(dir)).await;
            let _ = vol.create_dir(dir).await;
            match vol.open_dir(dir).await {
                Ok(c) => c,
                Err(_) => return,
            }
        }
    };
    let mut e = DirEntry::new();
    let mut max = 0u16;
    while let Ok(true) = vol.read_dir(&mut cur, &mut e).await {
        if let Some(n) = orpheus_power::drain::file_number(e.name()) {
            max = max.max(n);
        }
    }
    crate::drain::set_file(max);
}

/// [`drain_prepare`], then [`drain_flush_rows`] (idle loop, power-off,
/// hand-over).
pub(crate) async fn drain_flush(vol: &mut Volume<'static, Card>, buf: &mut [u8]) {
    drain_prepare(vol).await;
    drain_flush_rows(vol, buf).await;
}

/// Append what the battery log has due to its file, formatted in `buf`
/// (2 KiB or more). Nothing before [`drain_prepare`] chose the file.
pub(crate) async fn drain_flush_rows(vol: &mut Volume<'static, Card>, buf: &mut [u8]) {
    let mut path = [0u8; 64];
    let Some((n, rows, pl)) = crate::drain::begin_write(buf, &mut path) else { return };
    let ok = match path.get(..pl).map(core::str::from_utf8) {
        Some(Ok(p)) => match vol.open(p, OpenMode::Create).await {
            Ok(mut f) => {
                f.set_position(f.size());
                let wrote = vol.write(&mut f, buf.get(..n).unwrap_or(&[])).await.is_ok();
                vol.close(f).await.is_ok() && wrote && vol.flush().await.is_ok()
            }
            Err(_) => false,
        },
        _ => false,
    };
    crate::drain::end_write(ok, rows);
}

// ------------------------------------------------------------ step counter

/// Read `/.orpheus/steps.bin` at mount (SENS-1, `crate::sensor`).
async fn steps_load(vol: &mut Volume<'static, Card>, buf: &mut [u8]) {
    let mut h = None;
    if let Ok(mut f) = vol.open(orpheus_steps::history::FILE, OpenMode::Read).await {
        let want = orpheus_steps::history::FILE_LEN.min(buf.len());
        let n = vol.read_exact_or_eof(&mut f, &mut buf[..want]).await.unwrap_or(0);
        let _ = vol.close(f).await;
        h = orpheus_steps::History::decode(&buf[..n]);
    }
    crate::sensor::loaded(h);
}

/// Choose the trace's file number if not done yet: one more than the
/// largest `acc-<n>` in `/.orpheus/test/` (created if missing).
pub(crate) async fn trace_prepare(vol: &mut Volume<'static, Card>) {
    if !crate::sensor::trace_needs_file() {
        return;
    }
    let dir = orpheus_power::drain::dir();
    let mut cur = match vol.open_dir(dir).await {
        Ok(c) => c,
        Err(_) => {
            let _ = vol.create_dir(dir.get(..9).unwrap_or(dir)).await;
            let _ = vol.create_dir(dir).await;
            match vol.open_dir(dir).await {
                Ok(c) => c,
                Err(_) => return,
            }
        }
    };
    let mut e = DirEntry::new();
    let mut max = 0u16;
    while let Ok(true) = vol.read_dir(&mut cur, &mut e).await {
        if let Some(n) = orpheus_steps::trace::file_number(e.name()) {
            max = max.max(n);
        }
    }
    crate::sensor::set_trace_file(max);
}

/// [`trace_prepare`], then [`sensor_flush_rows`]; `force`: the steps file
/// even before it is due (power-off, card hand-over).
pub(crate) async fn sensor_flush(vol: &mut Volume<'static, Card>, buf: &mut [u8], force: bool) {
    trace_prepare(vol).await;
    steps_save(vol, buf, force).await;
    trace_write(vol, buf).await;
}

/// What the step counter has due, without choosing a file (playing).
pub(crate) async fn sensor_flush_rows(vol: &mut Volume<'static, Card>, buf: &mut [u8]) {
    steps_save(vol, buf, false).await;
    trace_write(vol, buf).await;
}

async fn steps_save(vol: &mut Volume<'static, Card>, _buf: &mut [u8], force: bool) {
    let Some(img) = crate::sensor::begin_save(force) else { return };
    let ok = match vol.open(orpheus_steps::history::FILE, OpenMode::Truncate).await {
        Ok(mut f) => {
            let wrote = vol.write(&mut f, &img).await.is_ok();
            vol.close(f).await.is_ok() && wrote && vol.flush().await.is_ok()
        }
        Err(_) => false,
    };
    crate::sensor::end_save(ok);
}

async fn trace_write(vol: &mut Volume<'static, Card>, buf: &mut [u8]) {
    let mut path = [0u8; 48];
    let Some((n, pl)) = crate::sensor::begin_trace_write(buf, &mut path) else { return };
    if let Some(Ok(p)) = path.get(..pl).map(core::str::from_utf8)
        && let Ok(mut f) = vol.open(p, OpenMode::Create).await
    {
        f.set_position(f.size());
        let _ = vol.write(&mut f, buf.get(..n).unwrap_or(&[])).await;
        let _ = vol.close(f).await;
        let _ = vol.flush().await;
    }
}

// ------------------------------------------------------------ alert tone

/// 2 kHz sine, one period in 32 steps, −12 dBFS.
static SINE: [i16; 32] = [
    0, 1561, 3062, 4445, 5657, 6652, 7391, 7846, 8000, 7846, 7391, 6652, 5657, 4445, 3062, 1561, 0, -1561,
    -3062, -4445, -5657, -6652, -7391, -7846, -8000, -7846, -7391, -6652, -5657, -4445, -3062, -1561,
];

/// The alert (UI-5d): four 100 ms beeps of 2 kHz, 100 ms apart, every
/// 1.2 s, at the current volume, while [`ALERT`] is set and nothing waits
/// for the card task (it comes back to sound on). At most a minute (the
/// UI ends the ringing then).
#[inline(never)]
async fn alert_tone(out: &mut PcmOut) {
    let rate = match RATE.load(Ordering::Relaxed) {
        0 => 44_100,
        r => r,
    };
    if !out.running {
        dac::set_rate(rate);
    }
    let step = ((2000u64 << 32) / u64::from(rate)) as u32;
    let beep = (rate / 10).max(1);
    let cycle = rate * 6 / 5;
    let mut phase = 0u32;
    let mut n = 0u32;
    let t0 = time::now_us();
    while ALERT.load(Ordering::Relaxed) && !work_waiting() && time::now_us() - t0 < 61_000_000 {
        for _ in 0..256 {
            let t = n % cycle;
            let on = t < 8 * beep && (t / beep) % 2 == 0;
            let v = if on { SINE.get((phase >> 27) as usize).copied().unwrap_or(0) } else { 0 };
            phase = phase.wrapping_add(step);
            n = n.wrapping_add(1);
            let s = u32::from(v as u16);
            out.push(s | s << 16).await;
        }
    }
    out.finish().await;
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
    Apart(steps_load(vol, arena)).await;
    Apart(srv.load_titles(vol)).await;
    Apart(crate::library::mounted(vol, srv)).await;
}

/// Before the card goes to the USB task: the player stops (the card is
/// gone; Resume continues later) and its state is saved.
async fn hand_over(vol: &mut Volume<'static, Card>, srv: &mut Server, scratch: &mut [u8]) {
    srv.play_req(PlayReq::Cmd(Command::Stop));
    save_store(vol, srv, scratch).await;
    if crate::drain::due() {
        drain_flush(vol, scratch).await;
    }
    sensor_flush(vol, scratch, true).await;
    srv.card_leaving(vol).await;
}

// ------------------------------------------------------------ index build

/// Something for the idle loop is waiting.
pub(crate) fn work_waiting() -> bool {
    !CMDS.is_empty() || !media::REQS.is_empty() || !media::PLAY.is_empty() || !crate::rec::REQ.is_empty()
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

/// A player request from the idle loop: what it asks for, then the player
/// (each a future of its own: this poll stays small, AU-PERF).
async fn player_request(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    r: PlayReq,
) {
    let Some(fl) = Apart(request_flow(vol, arena, srv, r)).await else { return };
    match fl.open {
        Some(o) => Apart(run_player(vol, ring, arena, srv, o)).await,
        None => Apart(save_store(vol, srv, arena)).await,
    }
}

/// The player's answer to a request (a new queue built and saved first);
/// `None`: power-off, saved (nothing to play).
async fn request_flow(
    vol: &mut Volume<'static, Card>,
    arena: &mut [u8],
    srv: &mut Server,
    r: PlayReq,
) -> Option<Flow> {
    let fl = match r {
        PlayReq::Shutdown => {
            // The resume record with the position, then the volume: before
            // `SHUT_DOWN` lets the power go.
            srv.feed(Event::Shutdown);
            save_store(vol, srv, arena).await;
            // The battery log's last rows (the UI ended it), the steps.
            drain_flush(vol, arena).await;
            sensor_flush(vol, arena, true).await;
            let _ = vol.flush().await;
            media::SHUT_DOWN.store(true, Ordering::Release);
            return None;
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
    Some(fl)
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
    unsafe { (&mut (*(&raw mut POOL)).arena)[..data.len()].copy_from_slice(data) };
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
        // PG-2: loads that ran an instruction of cold code from a
        // trampoline (02 §3.6).
        w.s(t!(b" bounces=")).u(u64::from(atj2157_hal::pager::stats().bounces));
        // The queue and the track (HIL tests need no key navigation).
        let (pos, len, shuffled, title, at_ms) = critical_section::with(|cs| {
            let p = media::PLAYER.borrow(cs).borrow();
            (p.queue_pos, p.queue_len, p.shuffled, p.title, p.position_ms)
        });
        let t = title.as_str();
        let mut n = t.len().min(24);
        while !t.is_char_boundary(n) {
            n -= 1;
        }
        // VER-12: the position in the track (soak seek checks).
        w.s(t!(b" pos=")).u(u64::from(at_ms));
        w.s(t!(b" q="))
            .u(u64::from(pos) + u64::from(len > 0))
            .s(b"/")
            .u(u64::from(len))
            .s(t!(b" shuf="))
            .u(u64::from(shuffled))
            .s(t!(b" title="))
            .s(&t.as_bytes()[..n]);
        // VER-13: the last pause's fade (ms), the last natural track
        // change (gapless or with a stop), the key clicks sounded.
        let ps = PLAY_STATS.load(Ordering::Relaxed);
        w.s(t!(b" fade="));
        match ps & 0xFFF {
            0xFFF => w.s(b"-"),
            ms => w.u(u64::from(ms)),
        };
        w.s(t!(b" gapless="))
            .s(match (ps >> 12) & 3 {
                1 => t!(b"1"),
                2 => t!(b"0"),
                _ => t!(b"-"),
            })
            .s(t!(b" clicks="))
            .u(u64::from(ps >> 16));
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
        synth::status(w);
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

/// What the idle loop hands to the task (`audio`).
enum Idle {
    /// A player request: playback (`player_request`).
    Play(PlayReq),
    /// The card goes to USB (handed over already): `release`, then
    /// `card_back`, with the drive's folders looked up again when set.
    Release { find_dirs: bool },
}

/// The card task (AU-PERF): start-up, then the idle loop and the player in
/// turn, each a future of its own (`Apart`). While a track plays this
/// poll function is only the step into the player's future: small, so the
/// per-block wake-up touches few of its lines (packed in `play-block`).
#[embassy_executor::task]
pub async fn audio() {
    // The volume (736 B) lives here, in the task's future, and is only
    // reached by reference: moved by value (out of `start`, into and out of
    // `release`) its copies made this poll function's frame 2,328 B, under
    // every chain of the card task (VER-10).
    let mut vol: Option<Volume<'static, Card>> = None;
    let Some((arena, srv)) = Apart(start(&mut vol)).await else { return };
    // Built here, not in `start`: held across its awaits it would take room
    // in that future as well as in this one.
    // SAFETY: the ring's half of the pool is taken once, here; only this
    // task uses it (through `ring`; a synth source borrows it from `ring`).
    let ring_mem: &'static mut [u8] = unsafe { &mut (*(&raw mut POOL)).ring };
    let mut ring = Ring {
        s: StreamRing::new(ring_mem),
        file: None,
        next: None,
        next_file: None,
        shown: None,
        prepared: None,
    };
    loop {
        let Some(v) = vol.as_mut() else { return };
        match Apart(idle(v, &mut ring, arena, srv)).await {
            Idle::Play(p) => Apart(player_request(v, &mut ring, arena, srv, p)).await,
            Idle::Release { find_dirs } => {
                // The volume leaves its slot for `release` and comes back
                // to it (`None` there: the remount failed).
                Apart(release(&mut vol)).await;
                let Some(v) = vol.as_mut() else { return };
                Apart(card_back(v, srv, find_dirs)).await;
            }
        }
    }
}

/// The card is back from the USB task (`release`): the music library, and
/// the drive's folders when asked.
async fn card_back(vol: &mut Volume<'static, Card>, srv: &mut Server, find_dirs: bool) {
    crate::library::card_back(vol, srv).await;
    if find_dirs {
        crate::drive::find_dirs(vol).await;
    }
}

/// The card opened and mounted into `slot`; `false` (replied) if either
/// failed. A future of its own: the card (608 B) and the volume (744 B)
/// come out of their futures by value, and those copies are in this poll
/// function's frame, not in `start`'s above the state loading (VER-10).
async fn open_card(slot: &mut Option<Volume<'static, Card>>) -> bool {
    let card = match Card::open().await {
        Ok(c) => c,
        Err(_) => {
            reply(|w| {
                w.s(b"card: init failed");
            });
            return false;
        }
    };
    atj2157_hal::log::line(b"audio: card open");
    *slot = mount(card).await;
    if slot.is_none() {
        reply(|w| {
            w.s(b"card: mount failed");
        });
        return false;
    }
    true
}

/// DAC up, the card opened and mounted into `slot`, the state loaded.
async fn start(slot: &mut Option<Volume<'static, Card>>) -> Option<(&'static mut [u8], &'static mut Server)> {
    // SAFETY: DMA channel 5 is the DAC's.
    unsafe { DmaCh::steal(DAC_CH) }.enable_irq_with(on_dac_dma, Level::Deadline);
    dac::power_up(44100, dac::VOL_0DB - 40);
    atj2157_hal::log::line(b"audio: dac up");
    if !Apart(open_card(slot)).await {
        return None;
    }
    let vol = slot.as_mut()?;
    crate::state::CARD_READY.store(true, Ordering::Relaxed);
    reply(|w| {
        w.s(b"card: mounted");
    });
    // SAFETY: this task is the arena's owner; the USB task borrows it only
    // while this task waits for the card (`usb_buffer`).
    let full: &'static mut [u8] = unsafe { &mut (*(&raw mut POOL)).arena };
    // Its last bytes hold the music library's build state (never lent out).
    let (arena, lib_state) = full.split_at_mut(ARENA - crate::library::STATE);
    crate::library::init(lib_state);
    let srv = SERVER.take();
    atj2157_hal::log::line(b"audio: mounted, loading state");
    Apart(load_state(vol, arena, srv)).await;
    // The listings' index folder into the walk cache while nothing plays:
    // otherwise the first listing searches the card's root for it during
    // playback (59 ms on the owner's card, 2026-11-25).
    // `/.orpheus` itself: the listings' index and the games' files both
    // resolve from it (a small folder) instead of the card's root (games
    // saves took 43–55 ms in their first piece while music played,
    // 2026-11-25).
    let _ = Apart(vol.warm_walk("/.orpheus")).await;
    Apart(crate::rec::find_dir(vol)).await;
    Apart(crate::drive::find_dirs(vol)).await;
    atj2157_hal::log::line(b"audio: state loaded");
    Apart(resume_at_start(srv)).await;
    Some((arena, srv))
}

/// Settings › Playback › Resume at start-up (VER-13, 05 §6.1): once the UI
/// has published its settings (at most 2 s), the saved place opens paused.
async fn resume_at_start(srv: &mut Server) {
    for _ in 0..100 {
        if media::opts() & media::opt::VALID != 0 {
            break;
        }
        Timer::after_millis(20).await;
    }
    if media::opts() & media::opt::RESUME != 0 && srv.resume_at_start() {
        atj2157_hal::log::line(t!(b"audio: resuming paused"));
    }
}

/// The idle loop: requests, developer commands and background work until a
/// player request or a card hand-over (for the task).
#[allow(clippy::too_many_lines)]
async fn idle(vol: &mut Volume<'static, Card>, ring: &mut Ring, arena: &mut [u8], srv: &mut Server) -> Idle {
    loop {
        // Whatever the last pass did with the arena, an EPUB's scratch
        // lent from it is not what its last step left (`crate::books`).
        crate::books::arena_taken();
        // A games save or read left in pieces by playback: finished first
        // (outside a chore all at once), so that a card hand-over or the
        // power-off waiting in `pending` comes after it.
        if srv.games_pending() {
            Apart(srv.games_step(vol)).await;
        }
        // A request left over from playback, else wait for one.
        let r = if let Some(p) = srv.pending_play.take() {
            Either3::Third(Either::First(p))
        } else if let Some(r) = srv.pending.take() {
            Either3::Second(r)
        } else {
            // A listing left in pieces by playback: finished now.
            while srv.has_job() && !work_waiting() {
                Apart(srv.step(vol)).await;
            }
            // Books card work (05 §7.15), before anything else uses the
            // arena in this pass: its first 35,072 bytes are an EPUB's
            // inflate window while the desk asks for it.
            if crate::books::wants_idle() && !work_waiting() {
                Apart(crate::books::run_idle(vol, arena)).await;
            }
            // The battery log (PWR-5) first: a minute's rows are small;
            // then the step counter's (SENS-1); an alert sounds (UI-5d).
            if crate::drain::due() && !work_waiting() {
                Apart(drain_flush(vol, arena)).await;
            }
            if crate::sensor::card_due() && !work_waiting() {
                Apart(sensor_flush(vol, arena, false)).await;
            }
            if ALERT.load(Ordering::Relaxed) && !work_waiting() {
                speech::end_alone();
                let mut out = PcmOut::new();
                Apart(alert_tone(&mut out)).await;
            }
            // TTS-3: the screen reader alone (the DAC at 16 kHz).
            let speaking = speech::alone();
            // A key click with nothing playing (never ahead of card work:
            // dropped then; nor over the reader's answer to the key).
            if CLICK_AT.load(Ordering::Relaxed) != u32::MAX {
                if work_waiting() || speech::alone_running() {
                    CLICK_AT.store(u32::MAX, Ordering::Relaxed);
                } else {
                    let mut out = PcmOut::new();
                    Apart(click_alone(&mut out)).await;
                }
            }
            // Background work waits while the reader speaks alone (a long
            // step would leave a gap in the speech).
            if srv.wants_build() && !work_waiting() && !speaking {
                Apart(build_slot(vol, srv, arena)).await;
            }
            // Card info's free space (counted once, then the volume's own).
            if !work_waiting() && !speaking {
                Apart(count_free(vol)).await;
            }
            // The music library builds when it is due and nothing else waits.
            if crate::library::wait_ms(srv) == Some(0) && !srv.wants_build() && !work_waiting() && !speaking {
                Apart(crate::library::slot(vol, srv, arena)).await;
            }
            // Player requests and the recorder's (AU-13) share the third
            // arm.
            let next = select3(
                CMDS.receive(),
                media::REQS.receive(),
                select(media::PLAY.receive(), crate::rec::REQ.receive()),
            );
            // Speech to take (TTS-3) wakes the loop too.
            let next = select(next, speech::wait());
            let r = match crate::library::wait_ms(srv).filter(|_| !speaking) {
                Some(ms) => match select(next, Timer::after_millis(ms.max(1))).await {
                    Either::First(r) => r,
                    Either::Second(()) => continue,
                },
                None => next.await,
            };
            match r {
                Either::First(r) => r,
                Either::Second(()) => continue,
            }
        };
        let cmd = match r {
            Either3::First(c) => c,
            Either3::Second(Req::Picture(j)) => {
                // No track loaded: the whole arena for the picture.
                Apart(crate::pictures::serve_idle(vol, ring, arena, srv, &j)).await;
                continue;
            }
            Either3::Second(r) => {
                if Apart(srv.serve(vol, r, arena, true)).await {
                    continue;
                }
                match srv.pending.take() {
                    Some(Req::Release) => {
                        Apart(hand_over(vol, srv, arena)).await;
                        return Idle::Release { find_dirs: true };
                    }
                    Some(Req::Image) => {
                        // USB Drive: the file's runs into the arena's
                        // extent area, then the card goes as for the card
                        // reader; a refusal keeps it here.
                        Apart(hand_over(vol, srv, arena)).await;
                        let ext = crate::drive::as_extents(extent_area(arena));
                        let m = Apart(crate::drive::map(vol, ext)).await;
                        let ok = m.is_ok();
                        crate::drive::MAPPED.signal(m);
                        if ok {
                            return Idle::Release { find_dirs: true };
                        }
                    }
                    _ => {}
                }
                continue;
            }
            Either3::Third(Either::First(p)) => {
                // The music takes the DAC; the speech goes on over it.
                speech::end_alone();
                return Idle::Play(p);
            }
            Either3::Third(Either::Second(c)) => {
                // Recording (AU-13): the arena is the recorder's until it
                // is done.
                Apart(crate::rec::run(vol, arena, c)).await;
                continue;
            }
        };
        let text = core::str::from_utf8(&cmd.text[..cmd.len]).unwrap_or("");
        if text == "release" {
            Apart(hand_over(vol, srv, arena)).await;
            return Idle::Release { find_dirs: false };
        } else if let Some(arg) = text.strip_prefix("playdir ") {
            // The player's queue on a folder, as Files → OK (in folder
            // order: shuffle off, for repeatable tests).
            match Apart(resolve_dir(vol, arg.trim())).await {
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
            Apart(ls(vol, p.trim())).await;
        } else if let Some(a) = text.strip_prefix("synth") {
            Apart(synth::command(a.trim(), ring, arena)).await;
        } else if let Some(p) = text.strip_prefix("play ") {
            for s in
                [&FRAMES, &UNDERRUNS, &SLOW_UNDERRUNS, &CORRUPT, &DEC_N, &DEC_US, &DEC_MAX_US, &DEC_LOADS]
            {
                s.store(0, Ordering::Relaxed);
            }
            let r = select(Apart(play_path(vol, ring, arena, p.trim(), srv)), core::future::pending::<()>())
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
                let _ = Apart(vol.create_dir(&path[..i])).await;
            }
            Apart(upload(vol, arena, path)).await;
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
