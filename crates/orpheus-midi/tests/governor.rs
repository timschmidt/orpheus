//! The demo songs through the load governor with the player's measured
//! costs (`orpheus_synth::load`, 2026-11-26): what each song needs on the
//! player, the clock and what the governor sheds, and that the render then
//! fits. The numbers these print are in 04 §9 ("Load governor").
//!
//! The model measures as the player does: a piece's true cost is the
//! engine's state through it (the estimate after every 32 frames, so notes
//! that start inside a piece count), optionally dearer for its first
//! milliseconds (cold code and patches) and with paging per piece; the
//! governor gets the estimate before and after the piece, the cycles and
//! the paging apart.

mod common;

use common::demo;
use orpheus_midi::write::Writer;
use orpheus_midi::{Division, MidiPlayer, Options, SliceSource};
use orpheus_synth::load::{BUDGET_PCT, CLOCKS_MHZ, Governor, Level, MIN_VOICES, PLAYER};

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
    /// Frames at each clock step (High, Turbo, Max).
    at_clock: [u64; 3],
    /// The highest ratio past the first second, % (and at the end).
    max_ratio_late: u32,
    ratio_end: u32,
    gave_up: bool,
}

/// A PCM half whose render takes more than this share of its real time
/// risks an underrun: the pace rule's rests leave the render two thirds,
/// and the DSP chain and the PCM ring take a few per cent.
const HALF_PCT: u64 = 62;
/// Frames of a PCM half.
const HALF: u64 = 1536;

/// How the model runs a song.
#[derive(Clone, Copy)]
struct Model {
    /// The clock at the start, Hz.
    hz: u32,
    rate: u32,
    /// The clock stays at `hz` (a power mode pins it); else it follows the
    /// governor's plan at once, as the firmware switches it.
    pinned: bool,
    /// The true cost as estimated × this / 100.
    true_pct: u32,
    /// The first `cold_ms` cost `cold_x` times as much.
    cold_x: u32,
    cold_ms: u32,
    /// Paging per piece, µs.
    page_us: u32,
}

const AT_144: Model =
    Model { hz: 144_000_000, rate: 44_100, pinned: true, true_pct: 100, cold_x: 1, cold_ms: 0, page_us: 0 };
const FREE: Model = Model { pinned: false, ..AT_144 };
const AT_288: Model = Model { hz: 288_000_000, ..AT_144 };

fn hz_of(step: u8) -> u32 {
    CLOCKS_MHZ[usize::from(step).min(2)] * 1_000_000
}

/// Render `data` as the firmware does: plan, clock, apply, render the
/// piece, measure.
fn run_data(data: &[u8], kar: bool, m: Model) -> Run {
    let mut p = Box::new(MidiPlayer::new(Options::new(m.rate)));
    p.open_from(&mut SliceSource::new(data), kar).unwrap();
    let mut src = SliceSource::new(data);
    let mut g = Governor::new(16, m.hz, m.rate);
    let mut out = [0i16; 64];
    let mut r = Run::default();
    let (mut cost, mut limit) = (0u64, 0u64);
    let (mut win_cyc, mut win_n, mut win_rt) = (0u64, 0u64, 0u64);
    loop {
        let e = p.engine().estimate(&PLAYER);
        let plan = g.plan(&e);
        let hz = if m.pinned { m.hz } else { hz_of(plan.clock) };
        g.set_clock(hz, m.rate);
        p.set_fx_enabled(plan.fx);
        p.set_voice_limit(usize::from(plan.limit));
        if let Some(k) = plan.keep {
            r.shed += p.shed(usize::from(k));
        }
        // The piece in 32-frame steps (the engine's output does not depend
        // on the cut); its true cost is the state through it.
        let (mut n, mut cyc) = (0u64, 0u64);
        while n < u64::from(plan.frames) {
            let want = (u64::from(plan.frames) - n).min(32) as usize;
            let got = p.render_from(&mut src, &mut out[..2 * want]).unwrap();
            if got == 0 {
                break;
            }
            cyc += u64::from(p.engine().estimate(&PLAYER).total()) * got as u64;
            n += got as u64;
        }
        if n == 0 {
            break;
        }
        cyc = cyc * u64::from(m.true_pct) / 100;
        if r.frames * 1000 < u64::from(m.cold_ms) * u64::from(m.rate) {
            cyc *= u64::from(m.cold_x);
        }
        let paging = u64::from(m.page_us) * u64::from(hz / 1_000_000);
        let after = p.engine().estimate(&PLAYER).total();
        g.measured(&e, after, n as u32, (cyc + paging).min(u64::from(u32::MAX)) as u32, paging as u32);
        r.gave_up |= g.give_up();
        let rt = u64::from(g.rt());
        let budget = rt * u64::from(BUDGET_PCT) / 100;
        let per = (cyc + paging) / n;
        if per > budget {
            r.over += n;
            if r.frames >= 2 * u64::from(m.rate) {
                r.over_late += n;
            }
        }
        if g.level() == Level::Full {
            r.full += n;
        }
        if g.level() < Level::Reverb {
            r.reverb += n;
        }
        r.max_level = r.max_level.max(Some(g.level()));
        r.at_clock[usize::from(plan.clock).min(2)] += n;
        if r.frames >= u64::from(m.rate) {
            r.max_ratio_late = r.max_ratio_late.max(g.ratio_pct());
        }
        cost += cyc + paging;
        win_cyc += cyc + paging;
        win_n += n;
        win_rt += rt * n;
        if win_n >= HALF {
            r.halves += 1;
            if win_cyc * 100 > win_rt * HALF_PCT && r.frames >= 2 * u64::from(m.rate) {
                r.halves_late += 1;
            }
            (win_cyc, win_n, win_rt) = (0, 0, 0);
        }
        limit += u64::from(g.limit()) * n;
        r.frames += n;
    }
    r.mean_cost = cost / r.frames.max(1);
    r.limit_x10 = limit * 10 / r.frames.max(1);
    r.ratio_end = g.ratio_pct();
    r
}

