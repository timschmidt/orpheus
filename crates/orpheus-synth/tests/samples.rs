//! Sample playback: WAV parsing, the cache, golden renders, streaming,
//! block-size independence and sample-accurate starts.
//!
//! Golden hashes in `tests/goldens-samples.txt` (rewritten with
//! `ORPHEUS_WRITE_GOLDENS=1`); the test samples are made by
//! `tests/common/samples.rs`.

#[path = "common/samples.rs"]
mod samples;

use orpheus_synth::demo::hash_i16;
use orpheus_synth::sample::{SampleCache, SliceSource, Want};
use orpheus_synth::wav::{self, Encoding, WavError};
use orpheus_synth::{Adsr, Engine, Event, Patch, SampleLoop, SampleParams};

const FILE: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens-samples.txt");

#[test]
fn golden_sample_renders() {
    let mut table = String::new();
    for sc in &samples::SCENARIOS {
        let (out, misses) = sc.render(256);
        let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap_or(0);
        assert!(peak > 1000, "{}: silent (peak {peak})", sc.name);
        assert_eq!(misses, 0, "{}: {misses} samples missed the cache", sc.name);
        table.push_str(&format!("{} {:016x} {}\n", sc.name, hash_i16(&out), sc.frames()));
    }
    if std::env::var_os("ORPHEUS_WRITE_GOLDENS").is_some() {
        std::fs::write(FILE, &table).unwrap();
        return;
    }
    let want = std::fs::read_to_string(FILE).expect("goldens-samples.txt missing: ORPHEUS_WRITE_GOLDENS=1");
    assert_eq!(table, want, "sample golden renders differ");
}

#[test]
fn streaming_through_a_small_cache_equals_the_pinned_render() {
    let pinned = samples::SCENARIOS.iter().find(|s| s.name == "sample/melody").unwrap();
    let streamed = samples::SCENARIOS.iter().find(|s| s.name == "sample/melody-streamed").unwrap();
    let (a, _) = pinned.render(256);
    // The head (4 pages, 1024 frames) must cover what a new note plays in
    // one block: up to 512 frames at the melody's top speed (1.84).
    for block in [64, 256, 512] {
        let (b, misses) = streamed.render(block);
        assert_eq!(misses, 0, "block {block}");
        assert!(a == b, "streamed render differs at block {block}");
    }
}

#[test]
fn sample_renders_do_not_depend_on_block_size() {
    for sc in samples::SCENARIOS.iter().filter(|s| s.head == u32::MAX) {
        let (reference, _) = sc.render(4096);
        for block in [1, 7, 333] {
            let (out, _) = sc.render(block);
            assert!(out == reference, "{} differs at block {block}", sc.name);
        }
    }
}

#[test]
fn sample_notes_start_on_their_sample() {
    // An impulse sample at the engine's rate, played at its root: the first
    // output sample of each note is exactly at the event's offset.
    let mut pcm = vec![[0.0f32; 2]; 64];
    pcm[0] = [0.5; 2];
    let file = samples::wav(48_000, 1, 16, &pcm, None);
    let files = [file.as_slice()];
    let mut src = SliceSource::new(&files);
    let mut mem = vec![0i16; 256 * 4];
    let mut cache = SampleCache::new(&mut mem);
    cache.load(&mut src, 5, 0, u32::MAX).unwrap();
    let p = Patch {
        amp_env: Adsr::GATE,
        velocity: 0.0,
        level: 1.0,
        ..Patch::sample(SampleParams { root: 60, looping: SampleLoop::Off, ..SampleParams::new(5) })
    };
    let mut rng = orpheus_synth::math::Rng::new(7);
    for _ in 0..200 {
        let mut e = Engine::new(48_000, 1);
        e.set_patch(0, &p);
        let at = rng.next_u32() % 900;
        let block = 1 + (rng.next_u32() % 300) as usize;
        let mut out = vec![0.0f32; 2 * 1024];
        let mut pos = 0;
        while pos < 1024 {
            let n = block.min(1024 - pos);
            let ev: Vec<Event> = if (at as usize) >= pos && (at as usize) < pos + n {
                vec![Event::note_on(at - pos as u32, 0, 60, 100)]
            } else {
                vec![]
            };
            e.render_f32_with(&mut out[2 * pos..2 * (pos + n)], &ev, &cache);
            pos += n;
        }
        let first = out.iter().step_by(2).position(|&x| x != 0.0);
        assert_eq!(first, Some(at as usize), "block {block}");
        // Played bit for bit: 0.5 → 16384/32768 × volume 0.8 × cos(π/4).
        let expect = 16_384.0 / 32_768.0 * 0.8 * orpheus_dsp::math::cos(core::f32::consts::FRAC_PI_4);
        assert!((out[2 * at as usize] - expect).abs() < 1e-6, "{}", out[2 * at as usize]);
    }
}

