//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-synth --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/synth_events/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

fn rec(op: u8, part: u8, a: u8, b: u8, x: [u8; 4]) -> [u8; 8] {
    [op, part, a, b, x[0], x[1], x[2], x[3]]
}

fn on(part: u8, note: u8, vel: u8, at: u16) -> [u8; 8] {
    let o = at.to_le_bytes();
    rec(0, part, note, vel, [o[0], o[1], 0, 0])
}

fn off(part: u8, note: u8, at: u16) -> [u8; 8] {
    let o = at.to_le_bytes();
    rec(1, part, note, 0, [o[0], o[1], 0, 0])
}

fn render(frames: u16, i16_out: bool) -> [u8; 8] {
    let f = frames.to_le_bytes();
    rec(if i16_out { 9 } else { 8 }, 0, f[0], f[1], [0; 4])
}

fn param(part: u8, p: u8, at4: u8, v: f32) -> [u8; 8] {
    rec(2, part, p, at4, v.to_le_bytes())
}

#[test]
fn synth_events_corpus() {
    replay::replay("synth_events", bodies::synth_events);
}

#[test]
fn seeds() {
    let mut v: Vec<(String, Vec<u8>)> = Vec::new();
    // Chords on every melodic part, a drum part, renders in both formats.
    let mut chords = vec![3, 1, 16, 0];
    for part in 0..14u8 {
        for k in 0..3u8 {
            chords.extend(on(part, 48 + 4 * k + part, 100, u16::from(k) * 37));
        }
    }
    chords.extend(render(2048, false));
    for part in 0..14u8 {
        chords.extend(off(part, 48 + part, 10));
    }
    chords.extend(render(1500, true));
    chords.extend(render(4096, false));
    v.push(("chords".into(), chords));
    // Parameters with extreme values, NaN and infinities, sweeps of every kind.
    let mut params = vec![2, 7, 4, 7];
    params.extend(on(0, 60, 127, 0));
    params.extend(on(1, 40, 90, 3));
    for (i, x) in
        [0.0f32, 1.0, -1.0, 1.0e30, -1.0e30, f32::NAN, f32::INFINITY, 0.5, 20_000.0].iter().enumerate()
    {
        for p in 0..14u8 {
            params.extend(param(i as u8 % 2, p, (i as u8) * 9, *x));
        }
        params.extend(rec(3, 0, i as u8, 3, x.to_le_bytes()));
        params.extend(rec(4, 0, 0, 1, x.to_le_bytes()));
        params.extend(rec(5, 0, 0, 2, x.to_le_bytes()));
        params.extend(render(300 + i as u16 * 7, i % 2 == 0));
    }
    v.push(("params".into(), params));
    // Patches with wild fields on every preset, then notes on them.
    let mut patches = vec![1, 9, 4, 0];
    for (k, x) in [f32::NAN, -5.0, 1.0e9, 0.0, 3.0].iter().enumerate() {
        for sel in 0..14u8 {
            patches.extend(rec(10, sel, sel, (k as u8) * 7 + sel, x.to_le_bytes()));
        }
        for part in 0..14u8 {
            patches.extend(on(part, 30 + 5 * part, 64 + k as u8 * 10, k as u16));
        }
        patches.extend(render(800, false));
    }
    v.push(("patches".into(), patches));
    // Stealing pressure: a low voice limit, many notes, resets, odd blocks.
    let mut steal = vec![0, 5, 3, 2];
    for i in 0..60u8 {
        steal.extend(on(i % 3, 20 + i, 1 + i * 2, u16::from(i) * 11));
        if i % 10 == 9 {
            steal.extend(render(97, i % 20 == 9));
            steal.extend(rec(11, 1, 1 + i % 16, 0, [0; 4]));
        }
    }
    steal.extend(rec(6, 0xFF, 0, 0, [0; 4]));
    steal.extend(rec(7, 0, 0, 0, [5, 0, 0, 0]));
    steal.extend(render(4096, false));
    steal.extend(rec(11, 0, 0, 0, 42u32.to_le_bytes()));
    steal.extend(render(1, false));
    v.push(("steal".into(), steal));
    for (_, d) in &v {
        bodies::synth_events(d);
    }
    replay::write_seeds("synth_events", &v);
}
