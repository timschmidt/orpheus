//! What the engine costs on the player, and the load governor that keeps a
//! render inside its share of the CPU (SYN-3 follow-up, 2026-11-26; 04 §9
//! "Load governor").
//!
//! # Costs
//!
//! [`PLAYER`] holds cycles per frame at 144 MHz as `synth bench` measured
//! them on the player (build `1fda608d`, 2026-11-26; no DWT counter on the
//! chip, so µs × 144). They include the pager's line loads during the
//! bench, as playback sees them. A voice costs by what it plays
//! ([`Class`]: subtractive, FM by operator count, each drum kind, sample);
//! an effect costs while it runs (enabled, and fed by a part's send or
//! still ringing). [`Engine::estimate`](crate::Engine::estimate) sums them
//! for the engine's state now.
//!
//! # The governor
//!
//! [`Governor`] runs once per render piece. Before it: [`Governor::plan`]
//! takes the estimate, scales it by the measured ratio of real to
//! estimated cycles (paging, interrupts and patches the table does not
//! know), and when the predicted cost is above [`BUDGET_PCT`] of the
//! frame's real time (or a smoothed cost is), it sheds, **in this order**:
//!
//! 1. the **chorus** (a widening of a few parts; General MIDI files leave
//!    its send at 0 unless they ask for it);
//! 2. the **delay** (not in the player's build; the Sequencer's later);
//! 3. the **reverb** (the room around every part: the notes stay, the
//!    sound gets drier — 671 cycles, the most any one step frees);
//! 4. **voices**: the voice limit comes down to what the budget affords and
//!    the voices above it fade out (256 samples), released ones (already
//!    dying away) oldest first, then the oldest held ones; never below
//!    [`MIN_VOICES`].
//!
//! The order keeps the notes as long as anything else can go: an effect
//! colours the music, a dropped voice changes it. An effect fades out over
//! 256 samples and is then cleared. The decisions leave out the voices
//! fading out (stolen into a tail, cut, shed: gone within 256 samples);
//! the piece's length counts them. After [`CALM_MS`] (a voice) or
//! [`CALM_FX_MS`] (an effect) of pieces in which the predicted cost *with
//! the next step undone* stays under [`RESTORE_PCT`], the last step is
//! undone: voices first, one at a time, then the reverb, the delay, the
//! chorus. Rendering at half the rate (22.05 kHz, which the
//! DAC plays natively) was considered and left out: the engine's state is
//! rate-dependent throughout (every coefficient, the reverb's line layout)
//! and a DAC rate change stops the PCM ring (a gap) — see 04 §9.
//!
//! [`Governor::frames`] bounds a piece: as many frames as fit
//! [`PIECE_US`] at the predicted cost (at least [`MIN_PIECE`], at most the
//! block), so no single render call keeps the CPU long whatever the song.
//! After it, [`Governor::measured`] updates the ratio. The firmware rests
//! by `orpheus_power::pace` before a piece that would end past the pace
//! rule's run ([`Plan::us`]); [`sim`] models the whole loop.
//!
//! With nothing shed the engine renders exactly what the host renders
//! (bit for bit); the governor's actions change the sound, and are counted.

use crate::engine::MAX_VOICES;

/// The drum kinds ([`crate::DrumKind`], in order).
pub const DRUM_KINDS: usize = 11;

/// What a voice plays, for its cost.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Class {
    /// Two oscillators, noise, filter, envelopes.
    Sub,
    /// FM with this many operators (2–4).
    Fm(u8),
    /// A drum of this kind (`DrumKind as usize`).
    Drum(u8),
    /// A sample or a kit's sample hit.
    Sample,
}

/// Cycles per frame of the parts of a render.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Costs {
    /// The engine with nothing sounding (mix, master, limiter, the block).
    pub empty: u32,
    /// A subtractive voice.
    pub sub: u32,
    /// A 2-operator FM voice.
    pub fm2: u32,
    /// Each operator past two.
    pub fm_op: u32,
    /// A drum voice by kind.
    pub drum: [u32; DRUM_KINDS],
    /// A sample voice.
    pub sample: u32,
    /// The send effects while they run: delay, chorus, reverb.
    pub fx: [u32; 3],
}

