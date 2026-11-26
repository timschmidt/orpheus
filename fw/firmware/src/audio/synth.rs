//! Synth sources on the player (SYN-3, 04 §9): the synthesis engine in the
//! pool, MIDI files played like other tracks, and the bench.
//!
//! **Memory.** A synth source takes the pool (`POOL`): the stream ring,
//! lent by `StreamRing::lend` (it is idle: a synth source reads its file
//! through a block cache, not the ring), and the decoder arena after it,
//! less the library's build state — 80 KiB less ≈ 1.5 KiB, one contiguous
//! region ([`region`]). `orpheus_audio::synth::carve` lays out the state
//! (a `MidiPlayer`, ≈ 70 KB, with the block cache's tags, the load
//! governor and a render block), the file block cache and the spare bytes
//! for card work while it plays. The state is built in place: zeroed
//! ([`place`]; an all-zero `MidiPlayer` is valid, `bytemuck::Zeroable`)
//! and then `init` (no value larger than ≈ 1 KiB passes through the
//! stack). A codec and the synthesiser are never live together: the borrow
//! of the ring and the arena lasts while the synth plays. No static RAM
//! but the statistics words below (12 bytes).
//!
//! **Playing** ([`play_midi`]): the file opens and is scanned with the DAC
//! stopped; then blocks of at most [`BLOCK`] frames at [`RATE`] go through
//! the DSP chain and the PCM ring as a decoder's do (`Out`, `Feeder`). The
//! parser's `Need`s are answered from the block cache; a miss reads one
//! card sector (with `Urgent`: no card rests). Requests, listings in
//! pieces, the battery and step logs run as chores by the same rules as
//! with a codec (`chore::decide`). The clock floor is High while the
//! synth plays; the load governor (`orpheus_audio::synth::VoiceGov`)
//! lowers the voice limit while a block's render takes more than 60 % of
//! its real time. Seek and pause as MIDI-1 built them; never gapless.
//!
//! **Bench** (`synth bench`, read-only: no card access): the fixed loads
//! of `orpheus_synth::bench`, rendered at the High clock with the cycle
//! counter; each reply carries the render's FNV-1a hash for comparison
//! with the host's (`orpheus-synth/tests/goldens-bench.txt`). A MIDI
//! file's whole render is checked through playback (`dsp off`, `crc on`,
//! `play PATH`: the CRC of the synth's samples, which
//! `orpheus-midi/tests/card.rs` predicts).

use core::sync::atomic::{AtomicU32, Ordering};

use atj2157_hal::{cycles, dac, time};
use embassy_futures::select::{Either3, select3};
use embassy_time::Timer;
use orpheus_audio::chore::{self, Ask, DecPeak, Verdict};
use orpheus_audio::pcm::{Fed, Feeder};
use orpheus_audio::synth::{VoiceGov, carve};
use orpheus_fs::{File, Volume};
use orpheus_midi::cache::{self, BlockCache, Miss};
use orpheus_midi::{MidiPlayer, Need, Options, Pull};
use orpheus_player::Event;
use orpheus_synth::demo::Fnv;
use orpheus_synth::{Engine, MAX_VOICES, bench};
use yoton_bsp::card::Card;

use super::{
    Blk, CHAIN, CODEC_FLOOR, DEC_LOADS, DEC_MAX_US, DEC_N, DEC_US, DECODE_US, DSP_ON, End, FRAMES, K_DRAIN,
    K_JOB, K_SENSOR, K_SERVE, LAST_DEC_US, LAST_SERVE_MS, MAX_UNDERRUNS, Out, PLAY_US, PLAYING, PcmOut, RATE,
    ROOMY, Ring, SCRATCH_FRAMES, SLOW_UNDERRUNS, SPARE, STOP, Shown, Start, T_DECODE, begin_block,
    dsp_clock_floor, failed, full_level, log_due, mark, ms_at, need_of, note_as, pager_loads, pause,
    pause_fades, piece_begin, piece_end, reply, resumed, save_store, scratch16, serve_one, sound_active,
};
use super::{drain_flush_rows, sensor_flush_rows};
use crate::media::{self, PlayReq, Req, Server};

