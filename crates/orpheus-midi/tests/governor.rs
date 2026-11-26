//! The demo songs through the load governor with the player's measured
//! costs (`orpheus_synth::load`, 2026-11-26): what each song needs on the
//! player, what the governor sheds, and that the render then fits. The
//! numbers these print are in 04 §9 ("Load governor").

mod common;

use common::demo;
use orpheus_midi::{MidiPlayer, Options, SliceSource};
use orpheus_synth::load::{BUDGET_PCT, Governor, Level, MIN_VOICES, PLAYER};

/// What a song did under the governor.
#[derive(Debug, Default)]
struct Run {
    frames: u64,
    /// Frames rendered while the true cost was over the budget.
    over: u64,
    /// The same after the first two seconds.
    over_late: u64,
    max_level: Option<Level>,
    /// Frames at the full level (the host's render bit for bit).
    full: u64,
    /// Frames with the reverb on.
    reverb: u64,
    /// The voice limit, mean over the frames × 10.
    limit_x10: u64,
    /// Voices the governor faded out.
    shed: usize,
    /// Cycles per frame, mean.
    mean_cost: u64,
    /// PCM halves (1,536 frames, 35 ms) past the first two seconds whose
    /// mean cost was above [`HALF_PCT`] of real time — where the ring can
    /// run dry — and all such halves.
    halves_late: u64,
    halves: u64,
}

/// A PCM half whose render takes more than this share of its real time
/// risks an underrun: the pace rule's rests leave the render two thirds,
/// and the DSP chain and the PCM ring take a few per cent.
const HALF_PCT: u64 = 62;
/// Frames of a PCM half.
const HALF: u64 = 1536;

/// Render `song` at `rate` as the firmware does: plan, apply, render the
/// piece, measure (the cost as estimated × `true_pct` / 100) at `hz`.
fn run(name: &str, song: fn(&mut [u8]) -> Option<usize>, hz: u32, rate: u32, true_pct: u32) -> Run {
    let data = demo(song);
    let mut p = Box::new(MidiPlayer::new(Options::new(rate)));
    p.open_from(&mut SliceSource::new(&data), name.ends_with(".kar")).unwrap();
    let mut src = SliceSource::new(&data);
    let mut g = Governor::new(16, hz, rate);
    let budget = u64::from(g.rt()) * u64::from(BUDGET_PCT) / 100;
    let mut out = vec![0i16; 512];
    let mut r = Run::default();
    let (mut cost, mut limit) = (0u64, 0u64);
    let (mut win_cyc, mut win_n) = (0u64, 0u64);
    loop {
        let e = p.engine().estimate(&PLAYER);
        let plan = g.plan(&e);
        p.set_fx_enabled(plan.fx);
        p.set_voice_limit(usize::from(plan.limit));
        if let Some(k) = plan.keep {
            r.shed += p.shed(usize::from(k));
        }
        let f = plan.frames as usize;
        let n = p.render_from(&mut src, &mut out[..2 * f]).unwrap();
        if n == 0 {
            break;
        }
        let n64 = n as u64;
        let true_cost = e.total() * true_pct / 100;
        g.measured(&e, n as u32, true_cost * n as u32);
        if u64::from(true_cost) > budget {
            r.over += n64;
            if r.frames >= 2 * u64::from(rate) {
                r.over_late += n64;
            }
        }
        if g.level() == Level::Full {
            r.full += n64;
        }
        if g.level() < Level::Reverb {
            r.reverb += n64;
        }
        r.max_level = r.max_level.max(Some(g.level()));
        cost += u64::from(true_cost) * n64;
        win_cyc += u64::from(true_cost) * n64;
        win_n += n64;
        if win_n >= HALF {
            r.halves += 1;
            if win_cyc * 100 > win_n * u64::from(g.rt()) * HALF_PCT && r.frames >= 2 * u64::from(rate) {
                r.halves_late += 1;
            }
            (win_cyc, win_n) = (0, 0);
        }
        limit += u64::from(g.limit()) * n64;
        r.frames += n64;
    }
    r.mean_cost = cost / r.frames.max(1);
    r.limit_x10 = limit * 10 / r.frames.max(1);
    r
}

