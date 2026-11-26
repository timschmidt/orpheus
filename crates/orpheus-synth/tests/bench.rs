//! The player's bench loads (`orpheus_synth::bench`, SYN-3) and building
//! the engine in place.
//!
//! `tests/goldens-bench.txt` holds the hash of each load's measured blocks
//! (8 voices, 8 warm-up blocks, 64 measured): the player's `synth bench I`
//! prints the same hash when its samples are bit-identical to the host's.
//! After an intended change of the sound: `ORPHEUS_WRITE_GOLDENS=1 cargo
//! test -p orpheus-synth --test bench`.

use orpheus_synth::bench::{self, BENCHES, BLOCKS, VOICES, WARM};
use orpheus_synth::demo::{SCENARIOS, Score, hash_i16};
use orpheus_synth::{Engine, Event, presets};

/// The table for this build's features: the player builds without the
/// delay and sample playback (`--no-default-features`), so its delay loads
/// (29, 30, 35) render dry: `tests/goldens-bench-player.txt` holds what
/// the player's `synth bench` prints.
const FILE: &str = if cfg!(feature = "delay") {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens-bench.txt")
} else {
    concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens-bench-player.txt")
};

fn zeroed() -> Box<Engine> {
    Box::new(<Engine as bytemuck::Zeroable>::zeroed())
}

#[test]
fn bench_hashes() {
    let mut table = String::new();
    let mut e = zeroed();
    let mut buf = vec![0i16; 2 * bench::BLOCK];
    for (i, b) in BENCHES.iter().enumerate() {
        let mut timed = 0;
        let (h, sounding) = bench::run(i, VOICES, WARM, BLOCKS, &mut e, &mut buf, |on| {
            if on {
                timed += 1;
            }
        })
        .unwrap();
        assert_eq!(timed, BLOCKS, "{}", b.name);
        let want = u32::from(bench::voices(b.load, VOICES));
        // Melodic and effect loads hold their voices; drums at least half
        // of the time.
        if !matches!(b.load, bench::Load::Drum(_)) {
            assert_eq!(sounding, want * BLOCKS, "{}: voices held", b.name);
        } else {
            assert!(sounding >= want * BLOCKS / 2, "{}: {sounding}", b.name);
        }
        table.push_str(&format!("{i} {} {h:016x}\n", b.name));
    }
    if std::env::var_os("ORPHEUS_WRITE_GOLDENS").is_some() {
        std::fs::write(FILE, &table).unwrap();
        return;
    }
    let want = std::fs::read_to_string(FILE).expect("the bench table: run with ORPHEUS_WRITE_GOLDENS=1");
    assert_eq!(table, want, "bench renders differ (ORPHEUS_WRITE_GOLDENS=1 after an intended change)");
}

#[test]
fn unknown_load_is_refused() {
    let mut e = zeroed();
    let mut buf = vec![0i16; 2 * bench::BLOCK];
    assert!(bench::run(BENCHES.len(), 8, 0, 1, &mut e, &mut buf, |_| {}).is_none());
    assert!(bench::run(0, 8, 0, 1, &mut e, &mut buf[..10], |_| {}).is_none());
}

/// `Engine::init` on an engine that has played (voices, effects, patches,
/// kits changed) leaves exactly what `Engine::new` builds: the same value
/// and the same render.
#[test]
fn init_in_place_equals_new() {
    let mut used = zeroed();
    used.init(44_100, 9);
    used.set_patch(3, &presets::PWM_PAD);
    used.set_kit(1, &orpheus_synth::Kit::new(orpheus_synth::kit::FACTORY[2].1));
    used.set_voice_limit(3);
    let mut buf = vec![0i16; 2 * 1000];
    used.render_i16(&mut buf, &[Event::note_on(0, 3, 60, 100), Event::note_on(5, 3, 64, 90)]);
    for (rate, seed) in [(48_000, 1), (44_100, 7), (8_000, 3)] {
        used.init(rate, seed);
        let fresh = Engine::new(rate, seed);
        assert_eq!(format!("{used:?}"), format!("{fresh:?}"), "init({rate}, {seed})");
    }
}

/// Every golden scenario set up in place on a used engine and score renders
/// its golden hash (the player's `synth golden I`).
#[test]
fn scenarios_set_up_in_place_render_their_goldens() {
    let mut e = zeroed();
    let mut s = Box::new(<Score as bytemuck::Zeroable>::zeroed());
    let table = std::fs::read_to_string(concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens.txt")).unwrap();
    for (sc, line) in SCENARIOS.iter().zip(table.lines()).step_by(5) {
        // The engine still holds the previous scenario.
        sc.setup(&mut e, &mut s);
        let mut out = vec![0i16; 2 * sc.frames()];
        orpheus_synth::demo::drive(&mut e, &s, sc.frames(), 256, |e, r, ev| {
            e.render_i16(&mut out[2 * r.start..2 * r.end], ev);
        });
        let want = line.split(' ').nth(1).unwrap();
        assert_eq!(format!("{:016x}", hash_i16(&out)), want, "{}", sc.name);
    }
}

/// The streaming hash equals the hash of the whole.
#[test]
fn fnv_in_pieces() {
    let s: Vec<i16> = (0..1000).map(|i| (i * 37 - 500) as i16).collect();
    let mut h = orpheus_synth::demo::Fnv::new();
    h.add(&s[..333]);
    h.add(&s[333..]);
    assert_eq!(h.0, hash_i16(&s));
}
