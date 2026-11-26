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
//! estimated cycles (interrupts and patches the table does not know), adds
//! the measured paging (the pager's line loads during the render, a cost
//! no voice count changes), and when the predicted cost is above
//! [`BUDGET_PCT`] of the frame's real time (or a smoothed cost is), it
//! first **raises the clock** (decision D-4, owner 2026-11-26: up to
//! 288 MHz when needed, low power preferred): to the lowest of
//! [`CLOCKS_MHZ`] (High 144, Turbo 192, Max 288) whose budget fits, the
//! firmware switching at once ([`Plan::clock`]). Only at the top clock
//! does it shed, **in this order**:
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
//! [`CALM_FX_MS`] (an effect) of pieces in which the smoothed predicted
//! cost (`status` `load=`) stays under [`RESTORE_PCT`] and the cost with
//! the next step undone fits [`BUDGET_PCT`], the last step is undone:
//! voices first, one at a time, then the reverb, the delay, the chorus;
//! at full quality, after [`CALM_CLOCK_MS`] in which the smoothed cost
//! stays under [`RESTORE_PCT`] of the next lower clock, the clock comes
//! down a step (D-4: quality back before the clock comes down). The calm
//! is net: a piece over takes back its own frames rather than starting
//! the stretch again (2026-11-26, the player: one note burst a second kept
//! "Hill Song" shed for its whole length at `load=30–43%`). Below the top
//! clock nothing stays shed: a file starts at High at full quality
//! ([`Governor::new`], whatever clock the last file left), and when the
//! clock was not at the top while something was shed (a pin that ended)
//! everything comes back at once. The decisions count the clock asked for
//! (or the lower one the CPU runs at, [`Governor::set_clock`]), not a
//! higher one the clock governor may take back. Rendering at half the
//! rate (22.05 kHz, which the
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
//! # The measurement
//!
//! The ratio is real **compute** over estimated cycles: the pager's line
//! loads are taken out (their time is measured, `pager::LOAD_US`) and kept
//! as their own figure; the estimate is the mean of the engine's before
//! and after the piece (notes that start inside a piece are not in the
//! estimate before it — the first piece of every song was measured against
//! the empty engine; the larger of the two made the ratio a little
//! optimistic, and a dense song a few % over its budget more often).
//! Nothing is measured for [`WARM_MS`] after an open or
//! a seek ([`Governor::restart`]: cold code, patch installs, the first
//! notes), nor a piece whose estimate is under twice the empty engine's.
//! The ratio falls by a quarter of the difference a piece, and rises by at
//! most [`RISE_PCT`] % a block's worth of frames: one outlier moves it a
//! little, a real misestimate still shows within a second (2026-11-26, the
//! player: the first piece's 10 × pinned the ratio at its 4.0 clamp at
//! once and every song was cut to two voices and stopped).
//!
//! # Giving up
//!
//! An underrun burst is better than ending the song. The governor gives
//! up ([`Governor::give_up`]) only when nothing can play: everything is
//! shed at the top clock (or the clock a power mode pins: Battery saver
//! runs the lowest clock the peripherals work at, and the governor sees
//! the CPU below the clock it asked for, [`Governor::pinned`]) and the measured
//! render (compute and paging) is still above [`HOPELESS_PCT`] of real time
//! for [`HOPELESS_MS`] of its own time (net: a piece under it takes back what one
//! over it added). Short of that the song plays, underrunning where it
//! must.
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
/// A shed step is undone when the smoothed cost stays under this (and
/// the cost with it undone fits [`BUDGET_PCT`]); the clock comes down
/// when the cost is under this share of the lower clock.
pub const RESTORE_PCT: u32 = 50;
/// Calm time (net) under [`RESTORE_PCT`] before a voice comes back, ms.
pub const CALM_MS: u32 = 1_000;
/// … before an effect comes back (an effect switching on and off is
/// heard; a note more is not).
pub const CALM_FX_MS: u32 = 3_000;
/// … at full quality, under [`RESTORE_PCT`] of the next lower clock,
/// before the clock comes down a step (low power whenever the music
/// allows; a switch masks the CPU for ≈ 250 µs).
pub const CALM_CLOCK_MS: u32 = 2_000;
/// The voice limit never goes below this.
pub const MIN_VOICES: u8 = 2;
/// The longest render piece, µs (half the pace rule's 8 ms run).
pub const PIECE_US: u32 = 4_000;
/// The fewest frames in a piece (one control period).
pub const MIN_PIECE: u32 = 32;
/// The most (the firmware's render block).
pub const BLOCK: u32 = 256;
/// The clocks the governor asks for, MHz, by step ([`Plan::clock`]): High,
/// Turbo, Max (the firmware's clock floors 2, 3 and 4).
pub const CLOCKS_MHZ: [u32; 3] = [144, 192, 288];
/// The highest step (Max, 288 MHz).
pub const TOP: u8 = 2;
/// After an open or a seek nothing is measured for this long, ms (cold
/// code paging, patch installs, the first notes).
pub const WARM_MS: u32 = 250;
/// The most the ratio rises in a block's worth of frames, % of 1.0 (from
/// 1 to its 4.0 ceiling takes ≥ 0.6 s of 256-frame pieces).
pub const RISE_PCT: u32 = 3;
/// The governor gives up only when, with everything shed at the top
/// clock (or the one a power mode pins), the measured render is above
/// this share of real time — at most half the music could be heard:
/// nothing really plays …
pub const HOPELESS_PCT: u32 = 200;
/// … for this long, ms (net, of the render's own time).
pub const HOPELESS_MS: u32 = 5_000;
/// The ratio's unit.
const ONE: u32 = 1024;
/// Its range.
const RATIO_MIN: u32 = ONE / 2;
const RATIO_MAX: u32 = 4 * ONE;
/// The piece-length ratio's ceiling.
const PACE_MAX: u32 = 16 * ONE;