/// The player's costs at 144 MHz (`synth bench`, build `1fda608d`,
/// 2026-11-26; `crates/orpheus-synth/tests/bench-player-1fda608d.txt`).
/// Per voice: (load − empty) / voices. Subtractive: 280, between the
/// presets' median (246) and their dearest (pwm-pad 308). FM: fm-bell and
/// fm-bass (2 operators) 288, fm-epiano (4) 453. Effects: the effect's load
/// with one sine voice less the same voice into the (compiled-out, dry)
/// delay — chorus 764 − 647 = 117, reverb 1,318 − 647 = 671 — and with
/// eight voices: chorus 2,905 − 2,711 = 194, reverb 2,996 − 2,344 = 652;
/// the larger of each. (The lead's first reading, 519 and 1,073, also
/// counted the sine voice and its line loads: 402.) The delay is not in
/// the player's build: SYN-2's host estimate × 2.3 (the factor between
/// the SYN-1/2 estimates and these measurements).
pub const PLAYER: Costs = Costs {
    empty: 245,
    sub: 280,
    fm2: 288,
    fm_op: 83,
    // kick, snare, closed hat, open hat, clap, tom, rim, cowbell, cymbal,
    // clave, shaker.
    drum: [296, 388, 696, 695, 159, 256, 273, 332, 737, 248, 186],
    sample: 300,
    fx: [210, 194, 671],
};

impl Costs {
    /// A voice of class `c`.
    #[must_use]
    pub fn voice(&self, c: Class) -> u32 {
        match c {
            Class::Sub => self.sub,
            Class::Fm(ops) => self.fm2 + u32::from(ops.saturating_sub(2)) * self.fm_op,
            Class::Drum(k) => self.drum.get(usize::from(k)).copied().unwrap_or(self.sub),
            Class::Sample => self.sample,
        }
    }
}

/// Effect bits ([`crate::Engine::set_fx_enabled`]).
pub const FX_DELAY: u8 = 1;
/// The chorus.
pub const FX_CHORUS: u8 = 2;
/// The reverb.
pub const FX_REVERB: u8 = 4;
/// Every effect.
pub const FX_ALL: u8 = 7;

/// The engine's cost now, in cycles per frame ([`crate::Engine::estimate`]).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Estimate {
    /// The empty engine.
    pub base: u32,
    /// The sounding voices (fading ones too).
    pub voices: u32,
    /// How many.
    pub n: u32,
    /// Of `voices`, those fading out (stolen into a tail, cut or shed:
    /// gone within 256 samples).
    pub fading: u32,
    /// How many.
    pub n_fading: u32,
    /// Each effect's cost while it runs (delay, chorus, reverb).
    pub fx: [u32; 3],
    /// The effects that run now (`FX_*`: enabled, and fed or ringing).
    pub running: u8,
    /// The effects that would run if enabled (a part sends to them, or
    /// they ring).
    pub wanted: u8,
}

impl Estimate {
    /// The effects of `mask` that run, summed.
    #[must_use]
    pub fn fx_of(&self, mask: u8) -> u32 {
        (0..3).filter(|i| mask & (1 << i) != 0).map(|i| self.fx[i]).sum()
    }

    /// Everything that runs now.
    #[must_use]
    pub fn total(&self) -> u32 {
        self.base + self.voices + self.fx_of(self.running)
    }

    /// Everything that runs now but the voices fading out: what the next
    /// pieces cost once they are gone.
    #[must_use]
    pub fn lasting(&self) -> u32 {
        self.total().saturating_sub(self.fading)
    }

    /// The mean cost of a voice that is not fading (a subtractive one's
    /// when none).
    #[must_use]
    pub fn per_voice(&self) -> u32 {
        match self.n.saturating_sub(self.n_fading) {
            0 => PLAYER.sub,
            n => self.voices.saturating_sub(self.fading).div_ceil(n),
        }
    }
}

