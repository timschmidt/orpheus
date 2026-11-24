//! One recording, from the first header byte to the closed file (04 §5.2,
//! 05 §7.5), without I/O: the caller feeds captured blocks in and copies
//! encoded bytes to its ring buffer (a `sink(ring offset, bytes)`), and a
//! second loop issues the card writes the session hands out. The firmware
//! runs those two loops concurrently (capture keeps going while the card
//! is slow); the simulator runs them one after the other.
//!
//! Rules:
//!
//! * The header goes first, as the file's first bytes, and is rewritten in
//!   place every [`HEADER_EVERY_MS`] with the counts of what is on the card
//!   ([`crate::Encoder::header_on_card`]).
//! * A block that does not fit in the ring is dropped whole, unencoded,
//!   counted, and marked in the file (a `dropout` cue point, WAV); so is
//!   audio the capture lost. Never silently.
//! * Stop: by the user, at the time limit (developer `rec start … secs`),
//!   at the automatic split, when the card would have less than
//!   [`CARD_RESERVE`] free, or when a write keeps failing.

use crate::chain::{Chain, ChainConfig};
use crate::encoder::{Encoder, MAX_HEADER, Params};
use crate::meter::Levels;
use crate::ring::{Write, WriteBehind};
use crate::wav::{Cue, CueKind};

/// Header rewrite period.
pub const HEADER_EVERY_MS: u64 = 10_000;
/// The recording stops with this much left on the card.
pub const CARD_RESERVE: u64 = 1 << 20;
/// Allowance for the last clusters and the closing chunks in the card-full
/// check.
pub const CLOSE_MARGIN: u64 = 64 * 1024;
/// Cue points kept (marks and dropouts).
pub const MAX_CUES: usize = 32;
/// A failing write is retried this often before the recording stops.
pub const MAX_WRITE_ERRORS: u32 = 3;

/// Where the recording is.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Recording,
    /// Paused by the user (the meter keeps running).
    Paused,
    /// The rest is being written; then the header, then close.
    Closing,
    /// Closed.
    Done,
}

/// Why it stopped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StopReason {
    User,
    /// The developer command's duration.
    TimeLimit,
    /// Automatic split: a new file follows.
    Split,
    /// Less than [`CARD_RESERVE`] left.
    CardFull,
    /// Writes failed (card removed?).
    CardError,
}

/// A recording's settings.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Config {
    pub params: Params,
    pub chain: ChainConfig,
    /// Stop after this long (developer command).
    pub limit_ms: Option<u64>,
    /// Split after this long.
    pub split_ms: Option<u64>,
    /// Write-behind ring: capacity, preferred write, latency bound.
    pub ring_bytes: usize,
    pub chunk: usize,
    pub max_wait_ms: u32,
}

/// What `rec status` and the screen show.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Status {
    pub state: State,
    pub stop: Option<StopReason>,
    /// Frames in the file (and the encoder's block buffer).
    pub frames: u64,
    /// Bytes produced (header included) and on the card.
    pub produced: u64,
    pub written: u64,
    /// Blocks dropped (ring full) and their frames; frames the capture
    /// lost; drop runs (= dropout cue points).
    pub dropped_blocks: u32,
    pub dropped_frames: u64,
    pub lost_frames: u64,
    pub dropouts: u32,
    pub marks: u32,
    /// Ring: bytes waiting, capacity, highest fill.
    pub ring_fill: u32,
    pub ring_cap: u32,
    pub ring_peak: u32,
    pub levels: Levels,
    pub agc_db10: i16,
    /// Voice activation is waiting for sound.
    pub waiting: bool,
    pub write_errors: u32,
}

/// The session (≈ 500 B) over its two buffers: the encoder's block buffer
/// `B` ([`crate::encoder::BUF_SAMPLES`] samples) and one encoded block of
/// scratch `S` ([`crate::encoder::MAX_BLOCK_BYTES`] bytes).
pub struct Session<B, S> {
    cfg: Config,
    enc: Encoder<B>,
    chain: Chain,
    ring: WriteBehind,
    scratch: S,
    state: State,
    stop: Option<StopReason>,
    cues: [Cue; MAX_CUES],
    ncues: usize,
    marks: u32,
    dropouts: u32,
    dropping: bool,
    dropped_blocks: u32,
    dropped_frames: u64,
    lost_frames: u64,
    header_ms: u64,
    write_errors: u32,
    /// Free bytes on the card when the file was opened.
    card_free: u64,
    /// Frames the session considers written into the stream (encoded +
    /// buffered) — the time position for cue points.
    position: u64,
}

impl<B, S> core::fmt::Debug for Session<B, S> {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        f.debug_struct("Session").field("state", &self.state).field("position", &self.position).finish()
    }
}

/// Where encoded bytes go: `(ring offset, bytes)`.
pub type Sink<'a> = &'a mut dyn FnMut(usize, &[u8]);

