//! The Games app on the player (05 §7.13; GAME-2 integration): the hub
//! the screens play in and its card files `/.orpheus/games/{blocks.sav,
//! 4096.sav,solitaire.sav,scores.bin}`.
//!
//! The UI's `GamesService` (`services.rs`) turns requests into card
//! operations in an `orpheus_apps::games::GamesIo` queue (host-tested, the
//! simulator's `set:gamesio=on` runs the same logic). One operation at a
//! time goes to the card task as `media::Req::Games` with the 512-byte
//! buffer of [`GamesRam`] lent: the UI neither reads nor writes it until
//! [`SLOT`] is 0 again (the Authenticator's import uses the same slot
//! protocol). The card task carries it out in steps ([`begin`], [`piece`]:
//! `orpheus_apps::games::CardRun`, host-tested): all at once when idle
//! (power-off, USB hand-over: the DAC is stopped); while music plays the
//! request's chore runs the first piece and the rest are listing-job
//! pieces of their own kind (`job:games`), each stopping at the chore's
//! budget (`audio::piece_left`): in one piece a save and a read were
//! 100 ms on the player (8 underruns, 2026-11-25). The answer comes back with the data
//! tag [`TAG`]; the UI applies it (`GamesIo::done`) and the screens hear
//! `other::GAMES`.
//!
//! RAM: `GamesRam` (the hub, at most 848 B, the queue and the job's
//! bookkeeping; taken once by `Fw::new`) and the 512-byte buffer `BUF`.

use core::cell::Cell;
use core::mem::MaybeUninit;
use core::sync::atomic::{AtomicU32, Ordering};

use critical_section::Mutex;
use orpheus_apps::games::engine::{Hub, SAVE_MAX};
use orpheus_apps::games::{CardAnswer, CardOp, CardRun, GamesIo};
use orpheus_fs::Volume;
use yoton_bsp::card::Card;

use crate::media;

/// The data-ready tag of a card operation's answer.
pub const TAG: u16 = 0x101;

/// How long a job may wait to be started before the UI takes it back
/// (the card task is busy elsewhere, or the card is with USB).
const START_TIMEOUT_MS: u64 = 3000;

/// One card operation and the bytes it may use (`buf..buf + SAVE_MAX`,
/// lent by the UI; `len` of them to write).
#[derive(Clone, Copy, Debug)]
pub struct GamesJob {
    pub seq: u32,
    pub op: CardOp,
    pub buf: usize,
    pub len: u16,
}

/// 0 none, `seq` queued, `seq | media::PIC_RUNNING` being carried out.
pub static SLOT: AtomicU32 = AtomicU32::new(0);
/// The last job's number and answer.
static RESULT: Mutex<Cell<(u32, CardAnswer)>> = Mutex::new(Cell::new((0, CardAnswer::FAILED)));

/// The games' RAM: the hub, the card queue, the buffer lent with a job.
pub struct GamesRam {
    pub hub: Hub,
    pub io: GamesIo,
    /// The job out: its number and when it was sent (ms).
    out: Option<(u32, u64)>,
    seq: u32,
}

/// In `.bss` (no initial image in the resident copy: a `const` hub is not
/// all zeros and would land in `.data`).
static mut RAM: MaybeUninit<GamesRam> = MaybeUninit::uninit();

/// The bytes a job reads into or writes from: the UI's while no job is
/// out, the card task's while one is. Reached only through raw pointers
/// (no reference outlives either side's turn).
static mut BUF: [u8; SAVE_MAX] = [0; SAVE_MAX];

/// The UI's view of [`BUF`].
///
/// # Safety
/// No job may be out ([`SLOT`] is 0 and the UI is not waiting for an
/// answer), and the result must not be kept past the call it is made for.
unsafe fn ui_buf() -> &'static mut [u8; SAVE_MAX] {
    // SAFETY: the caller guarantees the card task does not hold the
    // buffer; the UI task is the only other user.
    unsafe { &mut *(&raw mut BUF) }
}

/// Once, from `Fw::new`.
pub fn ram() -> &'static mut GamesRam {
    // SAFETY: called once (`Fw::new` runs once, in the UI task), so this is
    // the only reference to the static.
    unsafe { (*(&raw mut RAM)).write(GamesRam { hub: Hub::new(), io: GamesIo::new(), out: None, seq: 0 }) }
}