/// Share of a frame's real time the render may take (the DSP chain, the
/// PCM ring, chores and thread mode share the rest; the pace rule's rests
/// alone take a third of a busy stretch).
pub const BUDGET_PCT: u32 = 55;
/// A piece predicted above this share sheds at once, whatever the smoothed
/// cost says.
pub const HARD_PCT: u32 = 80;
/// A shed step is undone when the cost with it undone stays under this.
pub const RESTORE_PCT: u32 = 50;
/// Time under [`RESTORE_PCT`] before a voice comes back, ms.
pub const CALM_MS: u32 = 1_000;
/// … before an effect comes back (an effect switching on and off is
/// heard; a note more is not).
pub const CALM_FX_MS: u32 = 3_000;
/// The voice limit never goes below this.
pub const MIN_VOICES: u8 = 2;
/// The longest render piece, µs (half the pace rule's 8 ms run).
pub const PIECE_US: u32 = 4_000;
/// The fewest frames in a piece (one control period).
pub const MIN_PIECE: u32 = 32;
/// The most (the firmware's render block).
pub const BLOCK: u32 = 256;
/// The ratio's unit.
const ONE: u32 = 1024;

/// The governor's state (see the module documentation).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Governor {
    /// Measured / estimated cycles, × 1024 (smoothed).
    ratio: u32,
    /// Predicted cycles per frame, smoothed over about four pieces.
    load: u32,
    /// Real-time cycles per frame (clock / rate).
    rt: u32,
    /// Clock, MHz.
    mhz: u32,
    /// The effects the governor has turned off.
    fx_off: u8,
    limit: u8,
    max: u8,
    /// Frames of calm, counted toward [`CALM_MS`].
    calm: u32,
    /// Frames per second.
    rate: u32,
    /// Pieces over budget since the start.
    pub over: u32,
    /// Voices faded out by the governor since the start.
    pub shed: u32,
    /// Pieces measured.
    measures: u32,
}

/// What the next piece does ([`Governor::plan`]).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Plan {
    /// The effects enabled (`FX_*`).
    pub fx: u8,
    /// The voice limit for new notes.
    pub limit: u8,
    /// Fade out voices until this many sound (`None`: leave them).
    pub keep: Option<u8>,
    /// Frames to render.
    pub frames: u32,
    /// Their predicted time, µs (the pace rule rests first when the piece
    /// would end past its run).
    pub us: u32,
}

/// How far the governor has gone (`status` `syn: lvl=`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
#[repr(u8)]
pub enum Level {
    /// Nothing shed: the host's render bit for bit.
    Full = 0,
    /// The chorus (or the delay) off.
    Chorus = 1,
    /// The reverb off too.
    Reverb = 2,
    /// Voices limited below the maximum.
    Voices = 3,
}

impl Governor {
    /// A governor for `max` voices (1–16) at `clock_hz` and `rate` Hz.
    #[must_use]
    pub fn new(max: u8, clock_hz: u32, rate: u32) -> Self {
        let max = max.clamp(MIN_VOICES, MAX_VOICES as u8);
        let mut g = Self {
            ratio: ONE,
            load: 0,
            rt: 1,
            mhz: 1,
            fx_off: 0,
            limit: max,
            max,
            calm: 0,
            rate: 1,
            over: 0,
            shed: 0,
            measures: 0,
        };
        g.set_clock(clock_hz, rate);
        g
    }

    /// The clock or the rate changed.
    pub fn set_clock(&mut self, clock_hz: u32, rate: u32) {
        self.rt = (clock_hz / rate.max(1)).max(1);
        self.rate = rate.max(1);
        self.mhz = (clock_hz / 1_000_000).max(1);
    }

    /// Real-time cycles per frame.
    #[must_use]
    pub fn rt(&self) -> u32 {
        self.rt
    }

    /// The voice limit now.
    #[must_use]
    pub fn limit(&self) -> u8 {
        self.limit
    }

    /// The effects enabled now.
    #[must_use]
    pub fn fx(&self) -> u8 {
        FX_ALL & !self.fx_off
    }

    /// Measured / estimated cycles × 100.
    #[must_use]
    pub fn ratio_pct(&self) -> u32 {
        self.ratio * 100 / ONE
    }