/// The rate the synthesiser renders at (the DAC's; 44.1 kHz costs 8 %
/// less than 48 kHz).
pub(super) const RATE_HZ: u32 = 44_100;
/// Frames per render block (5.8 ms at 44.1 kHz).
const BLOCK: usize = 256;
/// Events per open/seek step (a few hundred µs).
const STEP_EVENTS: u32 = 256;
/// Fewest cache blocks worth playing with.
const MIN_BLOCKS: usize = 4;

/// While a synth source plays (`status` `syn:`), one word (the stack is a
/// few bytes from its next 256-byte step): the most voices sounding after
/// a block ([`F_PEAK`]), the governor's limit ([`F_LIMIT`]), its
/// overloaded blocks ([`F_OVER`], saturating at 255), the card blocks read
/// for the file while it plays ([`F_READS`], at 511), and the voice
/// limit's maximum for MIDI playback ([`F_VMAX`]: `synth voices N`; 0:
/// all [`MAX_VOICES`]; kept across files).
static SYN: AtomicU32 = AtomicU32::new(0);
/// `SYN` fields: (shift, bits).
const F_PEAK: (u32, u32) = (0, 5);
const F_LIMIT: (u32, u32) = (5, 5);
const F_OVER: (u32, u32) = (10, 8);
const F_READS: (u32, u32) = (18, 9);
const F_VMAX: (u32, u32) = (27, 5);

fn syn_get(f: (u32, u32)) -> u32 {
    (SYN.load(Ordering::Relaxed) >> f.0) & ((1 << f.1) - 1)
}

fn syn_set(f: (u32, u32), v: u32) {
    let max = (1 << f.1) - 1;
    let _ = SYN.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |s| {
        Some((s & !(max << f.0)) | (v.min(max) << f.0))
    });
}

/// What a MIDI file needs in the pool.
#[derive(bytemuck::Zeroable)]
struct MidiState {
    player: MidiPlayer,
    cache: BlockCache,
    gov: VoiceGov,
    pcm: [i16; 2 * BLOCK],
}

/// The pool a synth source gets: the ring and the arena less the library
/// state (`region`).
const POOL_BYTES: usize = super::RING + super::ARENA - crate::library::STATE;
/// A MIDI file's state, the fewest cache blocks and the spare bytes fit
/// the pool (with 8 bytes of alignment); the bench's engine too.
const _: () = assert!(
    core::mem::size_of::<MidiState>() + 8 + MIN_BLOCKS * cache::BLOCK + SPARE <= POOL_BYTES
        && core::mem::size_of::<Engine>() + 8 <= POOL_BYTES
);
/// The sizes (`synth bench` with no argument reports them): the MIDI
/// state, the pool, the engine, the library state.
const SIZES: [usize; 4] =
    [core::mem::size_of::<MidiState>(), POOL_BYTES, core::mem::size_of::<Engine>(), crate::library::STATE];

/// The `status` words of the synth source.
pub(super) fn status(w: &mut super::Writer<'_>) {
    w.s(t!(b" syn: v="))
        .u(u64::from(syn_get(F_PEAK)))
        .s(t!(b" vlim="))
        .u(u64::from(syn_get(F_LIMIT)))
        .s(t!(b" over="))
        .u(u64::from(syn_get(F_OVER)))
        .s(t!(b" reads="))
        .u(u64::from(syn_get(F_READS)));
}

/// The ring's memory and the arena as one region (they are adjacent in
/// `POOL`). `None` if they are not (never: `repr(C)`).
fn region<'a>(ring: &'a mut [u8], arena: &'a mut [u8]) -> Option<&'a mut [u8]> {
    let (r, a) = (ring.as_mut_ptr(), arena.as_mut_ptr());
    if r.wrapping_add(ring.len()) != a {
        return None;
    }
    let len = ring.len() + arena.len();
    // SAFETY: the two slices are adjacent (checked) and both borrowed
    // exclusively for 'a, so the joined slice is one exclusive borrow of
    // the same bytes for 'a.
    Some(unsafe { core::slice::from_raw_parts_mut(r, len) })
}

