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
/// fade-out tails of stolen voices, under 1 % of the PCM halves past the
/// first two seconds cost more than 62 % of their real time (where the
/// ring could run dry), and the songs play to their end. None plays bit
/// for bit with the host for long: `crc on` checks run with `synth gov
/// off`. (Since 2026-11-26 a step comes back when the smoothed cost is
/// under half of real time and the step fits the budget, through note
/// bursts: "Hill Song" here keeps a voice more and has single pieces over
/// the budget in 13 % of its frames, was 2 %, still no PCM half at risk;
/// "Sound tour" plays 7 % of its frames at full quality, was under 5 %.)
#[test]
fn demo_songs_fit_the_budget_at_144_mhz() {
    for (name, song) in orpheus_midi::demo::SONGS {
        let r = run(name, song, AT_144);
        println!("{name} @144: {r:?}");
        assert!(r.frames > 44_100 * 20, "{name} played to its end");
        assert!(pct(r.over_late, r.frames) < 15, "{name}: {} of {} frames over", r.over_late, r.frames);
        assert!(pct(r.halves_late, r.halves) < 1, "{name}: {} halves at risk", r.halves_late);
        assert_eq!(r.max_level, Some(Level::Voices), "{name}");
        assert!(pct(r.full, r.frames) < 10, "{name}");
        assert!(!r.gave_up, "{name}");
    }
}

/// At 288 MHz (the clock governor's top level, PWR-5) the budget
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

// ---------------------------------------------------------------- the loop

use orpheus_synth::load::sim::{self, Job, LINE_US};

/// The real-time executor's work beside the render while a MIDI file
/// plays (2026-11-26, the player) and how a piece pays for its notes.
#[derive(Clone, Copy, Debug)]
struct Rt {
    /// The render's true compute, % of the table (the player's `ratio=`).
    true_pct: u32,
    /// Lines a steady piece loads (the render's own paging).
    piece_lines: u32,
    /// Lines a piece in which notes start loads (the note path,
    /// `synth-note`, and the render lines it evicts).
    note_lines: u32,
    /// The rest of the real-time executor's work per piece (the PCM
    /// writer, the plan and the measurement), % of the piece's audio.
    stall_pct: u32,
    /// The outer part every 4,096 frames, µs.
    outer_us: u32,
}

/// A demo song through the firmware's loop (`orpheus_synth::load::sim`)
/// with thread mode as `s` sets it.
fn play(data: &[u8], kar: bool, s: &sim::Setup, rt: Rt) -> (sim::Report, Governor) {
    let mut p = Box::new(MidiPlayer::new(Options::new(s.rate)));
    p.open_from(&mut SliceSource::new(data), kar).unwrap();
    let mut setup = *s;
    setup.frames = p.duration();
    let src = core::cell::RefCell::new(SliceSource::new(data));
    let p = core::cell::RefCell::new(p);
    let mut g = Governor::new(16, 144_000_000, s.rate);
    let frames = core::cell::Cell::new(0u64);
    let stamp = core::cell::Cell::new(0u64);
    let r = sim::run(
        &mut g,
        &setup,
        || p.borrow().engine().estimate(&PLAYER),
        |plan| {
            let mut p = p.borrow_mut();
            p.set_fx_enabled(plan.fx);
            p.set_voice_limit(usize::from(plan.limit));
            if let Some(k) = plan.keep {
                p.shed(usize::from(k));
            }
            let mut out = [0i16; 64];
            let (mut n, mut cyc, mut notes) = (0u64, 0u64, false);
            while n < u64::from(plan.frames) {
                let want = (u64::from(plan.frames) - n).min(32) as usize;
                let got = p.render_from(&mut *src.borrow_mut(), &mut out[..2 * want]).unwrap_or(0);
                if got == 0 {
                    break;
                }
                cyc += u64::from(p.engine().estimate(&PLAYER).total()) * got as u64;
                let top = p.engine().voices().map(|v| v.stamp).max().unwrap_or(0);
                if top > stamp.get() {
                    stamp.set(top);
                    notes = true;
                }
                n += got as u64;
            }
            let per_frame = (cyc * u64::from(rt.true_pct) / 100 / n.max(1)) as u32;
            let mhz = CLOCKS_MHZ[usize::from(plan.clock).min(2)];
            let lines = rt.piece_lines + if notes { rt.note_lines } else { 0 };
            let len_us = u32::try_from(n * 1_000_000 / u64::from(s.rate)).unwrap_or(0);
            let f0 = frames.get();
            frames.set(f0 + n);
            let outer = if (f0 + n) / 4096 != f0 / 4096 { rt.outer_us } else { 0 };
            sim::Cost {
                per_frame: if n == 0 { 0 } else { per_frame },
                paging: lines * LINE_US * mhz,
                stall_us: len_us * rt.stall_pct / 100 + outer,
            }
        },
    );
    (r, g)
}