    /// The smoothed predicted cost, cycles per frame.
    #[must_use]
    pub fn load(&self) -> u32 {
        self.load
    }

    /// How far it has shed.
    #[must_use]
    pub fn level(&self) -> Level {
        if self.limit < self.max {
            Level::Voices
        } else if self.fx_off & FX_REVERB != 0 {
            Level::Reverb
        } else if self.fx_off != 0 {
            Level::Chorus
        } else {
            Level::Full
        }
    }

    /// Estimated cycles → predicted (the measured ratio applied).
    fn predict(&self, c: u32) -> u32 {
        (u64::from(c) * u64::from(self.ratio) / u64::from(ONE)).min(u64::from(u32::MAX)) as u32
    }

    fn pct(&self, p: u32) -> u32 {
        (u64::from(self.rt) * u64::from(p) / 100) as u32
    }

    /// Voices that fit `budget` predicted cycles beside the base and the
    /// effects of `fx`, at `e`'s mean voice cost.
    fn affordable(&self, e: &Estimate, fx: u8, budget: u32) -> u8 {
        let fixed = self.predict(e.base + e.fx_of(fx & e.wanted));
        let per = self.predict(e.per_voice()).max(1);
        let n = budget.saturating_sub(fixed) / per;
        // (`max` ≥ `MIN_VOICES` by construction; no `clamp`: its panic
        // would format the values.)
        n.min(u32::from(self.max)).max(u32::from(MIN_VOICES)) as u8
    }

    /// Before a piece: what to enable, the voice limit, voices to fade out
    /// and the piece's length, from the estimate `e` of the engine now.
    pub fn plan(&mut self, e: &Estimate) -> Plan {
        let budget = self.pct(BUDGET_PCT);
        // What sounds now plays this piece (shed voices and effects fade
        // over it): its length comes from the whole cost. The decisions
        // leave out the voices fading out: in a dense song every note
        // beyond the limit steals one into a tail, and counting those
        // brought the limit down again and again (to the floor).
        let all = self.predict(e.total());
        let frames = self.frames(all);
        let us = self.piece_us(frames, all);
        let mut p = self.predict(e.lasting());
        self.load = if self.load == 0 { p } else { self.load - self.load / 4 + p / 4 };
        let mut keep = None;
        if self.load > budget || p > self.pct(HARD_PCT) {
            self.over = self.over.saturating_add(1);
            self.calm = 0;
            // Shed in order until the prediction fits.
            for bit in [FX_CHORUS, FX_DELAY, FX_REVERB] {
                if p > budget && self.fx_off & bit == 0 && e.running & bit != 0 {
                    self.fx_off |= bit;
                    p = p.saturating_sub(self.predict(e.fx_of(bit)));
                }
            }
            if p > budget {
                let lim = self.affordable(e, self.fx(), budget).min(self.limit);
                self.limit = lim;
                let live = e.n.saturating_sub(e.n_fading);
                if live > u32::from(lim) {
                    keep = Some(lim);
                    self.shed = self.shed.saturating_add(live - u32::from(lim));
                    p = self
                        .predict(e.base + e.fx_of(e.running & self.fx()) + u32::from(lim) * e.per_voice());
                }
            }
            // The smoothed cost starts again from what is left.
            self.load = p;
        } else if self.level() != Level::Full {
            self.restore(e, p, frames);
        }
        Plan { fx: self.fx(), limit: self.limit, keep, frames, us }
    }

    /// Undo the last step after a calm stretch.
    fn restore(&mut self, e: &Estimate, p: u32, frames: u32) {
        let target = self.pct(RESTORE_PCT);
        // The cost with the next step undone, and how long it must fit.
        let (undo, extra, ms) = if self.limit < self.max {
            // One voice more than the limit, at the mean voice cost.
            (0, self.predict(e.per_voice()), CALM_MS)
        } else {
            let bit =
                [FX_REVERB, FX_DELAY, FX_CHORUS].into_iter().find(|b| self.fx_off & b != 0).unwrap_or(0);
            (bit, self.predict(e.fx_of(bit & e.wanted)), CALM_FX_MS)
        };
        if p + extra > target {
            self.calm = 0;
            return;
        }
        self.calm += frames;
        if u64::from(self.calm) * 1000 < u64::from(ms) * u64::from(self.rate) {
            return;
        }
        self.calm = 0;
        if undo == 0 {
            self.limit = self.limit.saturating_add(1).min(self.max);
        } else {
            self.fx_off &= !undo;
        }
    }