/// A zeroed `T` at the start of `mem` (which must be aligned for it).
fn place<T: bytemuck::Zeroable>(mem: &mut [u8]) -> Option<&mut T> {
    let n = core::mem::size_of::<T>();
    if mem.as_ptr().align_offset(core::mem::align_of::<T>()) != 0 || mem.len() < n {
        return None;
    }
    mem[..n].fill(0);
    // SAFETY: aligned and large enough (checked), every byte zero, and an
    // all-zero `T` is a valid value (`T: Zeroable`); the bytes stay
    // borrowed exclusively for the result's lifetime.
    Some(unsafe { &mut *mem.as_mut_ptr().cast::<T>() })
}

/// Lay out a `T` and `max_blocks` cache blocks (at least `min_blocks`) in
/// `mem`, keeping `spare` bytes: the state, the blocks, the spare.
fn split<T: bytemuck::Zeroable>(
    mem: &mut [u8],
    min_blocks: usize,
    max_blocks: usize,
    spare: usize,
) -> Option<(&mut T, &mut [u8], &mut [u8], usize)> {
    let l = carve(
        mem.as_ptr() as usize,
        mem.len(),
        core::mem::size_of::<T>(),
        core::mem::align_of::<T>(),
        cache::BLOCK,
        min_blocks,
        max_blocks,
        spare,
    )?;
    let (head, tail) = mem.split_at_mut_checked(l.cache.start)?;
    let st = place::<T>(head.get_mut(l.state.start..)?)?;
    let (blocks, rest) = tail.split_at_mut_checked(l.cache.len())?;
    Some((st, blocks, rest, l.blocks))
}

/// Let thread mode run when this task has kept the CPU for 4 ms (a long
/// open, seek or bench must not starve the UI and the watchdog).
async fn breathe(since: &mut u64) {
    let now = time::now_us();
    if now - *since >= 4_000 {
        Timer::after_micros(1_500).await;
        *since = time::now_us();
    }
}

/// Answer the parser's `need` from the block cache, reading what is
/// missing (each miss one card sector; a failed read answers short, which
/// ends that track).
async fn supply(
    vol: &mut Volume<'static, Card>,
    file: &mut File,
    cache: &mut BlockCache,
    mem: &mut [u8],
    player: &mut MidiPlayer,
    need: Need,
) {
    let mut out = [0u8; orpheus_midi::smf::WINDOW];
    for _ in 0..4 {
        match cache.answer(need, mem, &mut out) {
            Ok(n) => return player.supply(out.get(..n).unwrap_or_default()),
            Err(Miss { block }) => {
                let Some(s) = cache.slot(block, mem) else { break };
                file.set_position(u64::from(block) * cache::BLOCK as u64);
                let n = {
                    // A deadline while playing: no card rests.
                    let _urgent = yoton_bsp::card::Urgent::enter();
                    vol.read(file, s).await.unwrap_or(0)
                };
                cache.filled(block, n);
            }
        }
    }
    player.supply(&[]);
}

/// Step an open or a seek until the player is ready. False: the file is
/// not a MIDI file it can play.
async fn run_steps(
    vol: &mut Volume<'static, Card>,
    file: &mut File,
    st: &mut MidiState,
    mem: &mut [u8],
) -> bool {
    let mut since = time::now_us();
    loop {
        match st.player.step(STEP_EVENTS) {
            Ok(Pull::Need(n)) => supply(vol, file, &mut st.cache, mem, &mut st.player, n).await,
            Ok(Pull::Ready(true)) => return true,
            Ok(Pull::Ready(false)) => {}
            Ok(Pull::End) | Err(_) => return false,
        }
        breathe(&mut since).await;
    }
}