fn pct(a: u64, b: u64) -> u64 {
    a * 100 / b.max(1)
}

/// At 144 MHz (the clock floor while a synth source plays) every demo song
/// needs more than the budget (1,795 cycles a frame) at some point — the
/// reverb alone is 671, a voice 160–740 — so the governor acts on each.
/// Afterwards the render stays within the budget but for note bursts
/// between two plans and the fade-out tails of stolen voices (under 3 %
/// of the frames past the first two seconds), under 1 % of the PCM halves
/// past them cost more than 62 % of their real time (where the ring could
/// run dry: one in "Hill Song"), and the songs play to their end. None
/// plays bit for bit with the host for long: `crc on` checks run with
/// `synth gov off`.
#[test]
fn demo_songs_fit_the_budget_at_144_mhz() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let r = run(name, song, 144_000_000, 44_100, 100);
        println!("{name} @144: {r:?}");
        assert!(r.frames > 44_100 * 20, "{name} played to its end");
        assert!(pct(r.over_late, r.frames) < 3, "{name}: {} of {} frames over", r.over_late, r.frames);
        assert!(pct(r.halves_late, r.halves) < 1, "{name}: {} halves at risk", r.halves_late);
        assert_eq!(r.max_level, Some(Level::Voices), "{name}");
        assert!(pct(r.full, r.frames) < 5, "{name}");
    }
}

/// At 288 MHz (Settings › System › Performance: Maximum) the budget
/// doubles: "Sound tour" plays as the host renders it for most of its
/// length, the others keep more voices.
#[test]
fn demo_songs_at_288_mhz() {
    let mut full = Vec::new();
    for (name, song) in orpheus_midi::demo::SONGS {
        let r = run(name, song, 288_000_000, 44_100, 100);
        let slow = run(name, song, 144_000_000, 44_100, 100);
        println!("{name} @288: {r:?}");
        assert!(pct(r.over_late, r.frames) < 3, "{name}: {} of {} frames over", r.over_late, r.frames);
        assert!(pct(r.halves_late, r.halves) < 1, "{name}: {} halves at risk", r.halves_late);
        assert!(r.limit_x10 > slow.limit_x10 && r.reverb >= slow.reverb, "{name}");
        full.push(pct(r.full, r.frames));
    }
    assert!(full[2] >= 85, "Sound tour at full quality {} %", full[2]);
}

/// Half the rate (22.05 kHz, which the DAC plays natively) at 144 MHz,
/// considered and not built (04 §9): it halves every cost per second, so
/// each song keeps more voices and the reverb longer than at 44.1 kHz.
#[test]
fn half_rate_would_keep_more_of_each_song() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let half = run(name, song, 144_000_000, 22_050, 100);
        let full = run(name, song, 144_000_000, 44_100, 100);
        println!("{name} @144 22.05 kHz: {half:?}");
        assert!(half.limit_x10 > full.limit_x10 && half.reverb >= full.reverb, "{name}");
    }
}

/// A render 1.5 × dearer than the table (paging, a patch the table
/// underrates): the measured ratio raises the prediction and the governor
/// sheds more, so the true cost of "Sound tour" fits (under 1 % of its PCM
/// halves at risk);
/// "Etude" and "Hill Song" at that cost need more than the floor of voices
/// allows (the pace rule's rests then keep thread mode running; the DAC
/// underruns).
#[test]
fn a_misestimate_is_corrected_by_the_measurement() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let exact = run(name, song, 144_000_000, 44_100, 100);
        let dear = run(name, song, 144_000_000, 44_100, 150);
        println!("{name} ×1.5: {dear:?}");
        assert!(dear.limit_x10 < exact.limit_x10, "{name} sheds more");
        if name.starts_with("Sound") {
            assert!(dear.halves_late * 100 / dear.halves < 1, "{name}: {} halves at risk", dear.halves_late);
        } else {
            assert!(dear.limit_x10 < 10 * u64::from(MIN_VOICES) + 10, "{name} near the floor");
        }
    }
}