#[test]
fn wav_formats_parse_and_convert() {
    let frames: Vec<[f32; 2]> = (0..300).map(|i| [i as f32 / 300.0 - 0.5, 0.25]).collect();
    for (ch, bits, enc) in [
        (1, 8, Encoding::U8),
        (2, 8, Encoding::U8),
        (1, 16, Encoding::S16),
        (2, 16, Encoding::S16),
        (1, 24, Encoding::S24),
        (2, 24, Encoding::S24),
    ] {
        let f = samples::wav(32_000, ch, bits, &frames, Some((64, 10, 99)));
        let files = [f.as_slice()];
        let mut src = SliceSource::new(&files);
        let mut r = orpheus_synth::sample::Reader::new(&mut src, 0).unwrap();
        let info = wav::parse(&mut r).unwrap();
        assert_eq!(info.encoding, enc);
        assert_eq!(u16::from(info.channels), ch);
        assert_eq!(info.rate, 32_000);
        assert_eq!(info.frames, 300);
        assert_eq!(info.loop_points, Some((10, 100)));
        assert_eq!(info.unity_note, Some(64));
        let mut out = vec![0i16; 300];
        assert_eq!(wav::read_frames(&mut r, &info, 0, &mut out).unwrap(), 300);
        let tol = if bits == 8 { 300.0 } else { 2.0 };
        for (i, (&got, f)) in out.iter().zip(&frames).enumerate() {
            let want = if ch == 2 { (f[0] + f[1]) / 2.0 } else { f[0] } * 32767.0;
            assert!((f32::from(got) - want).abs() <= tol, "{ch}×{bits} frame {i}: {got} vs {want}");
        }
    }
}

#[test]
fn bad_wavs_are_refused_cleanly() {
    let good = samples::blip8();
    let parse = |d: &[u8]| {
        let files = [d];
        let mut src = SliceSource::new(&files);
        let mut r = orpheus_synth::sample::Reader::new(&mut src, 0).unwrap();
        wav::parse(&mut r)
    };
    assert!(parse(&good).is_ok());
    assert_eq!(parse(b"RIFX\0\0\0\0WAVE"), Err(WavError::NotWave));
    assert_eq!(parse(&good[..30]), Err(WavError::NoFormat));
    // Every truncation either parses (fewer frames) or fails; none panics.
    for cut in 0..good.len() {
        if let Ok(i) = parse(&good[..cut]) {
            assert!(u64::from(i.data_offset) + u64::from(i.frames) * u64::from(i.block_align) <= cut as u64);
        }
    }
    // Three channels, 12 bits, float 64: unsupported.
    for (off, val) in [(22usize, 3u16), (34, 12), (20, 3)] {
        let mut b = good.clone();
        b[off..off + 2].copy_from_slice(&val.to_le_bytes());
        if off == 20 {
            b[34..36].copy_from_slice(&64u16.to_le_bytes());
        }
        assert_eq!(parse(&b), Err(WavError::Unsupported), "field at {off}");
    }
    // A data chunk claiming more than the file: cut to the file.
    let mut b = good.clone();
    let at = b.windows(4).position(|w| w == b"data").unwrap();
    b[at + 4..at + 8].copy_from_slice(&u32::MAX.to_le_bytes());
    let i = parse(&b).unwrap();
    assert_eq!(i.frames as usize, b.len() - at - 8);
}

#[test]
fn cache_refuses_heads_that_do_not_fit_and_unloads() {
    let files = samples::files();
    let refs: Vec<&[u8]> = files.iter().map(Vec::as_slice).collect();
    let mut src = SliceSource::new(&refs);
    let mut mem = vec![0i16; 256 * 10];
    let mut cache = SampleCache::new(&mut mem);
    assert_eq!(cache.pages(), 10);
    // The pluck is 104 pages: whole does not fit, a head of 8 does.
    assert_eq!(cache.load(&mut src, 0, 0, u32::MAX).err(), Some(orpheus_synth::sample::LoadError::NoRoom));
    assert!(cache.info(0).is_none());
    cache.load(&mut src, 0, 0, 8).unwrap();
    assert_eq!(cache.free_pages(), 2);
    assert_eq!(cache.load(&mut src, 1, 1, 3).err(), Some(orpheus_synth::sample::LoadError::NoRoom));
    cache.unload(0);
    assert_eq!(cache.free_pages(), 10);
    assert_eq!(cache.load(&mut src, 40, 0, 1).err(), Some(orpheus_synth::sample::LoadError::BadSlot));
    // A want for an empty slot or past the end loads nothing.
    let w = [Want { slot: 9, frame: 0, speed: 1.0, looping: false }];
    assert_eq!(cache.service(&mut src, &w, 8), Ok(0));
}