/// What a request did to the synth's playback loop.
enum Head {
    Go,
    Seek(u64),
    End(Result<End, &'static [u8]>),
}

/// A request that arrived, a stop, the player's actions (as `head` does
/// for a codec; a seek is the caller's).
async fn head(
    b: &mut Blk,
    got: Option<Result<PlayReq, Req>>,
    vol: &mut Volume<'static, Card>,
    srv: &mut Server,
    spare: &mut [u8],
) -> Head {
    match got {
        Some(Ok(PlayReq::Edit(e))) if b.driven => {
            b.flow.merge(srv.edit(e, vol.serial()));
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
            b.flow.merge(srv.play_req(r));
        }
        Some(Ok(r)) => {
            srv.pending_play = Some(r);
            return Head::End(Ok(End::Pending));
        }
        Some(Err(r)) => {
            let go_on = srv.serve(vol, r, spare, false).await;
            if !go_on {
                return Head::End(Ok(End::Pending));
            }
        }
        None => {}
    }
    if STOP.swap(false, Ordering::Relaxed) {
        if b.driven {
            srv.play_req(PlayReq::Stop);
        }
        return Head::End(Ok(End::Idle));
    }
    if !b.flow.is_empty() {
        let fl = core::mem::take(&mut b.flow);
        if let Some(o) = fl.open {
            return Head::End(Ok(End::Open(o)));
        }
        if fl.stop {
            return Head::End(Ok(End::Idle));
        }
        if let Some(p) = fl.pause {
            b.paused = p;
        }
        if let Some(t) = fl.seek {
            return Head::Seek(t);
        }
    }
    Head::Go
}

/// A request or a listing piece as a chore, when `chore::decide` lets it
/// run (the codec loop's rules). `Some`: the file's playback ends.
async fn chore_step(
    b: &mut Blk,
    vol: &mut Volume<'static, Card>,
    srv: &mut Server,
    out: &mut PcmOut,
    spare: &mut [u8],
) -> Option<Result<End, &'static [u8]>> {
    let req = media::REQS.try_peek().ok().map(|r| r.kind());
    let (serve, kind) = match req {
        Some(r) => (true, K_SERVE + r),
        None if srv.has_job() => (false, K_JOB + srv.job_kind()),
        None => {
            b.chore_since = 0;
            return None;
        }
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
    let t0 = super::now32();
    piece_begin(kind, budget);
    let mut end = None;
    if serve {
        if !serve_one(vol, srv, spare).await {
            end = Some(Ok(End::Pending));
        }
    } else {
        srv.step(vol).await;
    }
    note_as(super::UB_SERVE, kind as u8, t0);
    piece_end(kind, matches!(v, Verdict::Starved { .. }));
    LAST_SERVE_MS.store((time::now_us() / 1000) as u32, Ordering::Relaxed);
    end
}

/// Play `ring.file`, a MIDI file (`kar`: karaoke), through the
/// synthesiser in the pool.
#[allow(clippy::too_many_lines)]
pub(super) async fn play_midi(
    vol: &mut Volume<'static, Card>,
    ring: &mut Ring,
    arena: &mut [u8],
    srv: &mut Server,
    out: &mut PcmOut,
    st: Start,
    kar: bool,
) -> Result<End, &'static [u8]> {
    mark(30);
    ring.cancel_next();
    ring.shown = None;
    ring.prepared = None;
    let Ring { s, file, .. } = ring;
    let file = file.as_mut().ok_or(t!(b"no file"))?;
    let size = file.size();
    let len = u32::try_from(size).map_err(|_| t!(b"MIDI file too large"))?;
    let mem = region(s.lend(), arena).ok_or(t!(b"pool not contiguous"))?;
    let (state, blocks, spare, n_blocks) =
        split::<MidiState>(mem, MIN_BLOCKS, cache::SLOTS, SPARE).ok_or(t!(b"pool too small for MIDI"))?;
    state.player.init(Options { kar, ..Options::new(RATE_HZ) });
    state.cache.reset(n_blocks);
    let max = match syn_get(F_VMAX) as u8 {
        0 => MAX_VOICES as u8,
        n => n.min(MAX_VOICES as u8),
    };
    state.gov = VoiceGov::new(max);
    state.player.set_voice_limit(usize::from(state.gov.limit()));
    SYN.store(syn_get(F_VMAX) << F_VMAX.0, Ordering::Relaxed);
    syn_set(F_LIMIT, u32::from(state.gov.limit()));
    // The High clock for the open's scan and the render (04 §9: MIDI
    // needs it).
    CODEC_FLOOR.store(2, Ordering::Relaxed);
    dsp_clock_floor(sound_active());
    out.crc = 0xFFFF_FFFF;
    state.player.open(len, kar);
    if !run_steps(vol, file, state, blocks).await {
        CODEC_FLOOR.store(0, Ordering::Relaxed);
        dsp_clock_floor(false);
        return Err(t!(b"not a MIDI file"));
    }
    let duration = state.player.duration();
    let mut pos = 0u64;
    if st.frame > 0 && st.frame < duration {
        if state.player.seek(st.frame).is_err() || !run_steps(vol, file, state, blocks).await {
            CODEC_FLOOR.store(0, Ordering::Relaxed);
            dsp_clock_floor(false);
            return Err(t!(b"seek failed"));
        }
        pos = state.player.position();
    }
    // SAFETY: initialised at the start of the audio task, which is the only
    // user.
    let chain = unsafe { (*(&raw mut CHAIN)).assume_init_mut() };
    let out_rate = chain
        .set_format(orpheus_dsp::Format {
            rate: RATE_HZ,
            channels: 2,
            kind: orpheus_audio::SampleKind::I16,
            bits: 16,
        })
        .map_err(|_| t!(b"rate not supported"))?;
    media::apply_sound(chain);
    full_level(chain);
    let sound_gen = media::SOUND_GEN.load(Ordering::Acquire);
    dsp_clock_floor(sound_active());
    if !out.running {
        dac::set_rate(out_rate);
    }
    RATE.store(out_rate, Ordering::Relaxed);
    let rate = u64::from(RATE_HZ);
    let duration_ms = ms_at(rate, duration);
    let show = Shown {
        paused: st.paused,
        start_ms: ms_at(rate, pos),
        duration_ms,
        codec: "MIDI",
        rate: RATE_HZ,
        channels: 2,
        bits: 16,
        size,
        kbps: orpheus_apps::services::average_kbps(0, size, duration_ms),
    };
    media::player_update(|p| show.apply(p));
    let driven = st.track.is_some();
    let mut flow = media::Flow::default();
    let info = orpheus_audio::StreamInfo {
        rate: RATE_HZ,
        channels: 2,
        bits: 16,
        sample: orpheus_audio::SampleKind::I16,
        frames: Some(duration),
        delay: 0,
        padding: 0,
        bitrate: 0,
        codec: orpheus_audio::CodecId::Unknown,
    };
    if let Some(track) = st.track {
        flow = srv.feed(Event::Opened { track, rate: RATE_HZ, frames: Some(duration) });
        if !out.running {
            save_store(vol, srv, spare).await;
        }
    }
    PLAYING.store(true, Ordering::Relaxed);
    STOP.store(false, Ordering::Relaxed);
    let t_start = time::now_us();
    let mut b = Blk {
        st,
        info,
        show,
        rate,
        ch: 2,
        stereo: true,
        dsp: DSP_ON.load(Ordering::Relaxed),
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
        last_tick: t_start,
        natural: false,
        serve_due: false,
        pf_due: None,
        meta: 0,
        lib_find: None,
        prev_dir: u32::MAX,
        chore_since: 0,
        dec: DecPeak(0),
        join: 0,
        early: None,
    };
    let mut empty = 0u32;
    let end = 'play: loop {
        // A request: waited for while paused, else one per block.
        let got = if b.paused && !b.fading {
            match select3(media::PLAY.receive(), media::REQS.receive(), Timer::after_millis(200)).await {
                Either3::First(r) => Some(Ok(r)),
                Either3::Second(r) => Some(Err(r)),
                Either3::Third(()) => None,
            }
        } else {
            media::PLAY.try_receive().ok().map(Ok)
        };
        let waiting = got.is_some() || STOP.load(Ordering::Relaxed) || !b.flow.is_empty();
        if waiting || b.paused && !b.fading {
            match super::Apart(head(&mut b, got, vol, srv, spare)).await {
                Head::Go => {}
                Head::End(e) => break 'play e,
                Head::Seek(t) => {
                    // The DAC stops for the re-scan (not an underrun).
                    out.stop();
                    if state.player.seek(t).is_err() || !run_steps(vol, file, state, blocks).await {
                        break 'play failed(srv, b.driven, t!(b"seek failed"));
                    }
                    b.pos = state.player.position();
                    let p = ms_at(rate, b.pos);
                    media::player_update(|s| s.position_ms = p);
                }
            }
        }
        if b.paused && !pause_fades(&mut b, out, chain) {
            super::Apart(pause(&mut b, vol, srv, out, spare)).await;
            continue;
        }
        if b.held || b.fading && !b.paused {
            resumed(&mut b, chain);
        }
        if SLOW_UNDERRUNS.load(Ordering::Relaxed) >= MAX_UNDERRUNS {
            break 'play failed(srv, b.driven, t!(b"synth too slow for this player"));
        }
        // One block (fewer frames when the parser needs file bytes first).
        mark(31);
        let (t0, l0) = (time::now_us(), pager_loads());
        let n = match state.player.render(&mut state.pcm) {
            Pull::Ready(n) => n,
            Pull::Need(need) => {
                syn_set(F_READS, syn_get(F_READS) + 1);
                supply(vol, file, &mut state.cache, blocks, &mut state.player, need).await;
                continue;
            }
            Pull::End => {
                b.natural = true;
                break 'play Ok(End::Idle);
            }
        };
        let dt = (time::now_us() - t0) as u32;
        T_DECODE.fetch_add(dt, Ordering::Relaxed);
        DEC_N.fetch_add(1, Ordering::Relaxed);
        DEC_US.fetch_add(dt, Ordering::Relaxed);
        DEC_MAX_US.fetch_max(dt, Ordering::Relaxed);
        LAST_DEC_US.store(dt, Ordering::Relaxed);
        DEC_LOADS.fetch_add(pager_loads().wrapping_sub(l0), Ordering::Relaxed);
        b.decode_us += u64::from(dt);
        b.dec = b.dec.after(dt);
        if n == 0 {
            empty += 1;
            if empty > 64 {
                break 'play failed(srv, b.driven, t!(b"MIDI player stuck"));
            }
            continue;
        }
        empty = 0;
        // The load governor and the statistics.
        let block_us = (n as u64 * 1_000_000 / u64::from(RATE_HZ)) as u32;
        let lim = state.gov.after(dt, block_us);
        state.player.set_voice_limit(usize::from(lim));
        let v = state.player.engine().active_voices() as u32;
        syn_set(F_PEAK, syn_get(F_PEAK).max(v));
        syn_set(F_LIMIT, u32::from(lim));
        syn_set(F_OVER, state.gov.over);
        // DSP, key clicks and the PCM ring, as for a decoder's block.
        begin_block(&mut b, chain);
        let (dsp, frames) = (b.dsp, &mut b.frames);
        let Some(s) = state.pcm.get_mut(..2 * n) else { continue };
        let sc = scratch16(SCRATCH_FRAMES * 2);
        let mut feed = Feeder::new();
        while feed.run(s, sc, 2, &mut Out { out, chain, dsp, stereo: true, frames }) == Fed::Wait {
            out.half().await;
        }
        // The position (UI, player), the player's ticks.
        b.pos = state.player.position();
        FRAMES.store(b.frames, Ordering::Relaxed);
        let p = ms_at(rate, b.pos);
        media::player_update(|s| s.position_ms = p);
        if b.driven {
            b.flow.merge(srv.position(b.pos));
            let now = time::now_us();
            if now - b.last_tick >= 1_000_000 {
                let ms = ((now - b.last_tick) / 1000) as u32;
                b.last_tick = now;
                b.flow.merge(srv.feed(Event::Tick { ms }));
            }
        }
        if let Some(e) = super::Apart(chore_step(&mut b, vol, srv, out, spare)).await {
            break 'play e;
        }
        if crate::drain::due()
            && let Some(budget) = log_due(&b, out, K_DRAIN)
        {
            let t0 = super::now32();
            piece_begin(K_DRAIN, budget);
            super::Apart(drain_flush_rows(vol, spare)).await;
            piece_end(K_DRAIN, false);
            note_as(super::UB_SERVE, K_DRAIN as u8, t0);
        }
        if crate::sensor::card_due()
            && let Some(budget) = log_due(&b, out, K_SENSOR)
        {
            let t0 = super::now32();
            piece_begin(K_SENSOR, budget);
            super::Apart(sensor_flush_rows(vol, spare)).await;
            piece_end(K_SENSOR, false);
            note_as(super::UB_SERVE, K_SENSOR as u8, t0);
        }
        if b.fading && b.paused && chain.fade_done() {
            super::Apart(pause(&mut b, vol, srv, out, spare)).await;
        }
    };
    // The end: the player says what comes next (never gapless: the next
    // file opens after this one has played out).
    let end = if b.natural && driven {
        let fl = srv.feed(Event::DecoderEnded);
        out.finish().await;
        match fl.open {
            Some(mut o) => {
                o.gapless = false;
                Ok(End::Open(o))
            }
            None => {
                if !fl.stop {
                    srv.feed(Event::Drained);
                }
                Ok(End::Idle)
            }
        }
    } else if b.natural {
        out.finish().await;
        end
    } else {
        if out.running {
            out.stop();
        }
        end
    };
    // `crc on`: the CRC of every frame written (with `dsp off`, the
    // synth's samples and the play-out silence; `orpheus-midi`'s
    // `tests/card.rs` predicts it for the fixtures).
    super::CRC.store(!out.crc, Ordering::Relaxed);
    DECODE_US.store(b.decode_us as u32, Ordering::Relaxed);
    PLAY_US.store((time::now_us() - t_start) as u32, Ordering::Relaxed);
    PLAYING.store(false, Ordering::Relaxed);
    CODEC_FLOOR.store(0, Ordering::Relaxed);
    dsp_clock_floor(false);
    mark(0);
    end
}

