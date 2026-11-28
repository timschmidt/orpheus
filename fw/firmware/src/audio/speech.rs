//! Speech in the card task (TTS-3, 04 §8 "Output"): the speech task's
//! samples (`crate::speech`, 16 kHz) into the output.
//!
//! * **Over music** ([`mix_i16`], [`mix_i32`], from `Out::piece` after the
//!   DSP chain: the chain is bypassed): resampled to the DAC's rate and
//!   added, the music lowered under it (`orpheus_tts::mix::Mixer`).
//! * **Alone** ([`alone`]: idle, paused, the card with USB): the DAC at
//!   16 kHz on its own `PcmOut`, started as soon as 32 ms are written (the
//!   first sound comes quickly), the ring cleared ahead of the writer so a
//!   late speech plays silence, never stale frames; a stop takes back what
//!   was queued for the DAC beyond 3 ms and fades it over 3 ms
//!   (`Book::rewind`); stopped once the speech has played out. It never
//!   waits: the loops call it and wait on [`wait`] beside their own work.
//!   Music takes the DAC back with [`end_alone`].

use super::{DAC_CH, DmaCh, HALF, PCM_ADDR, PCM_FRAMES, PLAYED, PcmOut, RATE, dac};
use core::sync::atomic::Ordering;
use orpheus_tts::mix::{self, Mixer, Source};

use crate::speech::{ALONE, DUCK, LATE, RT_WAKER, RT_WANTED, RingSource};

/// Frames written before the alone DAC starts (32 ms at 16 kHz).
const START: u32 = 512;
/// What a stop leaves before its fade (3 ms) and the fade (3 ms).
const MARGIN: u32 = 48;
const FADE: u32 = 48;

struct Rt {
    mix: Mixer,
    src: RingSource,
    out: PcmOut,
    /// Absolute frames (half · HALF + fill) cleared ahead of the writer.
    clear_to: u32,
}

static mut RT: Option<Rt> = None;

/// The card task's speech state (built on first use).
#[inline(never)]
fn rt() -> &'static mut Rt {
    // SAFETY: only the card task (one task on the RT executor) calls this.
    let slot = unsafe { &mut *(&raw mut RT) };
    slot.get_or_insert_with(|| Rt {
        mix: Mixer::new(mix::RATE),
        src: RingSource::new(),
        out: PcmOut::new(),
        clear_to: 0,
    })
}

/// Speech to mix: a cheap test for `Out::piece` (every piece).
#[inline(always)]
pub(super) fn wanted() -> bool {
    crate::speech::speaking() || DUCK.load(Ordering::Relaxed) != mix::Q16_ONE
}

fn rate() -> u32 {
    match RATE.load(Ordering::Relaxed) {
        0 => 44_100,
        r => r,
    }
}

/// Mix speech into a piece of 16-bit music (`n` valid samples, `ch` a
/// frame) at the DAC's rate.
#[inline(never)]
pub(super) fn mix_i16(s: &mut [i16], n: usize, ch: usize) {
    let rt = rt();
    rt.mix.set_rate(rate());
    let n = n.min(s.len());
    rt.mix.mix_i16(&mut s[..n], ch, crate::speech::gains(), &mut rt.src);
    DUCK.store(rt.mix.music_gain(), Ordering::Relaxed);
}

/// [`mix_i16`] for 32-bit pieces.
#[inline(never)]
pub(super) fn mix_i32(s: &mut [i32], n: usize, ch: usize) {
    let rt = rt();
    rt.mix.set_rate(rate());
    let n = n.min(s.len());
    rt.mix.mix_i32(&mut s[..n], ch, crate::speech::gains(), &mut rt.src);
    DUCK.store(rt.mix.music_gain(), Ordering::Relaxed);
}

fn abs(out: &PcmOut) -> u32 {
    out.book.written.wrapping_mul(HALF as u32).wrapping_add(out.book.fill)
}

fn ring_at(a: u32) -> *mut u32 {
    (PCM_ADDR as *mut u32).wrapping_add(a as usize % PCM_FRAMES)
}

/// One frame at the writer; true when it finished a half.
fn put(out: &mut PcmOut, f: u32) -> bool {
    // SAFETY: inside the PCM ring in SRAM3, at the writer (ahead of the
    // DMA, or the DAC is not running).
    unsafe { core::ptr::write_volatile(out.slot(), f) };
    out.book.wrote()
}

/// Silence the ring from the writer to one half ahead (what the DMA plays
/// if the speech is late), once per frame.
fn clear_ahead(rt: &mut Rt) {
    let a = abs(&rt.out);
    let to = a.wrapping_add(HALF as u32);
    let mut c = if rt.clear_to.wrapping_sub(a) as i32 > 0 { rt.clear_to } else { a };
    while to.wrapping_sub(c) as i32 > 0 {
        // SAFETY: ahead of the writer, inside the ring.
        unsafe { core::ptr::write_volatile(ring_at(c), 0) };
        c = c.wrapping_add(1);
    }
    rt.clear_to = to;
}