    /// Frames for a piece predicted at `p` cycles per frame.
    #[must_use]
    pub fn frames(&self, p: u32) -> u32 {
        let cycles = PIECE_US * self.mhz;
        (cycles / p.max(1)).clamp(MIN_PIECE, BLOCK)
    }

    /// The time of `frames` frames at `p` cycles per frame, µs.
    #[must_use]
    pub fn piece_us(&self, frames: u32, p: u32) -> u32 {
        (u64::from(frames) * u64::from(p) / u64::from(self.mhz)).min(u64::from(u32::MAX)) as u32
    }

    /// After a piece of `frames` estimated at `e` (all of it, fading voices
    /// too) took `cycles`.
    pub fn measured(&mut self, e: &Estimate, frames: u32, cycles: u32) {
        let est = u64::from(e.total()) * u64::from(frames);
        if frames < MIN_PIECE || est == 0 {
            return;
        }
        let r =
            (u64::from(cycles) * u64::from(ONE) / est).clamp(u64::from(ONE / 2), u64::from(4 * ONE)) as u32;
        // The first measurement as it is, then smoothed over about four.
        self.ratio = if self.measures == 0 { r } else { self.ratio - self.ratio / 4 + r / 4 };
        self.measures = self.measures.saturating_add(1);
    }
}

/// A model of the firmware's render loop for host tests: the governor, the
/// pieces, the pace rule's rests, the PCM ring and thread mode, in
/// simulated microseconds.
pub mod sim {
    use super::{Estimate, Governor, Plan};

    /// The pace rule (`orpheus_power::pace::RUN_US`): a rest after this much
    /// work without one.
    pub const RUN_US: u32 = 8_000;
    /// Its rest (`orpheus_power::pace::REST_US`); a wait for the ring this
    /// long counts as one.
    pub const REST_US: u32 = 4_000;
    /// The window thread mode's share is measured over, µs.
    pub const WINDOW_US: u64 = 100_000;

