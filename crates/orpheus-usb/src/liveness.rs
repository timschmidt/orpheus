//! USB liveness (USB-W1, 06-connectivity §2.6): from the controller
//! driver's counters, sampled about once a second, decide whether the
//! device still answers its host, when to re-initialise the controller,
//! and when to give up and let the watchdog reboot.
//!
//! Evidence of a host that is not being answered:
//!
//! * a bus reset after which no SETUP is read within [`RESET_ANSWER_MS`]
//!   (every enumeration, re-plug or host retry starts with a reset and is
//!   followed by a SETUP within milliseconds);
//! * a SETUP the controller latched that the driver has not read (or keeps
//!   failing to read) for [`SETUP_STUCK_MS`] — a lost waker, a masked
//!   interrupt or a stuck USB task.
//!
//! An unplugged player (no VBUS), a charger (no resets, no SETUPs) and an
//! idle host (SOFs only) give no evidence, so nothing ever happens there.
//!
//! Each finding asks for a re-initialisation (soft detach, controller
//! reset, re-enumeration). Consecutive ones without a SETUP answered in
//! between back off ([`BACKOFF_MS`]). The watchdog is only involved
//! ([`Verdict::Wedged`]) when re-initialising cannot help: the USB task does
//! not carry out a requested re-initialisation within [`TASK_STUCK_MS`], or
//! SETUPs stay unread after [`MAX_FAILED`] re-initialisations — and only
//! with VBUS present, after this boot answered at least one SETUP (a real
//! host, working hardware) and after [`MIN_UPTIME_MS`] (past the healthy-
//! start mark, so the reboot never counts as a failed start for stage 2).
//! Unanswered bus resets alone never reboot: a host port can leave the
//! lines in states that look like resets.

use orpheus_proto::devchan::usb as codes;

/// A bus reset must be followed by a SETUP within this time.
pub const RESET_ANSWER_MS: u32 = 2_000;
/// A latched SETUP may wait unread this long (the host's own timeout is
/// 2 s, so the transfer is already lost by then).
pub const SETUP_STUCK_MS: u32 = 2_500;
/// A requested re-initialisation must be carried out within this time.
pub const TASK_STUCK_MS: u32 = 10_000;
/// Re-initialisations without a SETUP answered in between before SETUPs
/// that stay unread count as a wedged controller.
pub const MAX_FAILED: u8 = 2;
/// No watchdog escalation before this uptime.
pub const MIN_UPTIME_MS: u32 = 60_000;
/// Wait after the n-th consecutive re-initialisation before the next one.
pub const BACKOFF_MS: [u32; 5] = [0, 5_000, 15_000, 30_000, 60_000];

/// One reading of the driver's counters.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Sample {
    /// Uptime (ms).
    pub now_ms: u32,
    pub vbus: bool,
    /// SETUP packets read (counter).
    pub setups: u32,
    /// Bus resets (counter).
    pub resets: u32,
    /// The controller holds a SETUP the driver has not read yet, or the
    /// last attempt to read one failed.
    pub setup_waiting: bool,
    /// Controller initialisations (counter): the USB task's progress after
    /// a re-initialisation was asked for.
    pub inits: u32,
}

/// Why a re-initialisation is asked for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Reason {
    /// A bus reset was not followed by a SETUP.
    ResetUnanswered,
    /// A SETUP stayed unread.
    SetupStuck,
}

impl Reason {
    /// Error code (`orpheus_proto::devchan::usb::ERR_*`).
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Reason::ResetUnanswered => codes::ERR_RECOVER_RESET,
            Reason::SetupStuck => codes::ERR_RECOVER_SETUP,
        }
    }
}

/// Why the controller counts as wedged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Wedge {
    /// A requested re-initialisation was never carried out.
    TaskStuck,
    /// SETUPs stay unread after [`MAX_FAILED`] re-initialisations.
    SetupStuck,
}

impl Wedge {
    #[must_use]
    pub fn code(self) -> u8 {
        match self {
            Wedge::TaskStuck => codes::ERR_WEDGED_TASK,
            Wedge::SetupStuck => codes::ERR_WEDGED_SETUP,
        }
    }
}

/// What to do after a sample.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Verdict {
    Ok,
    /// Re-initialise the controller now.
    Reinit(Reason),
    /// Re-initialising does not help: stop feeding the watchdog.
    Wedged(Wedge),
}