// ------------------------------------------------------------------ bench

/// The measurement of a run: cycles (if the counter counts), µs, pager
/// line loads, from [`Meter::on`] to [`Meter::off`], summed.
#[derive(Default)]
struct Meter {
    cyc: u64,
    us: u64,
    loads: u32,
    c0: u32,
    t0: u64,
    l0: u32,
}

impl Meter {
    #[inline(always)]
    fn on(&mut self) {
        self.l0 = pager_loads();
        self.t0 = time::now_us();
        self.c0 = cycles::now();
    }

    #[inline(always)]
    fn off(&mut self) {
        let c = cycles::now();
        self.us += time::now_us() - self.t0;
        self.cyc += u64::from(c.wrapping_sub(self.c0));
        self.loads = self.loads.wrapping_add(pager_loads().wrapping_sub(self.l0));
    }

    /// The reply's measurement words; `frames` rendered.
    fn put(&self, w: &mut super::Writer<'_>, frames: u64, cyc_ok: bool) {
        let mhz = u64::from(atj2157_hal::clock::current().hz() / 1_000_000);
        // Without the counter: µs × MHz (the clock was held at High).
        let cyc = if cyc_ok { self.cyc } else { self.us * mhz };
        w.s(t!(b" frames="))
            .u(frames)
            .s(t!(b" us="))
            .u(self.us)
            .s(t!(b" mhz="))
            .u(mhz)
            .s(t!(b" cyc_ok="))
            .u(u64::from(cyc_ok))
            .s(t!(b" cyc="))
            .u(cyc)
            .s(t!(b" cyc_per_frame="))
            .u(cyc / frames.max(1))
            .s(t!(b" loads="))
            .u(u64::from(self.loads));
    }
}