/// The governor's state (see the module documentation).
#[derive(bytemuck::Zeroable, Clone, Copy, Debug, PartialEq, Eq)]
pub struct Governor {
    /// Measured compute / estimated cycles, × 1024 (smoothed).
    ratio: u32,
    /// Predicted cycles per frame, smoothed over about four pieces.
    load: u32,
    /// Real-time cycles per frame (clock / rate).
    rt: u32,
    /// Clock, MHz.
    mhz: u32,
    /// Frames per second.
    rate: u32,
    /// Paging during the render, cycles per frame (smoothed).
    page: u32,
    /// The last piece's real cycles (compute and paging) / estimated,
    /// × 1024, unsmoothed and measured from the first piece: the next
    /// piece's length only (bounded in time even while the ratio is not
    /// measured or still rising).
    pace: u32,
    /// The effects the governor has turned off.
    fx_off: u8,
    limit: u8,
    max: u8,
    /// The clock step asked for ([`CLOCKS_MHZ`]).
    clk: u8,
    /// The highest step it may ask for.
    top: u8,
    /// The CPU runs below the clock asked for (a power mode pins it): no
    /// raise helps; it sheds at the clock it has. Set by every
    /// [`Governor::set_clock`] (it was for the rest of the file).
    pinned: bool,
    /// Frames of calm, counted toward [`CALM_MS`] (and the clock's).
    calm: u32,
    /// Frames not to measure yet ([`WARM_MS`]).
    warm: u32,
    /// Frames over [`HOPELESS_PCT`] with everything shed (net).
    hopeless: u32,
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
    /// Their predicted time with paging, µs (the pace rule rests first
    /// when the piece would end past its run).
    pub us: u32,
    /// The clock step to run at ([`CLOCKS_MHZ`]): the firmware raises the
    /// clock to it at once and lets it come down to it.
    pub clock: u8,
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

/// The clock step of `mhz` (the highest at or below it; High below High).
#[must_use]
pub fn step_of(mhz: u32) -> u8 {
    CLOCKS_MHZ.iter().rposition(|&m| m <= mhz).unwrap_or(0) as u8
}

impl Governor {
    /// A governor for `max` voices (1–16) at `clock_hz` and `rate` Hz, for
    /// a file just opened (the first [`WARM_MS`] are not measured).
    #[must_use]
    pub fn new(max: u8, clock_hz: u32, rate: u32) -> Self {
        let max = max.clamp(MIN_VOICES, MAX_VOICES as u8);
        let mut g = Self {
            ratio: ONE,
            load: 0,
            rt: 1,
            mhz: 1,
            rate: 1,
            page: 0,
            pace: ONE,
            fx_off: 0,
            limit: max,
            max,
            clk: 0,
            top: TOP,
            pinned: false,
            calm: 0,
            warm: 0,
            hopeless: 0,
            over: 0,
            shed: 0,
            measures: 0,
        };
        // Every file starts at full quality at High (the floor the
        // firmware sets for the open), whatever clock the last file left:
        // the first pieces raise it as far as they need (2026-11-26, the
        // player: a song opened after "Etude" took Max from the clock it
        // found, sat shed there and showed `clk=144`). A clock below High
        // now is not a pin yet: `set_clock` tells before every plan.
        g.set_clock(clock_hz, rate);
        g.restart();
        g
    }

    /// After a seek (or the open): the next [`WARM_MS`] are not measured.
    pub fn restart(&mut self) {
        self.warm = WARM_MS * self.rate / 1000;
        self.hopeless = 0;
    }

    /// The highest clock step to ask for (`0`: High only, never raise).
    pub fn set_top(&mut self, top: u8) {
        self.top = top.min(TOP);
        self.clk = self.clk.min(self.top);
    }

    /// The clock the CPU runs at now (`clock_hz`) and the rate, before
    /// every plan. It need not be the one asked for: below it, a power
    /// mode pins the clock ([`Governor::pinned`], until the clock follows
    /// again); above it, the clock governor raised it for other work and
    /// may take it back any second, so the decisions count only the clock
    /// asked for (2026-11-26: a file opened at the 288 MHz the last one
    /// left was judged against it).
    pub fn set_clock(&mut self, clock_hz: u32, rate: u32) {
        self.rate = rate.max(1);
        self.mhz = (clock_hz / 1_000_000).max(1);
        let asked = CLOCKS_MHZ.get(usize::from(self.clk)).copied().unwrap_or(CLOCKS_MHZ[0]);
        self.pinned = self.mhz < asked;
        self.rt = (self.mhz.min(asked) * 1_000_000 / self.rate).max(1);
    }

    /// The clock did not follow the governor (see [`Governor::set_clock`]).
    #[must_use]
    pub fn pinned(&self) -> bool {
        self.pinned
    }

    /// No faster clock to ask for: at the top step, or pinned.
    fn at_top(&self) -> bool {
        self.pinned || self.clk >= self.top
    }

    /// Real-time cycles per frame.
    #[must_use]
    pub fn rt(&self) -> u32 {
        self.rt
    }

    /// The clock now, MHz (as last set or raised to).
    #[must_use]
    pub fn mhz(&self) -> u32 {
        self.mhz
    }

    /// The clock step asked for.
    #[must_use]
    pub fn clock(&self) -> u8 {
        self.clk
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

    /// Measured compute / estimated cycles × 100.
    #[must_use]
    pub fn ratio_pct(&self) -> u32 {
        self.ratio * 100 / ONE
    }

    /// The smoothed predicted cost, cycles per frame (paging included).
    #[must_use]
    pub fn load(&self) -> u32 {
        self.load
    }

    /// Paging during the render, cycles per frame (smoothed).
    #[must_use]
    pub fn paging(&self) -> u32 {
        self.page
    }

    /// Pieces measured.
    #[must_use]
    pub fn measures(&self) -> u32 {
        self.measures
    }

    /// Everything is shed at the top clock and the render has stayed above
    /// [`HOPELESS_PCT`] of real time for [`HOPELESS_MS`]: the song cannot
    /// be played on this clock.
    #[must_use]
    pub fn give_up(&self) -> bool {
        u64::from(self.hopeless) * 1000 >= u64::from(HOPELESS_MS) * u64::from(self.rate)
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
        Self::scale(c, self.ratio)
    }

    fn scale(c: u32, ratio: u32) -> u32 {
        (u64::from(c) * u64::from(ratio) / u64::from(ONE)).min(u64::from(u32::MAX)) as u32
    }

    fn pct(&self, p: u32) -> u32 {
        pct_of(self.rt, p)
    }

    /// Real-time cycles per frame at clock step `s`.
    fn rt_at(&self, s: u8) -> u32 {
        let mhz = CLOCKS_MHZ.get(usize::from(s)).copied().unwrap_or(CLOCKS_MHZ[0]);
        mhz * 1_000_000 / self.rate
    }

    /// Voices that fit `budget` predicted cycles beside the base, paging
    /// and the effects of `fx`, at `e`'s mean voice cost.
    fn affordable(&self, e: &Estimate, fx: u8, budget: u32) -> u8 {
        let fixed = self.predict(e.base + e.fx_of(fx & e.wanted)).saturating_add(self.page);
        let per = self.predict(e.per_voice()).max(1);
        let n = budget.saturating_sub(fixed) / per;
        // (`max` ≥ `MIN_VOICES` by construction; no `clamp`: its panic
        // would format the values.)
        n.min(u32::from(self.max)).max(u32::from(MIN_VOICES)) as u8
    }

    /// Before a piece: the clock, what to enable, the voice limit, voices
    /// to fade out and the piece's length, from the estimate `e` of the
    /// engine now.
    pub fn plan(&mut self, e: &Estimate) -> Plan {
        // What sounds now plays this piece (shed voices and effects fade
        // over it): its length comes from the whole cost. The decisions
        // leave out the voices fading out: in a dense song every note
        // beyond the limit steals one into a tail, and counting those
        // brought the limit down again and again (to the floor).
        let all = Self::scale(e.total(), self.ratio.max(self.pace));
        let frames = self.frames(all);
        let us = self.piece_us(frames, all.max(self.predict(e.total()).saturating_add(self.page)));
        let p = self.predict(e.lasting()).saturating_add(self.page);
        self.load = if self.load == 0 { p } else { self.load - self.load / 4 + p / 4 };
        let mut keep = None;
        if self.load > self.pct(BUDGET_PCT) || p > self.pct(HARD_PCT) {
            keep = self.over_budget(e, p);
        } else if self.level() != Level::Full {
            self.restore(e, frames);
        } else if self.clk > 0 {
            self.lower(frames);
        }
        Plan { fx: self.fx(), limit: self.limit, keep, frames, us, clock: self.clk }
    }

    /// Over the budget (predicted `p`): a faster clock, else shed; the
    /// voices to keep, if some must fade out. Out of line, as
    /// [`Governor::restore`] and [`Governor::lower`]: the steady piece
    /// runs none of them, and the code a piece runs on the player must fit
    /// its code cache with the render's (SYN-3, `hot.toml` `synth-inner`).
    #[inline(never)]
    fn over_budget(&mut self, e: &Estimate, mut p: u32) -> Option<u8> {
        let mut keep = None;
        self.over = self.over.saturating_add(1);
        self.calm = 0;
        // A faster clock first: the lowest step above that fits, else
        // the top. The firmware switches at once; the next
        // `set_clock` says what the CPU really runs at.
        if !self.at_top() {
            let mut s = self.clk + 1;
            while s < self.top && p > pct_of(self.rt_at(s), BUDGET_PCT) {
                s += 1;
            }
            self.clk = s;
            self.rt = self.rt.max(self.rt_at(s));
            self.mhz = self.mhz.max(CLOCKS_MHZ.get(usize::from(s)).copied().unwrap_or(1));
        }
        let budget = self.pct(BUDGET_PCT);
        // Shed (only at the top clock) in order until it fits.
        if self.at_top() && p > budget {
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
                        .predict(e.base + e.fx_of(e.running & self.fx()) + u32::from(lim) * e.per_voice())
                        .saturating_add(self.page);
                }
            }
        }
        // The smoothed cost starts again from what is left.
        self.load = p;
        keep
    }