impl GamesRam {
    /// A job is out or operations wait.
    #[must_use]
    pub fn busy(&self) -> bool {
        self.out.is_some() || self.io.busy()
    }

    /// Every UI loop (and after each request): apply the answer of the job
    /// out, take back one that never started, send the next. Without a
    /// mounted card the operations fail at once (a read starts a new
    /// game, a save is not marked saved). True if the hub changed.
    pub fn pump(&mut self, now_ms: u64) -> bool {
        let mut changed = false;
        if let Some((seq, since)) = self.out {
            let slot = SLOT.load(Ordering::Acquire);
            let answer = if slot == 0 {
                let (rseq, a) = critical_section::with(|cs| RESULT.borrow(cs).get());
                Some(if rseq == seq { a } else { CardAnswer::FAILED })
            } else if slot == seq
                && now_ms.saturating_sub(since) > START_TIMEOUT_MS
                && SLOT.compare_exchange(seq, 0, Ordering::AcqRel, Ordering::Acquire).is_ok()
            {
                Some(CardAnswer::FAILED)
            } else {
                None
            };
            let Some(a) = answer else { return false };
            self.out = None;
            // SAFETY: the job is answered or taken back (`SLOT` is 0): the
            // buffer is the UI's again.
            let buf = unsafe { ui_buf() };
            changed |= self.io.done(&mut self.hub, a, buf);
        }
        let card = crate::state::CARD_READY.load(Ordering::Relaxed);
        loop {
            // SAFETY: no job is out (`self.out` is `None` here).
            let buf = unsafe { ui_buf() };
            let Some((op, len)) = self.io.next(&self.hub, buf) else { break };
            if !card {
                changed |= self.io.done(&mut self.hub, CardAnswer::FAILED, buf);
                continue;
            }
            self.seq = (self.seq.wrapping_add(1) & !media::PIC_RUNNING).max(1);
            let job = GamesJob {
                seq: self.seq,
                op,
                buf: (&raw mut BUF) as usize,
                len: u16::try_from(len).unwrap_or(0),
            };
            SLOT.store(job.seq, Ordering::Release);
            if media::try_request(media::Req::Games(job)) {
                self.out = Some((job.seq, now_ms));
            } else {
                // The request queue is full: again in the next loop.
                SLOT.store(0, Ordering::Release);
                self.io.put_back();
            }
            break;
        }
        changed
    }
}

/// The card task: start job `j` unless the UI took it back.
fn start(j: &GamesJob) -> bool {
    SLOT.compare_exchange(j.seq, j.seq | media::PIC_RUNNING, Ordering::AcqRel, Ordering::Acquire).is_ok()
}

/// The card task: job `j` is done (the buffer goes back to the UI).
fn done(j: &GamesJob, a: CardAnswer) {
    critical_section::with(|cs| RESULT.borrow(cs).set((j.seq, a)));
    SLOT.store(0, Ordering::Release);
    media::ready(TAG);
}

/// The card task took job `j` from its queue: its steps, unless the UI
/// took it back.
pub fn begin(j: &GamesJob) -> Option<CardRun> {
    start(j).then(|| CardRun::new(j.op, usize::from(j.len)))
}

/// The card task: one piece of job `j` (only under `/.orpheus/games/`,
/// CLAUDE.md rule 5): its steps while `audio::piece_left` allows (while
/// music plays a chore's budget, `job:games`; idle, all of them). True
/// when it is done and answered.
pub async fn piece(vol: &mut Volume<'static, Card>, j: &GamesJob, run: &mut CardRun) -> bool {
    // SAFETY: the UI lent `buf..buf + SAVE_MAX` (`BUF`, a static) for this
    // job and neither reads nor writes it until `SLOT` is 0 again (`done`);
    // `start` made this task its only user, until then, across pieces.
    let buf = unsafe { core::slice::from_raw_parts_mut(j.buf as *mut u8, SAVE_MAX) };
    match run.piece(vol, buf, crate::audio::piece_left).await {
        Some(a) => {
            done(j, a);
            true
        }
        None => false,
    }
}