/// The 64-bit hash in 16 hex digits.
fn put_hash(w: &mut super::Writer<'_>, h: u64) {
    w.s(t!(b" hash="));
    for k in (0..16).rev() {
        let d = ((h >> (4 * k)) & 15) as u8;
        w.s(&[if d < 10 { b'0' + d } else { b'a' + d - 10 }]);
    }
}

/// `synth …` (test commands from the idle loop; read-only: nothing is
/// written to the card; the pool is lent while it runs):
///
/// * `synth bench I [N] [BLOCKS]`: load I of `orpheus_synth::bench` with N
///   voices (8), BLOCKS measured blocks (64) after 8 warm-up blocks;
/// * `synth voices N`: MIDI playback's voice limit at most N (0: 16).
///
/// A MIDI file's samples are checked through playback itself: with `dsp
/// off` and `crc on`, `play PATH` replies with the CRC of the synth's
/// output as rendered (`orpheus-midi/tests/card.rs` computes the host's).
pub(super) async fn command(arg: &str, ring: &mut Ring, arena: &mut [u8]) {
    let mut it = arg.split_ascii_whitespace();
    let what = it.next().unwrap_or("");
    let mut num = |d: u32| it.next().and_then(|t| t.parse::<u32>().ok()).unwrap_or(d);
    let (a, b, c) = (num(u32::MAX), num(u32::MAX), num(bench::BLOCKS));
    if what == "voices" && a != u32::MAX {
        let n = a.min(MAX_VOICES as u32) as u8;
        syn_set(F_VMAX, u32::from(n));
        reply(|w| {
            w.s(t!(b"synth voices ")).u(u64::from(n));
        });
        return;
    }
    if what != "bench" || a == u32::MAX {
        reply(|w| {
            w.s(t!(b"synth bench I [N] [BLOCKS] | voices N; loads ")).u(bench::LOADS.len() as u64);
            w.s(t!(b" midi_state=")).u(SIZES[0] as u64).s(t!(b" pool=")).u(SIZES[1] as u64);
            w.s(t!(b" engine=")).u(SIZES[2] as u64).s(t!(b" lib_state=")).u(SIZES[3] as u64);
        });
        return;
    }
    ring.cancel_next();
    let Some(mem) = region(ring.s.lend(), arena) else { return };
    // The High clock, held, for comparable numbers.
    CODEC_FLOOR.store(2, Ordering::Relaxed);
    dsp_clock_floor(false);
    let cyc_ok = cycles::start();
    let n = if b == u32::MAX { bench::VOICES } else { b.min(255) as u8 };
    bench_load(mem, a as usize, n, c.clamp(1, 100_000), cyc_ok).await;
    CODEC_FLOOR.store(0, Ordering::Relaxed);
    dsp_clock_floor(false);
}