    /// Undo the last step after a calm stretch; everything at once below
    /// the top clock (D-4: shed only at the top; the clock comes down only
    /// at full quality, so this is a pin that ended or a top raised).
    #[inline(never)]
    fn restore(&mut self, e: &Estimate, frames: u32) {
        if !self.at_top() {
            self.fx_off = 0;
            self.limit = self.max;
            self.calm = 0;
            return;
        }
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
        let fits = self.load <= target && self.load + extra <= self.pct(BUDGET_PCT);
        if !self.calm_for(fits, frames, ms) {
            return;
        }
        if undo == 0 {
            // As many voices as fit the restore share (one at the least):
            // a song that sounds fewer than the limit gets its full limit
            // back at once, and the clock can come down sooner.
            let fit = self.affordable(e, self.fx(), target);
            self.limit = self.limit.saturating_add(1).max(fit).min(self.max);
        } else {
            self.fx_off &= !undo;
        }
    }

    /// At full quality: the clock a step down after a calm stretch at
    /// which the cost fits the lower clock with room.
    #[inline(never)]
    fn lower(&mut self, frames: u32) {
        let s = self.clk - 1;
        if self.calm_for(self.load <= pct_of(self.rt_at(s), RESTORE_PCT), frames, CALM_CLOCK_MS) {
            self.clk = s;
        }
    }

    /// Count `frames` of calm (`under`: the smoothed cost fits the
    /// target) toward `ms`; true (and the count reset) when they add up.
    /// A piece over the target takes its frames back rather than starting
    /// the stretch again: a note burst in a song whose cost sits under the
    /// target most of the time no longer keeps every step shed
    /// (2026-11-26, the player: "Hill Song" at `load=30–43%` with the
    /// reverb and nine voices shed for its whole length).
    fn calm_for(&mut self, under: bool, frames: u32, ms: u32) -> bool {
        if !under {
            self.calm = self.calm.saturating_sub(frames);
            return false;
        }
        self.calm += frames;
        if u64::from(self.calm) * 1000 < u64::from(ms) * u64::from(self.rate) {
            return false;
        }
        self.calm = 0;
        true
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

    /// After a piece of `frames` estimated at `before` (all of it, fading
    /// voices too) whose engine was estimated at `after` total cycles per
    /// frame when it ended took `cycles`, of which `paging` waited for the
    /// pager's line loads (see the module documentation, "The
    /// measurement").
    pub fn measured(&mut self, before: &Estimate, after: u32, frames: u32, cycles: u32, paging: u32) {
        let est = before.total().midpoint(after).max(before.base).max(1);
        if frames > 0 {
            self.pace = (u64::from(cycles) * u64::from(ONE) / (u64::from(est) * u64::from(frames)))
                .clamp(u64::from(RATIO_MIN), u64::from(PACE_MAX)) as u32;
        }
        if self.warm > 0 {
            self.warm = self.warm.saturating_sub(frames);
            return;
        }
        // Hopeless: everything shed at the top clock and still over.
        let floor = self.at_top() && self.limit <= MIN_VOICES && self.fx() & before.wanted == 0;
        // (Counted in the piece's own time, in frames of real time: how
        // long the CPU spent on music it could not play.)
        let spent = (cycles / self.rt.max(1)).max(1);
        let real = u64::from(cycles) * 100;
        if floor && real > u64::from(self.rt) * u64::from(frames) * u64::from(HOPELESS_PCT) {
            self.hopeless = self.hopeless.saturating_add(spent);
        } else {
            self.hopeless = self.hopeless.saturating_sub(spent);
        }
        if frames < MIN_PIECE {
            return;
        }
        // Paging: smoothed over about eight pieces, rising by at most
        // 1/32 of real time a block.
        let pf = paging / frames;
        let cap = (self.rt / 32 * frames / BLOCK).max(1);
        self.page = if pf > self.page {
            self.page + ((pf - self.page) / 8).min(cap)
        } else {
            self.page - (self.page - pf) / 8
        };
        // Compute against the mean estimate; none for a near-empty
        // engine (nothing there to scale, and the most to misjudge).
        if est < 2 * before.base.max(1) {
            return;
        }
        let compute = u64::from(cycles.saturating_sub(paging));
        let r = (compute * u64::from(ONE) / (u64::from(est) * u64::from(frames)))
            .clamp(u64::from(RATIO_MIN), u64::from(RATIO_MAX)) as u32;
        self.ratio = if r > self.ratio {
            let cap = (ONE * RISE_PCT / 100 * frames / BLOCK).max(1);
            self.ratio + ((r - self.ratio) / 4).min(cap)
        } else {
            self.ratio - (self.ratio - r) / 4
        };
        self.measures = self.measures.saturating_add(1);
    }
}

fn pct_of(rt: u32, p: u32) -> u32 {
    (u64::from(rt) * u64::from(p) / 100) as u32
}

/// A model of the firmware's render loop for host tests: the governor, the
/// clock it asks for, the pieces, the pace rule's rests, the PCM ring and
/// thread mode, in simulated microseconds.
pub mod sim {
    use super::{CLOCKS_MHZ, Estimate, Governor, Level, Plan, TOP};
    use orpheus_power::pace::{self, MARGIN_US};