fn run(name: &str, song: fn(&mut [u8]) -> Option<usize>, m: Model) -> Run {
    run_data(&demo(song), name.ends_with(".kar"), m)
}

fn pct(a: u64, b: u64) -> u64 {
    a * 100 / b.max(1)
}

/// At 144 MHz (the clock pinned there) every demo song needs more than the
/// budget (1,795 cycles a frame) at some point — the reverb alone is 671,
/// a voice 160–740 — so the governor acts on each. Afterwards the render
/// stays within the budget but for note bursts between two plans and the
/// fade-out tails of stolen voices (under 3 % of the frames past the
/// first two seconds), under 1 % of the PCM halves past them cost more
/// than 62 % of their real time (where the ring could run dry), and the
/// songs play to their end. None plays bit for bit with the host for
/// long: `crc on` checks run with `synth gov off`.
#[test]
fn demo_songs_fit_the_budget_at_144_mhz() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let r = run(name, song, AT_144);
        println!("{name} @144: {r:?}");
        assert!(r.frames > 44_100 * 20, "{name} played to its end");
        assert!(pct(r.over_late, r.frames) < 3, "{name}: {} of {} frames over", r.over_late, r.frames);
        assert!(pct(r.halves_late, r.halves) < 1, "{name}: {} halves at risk", r.halves_late);
        assert_eq!(r.max_level, Some(Level::Voices), "{name}");
        assert!(pct(r.full, r.frames) < 5, "{name}");
        assert!(!r.gave_up, "{name}");
    }
}

/// At 288 MHz (the clock pinned there: High performance) the budget
/// doubles: "Sound tour" plays as the host renders it for most of its
/// length, the others keep more voices.
#[test]
fn demo_songs_at_288_mhz() {
    let mut full = Vec::new();
    for (name, song) in orpheus_midi::demo::SONGS {
        let r = run(name, song, AT_288);
        let slow = run(name, song, AT_144);
        println!("{name} @288: {r:?}");
        assert!(pct(r.over_late, r.frames) < 3, "{name}: {} of {} frames over", r.over_late, r.frames);
        assert!(pct(r.halves_late, r.halves) < 1, "{name}: {} halves at risk", r.halves_late);
        assert!(r.limit_x10 > slow.limit_x10 && r.reverb >= slow.reverb, "{name}");
        full.push(pct(r.full, r.frames));
    }
    assert!(full[2] >= 85, "Sound tour at full quality {} %", full[2]);
}

/// D-4 (owner, 2026-11-26: up to 288 MHz when needed, low power
/// preferred): with the clock free the governor raises it before it sheds
/// anything. "Etude" (the heaviest) reaches Max and keeps what the 288 MHz
/// run keeps; "Sound tour" plays at full quality where 288 MHz suffices;
/// each song spends time below Max where the music allows, and fits the
/// budget as well as the fixed clocks do.
#[test]
fn demo_songs_raise_the_clock_before_shedding() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let free = run(name, song, FREE);
        let max = run(name, song, AT_288);
        println!("{name} free: {free:?}");
        assert!(free.frames > 44_100 * 20 && !free.gave_up, "{name} played to its end");
        assert!(free.at_clock[2] > 0, "{name} reached Max");
        assert!(pct(free.over_late, free.frames) < 3, "{name}: {} frames over", free.over_late);
        assert!(pct(free.halves_late, free.halves) < 1, "{name}: {} halves at risk", free.halves_late);
        // As good as the fixed 288 MHz run (within a few per cent: the
        // raise takes a piece, the way down hysteresis).
        assert!(
            free.limit_x10 * 100 >= max.limit_x10 * 95,
            "{name}: {} vs {}",
            free.limit_x10,
            max.limit_x10
        );
        let (f, m) = (pct(free.full, free.frames), pct(max.full, max.frames));
        assert!(f + 2 >= m, "{name}: full {f} % vs {m} %");
        let (f, m) = (pct(free.reverb, free.frames), pct(max.reverb, max.frames));
        assert!(f + 2 >= m, "{name}: reverb {f} % vs {m} %");
        if name.starts_with("Etude") {
            assert!(pct(free.at_clock[2], free.frames) > 50, "{name} mostly at Max: {:?}", free.at_clock);
        }
        if name.starts_with("Sound") {
            assert!(
                pct(free.full, free.frames) >= 85,
                "{name} at full quality {} %",
                pct(free.full, free.frames)
            );
            assert!(
                free.at_clock[0] + free.at_clock[1] > 0,
                "{name} below Max at times: {:?}",
                free.at_clock
            );
        }
    }
}