impl<B: AsRef<[i16]> + AsMut<[i16]>, S: AsRef<[u8]> + AsMut<[u8]>> Session<B, S> {
    /// A new recording; `card_free` is the free space when the file is
    /// created. Call [`Session::begin`] next.
    #[must_use]
    pub fn new(cfg: Config, card_free: u64, now_ms: u64, enc_buf: B, scratch: S) -> Self {
        Session {
            cfg,
            enc: Encoder::new(cfg.params, enc_buf),
            chain: Chain::new(cfg.chain, cfg.params.rate, cfg.params.channels),
            // Chunks of at most half the ring: the other half takes blocks
            // while one is written.
            ring: WriteBehind::new(cfg.ring_bytes, cfg.chunk.min(cfg.ring_bytes / 2), cfg.max_wait_ms),
            scratch,
            state: State::Recording,
            stop: None,
            cues: [Cue { frame: 0, kind: CueKind::Mark }; MAX_CUES],
            ncues: 0,
            marks: 0,
            dropouts: 0,
            dropping: false,
            dropped_blocks: 0,
            dropped_frames: 0,
            lost_frames: 0,
            header_ms: now_ms,
            write_errors: 0,
            card_free,
            position: 0,
        }
    }

    #[must_use]
    pub fn config(&self) -> &Config {
        &self.cfg
    }

    #[must_use]
    pub fn params(&self) -> Params {
        self.cfg.params
    }

    #[must_use]
    pub fn state(&self) -> State {
        self.state
    }

    /// Change the processing while recording (AGC, low-cut, VOX).
    pub fn set_chain(&mut self, c: ChainConfig) {
        self.cfg.chain = c;
        self.chain.set_config(c);
    }

    fn emit(&mut self, n: usize, now_ms: u64, sink: Sink<'_>) -> bool {
        let Some(parts) = self.ring.put_at(n) else { return false };
        let mut o = 0;
        for (at, len) in parts {
            if len > 0 {
                sink(at, self.scratch.as_ref().get(o..o + len).unwrap_or(&[]));
                o += len;
            }
        }
        self.ring.put(n, now_ms);
        true
    }

    /// Put the header (placeholder counts) into the ring: the file's first
    /// bytes.
    pub fn begin(&mut self, now_ms: u64, sink: Sink<'_>) {
        let mut h = [0u8; MAX_HEADER];
        let n = self.enc.header(&mut h, 0);
        if let Some(d) = self.scratch.as_mut().get_mut(..n) {
            d.copy_from_slice(&h[..n]);
        }
        self.emit(n, now_ms, sink);
        self.header_ms = now_ms;
    }

    fn cue(&mut self, kind: CueKind) {
        if let Some(c) = self.cues.get_mut(self.ncues) {
            *c = Cue { frame: u32::try_from(self.position).unwrap_or(u32::MAX), kind };
            self.ncues += 1;
        }
    }

    fn dropped(&mut self, frames: u64, block: bool) {
        // The frames are not in the file: positions after them move back.
        self.position = self.position.saturating_sub(frames);
        if !self.dropping {
            self.dropping = true;
            self.dropouts += 1;
            self.cue(CueKind::Dropout);
        }
        if block {
            self.dropped_blocks += 1;
        }
        self.dropped_frames += frames;
    }

    /// Encode the waiting block into the ring, or drop it if it might not
    /// fit.
    fn flush_block(&mut self, now_ms: u64, sink: Sink<'_>) {
        if self.ring.free() < self.enc.max_block_bytes().min(self.ring.capacity()) {
            let f = self.enc.discard();
            self.dropped(f as u64, true);
            return;
        }
        let n = self.enc.encode(self.scratch.as_mut());
        if n > 0 && self.emit(n, now_ms, sink) {
            self.dropping = false;
        }
    }

    /// One captured block (interleaved, the session's channels), processed
    /// in place. While paused only the meter runs.
    pub fn feed(&mut self, block: &mut [i16], now_ms: u64, sink: Sink<'_>) {
        let keep = self.chain.process(block);
        if self.state != State::Recording || !keep {
            return;
        }
        let ch = usize::from(self.cfg.params.channels);
        let mut rest: &[i16] = block;
        while !rest.is_empty() {
            let n = self.enc.push(rest);
            self.position += (n / ch) as u64;
            rest = rest.get(n..).unwrap_or(&[]);
            if self.enc.ready() {
                self.flush_block(now_ms, sink);
            }
        }
    }

    /// The capture lost `frames` (its DMA lapped the reader).
    pub fn lost(&mut self, frames: u64) {
        if frames > 0 {
            self.lost_frames += frames;
            self.dropped(0, false);
            self.dropping = false;
        }
    }

    pub fn pause(&mut self) {
        if self.state == State::Recording {
            self.state = State::Paused;
        }
    }

    pub fn resume(&mut self) {
        if self.state == State::Paused {
            self.state = State::Recording;
        }
    }

    /// A mark at the current position (05 §7.5 Next).
    pub fn mark(&mut self) {
        if matches!(self.state, State::Recording | State::Paused) {
            self.marks += 1;
            self.cue(CueKind::Mark);
        }
    }