/// The real-time side as the player measured it (2026-11-26, build
/// `b694ac61`): the render's compute 78 % of the table (`ratio=` 76–82),
/// 2 line loads a steady piece (the emulated working set, 2.3), 8 more
/// in a piece in which notes start (the note path, `synth-note`, and the
/// render lines it evicts: pieces of 8–11 ms, `ub=synth:render:8–11ms`),
/// 6 % of each piece's audio for the PCM writer, the plan and the
/// measurement, 1.5 ms for the outer part every 4,096 frames.
const PLAYER_RT: Rt = Rt { true_pct: 78, piece_lines: 2, note_lines: 8, stall_pct: 6, outer_us: 1_500 };

/// What the firmware does about thread mode's line loads.
#[derive(Clone, Copy)]
struct Fixes {
    /// The main executor's loop off the synth's lines (`hot.toml`
    /// `synth-block`; before: lines 9–10, two a slot).
    main_loop: bool,
    /// The USB supervisor without its 20 ms timer.
    usb: bool,
    /// The budget follows the PCM ring (`Governor::set_head`).
    ring: bool,
}

const TODAY: Fixes = Fixes { main_loop: false, usb: false, ring: false };
const ALL: Fixes = Fixes { main_loop: true, usb: true, ring: true };

/// Thread mode while the demo songs play on the player (Automatic, the
/// cable in, the test look on, Now Playing), as jobs with the code-cache
/// lines of the synth's working set they load (`layout.json`, build
/// `b694ac61`): the key tick (10 ms, on the wake-up entry's line since
/// `82b61a1e`: none), the USB supervisor (20 ms: the device, the bus, the
/// timer's and its own polls, 5 lines), the UI's tick (`ui_us`, `ui_lines`
/// every 33 ms), the step sampler (40 ms, 0.95 ms of I2C, `steps-sample`
/// on 4 render lines), the once-a-second tasks (10 lines). The UI's and
/// the USB task's polls are on lines 10–0, the per-piece and wake-up code:
/// 30 % of the lines thread mode evicts are the render's own (measured in
/// the next piece, `pg=`). The UI's share is what the player's `cut=`
/// (≈ 50 % of `rests=`) leaves: ≈ 2.2 ms and 9 lines a tick.
fn thread(f: Fixes, ui_us: u32, ui_lines: u32) -> sim::Setup {
    let mut s = sim::Setup::new(44_100, 0);
    s.rest_ends_idle = true;
    s.ring_budget = f.ring;
    s.slot_lines = if f.main_loop { 0 } else { 2 };
    s.render_share_pct = 30;
    s.jobs[0] = Job { period_us: 10_000, work_us: 40, lines: 0 };
    let usb_period = if f.usb { 1_000_000 } else { 20_000 };
    s.jobs[1] = Job { period_us: usb_period, work_us: 80, lines: 5 };
    s.jobs[2] = Job { period_us: 33_000, work_us: ui_us, lines: ui_lines };
    s.jobs[3] = Job { period_us: 40_000, work_us: 950, lines: 4 };
    s.jobs[4] = Job { period_us: 1_000_000, work_us: 800, lines: 10 };
    s
}

/// Each demo song through the loop: (name, report, the governor after).
fn songs(s: &sim::Setup, rt: Rt) -> Vec<(&'static str, sim::Report, Governor)> {
    orpheus_midi::demo::SONGS
        .into_iter()
        .map(|(name, song)| {
            let (r, g) = play(&demo(song), name.ends_with(".kar"), s, rt);
            println!(
                "{name}: underruns {} rests {} cut {} % min head {} µs reload {} ms shed {} % lim {} pg {} % load {} %",
                r.underruns,
                r.rests,
                r.cut * 100 / r.rests.max(1),
                r.min_head_us,
                r.reload_us / 1000,
                r.shed_frames * 100 / r.written.max(1),
                g.limit(),
                g.paging() * 100 / g.rt().max(1),
                g.load() * 100 / g.rt().max(1),
            );
            (name, r, g)
        })
        .collect()
}