    /// The pace rule ([`orpheus_power::pace::RUN_US`]): a rest after this
    /// much work without one.
    pub const RUN_US: u32 = pace::RUN_US;
    /// Its rest ([`orpheus_power::pace::REST_US`]); a wait for the ring
    /// this long counts as one.
    pub const REST_US: u32 = pace::REST_US;
    /// The window thread mode's share is measured over, µs.
    pub const WINDOW_US: u64 = 100_000;

    /// The loop's setting.
    #[derive(Clone, Copy, Debug)]
    pub struct Setup {
        /// Frames per second.
        pub rate: u32,
        /// The PCM ring, frames (the DAC starts when it is full).
        pub ring: u32,
        /// Frames to render.
        pub frames: u64,
        /// The clock stays here whatever the governor asks (a power mode
        /// pins it), Hz; `None`: the clock follows [`Plan::clock`] at once.
        pub pinned_hz: Option<u32>,
        /// The clock the CPU runs at when the file opens (the last file
        /// left it, or the clock governor chose it for other work), Hz,
        /// and for how long, µs: the clock is at least this until then
        /// (the clock governor takes it back after its calm steps).
        /// `None`: the clock asked for from the start.
        pub left: Option<(u32, u64)>,
        /// The clock at the first plan, Hz (the clock governor lowered
        /// what was left to the open's High floor during the scan); a
        /// raise in that plan applies at once, as always.
        pub first_hz: Option<u32>,
    }

    impl Setup {
        /// `seconds` at `rate` into the firmware's ring (3,072 frames), the
        /// clock following the governor.
        #[must_use]
        pub fn new(rate: u32, seconds: u64) -> Self {
            Self {
                rate,
                ring: 3072,
                frames: u64::from(rate) * seconds,
                pinned_hz: None,
                left: None,
                first_hz: None,
            }
        }
    }

    /// What a piece really cost.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Cost {
        /// Compute, cycles per frame.
        pub per_frame: u32,
        /// Paging during the piece, cycles.
        pub paging: u32,
        /// Other work on the real-time executor before the piece (the
        /// outer loop's part, a card read for the parser), µs: no rest,
        /// not measured.
        pub stall_us: u32,
    }

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
        /// The highest measured ratio, %.
        pub max_ratio_pct: u32,
        /// The highest clock step asked for.
        pub max_clock: u8,
        /// Frames rendered at each clock step asked for.
        pub at_clock: [u64; 3],
        /// The governor gave up (the loop stopped there).
        pub stopped: bool,
        /// Frames written.
        pub written: u64,
        /// Rests that started with less audio in the ring than the rest,
        /// the piece after it and the margin ([`orpheus_power::pace::rest_first`]),
        /// and of them those that were not forced by its bound.
        pub short_rests: u32,
        pub short_rests_unforced: u32,
        /// Frames rendered with something shed while the CPU ran below the
        /// top clock and no power mode pinned it (D-4: shed only at the
        /// top).
        pub shed_below_top: u64,
        /// The clock step of the first piece.
        pub first_clock: u8,
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

