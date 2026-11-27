//! MIDI clock out: the player's tempo as MIDI real-time messages (Start,
//! 24 Clocks a quarter note, Stop), each on its frame of the player's own
//! clock — the host-side model of 05 §7.16's "MIDI clock out over USB
//! MIDI". The USB MIDI transport (a MIDI streaming interface beside M16's
//! USB audio personality) and its setting are **deferred to M16**; what is
//! here is everything the transport needs: which byte, on which sample,
//! and the USB MIDI 1.0 event packet that carries it.
//!
//! # Timing
//!
//! The clock counts in the player's units (see [`crate::player`]): a
//! quarter note is `beat_steps × 28,800` units, a clock 1/24 of that
//! ([`tick_units`]: 4,800 units for 1/16 steps, 2,400 for 1/8, 9,600 for
//! 1/32), and a clock's frame comes from the same
//! `anchor + ⌊units · rate · 5 · m / (tempo · 96 · d)⌋` as every step and
//! hit. So at a steady tempo clock `k` of a play is on
//! `start + ⌊k · rate · 250 / tempo⌋` (tempo in 1/100 BPM) — the floor of
//! its exact time, whatever the step length — and with swing 50 every step
//! starts on a clock (every 6th for 1/16 steps). Swing moves the odd steps,
//! not the clock: the clock is the tempo, a receiver swings by itself.
//!
//! A tempo or step-length change takes effect, as in the player, at the
//! next step: the part of a clock already elapsed at that step is kept
//! (in units, scaled by the step lengths' ratio), so the clock keeps its
//! phase and the step of the change lands on a clock when it would have.
//!
//! # Transport
//!
//! * **Start** (`0xFA`) on the first frame of the first step — after the
//!   count-in, which sends nothing — followed on the same frame by the
//!   first Clock (the receiver's downbeat). Orpheus always plays from the
//!   start of a pattern or of the song, so no Song Position Pointer is sent
//!   (Start means position 0).
//! * **Clock** (`0xF8`) 24 a quarter note while playing.
//! * **Stop** (`0xFC`) on the first frame after [`Sequencer::stop`], when
//!   the clock is turned off while running, and before a restart (a Play
//!   while playing: Stop, then Start at the new first step).
//!
//! Turning the clock on takes effect at the next play (like the
//! count-in). The messages wait in a queue of [`QUEUE`] for
//! [`Sequencer::take_clock`], which the caller calls after each render
//! (the firmware: hand each to the USB MIDI endpoint when the DAC reaches
//! its frame, less the USB latency). A full queue drops a message and
//! counts it ([`Sequencer::clock_dropped`]); the clock's phase goes on, so
//! the next ones are still on their frames. At 240 BPM there are 96 clocks
//! a second: a render block of up to 1/3 s never fills the queue.
//!
//! [`Sequencer::stop`]: crate::Sequencer::stop
//! [`Sequencer::take_clock`]: crate::Sequencer::take_clock
//! [`Sequencer::clock_dropped`]: crate::Sequencer::clock_dropped

/// Clocks per quarter note (MIDI 1.0).
pub const PPQN: u64 = 24;
/// Messages waiting for [`crate::Sequencer::take_clock`].
pub const QUEUE: usize = 32;

/// A MIDI real-time message of the clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Realtime {
    /// Timing clock, `0xF8`.
    Clock,
    /// Start, `0xFA`.
    Start,
    /// Stop, `0xFC`.
    Stop,
}

impl Realtime {
    /// The status byte.
    #[must_use]
    pub const fn byte(self) -> u8 {
        match self {
            Realtime::Clock => 0xF8,
            Realtime::Start => 0xFA,
            Realtime::Stop => 0xFC,
        }
    }

    /// The USB MIDI 1.0 event packet for virtual cable `cable` (0–15):
    /// code index 0xF, a single byte (USB MIDI 1.0 §4, table 4-1).
    #[must_use]
    pub const fn usb_packet(self, cable: u8) -> [u8; 4] {
        [((cable & 0x0F) << 4) | 0x0F, self.byte(), 0, 0]
    }
}

/// One message on its frame of the player's clock.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ClockEvent {
    /// Frame on the player's clock ([`crate::Sequencer::clock`]).
    pub frame: u64,
    /// The message.
    pub msg: Realtime,
}

/// Units of one MIDI clock at step length ratio `(m, d)`
/// ([`crate::StepLen::ratio`]): 4,800 for 1/16 steps.
#[must_use]
pub const fn tick_units((m, d): (u64, u64)) -> u64 {
    crate::player::UNITS_PER_STEP * 4 / PPQN * d / m
}

/// The clock's state inside the player.
#[derive(Clone, Copy, Debug)]
pub(crate) struct State {
    /// Turned on (takes effect at the next play).
    pub on: bool,
    /// Started (Start sent) and not stopped.
    pub running: bool,
    /// Units of the next clock from the player's anchor.
    pub next: u64,
    /// Frames with the message in the top two bits (8 bytes a message).
    queue: [u64; QUEUE],
    n: u8,
    pub dropped: u32,
}

const MSG_SHIFT: u32 = 62;
const FRAME_MASK: u64 = (1 << MSG_SHIFT) - 1;

impl State {
    pub const OFF: State = State { on: false, running: false, next: 0, queue: [0; QUEUE], n: 0, dropped: 0 };

    pub fn push(&mut self, frame: u64, msg: Realtime) {
        if usize::from(self.n) == QUEUE {
            self.dropped = self.dropped.saturating_add(1);
        } else {
            let code = match msg {
                Realtime::Clock => 0,
                Realtime::Start => 1,
                Realtime::Stop => 2,
            };
            // 2^62 frames are millions of years at any rate.
            self.queue[usize::from(self.n)] = (frame & FRAME_MASK) | (code << MSG_SHIFT);
            self.n += 1;
        }
    }

    /// Send Stop at `frame` if running.
    pub fn halt(&mut self, frame: u64) {
        if self.running {
            self.running = false;
            self.push(frame, Realtime::Stop);
        }
    }

    /// The anchor moves to `units` (the step where the tempo or the step
    /// length changes, from the old anchor); the step length's ratio goes
    /// from `old` to `new`.
    pub fn reanchor(&mut self, units: u64, old: (u64, u64), new: (u64, u64)) {
        let rel = self.next.saturating_sub(units);
        self.next =
            u64::try_from(u128::from(rel) * u128::from(tick_units(new)) / u128::from(tick_units(old)))
                .unwrap_or(u64::MAX);
    }

    pub fn take(&mut self, mut f: impl FnMut(ClockEvent)) {
        for e in &self.queue[..usize::from(self.n)] {
            let msg = match e >> MSG_SHIFT {
                0 => Realtime::Clock,
                1 => Realtime::Start,
                _ => Realtime::Stop,
            };
            f(ClockEvent { frame: e & FRAME_MASK, msg });
        }
        self.n = 0;
    }
}