    /// Length of the recording so far.
    #[must_use]
    pub fn elapsed_ms(&self) -> u64 {
        self.position * 1000 / u64::from(self.cfg.params.rate.max(1))
    }

    /// A reason to stop now, if any (the caller then calls
    /// [`Session::finish`]). Card full counts what is still in the ring.
    #[must_use]
    pub fn due_stop(&self) -> Option<StopReason> {
        if self.stop.is_some() {
            return self.stop;
        }
        let e = self.elapsed_ms();
        if self.cfg.limit_ms.is_some_and(|l| e >= l) {
            return Some(StopReason::TimeLimit);
        }
        if self.cfg.split_ms.is_some_and(|l| e >= l) {
            return Some(StopReason::Split);
        }
        // What the file will take: produced, the next block, and room for
        // cluster rounding and the closing chunks.
        let used = self.ring.produced() + self.enc.max_block_bytes() as u64 + CLOSE_MARGIN;
        if self.card_free.saturating_sub(used) < CARD_RESERVE {
            return Some(StopReason::CardFull);
        }
        None
    }

    /// Stop: encode what waits, add the closing chunks. Then drain the
    /// writes ([`Session::next_write`] with everything due) and write the
    /// final header ([`Session::final_header`]).
    pub fn finish(&mut self, why: StopReason, now_ms: u64, sink: Sink<'_>) {
        if matches!(self.state, State::Closing | State::Done) {
            return;
        }
        self.stop.get_or_insert(why);
        if why != StopReason::CardError {
            self.flush_block(now_ms, sink);
            let cues = self.cues;
            let cues = cues.get(..self.ncues).unwrap_or(&[]);
            let n = self.enc.trailer_len(cues);
            if n > 0 && n <= self.scratch.as_ref().len() {
                let n = self.enc.trailer(self.scratch.as_mut(), cues);
                if !self.emit(n, now_ms, sink) {
                    // No room for the cue chunks: the audio matters more.
                    self.ncues = 0;
                }
            } else {
                self.ncues = 0;
            }
        }
        self.state = State::Closing;
    }

    /// The next card write, if due (all of it while closing).
    pub fn next_write(&mut self, now_ms: u64) -> Option<Write> {
        self.ring.next_write(now_ms, self.state == State::Closing)
    }

    /// The write handed out finished or failed.
    pub fn written(&mut self, ok: bool, now_ms: u64) {
        self.ring.done(ok, now_ms);
        if ok {
            self.write_errors = 0;
        } else {
            self.write_errors += 1;
            if self.write_errors >= MAX_WRITE_ERRORS {
                self.stop = Some(StopReason::CardError);
                self.state = State::Done;
            }
        }
    }

    /// The periodic header rewrite is due (only while recording).
    #[must_use]
    pub fn header_due(&self, now_ms: u64) -> bool {
        // Not before the placeholder's sector is on the card: a later ring
        // write would put the placeholder back over the rewrite.
        matches!(self.state, State::Recording | State::Paused)
            && now_ms.saturating_sub(self.header_ms) >= HEADER_EVERY_MS
            && self.ring.written() >= self.enc.header_len() as u64
    }

    /// The header for a rewrite now (counts of what is on the card); write
    /// it at file offset 0.
    pub fn header(&mut self, out: &mut [u8; MAX_HEADER], now_ms: u64) -> usize {
        self.header_ms = now_ms;
        self.enc.header_on_card(out, self.ring.written())
    }

    /// Everything is on the card (closing and the ring empty).
    #[must_use]
    pub fn drained(&self) -> bool {
        self.ring.fill() == 0
    }

    /// The final header: every count, and the closing chunks' length. Then
    /// the file is closed and the session is done.
    pub fn final_header(&mut self, out: &mut [u8; MAX_HEADER]) -> usize {
        let cues = self.cues;
        let t = self.enc.trailer_len(cues.get(..self.ncues).unwrap_or(&[]));
        self.state = State::Done;
        self.enc.header(out, u32::try_from(t).unwrap_or(0))
    }

    #[must_use]
    pub fn status(&self) -> Status {
        Status {
            state: self.state,
            stop: self.stop,
            frames: self.position,
            produced: self.ring.produced(),
            written: self.ring.written(),
            dropped_blocks: self.dropped_blocks,
            dropped_frames: self.dropped_frames,
            lost_frames: self.lost_frames,
            dropouts: self.dropouts,
            marks: self.marks,
            ring_fill: u32::try_from(self.ring.fill()).unwrap_or(u32::MAX),
            ring_cap: u32::try_from(self.ring.capacity()).unwrap_or(u32::MAX),
            ring_peak: u32::try_from(self.ring.peak()).unwrap_or(u32::MAX),
            levels: self.chain.levels(),
            agc_db10: self.chain.agc_db10(),
            waiting: self.chain.waiting(),
            write_errors: self.write_errors,
        }
    }
}