    /// The firmware's render loop as `s` sets it: `estimate()` is the
    /// engine's estimate now, `render(plan)` renders the piece the plan
    /// asks for and returns its true cost. Before a piece that would end
    /// more than [`RUN_US`] after the last rest (or a wait for the ring as
    /// long as one), the loop rests [`REST_US`] if the ring covers it
    /// (`orpheus_power::pace::rest_first`, as the firmware); a full ring
    /// waits. The governor's clock applies before the piece (unless
    /// pinned; [`Setup::left`] for a clock the last file left).
    pub fn run(
        gov: &mut Governor,
        s: &Setup,
        mut estimate: impl FnMut() -> Estimate,
        mut render: impl FnMut(&Plan) -> Cost,
    ) -> Report {
        let rate = u64::from(s.rate.max(1));
        let mut r = Report { min_thread_pct: 100, ..Report::default() };
        let mut c = Clock { t: 0, win_start: 0, win_thread: 0 };
        let (mut written, mut busy, mut start, mut deferred) = (0u64, 0u64, None::<u64>, 0u32);
        let played = |t: u64, start: Option<u64>| start.map_or(0, |s| (t - s) * rate / 1_000_000);
        let hz_of = |step: u8| CLOCKS_MHZ[usize::from(step).min(2)] * 1_000_000;
        // The clock the CPU runs at for the step asked: at once (the
        // firmware raises the floor at once), at least what the last file
        // left until the clock governor takes it back, the first piece's
        // as given.
        let cpu = |step: u8, t: u64, first: bool| {
            let asked = hz_of(step);
            match (s.pinned_hz, s.left, s.first_hz) {
                (Some(hz), ..) => hz,
                (None, _, Some(hz)) if first => hz,
                (None, Some((hz, until)), _) if t < until => asked.max(hz),
                _ => asked,
            }
        };
        while written < s.frames {
            let room = played(c.t, start) + u64::from(s.ring);
            if written >= room {
                let dt = (written - room + 1) * 1_000_000 / rate + 1;
                c.advance(&mut r, dt, true);
                if dt >= u64::from(REST_US) {
                    (busy, deferred) = (0, 0);
                }
            }
            gov.set_clock(cpu(gov.clock(), c.t, r.pieces == 0), s.rate);
            let e = estimate();
            let plan = gov.plan(&e);
            // The firmware switches the clock at once (a change of step).
            if r.pieces == 0 {
                r.first_clock = plan.clock;
            }
            gov.set_clock(cpu(plan.clock, c.t, false), s.rate);
            r.max_clock = r.max_clock.max(plan.clock);
            if gov.level() != Level::Full && gov.mhz() < CLOCKS_MHZ[usize::from(TOP)] && s.pinned_hz.is_none()
            {
                r.shed_below_top += u64::from(plan.frames);
            }
            let cost = render(&plan);
            if cost.stall_us > 0 {
                c.advance(&mut r, u64::from(cost.stall_us), false);
                busy += u64::from(cost.stall_us);
            }
            // The pace rule, before a piece that would end past the run,
            // if the ring covers the rest (`pace::rest_first`).
            let head = |t: u64| match start {
                None => u32::MAX,
                Some(_) => (written.saturating_sub(played(t, start)) * 1_000_000 / rate) as u32,
            };
            let h = head(c.t);
            let len = (u64::from(plan.frames) * 1_000_000 / rate) as u32;
            let d0 = deferred;
            if pace::rest_first(busy.min(u64::from(u32::MAX)) as u32, plan.us, len, h, &mut deferred) {
                if h < REST_US + plan.us + MARGIN_US {
                    r.short_rests += 1;
                    if plan.us * 3 <= len * 2 && d0 + plan.us <= pace::DEFER_US {
                        r.short_rests_unforced += 1;
                    }
                }
                c.advance(&mut r, u64::from(REST_US), true);
                busy = 0;
            }
            let cycles = u64::from(cost.per_frame) * u64::from(plan.frames) + u64::from(cost.paging);
            let dt = cycles.div_ceil(u64::from(gov.mhz()));
            let after = estimate().total();
            gov.measured(&e, after, plan.frames, cycles.min(u64::from(u32::MAX)) as u32, cost.paging);
            r.pieces += 1;
            r.max_piece_us = r.max_piece_us.max(dt as u32);
            r.max_ratio_pct = r.max_ratio_pct.max(gov.ratio_pct());
            r.at_clock[usize::from(plan.clock).min(2)] += u64::from(plan.frames);
            if cycles * 100 > u64::from(gov.rt()) * u64::from(plan.frames) * u64::from(super::BUDGET_PCT) {
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
            if start.is_none() && written >= u64::from(s.ring) {
                start = Some(c.t);
            }
            if gov.give_up() {
                r.stopped = true;
                break;
            }
        }
        r.us = c.t;
        r.written = written;
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

    /// A governor past its warm-up.
    fn warm(g: &mut Governor) {
        let e = est(&[], 0);
        g.measured(&e, 0, WARM_MS * RATE / 1000, 0, 0);
    }

    #[test]
    fn a_light_song_sheds_nothing() {
        let mut g = Governor::new(16, HZ, RATE);
        let e = est(&[280, 280, 296], FX_REVERB);
        for _ in 0..10_000 {
            let p = g.plan(&e);
            assert_eq!((p.fx, p.limit, p.keep, p.clock), (FX_ALL, 16, None, 0));
            g.measured(&e, e.total(), p.frames, e.total() * p.frames, 0);
        }
        assert_eq!((g.level(), g.over, g.shed, g.clock()), (Level::Full, 0, 0, 0));
    }

    /// Over budget at High (clock pinned there): the chorus goes first,
    /// then the reverb, then voices; the voices that stay fit the budget.
    #[test]
    fn sheds_in_order() {
        let high = || {
            let mut g = Governor::new(16, HZ, RATE);
            g.set_top(0);
            g
        };
        let mut g = high();
        // 3 voices + chorus + reverb: 245 + 840 + 865 = 1,950 > 1,795.
        let p = g.plan(&est(&[280; 3], FX_CHORUS | FX_REVERB));
        assert_eq!((p.fx, p.keep, p.clock), (FX_ALL & !FX_CHORUS, None, 0));
        assert_eq!(g.level(), Level::Chorus);
        // 5 voices + reverb: 245 + 1,400 + 671 > budget: the reverb goes.
        let mut g = high();
        let p = g.plan(&est(&[280; 5], FX_CHORUS | FX_REVERB));
        assert_eq!((p.fx, p.keep), (FX_DELAY, None));
        assert_eq!(g.level(), Level::Reverb);
        // 10 voices: down to (1,795 − 245) / 280 = 5.
        let mut g = high();
        let p = g.plan(&est(&[280; 10], FX_CHORUS | FX_REVERB));
        assert_eq!((p.fx, p.limit, p.keep), (FX_DELAY, 5, Some(5)));
        assert_eq!((g.level(), g.shed), (Level::Voices, 5));
    }

    /// D-4 (owner, 2026-11-26): the clock goes up before anything is shed
    /// — to the lowest step that fits — and the governor sheds only at
    /// Max.
    #[test]
    fn raises_the_clock_before_shedding() {
        // 1,950 a frame: Turbo's budget (2,394) fits.
        let mut g = Governor::new(16, HZ, RATE);
        let p = g.plan(&est(&[280; 3], FX_CHORUS | FX_REVERB));
        assert_eq!((p.clock, p.fx, p.keep, g.level()), (1, FX_ALL, None, Level::Full));
        // 245 + 2,800 + 865 = 3,910: over Max's 3,591 — Max, then the
        // chorus and the reverb (3,045 fits); every voice stays.
        let mut g = Governor::new(16, HZ, RATE);
        let p = g.plan(&est(&[280; 10], FX_CHORUS | FX_REVERB));
        assert_eq!((p.clock, p.fx, p.keep, p.limit), (2, FX_DELAY, None, 16));
        // 16 voices: Max, then voices: (3,591 − 245) / 280 = 11.
        let mut g = Governor::new(16, HZ, RATE);
        let p = g.plan(&est(&[280; 16], FX_CHORUS | FX_REVERB));
        assert_eq!((p.clock, p.limit, p.keep), (2, 11, Some(11)));
        // Already at Max (a power mode pins it): nothing to raise; the
        // budget is Max's.
        let mut g = Governor::new(16, 288_000_000, RATE);
        let p = g.plan(&est(&[280; 10], 0));
        assert_eq!((p.clock, p.fx, p.limit), (2, FX_ALL, 16));
    }

    /// Calm again: steps come back in reverse order, one per calm stretch
    /// (at High: the clock pinned there).
    #[test]
    fn restores_in_reverse_order() {
        let mut g = Governor::new(16, HZ, RATE);
        g.set_top(0);
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
        // As many as fit half of real time at once ((1,632 − 245) / 159 =
        // 8), then one a calm second.
        let mut want = vec![(Level::Voices, 5)];
        want.extend((8..16).map(|n| (Level::Voices, n)));
        want.extend([(Level::Reverb, 16), (Level::Chorus, 16), (Level::Full, 16)]);
        assert_eq!(steps, want);
        // A song of 280-cycle voices: as many as fit half of High
        // ((1,632 − 245) / 280 = 4 … the limit 5 first goes up by one),
        // then one a calm second while each fits.
        let mut g = Governor::new(16, HZ, RATE);
        g.set_top(0);
        g.plan(&est(&[280; 10], FX_CHORUS | FX_REVERB));
        let few = Estimate { wanted: FX_CHORUS | FX_REVERB, ..est(&[280; 4], 0) };
        let mut limits = Vec::new();
        for _ in 0..20 * CALM_MS * RATE / 1000 / BLOCK {
            g.plan(&few);
            if limits.last() != Some(&g.limit()) {
                limits.push(g.limit());
            }
        }
        // (The song sounds 4 voices whatever the limit: 1,365 a frame is
        // under half of High and one more voice fits the budget, so the
        // limit comes back one a calm second — before 2026-11-26 the
        // restore asked for 1,645 under 1,632 and none came back.)
        assert_eq!(limits, (5..=16).collect::<Vec<u8>>(), "a voice back a calm second");
    }

    /// … and with the clock free: voices and effects come back at Max
    /// first, then the clock steps down, one step per calm stretch, to
    /// High.
    #[test]
    fn restores_then_lowers_the_clock() {
        let mut g = Governor::new(16, HZ, RATE);
        let p = g.plan(&est(&[280; 16], FX_CHORUS | FX_REVERB));
        assert_eq!((p.clock, g.level()), (2, Level::Voices));
        let quiet = Estimate { wanted: FX_CHORUS | FX_REVERB, ..est(&[159], 0) };
        let mut steps = Vec::new();
        for _ in 0..30 * CALM_FX_MS * RATE / 1000 / BLOCK {
            let p = g.plan(&quiet);
            g.set_clock(CLOCKS_MHZ[usize::from(p.clock)] * 1_000_000, RATE);
            let s = (g.level(), p.clock);
            if steps.last() != Some(&s) {
                steps.push(s);
            }
        }
        assert_eq!(
            steps,
            [
                (Level::Voices, 2),
                (Level::Reverb, 2),
                (Level::Chorus, 2),
                (Level::Full, 2),
                (Level::Full, 1),
                (Level::Full, 0)
            ]
        );
        // A cost between: 1,600 a frame is over half of High (1,632) — no;
        // 1,700 stays at Turbo (over 50 % of High, under Turbo's budget).
        let mut g = Governor::new(16, HZ, RATE);
        g.plan(&est(&[280; 3], FX_CHORUS | FX_REVERB));
        let mid = est(&[280; 5], 0);
        for _ in 0..10 * CALM_CLOCK_MS * RATE / 1000 / BLOCK {
            let p = g.plan(&mid);
            g.set_clock(CLOCKS_MHZ[usize::from(p.clock)] * 1_000_000, RATE);
        }
        assert_eq!((g.clock(), g.level()), (1, Level::Full));
    }

    #[test]
    fn pieces_are_bounded() {
        let g = Governor::new(16, HZ, RATE);
        assert_eq!(g.frames(245), BLOCK);
        // 576,000 cycles / 4,800 = 120 frames.
        assert_eq!(g.frames(4_800), 120);
        assert_eq!(g.frames(u32::MAX), MIN_PIECE);
        assert_eq!(g.rt(), 3_265);
        assert_eq!((step_of(24), step_of(144), step_of(200), step_of(288), step_of(400)), (0, 0, 1, 2, 2));
    }

    /// The measured ratio follows the real compute, rises slowly, falls
    /// fast, stays in 0.5–4, leaves paging out, and does not measure the
    /// warm-up, short pieces or a near-empty engine.
    #[test]
    fn ratio_follows_the_measurement() {
        let mut g = Governor::new(16, HZ, RATE);
        let e = est(&[280; 2], 0);
        // The warm-up is not measured.
        g.measured(&e, e.total(), 256, u32::MAX, 0);
        assert_eq!((g.ratio_pct(), g.measures()), (100, 0));
        warm(&mut g);
        // One piece 4 × dearer moves it by at most RISE_PCT.
        g.measured(&e, e.total(), 256, e.total() * 256 * 4, 0);
        assert_eq!(g.ratio_pct(), 100 + RISE_PCT - 1, "rounded down");
        for _ in 0..200 {
            g.measured(&e, e.total(), 256, e.total() * 256 * 3 / 2, 0);
        }
        assert!((148..=150).contains(&g.ratio_pct()), "{}", g.ratio_pct());
        for _ in 0..400 {
            g.measured(&e, e.total(), 256, u32::MAX, 0);
        }
        assert!(g.ratio_pct() <= 400);
        g.measured(&e, e.total(), 8, 0, 0);
        assert!(g.ratio_pct() >= 395, "short pieces are not measured");
        // Back to the table within ≈ 25 pieces.
        for _ in 0..25 {
            g.measured(&e, e.total(), 256, e.total() * 256, 0);
        }
        assert!(g.ratio_pct() <= 105, "{}", g.ratio_pct());
        // Paging is not compute: 2 × the cycles, half of them paging.
        for _ in 0..100 {
            g.measured(&e, e.total(), 256, e.total() * 512, e.total() * 256);
        }
        assert!((99..=101).contains(&g.ratio_pct()), "{}", g.ratio_pct());
        assert!((e.total() * 95 / 100..=e.total()).contains(&g.paging()), "{}", g.paging());
        // The mean of the estimates before and after: notes that started
        // halfway through the piece (half its frames with them) measure
        // 1.0, where the estimate before alone said 1.5.
        let mut g = Governor::new(16, HZ, RATE);
        warm(&mut g);
        let empty = est(&[], 0);
        let half = (empty.total() + e.total()) * 128;
        for _ in 0..100 {
            g.measured(&empty, e.total(), 256, half, 0);
        }
        assert!((99..=101).contains(&g.ratio_pct()), "{}", g.ratio_pct());
        // A near-empty engine is not measured.
        g.measured(&empty, empty.total(), 256, u32::MAX, 0);
        assert!((99..=101).contains(&g.ratio_pct()));
    }

    #[test]
    fn zeroed_and_edge_cases() {
        let z = <Governor as bytemuck::Zeroable>::zeroed();
        assert_eq!((z.limit(), z.level(), z.clock()), (0, Level::Full, 0));
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
    fn song(g: &mut Governor, s: &sim::Setup, want: u32, per: u32, true_x10: u32) -> sim::Report {
        let fed = FX_CHORUS | FX_REVERB;
        let state = core::cell::Cell::new((FX_ALL, want));
        let e_of = |(fx, n): (u8, u32)| Estimate { wanted: fed, ..est(&vec![per; n as usize], fed & fx) };
        sim::run(
            g,
            s,
            || e_of(state.get()),
            |p| {
                let before = state.get();
                let n = p.keep.map_or(want, u32::from).min(u32::from(p.limit)).min(want);
                state.set((p.fx, n));
                sim::Cost { per_frame: e_of(before).total() * true_x10 / 10, paging: 0, stall_us: 0 }
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
    /// than the table says (6,513), the clock held at High (a power mode
    /// that pins it) and free to rise. Within 12 pieces (≈ 40 ms) the
    /// render fits the budget and stays there; thread mode keeps at least
    /// 30 % of every 100 ms; no piece is longer than 4 ms and no stretch
    /// without thread mode longer than the pace rule's 8 ms run (× the
    /// misestimate); the ring never runs dry.
    #[test]
    fn regress_synth_render_starved_thread_mode() {
        for top in [0, TOP] {
            let mut g = Governor::new(16, HZ, RATE);
            g.set_top(top);
            let r = song(&mut g, &sim::Setup::new(RATE, 30), 10, 390, 13);
            assert!(r.last_over <= 80, "top {top}: sustainable after piece {}", r.last_over);
            assert!(r.min_thread_pct >= 30, "top {top}: thread mode {} %", r.min_thread_pct);
            assert!(r.max_piece_us <= PIECE_US * 13 / 10, "piece {} µs", r.max_piece_us);
            assert!(r.max_busy_us <= sim::RUN_US * 13 / 10, "busy {} µs", r.max_busy_us);
            assert_eq!((r.underrun_frames, r.stopped), (0, false));
            assert!(r.thread_us * 100 / r.us >= 40, "thread mode {} % overall", r.thread_us * 100 / r.us);
        }
        // At High the effects went first, then voices: (1,795 − 318) / 507
        // = 2. With the clock free: Max, the effects off and
        // (3,591 − 318) / 507 = 6 voices.
        let mut g = Governor::new(16, HZ, RATE);
        g.set_top(0);
        song(&mut g, &sim::Setup::new(RATE, 30), 10, 390, 13);
        assert_eq!((g.level(), g.fx(), g.limit()), (Level::Voices, FX_DELAY, 2));
        let mut g = Governor::new(16, HZ, RATE);
        song(&mut g, &sim::Setup::new(RATE, 30), 10, 390, 13);
        assert_eq!((g.level(), g.fx(), g.limit(), g.clock()), (Level::Voices, FX_DELAY, 6, TOP));
    }

    /// Regression (2026-11-26, the player, build `78b316b5`): every MIDI
    /// song stopped within 3–20 s ("synth too slow for this player");
    /// `status` showed `ratio=400 lvl=3/3 vlim=2`. The ratio's first
    /// measurement was taken as it was: the first piece is estimated with
    /// the empty engine (245 cycles a frame) while the song's first notes
    /// start inside it, and its code and patches page in cold, so it
    /// measured ≥ 10 × and pinned the ratio at its 4.0 clamp; the governor
    /// then cut every song to two voices, and eight underrun halves
    /// stopped it. Model: a song that fits at High (2 voices with the
    /// reverb, 1,476 cycles a frame) whose notes start inside the first
    /// piece, its first 50 ms 10 × dearer, its paging 20 µs a piece; then
    /// a 2 s burst at 3 × the table. The ratio never passes 1.1 (the burst
    /// apart), nothing is shed, the song is not stopped, the clock rises
    /// for the burst and comes back to High.
    #[test]
    fn regress_synth_ratio_pinned_by_cold_start() {
        let mut g = Governor::new(16, HZ, RATE);
        let started = core::cell::Cell::new(false);
        let t = core::cell::Cell::new(0u64);
        let song = |on: bool| if on { est(&[280; 2], FX_REVERB) } else { est(&[], 0) };
        let s = sim::Setup::new(RATE, 40);
        let r = sim::run(
            &mut g,
            &s,
            || song(started.get()),
            |p| {
                started.set(true);
                let f = t.get();
                t.set(f + u64::from(p.frames));
                let x = match f * 1000 / u64::from(RATE) {
                    0..50 => 100,
                    8_000..10_000 => 30,
                    _ => 10,
                };
                sim::Cost { per_frame: song(true).total() * x / 10, paging: 20 * 144, stall_us: 0 }
            },
        );
        assert!(!r.stopped);
        assert!(r.written >= s.frames, "played to its end");
        assert_eq!((g.level(), g.clock(), g.shed), (Level::Full, 0, 0));
        assert!((95..=105).contains(&g.ratio_pct()), "ratio {}", g.ratio_pct());
        // The burst raised the clock (Max: 3 × 1,476 is over Turbo's
        // budget) and shed; all of it came back, then the clock.
        assert_eq!(r.max_clock, TOP);
        assert!(r.at_clock[0] > s.frames / 2, "mostly at High: {:?}", r.at_clock);
        let mut g = Governor::new(16, HZ, RATE);
        started.set(false);
        t.set(0);
        let r = sim::run(
            &mut g,
            &sim::Setup::new(RATE, 7),
            || song(started.get()),
            |p| {
                started.set(true);
                let f = t.get();
                t.set(f + u64::from(p.frames));
                let x = if f * 1000 / u64::from(RATE) < 50 { 100 } else { 10 };
                sim::Cost { per_frame: song(true).total() * x / 10, paging: 20 * 144, stall_us: 0 }
            },
        );
        assert!(r.max_ratio_pct <= 110, "ratio {}", r.max_ratio_pct);
        assert_eq!((r.max_clock, r.stopped, g.level()), (0, false, Level::Full));
    }

    /// No state of the song fits (every voice 4 × dearer than estimated,
    /// down to the floor at Max: 105 % of real time): the render cannot
    /// keep up and the DAC underruns, but thread mode still gets a third
    /// of the CPU and no piece or busy stretch grows — the player degrades,
    /// it does not hang, and the song plays on (an underrun burst is
    /// better than ending it).
    #[test]
    fn overload_that_cannot_be_fixed_still_never_starves_thread_mode() {
        let mut g = Governor::new(16, HZ, RATE);
        let s = sim::Setup::new(RATE, 20);
        let r = song(&mut g, &s, 16, 737, 40);
        assert!(r.underrun_frames > 0);
        assert!(r.min_thread_pct >= 30, "thread mode {} %", r.min_thread_pct);
        assert!(r.max_piece_us <= PIECE_US * 4, "piece {} µs", r.max_piece_us);
        assert!(r.max_busy_us <= sim::RUN_US + PIECE_US * 4, "busy {} µs", r.max_busy_us);
        assert!(r.thread_us * 100 / r.us >= 32, "thread mode {} % overall", r.thread_us * 100 / r.us);
        assert_eq!((g.limit(), g.clock()), (MIN_VOICES, TOP));
        assert!(!r.stopped && r.written >= s.frames, "played on");
    }

    /// Nothing can play: everything shed at Max and the render still 2.6 ×
    /// real time (10 × the table). The governor gives up — after
    /// `HOPELESS_MS` of the render's time so, not before — and thread mode
    /// ran throughout.
    #[test]
    fn gives_up_only_when_nothing_can_play() {
        let mut g = Governor::new(16, HZ, RATE);
        let r = song(&mut g, &sim::Setup::new(RATE, 60), 16, 737, 100);
        assert!(r.stopped);
        let secs = r.us / 1_000_000;
        assert!((5..=12).contains(&secs), "gave up after {secs} s");
        // (A quarter at the least in a 100 ms window at 10 × the table.)
        assert!(r.min_thread_pct >= 25, "thread mode {} %", r.min_thread_pct);
        // (The first piece, before anything is measured, takes 10 × its
        // prediction; the next ones are cut to fit.)
        assert!(r.max_piece_us <= PIECE_US * 5, "piece {} µs", r.max_piece_us);
    }

    /// Battery saver (owner, 2026-11-26: the lowest clock the peripherals
    /// work at): the clock is pinned whatever the governor asks — here at
    /// 24 MHz (544 cycles a frame, a budget of 299: under the empty
    /// engine's 245 and one voice) and at 48. It notices the pin, sheds to
    /// its floor and renders what it can: the DAC underruns at 24 MHz,
    /// thread mode still runs, no piece grows, and the song is not ended
    /// (two voices are 148 % of real time: something still plays).
    #[test]
    fn battery_saver_degrades_and_never_stops() {
        for (mhz, under) in [(24, true), (48, true), (96, false)] {
            let mut g = Governor::new(16, HZ, RATE);
            let s = sim::Setup { pinned_hz: Some(mhz * 1_000_000), ..sim::Setup::new(RATE, 20) };
            let r = song(&mut g, &s, 6, 280, 10);
            assert_eq!(r.underrun_frames > 0, under, "{mhz} MHz: {r:?}");
            assert!(r.min_thread_pct >= 30, "{mhz} MHz: thread mode {} %", r.min_thread_pct);
            assert!(r.max_piece_us <= PIECE_US * 2, "{mhz} MHz: piece {} µs", r.max_piece_us);
            assert!(r.max_busy_us <= sim::RUN_US + PIECE_US * 2, "{mhz} MHz: busy {} µs", r.max_busy_us);
            assert!(g.pinned(), "{mhz} MHz");
            assert!(g.limit() <= 3 && g.fx() & FX_REVERB == 0, "{mhz} MHz");
            assert!(!r.stopped && r.written >= s.frames, "{mhz} MHz played on");
        }
    }

    /// Regression (2026-11-26, the player, build `4e1e9a49`, `playdir` of
    /// the demo songs in Automatic): "Hill Song", after "Etude", showed
    /// `lvl=3/3 vlim=7 clk=144/144` at `load=30–43%` for its whole
    /// length. A new file's governor took its clock step from the clock
    /// it found (Max, left by "Etude"), while `status` showed the High it
    /// resets to and the file never asked for less; and a clock below the
    /// step at any plan (the clock governor lowering what was left to the
    /// open's High floor during the scan) pinned it for the whole file —
    /// no raise again, shedding at whatever clock the CPU ran. Model: a
    /// heavy file ends at Max, shed; the next starts with the clock it
    /// left for 2 s, the first plan at High. A light file then starts at
    /// High at full quality and stays there; a heavy one (after a file
    /// that left Turbo or Max) raises to Max before shedding, never sheds
    /// below it, and is not pinned.
    #[test]
    fn regress_synth_shed_carried_into_the_next_file() {
        let mut g = Governor::new(16, HZ, RATE);
        song(&mut g, &sim::Setup::new(RATE, 10), 16, 400, 10);
        assert_eq!((g.level(), g.clock()), (Level::Voices, TOP), "the heavy file");
        let next = |left: u32| sim::Setup {
            left: Some((left, 2_000_000)),
            first_hz: Some(HZ),
            ..sim::Setup::new(RATE, 20)
        };
        let max = CLOCKS_MHZ[usize::from(TOP)] * 1_000_000;
        let mut g = Governor::new(16, max, RATE);
        assert_eq!((g.clock(), g.level(), g.limit()), (0, Level::Full, 16), "a new file");
        let r = song(&mut g, &next(max), 2, 280, 10);
        assert_eq!((r.first_clock, r.max_clock, r.shed_below_top, g.over), (0, 0, 0, 0), "light: {r:?}");
        assert_eq!(g.level(), Level::Full);
        for left in [CLOCKS_MHZ[1] * 1_000_000, max] {
            let mut g = Governor::new(16, left, RATE);
            let r = song(&mut g, &next(left), 16, 400, 10);
            assert_eq!(
                (r.shed_below_top, r.max_clock, g.pinned()),
                (0, TOP, false),
                "heavy after {left}: {r:?}"
            );
            assert_eq!(g.level(), Level::Voices);
        }
    }

    /// Regression (2026-11-26, the player, build `4e1e9a49`): at Max,
    /// "Hill Song" kept the reverb and voices shed for its whole length
    /// at `load=30–43%`. A step came back only after a whole calm second
    /// (three for an effect) in which every piece's own prediction with
    /// the step undone stayed under half of real time: one note burst
    /// started the stretch again. Now the smoothed cost counts (`status`
    /// `load=`): under half of real time, with the step fitting the
    /// budget, the stretch adds up, and a piece over takes back only its
    /// own frames. Model at Max: shed by a heavy start, then 8 voices
    /// (38 % of real time) with a burst of 14 (64 %) every fourth piece:
    /// full quality again within 12 s, at Max all the while.
    #[test]
    fn regress_synth_restore_held_off_by_note_bursts() {
        let max = CLOCKS_MHZ[usize::from(TOP)] * 1_000_000;
        let mut g = Governor::new(16, HZ, RATE);
        let p = g.plan(&est(&[400; 16], FX_REVERB));
        g.set_clock(max, RATE);
        assert_eq!((p.clock, g.level()), (TOP, Level::Voices));
        let (quiet, burst) = (est(&[280; 8], 0), est(&[280; 14], 0));
        let mut back = None;
        for i in 0..20 * RATE / BLOCK {
            g.plan(if i % 4 == 3 { &burst } else { &quiet });
            g.set_clock(max, RATE);
            if back.is_none() && g.level() == Level::Full {
                back = Some(i * BLOCK * 10 / RATE);
            }
        }
        assert!(back.is_some_and(|ds| ds <= 120), "full quality after {back:?} × 0.1 s");
        assert_eq!((g.level(), g.clock(), g.over), (Level::Full, TOP, 1));
    }

    /// Regression (2026-11-26, the player, build `4e1e9a49`): underruns
    /// labelled `ub=synth:rest:4ms` — the pace rule's rest started with
    /// less audio in the PCM ring than it lasts (after other work on the
    /// real-time executor had drained it: the outer loop's part, a card
    /// read). Model: a song at 45 % of High, the ring full, and once a
    /// second 50–62 ms of other work before a piece (the ring left with
    /// 8–20 ms). No rest starts with less in the ring than the rest, the
    /// piece after it and the margin (`orpheus_power::pace::rest_first`:
    /// the piece renders first), and nothing underruns; with the rule
    /// `pace::due` alone (a rest whatever the ring holds) rests start
    /// short.
    #[test]
    fn regress_synth_rest_with_a_short_ring() {
        let mut g = Governor::new(16, HZ, RATE);
        let e = est(&[280, 280], FX_REVERB);
        let (t, k) = (core::cell::Cell::new(0u64), core::cell::Cell::new(0u32));
        let s = sim::Setup::new(RATE, 30);
        let r = sim::run(
            &mut g,
            &s,
            || e,
            |p| {
                let f = t.get();
                t.set(f + u64::from(p.frames));
                let stall = if f / u64::from(RATE) != (f + u64::from(p.frames)) / u64::from(RATE) {
                    k.set(k.get() + 1);
                    50_000 + (k.get() % 7) * 2_000
                } else {
                    0
                };
                sim::Cost { per_frame: e.total(), paging: 0, stall_us: stall }
            },
        );
        assert!(k.get() >= 25, "stalls {}", k.get());
        assert_eq!((r.short_rests, r.underrun_frames, r.stopped), (0, 0, false), "{r:?}");
        assert_eq!(g.level(), Level::Full);
    }

    /// A song that fits renders at full quality with no rest and no
    /// underrun, at High: the governor, the clock and the pace rule leave
    /// it alone.
    #[test]
    fn a_song_that_fits_is_left_alone() {
        let mut g = Governor::new(16, HZ, RATE);
        let r = song(&mut g, &sim::Setup::new(RATE, 10), 2, 280, 10);
        assert_eq!((r.underrun_frames, r.last_over, g.over, r.max_clock), (0, 0, 0, 0));
        assert_eq!(g.level(), Level::Full);
    }
}