/// A stop while the DAC plays speech: what is queued beyond `MARGIN`
/// frames is taken back; the next `FADE` frames fade what was there, the
/// rest is silence.
#[inline(never)]
fn hush(rt: &mut Rt) {
    let out = &mut rt.out;
    let back = out.book.rewind(PcmOut::reading(), MARGIN);
    if back == 0 {
        return;
    }
    let a = abs(out);
    for k in 0..back {
        // SAFETY: frames the DMA has not reached (beyond the margin).
        unsafe {
            let p = ring_at(a.wrapping_add(k));
            let v = if k < FADE {
                let f = core::ptr::read_volatile(p) as u16 as i16;
                let s = (i32::from(f) * (FADE - k) as i32 / FADE as i32) as i16 as u16;
                u32::from(s) | u32::from(s) << 16
            } else {
                0
            };
            core::ptr::write_volatile(p, v);
        }
    }
    // The faded frames are the writer's: the next speech follows them.
    let fade = back.min(FADE);
    for _ in 0..fade {
        if out.book.wrote() {
            let r = PcmOut::reading();
            out.book.boundary(r);
        }
    }
    rt.clear_to = a.wrapping_add(back);
}

/// Speech alone (nothing plays): see the module documentation. True while
/// its DAC runs.
#[inline(never)]
pub(super) fn alone() -> bool {
    let rt = rt();
    let live = mix::Source::sounding(&rt.src);
    if !rt.out.running && rt.out.book.written == 0 && rt.out.book.fill == 0 {
        // A stop with nothing written: drop the stopped speech.
        if rt.src.stopped() {
            rt.src.drop_stopped();
        }
        if rt.src.ready() == 0 {
            return false;
        }
        // The ring silenced (no music from before), the DAC at 16 kHz.
        for k in 0..PCM_FRAMES as u32 {
            // SAFETY: the DAC is stopped; the ring is this task's.
            unsafe { core::ptr::write_volatile(ring_at(k), 0) };
        }
        rt.clear_to = PCM_FRAMES as u32;
        dac::set_rate(mix::RATE);
        rt.mix.reset();
        ALONE.store(true, Ordering::Relaxed);
    }
    if rt.src.stopped() {
        if rt.out.running {
            hush(rt);
        } else {
            // Not heard yet: forget it.
            rt.out.book.reset();
            rt.clear_to = 0;
        }
        rt.src.drop_stopped();
    }
    let g = crate::speech::gains();
    loop {
        if rt.out.running && rt.out.book.full(PLAYED.load(Ordering::Acquire)) {
            break;
        }
        let Some(s) = rt.mix.alone(g, &mut rt.src) else { break };
        let v = u32::from(s as u16);
        if put(&mut rt.out, v | v << 16) && rt.out.running {
            let r = PcmOut::reading();
            let before = rt.out.book.written;
            rt.out.book.boundary(r);
            if rt.out.book.written != before {
                // The DMA overtook the writer: the speech was late.
                LATE.fetch_add(1, Ordering::Relaxed);
            }
        }
        if !rt.out.running && abs(&rt.out) >= START {
            rt.out.start();
        }
    }
    if !rt.out.running {
        if abs(&rt.out) == 0 {
            end_alone();
            return false;
        }
        if !live {
            // A short utterance or an earcon: start with what there is.
            rt.out.start();
        }
    }
    clear_ahead(rt);
    if rt.out.running && !live && rt.out.book.headroom(PcmOut::reading()) <= 0 {
        // Played out (the frames after it are silence).
        end_alone();
        return false;
    }
    rt.out.running
}

/// Speech alone is on the DAC.
#[inline]
pub(super) fn alone_running() -> bool {
    crate::speech::ALONE.load(Ordering::Relaxed)
}

/// The DAC back for the music (or nothing): the alone output stopped, the
/// music's rate set again. The speech not yet heard goes on over the
/// music.
#[inline(never)]
pub(super) fn end_alone() {
    if !ALONE.swap(false, Ordering::Relaxed) {
        return;
    }
    let rt = rt();
    if rt.out.running {
        // SAFETY: channel 5 is this module's parent's; the alone output
        // owns it while `ALONE` was set.
        unsafe { DmaCh::steal(DAC_CH) }.abort();
    }
    rt.out.running = false;
    rt.out.book.reset();
    rt.clear_to = 0;
    let r = RATE.load(Ordering::Relaxed);
    if r != 0 {
        dac::set_rate(r);
    }
}

/// How long the alone output waits at most between two top-ups (it is
/// woken after each batch of the speech task as well): well inside the
/// half (96 ms) cleared ahead of the writer. TTS-3b, 2026-11-28: 10 ms
/// before, 100 wake-ups of the card task a second while speaking alone,
/// each one through the paused or idle loop's code.
const TOP_UP_MS: u64 = 32;
const _: () = assert!(TOP_UP_MS * mix::RATE as u64 / 1000 < HALF as u64 / 2);

/// Wait for speech to take or a stop; while the alone output runs, at most
/// [`TOP_UP_MS`] (it tops up and clears ahead).
pub(super) async fn wait() {
    let woken = core::future::poll_fn(|cx| {
        RT_WAKER.register(cx.waker());
        if RT_WANTED.swap(false, Ordering::AcqRel) {
            core::task::Poll::Ready(())
        } else {
            core::task::Poll::Pending
        }
    });
    if alone_running() {
        let _ = embassy_futures::select::select(woken, embassy_time::Timer::after_millis(TOP_UP_MS)).await;
    } else {
        woken.await;
    }
}

/// The card is with USB (`release`): speech alone until the card comes
/// back (this future is dropped then).
pub(super) async fn serve() -> core::convert::Infallible {
    loop {
        alone();
        wait().await;
    }
}
