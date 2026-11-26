//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-synth --test
//! fuzz_corpus seeds` rewrites `tests/fuzz-corpus/synth_events/seed-*`).

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;
#[path = "common/samples.rs"]
mod samples;

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
fn wav_parse_corpus() {
    assert!(replay::replay("wav_parse", bodies::wav_parse) >= 6);
}

#[test]
fn bank_decode_corpus() {
    assert!(replay::replay("bank_decode", bodies::bank_decode) >= 3);
}

/// Seeds of `wav_parse` (`ORPHEUS_WRITE_SEEDS=1` rewrites them): each
/// format, a loop, extensible headers, odd chunks, truncation.
#[test]
fn wav_seeds() {
    let tone = |n: usize, ch: usize| -> Vec<[f32; 2]> {
        (0..n)
            .map(|i| {
                let x = (i as f32 * 0.07).sin() * 0.7;
                if ch == 2 { [x, -x * 0.5] } else { [x; 2] }
            })
            .collect()
    };
    let mut v: Vec<(String, Vec<u8>)> = vec![
        ("u8-mono".into(), samples::wav(22_050, 1, 8, &tone(900, 1), None)),
        ("s16-stereo-loop".into(), samples::wav(44_100, 2, 16, &tone(2000, 2), Some((60, 400, 1599)))),
        ("s24-mono-loop".into(), samples::wav(48_000, 1, 24, &tone(1500, 1), Some((72, 0, 1499)))),
        ("s24-stereo".into(), samples::wav(32_000, 2, 24, &tone(700, 2), None)),
    ];
    // WAVE_FORMAT_EXTENSIBLE (fmt of 40 bytes, sub-format PCM, then float).
    let base = samples::wav(48_000, 1, 16, &tone(600, 1), None);
    let mut ext = base[..12].to_vec();
    ext.extend_from_slice(b"fmt ");
    ext.extend_from_slice(&40u32.to_le_bytes());
    ext.extend_from_slice(&0xFFFEu16.to_le_bytes());
    ext.extend_from_slice(&base[22..36]);
    ext.extend_from_slice(&22u16.to_le_bytes());
    ext.extend_from_slice(&16u16.to_le_bytes());
    ext.extend_from_slice(&4u32.to_le_bytes());
    ext.extend_from_slice(&1u16.to_le_bytes());
    ext.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    ext.extend_from_slice(&base[36..]);
    v.push(("extensible-pcm".into(), ext.clone()));
    let mut float = ext[..20].to_vec();
    float.extend_from_slice(&0xFFFEu16.to_le_bytes());
    float.extend_from_slice(&1u16.to_le_bytes());
    float.extend_from_slice(&48_000u32.to_le_bytes());
    float.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    float.extend_from_slice(&4u16.to_le_bytes());
    float.extend_from_slice(&32u16.to_le_bytes());
    float.extend_from_slice(&22u16.to_le_bytes());
    float.extend_from_slice(&32u16.to_le_bytes());
    float.extend_from_slice(&4u32.to_le_bytes());
    float.extend_from_slice(&3u16.to_le_bytes());
    float.extend_from_slice(&[0, 0, 0, 0, 0x10, 0, 0x80, 0, 0, 0xAA, 0, 0x38, 0x9B, 0x71]);
    float.extend_from_slice(b"data");
    let pcm: Vec<u8> = (0..500).flat_map(|i| ((i as f32 * 0.05).sin() * 1.5).to_le_bytes()).collect();
    float.extend_from_slice(&(pcm.len() as u32).to_le_bytes());
    float.extend_from_slice(&pcm);
    v.push(("extensible-float".into(), float));
    // A data chunk that claims more than the file, and a truncated file.
    let mut long = samples::wav(8000, 1, 16, &tone(300, 1), Some((50, 10, 5000)));
    let at = long.windows(4).position(|w| w == b"data").unwrap();
    long[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    v.push(("data-too-long".into(), long));
    let cut = samples::wav(44_100, 2, 16, &tone(400, 2), None);
    v.push(("truncated".into(), cut[..cut.len() * 2 / 3].to_vec()));
    for (name, d) in &v {
        // Every seed is a usable file (so the fuzzer starts past the
        // header checks).
        let files = [d.as_slice()];
        let mut src = orpheus_synth::sample::SliceSource::new(&files);
        let mut r = orpheus_synth::sample::Reader::new(&mut src, 0).unwrap();
        let info = orpheus_synth::wav::parse(&mut r);
        assert!(info.is_ok_and(|i| i.frames > 100), "{name}: {info:?}");
        bodies::wav_parse(d);
    }
    replay::write_seeds("wav_parse", &v);
}

/// Seeds of `bank_decode`: the factory bank, one bare patch per kind, a
/// corrupt bank.
#[test]
fn bank_seeds() {
    use orpheus_synth::bank::{Writer, encode_patch};
    use orpheus_synth::{Kit, Patch, presets};
    let mut buf = vec![0u8; 16 * 1024];
    let mut w = Writer::new(&mut buf).unwrap();
    for (name, p) in presets::MELODIC {
        w.patch(name, &p).unwrap();
    }
    for (name, d) in presets::DRUMS {
        w.patch(name, &Patch::drum(d)).unwrap();
    }
    w.patch("sampler", &presets::SAMPLER).unwrap();
    for (name, k) in orpheus_synth::kit::FACTORY {
        w.kit(name, &Kit::new(k)).unwrap();
    }
    let n = w.len();
    buf.truncate(n);
    let mut v: Vec<(String, Vec<u8>)> = vec![("factory".into(), buf.clone())];
    for (name, p) in [("fm", presets::FM_EPIANO), ("kit", Patch::KIT), ("sample", presets::SAMPLER)] {
        let mut b = vec![0u8; 300];
        let n = encode_patch(&p, &mut b).unwrap();
        b.truncate(n);
        v.push((format!("patch-{name}"), b));
    }
    let mut bad = buf;
    for i in (40..bad.len()).step_by(97) {
        bad[i] ^= 0x5A;
    }
    v.push(("corrupt".into(), bad));
    for (_, d) in &v {
        bodies::bank_decode(d);
    }
    replay::write_seeds("bank_decode", &v);
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
