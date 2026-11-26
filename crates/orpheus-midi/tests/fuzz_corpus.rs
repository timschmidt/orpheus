//! The fuzz target's committed corpus and regression inputs, replayed on
//! stable through the body the cargo-fuzz target runs (VER-4); and the seed
//! inputs (`ORPHEUS_WRITE_SEEDS=1 cargo test -p orpheus-midi --test
//! fuzz_corpus` rewrites `tests/fuzz-corpus/midi_parse/seed-*`).

mod common;

#[path = "../fuzz/src/lib.rs"]
mod bodies;
#[path = "../../../tests/replay.rs"]
mod replay;

use common::{demo, file};
use orpheus_midi::Division;

#[test]
fn midi_parse_corpus() {
    assert!(replay::replay("midi_parse", bodies::midi_parse) >= 10);
}

fn with_opt(opt: u8, f: Vec<u8>) -> Vec<u8> {
    let mut v = vec![opt];
    v.extend(f);
    v
}

/// Seeds: the demo songs (each option bit), every format and division,
/// the adversarial shapes of `tests/parse.rs`.
#[test]
fn midi_seeds() {
    let d96 = Division::Metrical(96);
    let mut v: Vec<(String, Vec<u8>)> = Vec::new();
    v.push(("etude".into(), with_opt(0x0A, demo(orpheus_midi::demo::etude))));
    v.push(("karaoke".into(), with_opt(0x07, demo(orpheus_midi::demo::karaoke))));
    v.push(("tour".into(), with_opt(0x92, demo(orpheus_midi::demo::tour))));
    v.push((
        "format2".into(),
        with_opt(
            0x40,
            file(2, d96, |w| {
                w.track();
                w.tempo(0, 1_000_000);
                w.channel(0, 0x90, 60, 100);
                w.end_track(96);
                w.track();
                w.channel(48, 0x90, 62, 100);
                w.end_track(96);
            }),
        ),
    ));
    v.push((
        "smpte".into(),
        with_opt(
            0x20,
            file(0, Division::Smpte { fps: 29, tpf: 40 }, |w| {
                w.track();
                w.tempo(0, 250_000);
                w.channel(10, 0x90, 60, 100);
                w.meta(20, 0x05, b"la\r");
                w.channel(1200, 0x80, 60, 0);
                w.end_track(1300);
            }),
        ),
    ));
    v.push((
        "controllers".into(),
        with_opt(
            0x06,
            file(0, d96, |w| {
                w.track();
                for (cc, val) in [
                    (101u8, 0u8),
                    (100, 0),
                    (6, 12),
                    (38, 50),
                    (96, 0),
                    (97, 0),
                    (1, 127),
                    (7, 0),
                    (11, 64),
                    (10, 0),
                    (64, 127),
                    (91, 127),
                    (93, 127),
                    (121, 0),
                    (120, 0),
                    (123, 0),
                ] {
                    w.channel(0, 0xB0, cc, val);
                    w.channel(1, 0x90, 60, 100);
                }
                w.channel(2, 0xE0, 0x7F, 0x7F);
                w.channel(3, 0xD0, 100, 0);
                w.channel(4, 0xA0, 60, 100);
                w.channel(5, 0xC9, 30, 0);
                w.channel(6, 0x99, 38, 127);
                w.sysex(7, &[0x7E, 0x7F, 9, 1, 0xF7]);
                w.end_track(200);
            }),
        ),
    ));
    v.push((
        "corrupt".into(),
        with_opt(
            0,
            file(1, d96, |w| {
                w.raw(b"JUNK\0\0\0\x02ab");
                w.track();
                w.channel(0, 0x90, 60, 100);
                w.raw(&[0x00, 0xF4, 0x80, 0x80, 0x80, 0x80, 0x00]);
                w.end_track(10);
                w.track();
                w.raw(&[0x00, 0x3C, 0x40]); // data byte without status
                w.end_track(10);
                w.track();
                w.meta(0, 0x01, &[b'x'; 200]);
                w.meta(1, 0x51, &[]);
                w.close(); // no end of track
            }),
        ),
    ));
    v.push((
        "huge-deltas".into(),
        with_opt(
            0x01,
            file(0, d96, |w| {
                w.track();
                w.channel(0, 0x90, 60, 100);
                for i in 1..6u64 {
                    w.channel(i * 0x0FFF_FFFF, 0x80, 60, 0);
                }
                w.end_track(6 * 0x0FFF_FFFF);
            }),
        ),
    ));
    let mut cut = demo(orpheus_midi::demo::karaoke);
    cut.truncate(cut.len() * 2 / 3);
    v.push(("truncated".into(), with_opt(0x01, cut)));
    let mut rmid = Vec::new();
    let inner = demo(orpheus_midi::demo::etude);
    rmid.extend_from_slice(b"RIFF");
    rmid.extend_from_slice(&((inner.len() + 12) as u32).to_le_bytes());
    rmid.extend_from_slice(b"RMIDdata");
    rmid.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    rmid.extend_from_slice(&inner);
    v.push(("rmid".into(), with_opt(0x30, rmid)));
    v.push((
        "many-tracks".into(),
        with_opt(
            0,
            file(1, d96, |w| {
                for t in 0..64u64 {
                    w.track();
                    w.channel(t, 0x90 | (t % 16) as u8, 40 + t as u8, 90);
                    w.end_track(t + 10);
                }
            }),
        ),
    ));
    for (_, d) in &v {
        bodies::midi_parse(d);
    }
    replay::write_seeds("midi_parse", &v);
}