    /// What the model measured.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Report {
        /// Simulated time, µs.
        pub us: u64,
        /// Of it, thread mode's (rests and waits for the ring).
        pub thread_us: u64,
        /// The smallest share of thread mode in a [`WINDOW_US`] window, %.
        pub min_thread_pct: u32,
        /// The longest piece, µs.
        pub max_piece_us: u32,
        /// The longest stretch without a rest or a wait as long, µs.
        pub max_busy_us: u32,
        /// Frames the DAC played that the writer had not written.
        pub underrun_frames: u64,
        /// The last piece whose true cost was over the budget (0: none).
        pub last_over: u32,
        /// Pieces rendered.
        pub pieces: u32,
    }

    /// Thread mode's time.
    struct Clock {
        t: u64,
        win_start: u64,
        win_thread: u64,
    }

    impl Clock {
        fn advance(&mut self, r: &mut Report, dt: u64, thread: bool) {
            self.t += dt;
            if thread {
                r.thread_us += dt;
                self.win_thread += dt;
            }
            if self.t - self.win_start >= WINDOW_US {
                let pct = (self.win_thread * 100 / (self.t - self.win_start)) as u32;
                r.min_thread_pct = r.min_thread_pct.min(pct);
                self.win_start = self.t;
                self.win_thread = 0;
            }
        }
    }

    /// The firmware's render loop for `frames` frames at `rate` Hz into a
    /// PCM ring of `ring` frames (the DAC starts when it is full):
    /// `estimate()` is the engine's estimate now, `render(plan)` renders
    /// the piece the plan asks for and returns its true cost in cycles per
    /// frame. Before a piece that would end more than [`RUN_US`] after the
    /// last rest (or a wait for the ring as long as one), the loop rests
    /// [`REST_US`]; a full ring waits.
    pub fn run(
        gov: &mut Governor,
        rate: u32,
        ring: u32,
        frames: u64,
        mut estimate: impl FnMut() -> Estimate,
        mut render: impl FnMut(&Plan) -> u32,
    ) -> Report {
        let mhz = u64::from(gov.mhz);
        let rate = u64::from(rate.max(1));
        let mut r = Report { min_thread_pct: 100, ..Report::default() };
        let mut c = Clock { t: 0, win_start: 0, win_thread: 0 };
        let (mut written, mut busy, mut start) = (0u64, 0u64, None::<u64>);
        let played = |t: u64, start: Option<u64>| start.map_or(0, |s| (t - s) * rate / 1_000_000);
        while written < frames {
            let room = played(c.t, start) + u64::from(ring);
            if written >= room {
                let dt = (written - room + 1) * 1_000_000 / rate + 1;
                c.advance(&mut r, dt, true);
                if dt >= u64::from(REST_US) {
                    busy = 0;
                }
            }
            let e = estimate();
            let plan = gov.plan(&e);
            // The pace rule, before a piece that would end past the run.
            if busy + u64::from(plan.us) > u64::from(RUN_US) {
                c.advance(&mut r, u64::from(REST_US), true);
                busy = 0;
            }
            let cost = render(&plan);
            let cycles = u64::from(cost) * u64::from(plan.frames);
            let dt = cycles.div_ceil(mhz);
            gov.measured(&e, plan.frames, cycles.min(u64::from(u32::MAX)) as u32);
            r.pieces += 1;
            r.max_piece_us = r.max_piece_us.max(dt as u32);
            if u64::from(cost) * 100 > u64::from(gov.rt()) * u64::from(super::BUDGET_PCT) {
                r.last_over = r.pieces;
            }
            c.advance(&mut r, dt, false);
            busy += dt;
            r.max_busy_us = r.max_busy_us.max(busy as u32);
            // The DAC passed the writer: those frames were missed, and the
            // writer goes on from where the DAC is.
            let now = played(c.t, start);
            if start.is_some() && now > written {
                r.underrun_frames += now - written;
                written = now;
            }
            written += u64::from(plan.frames);
            if start.is_none() && written >= u64::from(ring) {
                start = Some(c.t);
            }
        }
        r.us = c.t;
        r
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec;
    use std::vec::Vec;

    const HZ: u32 = 144_000_000;
    const RATE: u32 = 44_100;

    fn est(voices: &[u32], running: u8) -> Estimate {
        Estimate {
            base: PLAYER.empty,
            voices: voices.iter().sum(),
            n: voices.len() as u32,
            fx: PLAYER.fx,
            running,
            wanted: running,
            ..Estimate::default()
        }
    }

    #[test]
    fn a_light_song_sheds_nothing() {
        let mut g = Governor::new(16, HZ, RATE);
        let e = est(&[280, 280, 296], FX_REVERB);
        for _ in 0..10_000 {
            let p = g.plan(&e);
            assert_eq!((p.fx, p.limit, p.keep), (FX_ALL, 16, None));
            g.measured(&e, p.frames, e.total() * p.frames);
        }
        assert_eq!((g.level(), g.over, g.shed), (Level::Full, 0, 0));
    }

    /// Over budget: the chorus goes first, then the reverb, then voices;
    /// the voices that stay fit the budget.
    #[test]
    fn sheds_in_order() {
        let mut g = Governor::new(16, HZ, RATE);
        // 3 voices + chorus + reverb: 245 + 840 + 865 = 1,950 > 1,795.
        let p = g.plan(&est(&[280; 3], FX_CHORUS | FX_REVERB));
        assert_eq!((p.fx, p.keep), (FX_ALL & !FX_CHORUS, None));
        assert_eq!(g.level(), Level::Chorus);
        // 5 voices + reverb: 245 + 1,400 + 671 > budget: the reverb goes.
        let mut g = Governor::new(16, HZ, RATE);
        let p = g.plan(&est(&[280; 5], FX_CHORUS | FX_REVERB));
        assert_eq!((p.fx, p.keep), (FX_DELAY, None));
        assert_eq!(g.level(), Level::Reverb);
        // 10 voices: down to (1,795 − 245) / 280 = 5.
        let mut g = Governor::new(16, HZ, RATE);
        let p = g.plan(&est(&[280; 10], FX_CHORUS | FX_REVERB));
        assert_eq!((p.fx, p.limit, p.keep), (FX_DELAY, 5, Some(5)));
        assert_eq!((g.level(), g.shed), (Level::Voices, 5));
    }

    /// Calm again: steps come back in reverse order, one per calm stretch.
    #[test]
    fn restores_in_reverse_order() {
        let mut g = Governor::new(16, HZ, RATE);
        g.plan(&est(&[280; 10], FX_CHORUS | FX_REVERB));
        assert_eq!(g.level(), Level::Voices);
        // One quiet voice; both effects wanted.
        let quiet = Estimate { wanted: FX_CHORUS | FX_REVERB, ..est(&[159], 0) };
        let mut steps = Vec::new();
        for _ in 0..20 * CALM_FX_MS * RATE / 1000 / BLOCK {
            g.plan(&quiet);
            if steps.last() != Some(&(g.level(), g.limit())) {
                steps.push((g.level(), g.limit()));
            }
        }
        assert_eq!(
            steps,
            [
                (Level::Voices, 5),
                (Level::Voices, 6),
                (Level::Voices, 7),
                (Level::Voices, 8),
                (Level::Voices, 9),
                (Level::Voices, 10),
                (Level::Voices, 11),
                (Level::Voices, 12),
                (Level::Voices, 13),
                (Level::Voices, 14),
                (Level::Voices, 15),
                (Level::Reverb, 16),
                (Level::Chorus, 16),
                (Level::Full, 16)
            ]
        );
    }

    #[test]
    fn pieces_are_bounded() {
        let g = Governor::new(16, HZ, RATE);
        assert_eq!(g.frames(245), BLOCK);
        // 576,000 cycles / 4,800 = 120 frames.
        assert_eq!(g.frames(4_800), 120);
        assert_eq!(g.frames(u32::MAX), MIN_PIECE);
        assert_eq!(g.rt(), 3_265);
    }

    /// The measured ratio follows the real cost and stays in 0.5–4.
    #[test]
    fn ratio_follows_the_measurement() {
        let mut g = Governor::new(16, HZ, RATE);
        let e = est(&[280; 2], 0);
        for _ in 0..200 {
            g.measured(&e, 256, e.total() * 256 * 3 / 2);
        }
        assert!((148..=150).contains(&g.ratio_pct()), "{}", g.ratio_pct());
        for _ in 0..200 {
            g.measured(&e, 256, u32::MAX);
        }
        assert!(g.ratio_pct() <= 400);
        g.measured(&e, 8, 0);
        assert!(g.ratio_pct() >= 395, "short pieces are not measured");
    }

    #[test]
    fn zeroed_and_edge_cases() {
        let z = <Governor as bytemuck::Zeroable>::zeroed();
        assert_eq!((z.limit(), z.level()), (0, Level::Full));
        let g = Governor::new(0, HZ, 0);
        assert_eq!(g.limit(), MIN_VOICES);
        assert_eq!(Estimate::default().per_voice(), PLAYER.sub);
        assert_eq!(PLAYER.voice(Class::Fm(4)), 454);
        assert_eq!(PLAYER.voice(Class::Drum(200)), PLAYER.sub);
    }

    /// A song in the model: it asks for `want` voices of `per` estimated
    /// cycles, sends to the chorus and the reverb, and really costs
    /// `true_x10` / 10 times the estimate. The governor's plan applies as
    /// the firmware applies it (effects off, voices faded to `keep`, new
    /// notes up to the limit); a piece costs what sounded at its start.
    fn song(g: &mut Governor, want: u32, per: u32, true_x10: u32, seconds: u64) -> sim::Report {
        let fed = FX_CHORUS | FX_REVERB;
        let state = core::cell::Cell::new((FX_ALL, want));
        let e_of = |(fx, n): (u8, u32)| Estimate { wanted: fed, ..est(&vec![per; n as usize], fed & fx) };
        sim::run(
            g,
            RATE,
            3072,
            u64::from(RATE) * seconds,
            || e_of(state.get()),
            |p| {
                let before = state.get();
                let n = p.keep.map_or(want, u32::from).min(u32::from(p.limit)).min(want);
                state.set((p.fx, n));
                e_of(before).total() * true_x10 / 10
            },
        )
    }

    /// Regression (2026-11-26, the player, build `1fda608d`): the three
    /// demo songs (`playdir /.orpheus/test/suite/midi`) underran and the
    /// watchdog reset the player, its last sample in `Chorus::process`. A
    /// 10-voice song with chorus and reverb costs ≈ 4,800 cycles a frame
    /// against 3,265 of real time at 144 MHz; the render ran on the
    /// real-time executor in 256-frame blocks with no rest, so thread mode
    /// (UI, USB, the watchdog's feeder) never ran, and the old governor
    /// only lowered the voice limit (to 4 at the least), which cannot
    /// recover the effects' fixed cost. Model: 10 voices of 390 estimated
    /// cycles with chorus and reverb (5,010 a frame), rendering 1.3 × dearer
    /// than the table says (6,513). Within 12 pieces (≈ 40 ms) the render fits the
    /// budget and stays there; thread mode keeps at least 30 % of every
    /// 100 ms; no piece is longer than 4 ms and no stretch without thread
    /// mode longer than the pace rule's 8 ms run (× the misestimate); the
    /// ring never runs dry.
    #[test]
    fn regress_synth_render_starved_thread_mode() {
        let mut g = Governor::new(16, HZ, RATE);
        let r = song(&mut g, 10, 390, 13, 30);
        assert!(r.last_over <= 12, "sustainable after piece {}", r.last_over);
        assert!(r.min_thread_pct >= 30, "thread mode {} %", r.min_thread_pct);
        assert!(r.max_piece_us <= PIECE_US * 13 / 10, "piece {} µs", r.max_piece_us);
        assert!(r.max_busy_us <= sim::RUN_US * 13 / 10, "busy {} µs", r.max_busy_us);
        assert_eq!(r.underrun_frames, 0);
        // The effects went first, then voices: (1,795 − 318) / 507 = 2.
        assert_eq!((g.level(), g.fx(), g.limit()), (Level::Voices, FX_DELAY, 2));
        assert!(r.thread_us * 100 / r.us >= 50, "thread mode {} % overall", r.thread_us * 100 / r.us);
    }

    /// No state of the song fits (every voice 4 × dearer than estimated,
    /// down to the floor): the render cannot keep up and the DAC underruns,
    /// but thread mode still gets a third of the CPU and no piece or busy
    /// stretch grows — the player degrades, it does not hang.
    #[test]
    fn overload_that_cannot_be_fixed_still_never_starves_thread_mode() {
        let mut g = Governor::new(16, HZ, RATE);
        let r = song(&mut g, 16, 737, 40, 10);
        assert!(r.underrun_frames > 0);
        assert!(r.min_thread_pct >= 30, "thread mode {} %", r.min_thread_pct);
        assert!(r.max_piece_us <= PIECE_US * 4, "piece {} µs", r.max_piece_us);
        assert!(r.max_busy_us <= sim::RUN_US + PIECE_US * 4, "busy {} µs", r.max_busy_us);
        assert!(r.thread_us * 100 / r.us >= 32, "thread mode {} % overall", r.thread_us * 100 / r.us);
        assert_eq!(g.limit(), MIN_VOICES);
    }

    /// A song that fits renders at full quality with no rest and no
    /// underrun: the governor and the pace rule leave it alone.
    #[test]
    fn a_song_that_fits_is_left_alone() {
        let mut g = Governor::new(16, HZ, RATE);
        let r = song(&mut g, 2, 280, 10, 10);
        assert_eq!((r.underrun_frames, r.last_over, g.over), (0, 0, 0));
        assert_eq!(g.level(), Level::Full);
    }
}