/// The detector; one per firmware, in the watchdog task's future.
#[derive(Clone, Debug, Default)]
pub struct Liveness {
    last: Option<Sample>,
    /// An unanswered bus reset (time it was first seen).
    reset_at: Option<u32>,
    /// A SETUP waiting since (first sample that saw it).
    waiting_since: Option<u32>,
    /// A requested re-initialisation: (asked at, `inits` then).
    pending: Option<(u32, u32)>,
    /// Consecutive re-initialisations without a SETUP answered.
    failed: u8,
    /// No re-initialisation before this time (backoff).
    next_ms: u32,
    /// This boot answered a SETUP.
    ever_served: bool,
    wedged: Option<Wedge>,
}

impl Liveness {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            last: None,
            reset_at: None,
            waiting_since: None,
            pending: None,
            failed: 0,
            next_ms: 0,
            ever_served: false,
            wedged: None,
        }
    }

    /// Consecutive re-initialisations without a SETUP answered.
    #[must_use]
    pub fn failed(&self) -> u8 {
        self.failed
    }

    /// The controller counts as wedged (the watchdog is not fed).
    #[must_use]
    pub fn wedged(&self) -> bool {
        self.wedged.is_some()
    }

    /// Feed one sample (about once a second).
    pub fn step(&mut self, s: Sample) -> Verdict {
        let Some(prev) = self.last.replace(s) else {
            self.ever_served = s.setups != 0;
            return Verdict::Ok;
        };
        let served = s.setups != prev.setups;
        if served {
            self.ever_served = true;
            self.failed = 0;
            self.next_ms = 0;
            self.reset_at = None;
            self.waiting_since = None;
            self.wedged = None;
        }
        if !s.vbus {
            // Unplugged (or a charger that dropped out): nothing to answer.
            // The driver re-initialises the controller itself when VBUS
            // comes back.
            self.reset_at = None;
            self.waiting_since = None;
            self.failed = 0;
            self.next_ms = 0;
            self.wedged = None;
            if let Some((_, inits)) = self.pending
                && s.inits != inits
            {
                self.pending = None;
            }
            return Verdict::Ok;
        }
        if let Some(w) = self.wedged {
            return Verdict::Wedged(w);
        }
        if let Some((at, inits)) = self.pending {
            if s.inits == inits {
                if s.now_ms.wrapping_sub(at) >= TASK_STUCK_MS && self.may_escalate(s) {
                    self.wedged = Some(Wedge::TaskStuck);
                    return Verdict::Wedged(Wedge::TaskStuck);
                }
                return Verdict::Ok;
            }
            // A fresh controller: evidence starts again.
            self.pending = None;
            self.reset_at = None;
            self.waiting_since = None;
            return Verdict::Ok;
        }
        if s.resets != prev.resets && !served && self.reset_at.is_none() {
            self.reset_at = Some(s.now_ms);
        }
        if s.setup_waiting && !served {
            if self.waiting_since.is_none() {
                self.waiting_since = Some(s.now_ms);
            }
        } else {
            self.waiting_since = None;
        }
        let since = |t: Option<u32>, limit: u32| t.is_some_and(|t| s.now_ms.wrapping_sub(t) >= limit);
        let reason = if since(self.waiting_since, SETUP_STUCK_MS) {
            Reason::SetupStuck
        } else if since(self.reset_at, RESET_ANSWER_MS) {
            Reason::ResetUnanswered
        } else {
            return Verdict::Ok;
        };
        if reason == Reason::SetupStuck && self.failed >= MAX_FAILED && self.may_escalate(s) {
            self.wedged = Some(Wedge::SetupStuck);
            return Verdict::Wedged(Wedge::SetupStuck);
        }
        if s.now_ms < self.next_ms {
            return Verdict::Ok;
        }
        self.failed = self.failed.saturating_add(1);
        let i = usize::from(self.failed - 1).min(BACKOFF_MS.len() - 1);
        self.next_ms = s.now_ms.saturating_add(BACKOFF_MS[i]);
        self.pending = Some((s.now_ms, s.inits));
        self.reset_at = None;
        self.waiting_since = None;
        Verdict::Reinit(reason)
    }

    fn may_escalate(&self, s: Sample) -> bool {
        s.vbus && self.ever_served && s.now_ms >= MIN_UPTIME_MS
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::vec::Vec;

    /// A scripted controller: counters as the HAL keeps them.
    struct Rig {
        l: Liveness,
        s: Sample,
    }

    impl Rig {
        fn new(t0: u32) -> Self {
            let s = Sample { now_ms: t0, vbus: true, setups: 10, ..Sample::default() };
            let mut l = Liveness::new();
            assert_eq!(l.step(s), Verdict::Ok);
            Self { l, s }
        }
        /// One second passes.
        fn tick(&mut self) -> Verdict {
            self.s.now_ms += 1000;
            self.l.step(self.s)
        }
        fn ticks(&mut self, n: usize) -> Vec<Verdict> {
            (0..n).map(|_| self.tick()).collect()
        }
        /// The USB task re-initialised the controller.
        fn reinit_done(&mut self) {
            self.s.inits += 1;
            self.s.setup_waiting = false;
        }
    }

    fn reinits(v: &[Verdict]) -> usize {
        v.iter().filter(|v| matches!(v, Verdict::Reinit(_))).count()
    }

    #[test]
    fn idle_host_unplugged_and_charger_never_act() {
        let mut r = Rig::new(100_000);
        // Idle host: nothing changes for ten minutes.
        assert!(r.ticks(600).iter().all(|v| *v == Verdict::Ok));
        // Unplugged with a SETUP flag left over: still nothing.
        r.s.vbus = false;
        r.s.setup_waiting = true;
        assert!(r.ticks(600).iter().all(|v| *v == Verdict::Ok));
        // A charger: VBUS, no resets, no SETUPs.
        r.s.vbus = true;
        r.s.setup_waiting = false;
        assert!(r.ticks(600).iter().all(|v| *v == Verdict::Ok));
    }

    #[test]
    fn busy_host_is_fine() {
        let mut r = Rig::new(100_000);
        for i in 0..600 {
            r.s.setups += 3;
            // A SETUP happens to be in flight at some samples.
            r.s.setup_waiting = i % 2 == 0;
            if i % 100 == 0 {
                r.s.resets += 1; // re-enumeration (personality switch)
            }
            assert_eq!(r.tick(), Verdict::Ok);
        }
    }

    #[test]
    fn enumeration_after_a_reset_is_fine() {
        let mut r = Rig::new(100_000);
        r.s.resets += 1;
        assert_eq!(r.tick(), Verdict::Ok);
        r.s.setups += 5;
        assert!(r.ticks(30).iter().all(|v| *v == Verdict::Ok));
    }

    #[test]
    fn unanswered_reset_reinitialises_then_backs_off_without_reboot() {
        let mut r = Rig::new(100_000);
        r.s.resets += 1;
        let v = r.ticks(3);
        assert_eq!(v[2], Verdict::Reinit(Reason::ResetUnanswered), "{v:?}");
        // The re-initialisation happens; the host keeps resetting, nothing
        // is answered (a port that looks like resets): more re-inits,
        // spaced out, never a wedge.
        r.reinit_done();
        let mut all = Vec::new();
        for _ in 0..600 {
            r.s.resets += 1;
            let v = r.tick();
            if matches!(v, Verdict::Reinit(_)) {
                r.reinit_done();
            }
            all.push(v);
        }
        assert!(all.iter().all(|v| !matches!(v, Verdict::Wedged(_))));
        let n = reinits(&all);
        // 600 s at the 60 s steady backoff: about ten, not hundreds.
        assert!((8..=16).contains(&n), "{n} re-inits");
        assert!(r.l.failed() >= 5);
        // A SETUP answered: all forgiven.
        r.s.setups += 1;
        assert_eq!(r.tick(), Verdict::Ok);
        assert_eq!(r.l.failed(), 0);
    }

    #[test]
    fn stuck_setup_reinitialises_and_recovery_clears() {
        let mut r = Rig::new(100_000);
        r.s.setup_waiting = true;
        let v = r.ticks(4);
        // Seen at 1 s, stuck from 3.5 s on: the request on the fourth tick.
        assert_eq!(&v[..3], &[Verdict::Ok; 3]);
        assert_eq!(v[3], Verdict::Reinit(Reason::SetupStuck));
        // Until the task re-initialises, nothing more is asked.
        assert!(r.ticks(5).iter().all(|v| *v == Verdict::Ok));
        r.reinit_done();
        r.s.resets += 1; // the host enumerates the fresh controller
        r.s.setups += 4;
        assert!(r.ticks(60).iter().all(|v| *v == Verdict::Ok));
        assert_eq!(r.l.failed(), 0);
        assert!(!r.l.wedged());
    }

    #[test]
    fn setups_unread_after_two_reinits_is_a_wedge() {
        let mut r = Rig::new(100_000);
        let mut verdicts = Vec::new();
        for _ in 0..40 {
            r.s.setup_waiting = true;
            let v = r.tick();
            if matches!(v, Verdict::Reinit(_)) {
                r.reinit_done();
            }
            verdicts.push(v);
            if matches!(v, Verdict::Wedged(_)) {
                break;
            }
        }
        assert_eq!(reinits(&verdicts), 2, "{verdicts:?}");
        assert_eq!(verdicts.last(), Some(&Verdict::Wedged(Wedge::SetupStuck)));
        assert!(r.l.wedged());
        // Stays wedged while plugged in…
        r.s.setup_waiting = true;
        assert_eq!(r.tick(), Verdict::Wedged(Wedge::SetupStuck));
        // …and clears when unplugged (the player is not rebooted for USB
        // with no cable).
        r.s.vbus = false;
        assert_eq!(r.tick(), Verdict::Ok);
        assert!(!r.l.wedged());
    }

    #[test]
    fn a_task_that_never_reinitialises_is_a_wedge() {
        let mut r = Rig::new(100_000);
        r.s.setup_waiting = true;
        let v = r.ticks(4);
        assert_eq!(v[3], Verdict::Reinit(Reason::SetupStuck));
        // `inits` never moves.
        let v = r.ticks(12);
        assert_eq!(v.iter().filter(|v| **v == Verdict::Wedged(Wedge::TaskStuck)).count(), 3, "{v:?}");
        assert!(v[..9].iter().all(|v| *v == Verdict::Ok));
    }

    #[test]
    fn no_wedge_early_in_a_boot_or_before_any_setup() {
        // Early: re-inits, but no watchdog before MIN_UPTIME_MS.
        let mut r = Rig::new(1_000);
        let mut wedged_at = None;
        for _ in 0..120 {
            r.s.setup_waiting = true;
            let v = r.tick();
            if matches!(v, Verdict::Reinit(_)) {
                r.reinit_done();
            }
            if matches!(v, Verdict::Wedged(_)) {
                wedged_at = Some(r.s.now_ms);
                break;
            }
        }
        assert!(wedged_at.is_some_and(|t| t >= MIN_UPTIME_MS), "{wedged_at:?}");
        // A boot that never answered a SETUP: never a wedge.
        let mut l = Liveness::new();
        let mut s = Sample { now_ms: 100_000, vbus: true, ..Sample::default() };
        l.step(s);
        for _ in 0..600 {
            s.now_ms += 1000;
            s.setup_waiting = true;
            if matches!(l.step(s), Verdict::Reinit(_)) {
                s.inits += 1;
            }
        }
        assert!(!l.wedged());
    }

    /// Regression: USB-W1 (2026-11-24, first soak): with the controller
    /// deaf, nothing brought it back short of a restart; the incident's
    /// sequence now ends in a recovery and a working channel, no reboot.
    #[test]
    fn regress_usb_w1_liveness_recovers_the_incident() {
        // The soak's failure (STATUS 2026-11-24): working channel, then a
        // SETUP latched and never read (the read future was dropped with the
        // interrupt masked), then re-plugs: resets, nothing answered.
        let mut r = Rig::new(200_000);
        for _ in 0..30 {
            r.s.setups += 4;
            assert_eq!(r.tick(), Verdict::Ok);
        }
        r.s.setup_waiting = true;
        let v = r.ticks(4);
        assert_eq!(v.last(), Some(&Verdict::Reinit(Reason::SetupStuck)));
        // The fixed driver: the fresh controller is enumerated again.
        r.reinit_done();
        r.s.resets += 2;
        r.s.setups += 6;
        assert!(r.ticks(10).iter().all(|v| *v == Verdict::Ok));
    }

    #[test]
    fn codes() {
        assert_eq!(Reason::SetupStuck.code(), codes::ERR_RECOVER_SETUP);
        assert_eq!(Reason::ResetUnanswered.code(), codes::ERR_RECOVER_RESET);
        assert_eq!(Wedge::TaskStuck.code(), codes::ERR_WEDGED_TASK);
        assert_eq!(Wedge::SetupStuck.code(), codes::ERR_WEDGED_SETUP);
    }
}