/// A light song (one flute voice at a time and the reverb, ≈ 1,200 cycles
/// a frame, 37 % of High) stays at High, at full quality, the host's
/// render bit for bit.
#[test]
fn a_light_song_stays_at_high() {
    let mut buf = vec![0u8; 4096];
    let mut w = Writer::new(&mut buf, 0, Division::Metrical(480));
    w.track();
    w.tempo(0, 500_000);
    w.channel(0, 0xC0, 73, 0);
    for i in 0..48u64 {
        let note = 60 + (i % 12) as u8;
        w.channel(i * 480, 0x90, note, 90);
        w.channel(i * 480 + 400, 0x80, note, 0);
    }
    w.end_track(48 * 480);
    let n = w.finish().unwrap();
    let r = run_data(&buf[..n], false, FREE);
    println!("light: {r:?}");
    assert!(r.frames > 44_100 * 20);
    assert_eq!((r.at_clock[1], r.at_clock[2], r.full, r.max_level), (0, 0, r.frames, Some(Level::Full)));
}

/// Half the rate (22.05 kHz, which the DAC plays natively) at 144 MHz,
/// considered and not built (04 §9): it halves every cost per second, so
/// each song keeps more voices and the reverb longer than at 44.1 kHz.
#[test]
fn half_rate_would_keep_more_of_each_song() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let half = run(name, song, Model { rate: 22_050, ..AT_144 });
        let full = run(name, song, AT_144);
        println!("{name} @144 22.05 kHz: {half:?}");
        assert!(half.limit_x10 > full.limit_x10 && half.reverb >= full.reverb, "{name}");
    }
}

/// A render 1.5 × dearer than the table (a patch the table underrates):
/// the measured ratio raises the prediction and the governor sheds more,
/// so the true cost of "Sound tour" fits (under 1 % of its PCM halves at
/// risk); "Etude" and "Hill Song" at that cost need more than the floor of
/// voices allows at 144 MHz (the pace rule's rests then keep thread mode
/// running; the DAC underruns) — but none is given up.
#[test]
fn a_misestimate_is_corrected_by_the_measurement() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let exact = run(name, song, AT_144);
        let dear = run(name, song, Model { true_pct: 150, ..AT_144 });
        println!("{name} ×1.5: {dear:?}");
        assert!(dear.limit_x10 < exact.limit_x10, "{name} sheds more");
        assert!((140..=160).contains(&dear.max_ratio_late), "{name}: ratio {}", dear.max_ratio_late);
        assert!(!dear.gave_up, "{name}");
        if name.starts_with("Sound") {
            assert!(dear.halves_late * 100 / dear.halves < 1, "{name}: {} halves at risk", dear.halves_late);
        } else {
            assert!(dear.limit_x10 < 10 * u64::from(MIN_VOICES) + 10, "{name} near the floor");
        }
    }
}

/// Regression (2026-11-26, the player, build `78b316b5`): every demo song
/// stopped within 3–20 s with `status` `ratio=400 lvl=3/3 vlim=2`: the
/// first piece was measured against the empty engine (its notes start
/// inside it) and taken as it was, so the ratio sat at its 4.0 clamp and
/// the governor cut every song to two voices. The songs as the player
/// plays them — their first 50 ms 10 × dearer (cold code and patches),
/// 2 line loads (350 µs) of paging a piece — measure ≈ 1.0 past the first
/// second, keep what the same songs without the cold start keep, are
/// never given up, and play
/// to their ends (the frames `goldens-play.txt` lists).
#[test]
fn regress_synth_ratio_pinned_by_cold_start() {
    let frames = [1_131_268u64, 1_146_600, 2_469_600];
    for ((name, song), want) in orpheus_midi::demo::SONGS.into_iter().zip(frames) {
        let cold = Model { cold_x: 10, cold_ms: 50, page_us: 350, ..FREE };
        let r = run(name, song, cold);
        let exact = run(name, song, Model { page_us: 350, ..FREE });
        println!("{name} cold: {r:?}");
        assert_eq!(r.frames, want, "{name} played to its end");
        assert!(!r.gave_up, "{name}");
        assert!(r.max_ratio_late <= 115, "{name}: ratio {} past the first second", r.max_ratio_late);
        assert!((95..=105).contains(&r.ratio_end), "{name}: ratio {} at the end", r.ratio_end);
        assert!(r.limit_x10 * 100 >= exact.limit_x10 * 90, "{name}: {} vs {}", r.limit_x10, exact.limit_x10);
    }
}