/// Regression (2026-11-26, the player, build `b694ac61`, Automatic, the
/// cable in, the test look on, `playdir /.orpheus/test/suite/midi`):
/// underruns in every demo song — "Etude" 18, "Hill Song" 3, "Sound
/// tour" 9 — labelled `ub=synth:render:8–11ms` and `ub=synth:rest:5ms`,
/// at a governor load of only 24–46 % at 288 MHz, with `cut=` ≈ 50 % of
/// `rests=`. Thread mode's code shares the code cache with the synth's
/// whole working set: every line it loads in a rest the render loads
/// again after it, the governor sees only the share inside the render
/// (`pg=` 4–11 %), and with thread mode busy through half the rests the
/// real-time executor fell behind at a render load the budget allowed.
/// Model (thread mode as the player's `cut=` shows it, its jobs and lines
/// from `layout.json`): the code of that day underruns in "Etude"; with
/// the same overheads a budget that follows the PCM ring
/// (`orpheus_synth::load::budget_pct`) plays all three songs without
/// one; with the main loop off the synth's lines and the USB supervisor's
/// 20 ms timer gone as well, without one and with no more shed than the
/// budget alone sheds, the ring never under 20 ms.
#[test]
fn regress_synth_underruns_at_low_load() {
    let today = songs(&thread(TODAY, 2_200, 9), PLAYER_RT);
    let (_, etude, _) = &today[0];
    assert!(etude.underruns >= 10, "the code of 2026-11-26: {etude:?}");
    assert!((40..=70).contains(&(etude.cut * 100 / etude.rests)), "cut= as on the player: {etude:?}");
    let ring_only = Fixes { main_loop: false, usb: false, ring: true };
    let ring = songs(&thread(ring_only, 2_200, 9), PLAYER_RT);
    let all = songs(&thread(ALL, 2_200, 9), PLAYER_RT);
    for ((name, r, _), (_, a, _)) in ring.iter().zip(&all) {
        assert_eq!((r.underruns, r.stopped), (0, false), "{name}, the ring's budget: {r:?}");
        assert_eq!((a.underruns, a.stopped), (0, false), "{name}, everything: {a:?}");
        assert!(a.min_head_us >= 20_000, "{name}: {a:?}");
        assert!(
            a.shed_frames <= r.shed_frames + r.written / 50,
            "{name}: {} > {}",
            a.shed_frames,
            r.shed_frames
        );
        assert!(a.reload_us < r.reload_us, "{name}");
    }
}

/// The fixes hold with more of thread mode than the player showed: a UI
/// tick of 3–4 ms over 12–14 lines, more real-time work per piece and in
/// the outer part, or the USB task as the heavy job (2 ms, 10 lines every
/// 20 ms, the UI light). The code of 2026-11-26 underruns in every case;
/// with the fixes nothing does, and "Sound tour" keeps most of its
/// frames unshed.
#[test]
fn demo_songs_play_through_more_thread_mode() {
    let heavy_rt = Rt { stall_pct: 10, outer_us: 3_000, ..PLAYER_RT };
    let usb_heavy = |f: Fixes| {
        let mut s = thread(f, 800, 6);
        if !f.usb {
            s.jobs[1] = Job { period_us: 20_000, work_us: 2_000, lines: 10 };
        }
        s
    };
    for (label, old, new, rt) in [
        ("UI 4 ms", thread(TODAY, 4_000, 12), thread(ALL, 4_000, 12), PLAYER_RT),
        ("UI 3 ms, RT 10 %", thread(TODAY, 3_000, 14), thread(ALL, 3_000, 14), heavy_rt),
        ("USB", usb_heavy(TODAY), usb_heavy(ALL), Rt { stall_pct: 8, ..PLAYER_RT }),
    ] {
        println!("{label}");
        let before = songs(&old, rt);
        assert!(before.iter().any(|(_, r, _)| r.underruns > 10), "{label}: the code of 2026-11-26");
        for (name, r, _) in songs(&new, rt) {
            assert_eq!((r.underruns, r.stopped), (0, false), "{label}, {name}: {r:?}");
            if name.starts_with("Sound") {
                assert!(r.shed_frames * 100 / r.written < 50, "{label}, {name}: {r:?}");
            }
        }
    }
}