/// `synth bench I N BLOCKS`: as `orpheus_synth::bench::run` (the same
/// blocks and events, so the hash is the host's), with breaks for thread
/// mode outside the measurement.
async fn bench_load(mem: &mut [u8], i: usize, n: u8, blocks: u32, cyc_ok: bool) {
    let Some(e) = place::<Engine>(mem) else { return };
    let Some(&load) = bench::LOADS.get(i).filter(|_| bench::setup(i, n, e)) else {
        reply(|w| {
            w.s(t!(b"synth bench: no load ")).u(i as u64);
        });
        return;
    };
    let buf = scratch16(2 * bench::BLOCK);
    let mut ev: [orpheus_synth::Event; MAX_VOICES] = bytemuck::Zeroable::zeroed();
    let (mut h, mut m, mut sounding, mut peak) = (Fnv::new(), Meter::default(), 0u64, 0usize);
    let mut since = time::now_us();
    for k in 0..bench::WARM + blocks {
        let ne = bench::events(i, n, k, &mut ev);
        let timed = k >= bench::WARM;
        if timed {
            m.on();
        }
        e.render_i16(buf, ev.get(..ne).unwrap_or_default());
        if timed {
            m.off();
            h.add(buf);
            sounding += e.active_voices() as u64;
            peak = peak.max(e.active_voices());
        }
        breathe(&mut since).await;
    }
    let frames = u64::from(blocks) * bench::BLOCK as u64;
    reply(|w| {
        // The index only: the load names (`tests/goldens-bench.txt`) would
        // be resident strings.
        w.s(t!(b"bench ")).u(i as u64);
        w.s(t!(b" n=")).u(u64::from(bench::voices(load, n)));
        m.put(w, frames, cyc_ok);
        w.s(t!(b" voices_x100="))
            .u(sounding * 100 / u64::from(blocks.max(1)))
            .s(t!(b" peak="))
            .u(peak as u64);
        put_hash(w, h.0);
    });
}
