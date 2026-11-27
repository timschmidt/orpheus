//! Adversarial MIDI files (VER-3, 07 §3.7) through the fuzz body
//! (`fuzz/src/lib.rs`: parser, summary, player; no panic, progress, an
//! end, finite samples in [−1, 1], positions inside the song): every seed
//! swept (cut, damaged, 16/32-bit fields at their edges over its head);
//! the header and track chunk fields at their edges; overlong and
//! unterminated variable-length quantities; running status with nothing
//! to run; meta and SysEx lengths at their edges; many and empty tracks;
//! token soup of events (proptest).

#[path = "../../../tests/adversarial.rs"]
mod adversarial;
#[path = "../fuzz/src/lib.rs"]
mod bodies;

use adversarial::{config, edges, put_be, seeds, sweep};
use proptest::prelude::*;

/// The body with options `opt` (bit 0 karaoke, the rest effects, mute,
/// seek target).
fn run(opt: u8, file: &[u8]) {
    let mut d = vec![opt];
    d.extend_from_slice(file);
    bodies::midi_parse(&d);
}

#[test]
fn every_seed_swept() {
    for (name, s) in seeds("midi_parse") {
        eprintln!("{name}");
        sweep(&s, 1, 48, bodies::midi_parse);
    }
}

/// A format-`fmt` file of `tracks`, `division` ticks.
fn smf(fmt: u16, division: u16, tracks: &[Vec<u8>]) -> Vec<u8> {
    let mut b = b"MThd\0\0\0\x06".to_vec();
    b.extend(fmt.to_be_bytes());
    b.extend((tracks.len() as u16).to_be_bytes());
    b.extend(division.to_be_bytes());
    for t in tracks {
        b.extend_from_slice(b"MTrk");
        b.extend((t.len() as u32).to_be_bytes());
        b.extend_from_slice(t);
    }
    b
}

fn notes() -> Vec<u8> {
    vec![
        0x00, 0xFF, 0x51, 0x03, 0x07, 0xA1, 0x20, 0x00, 0x90, 60, 100, 0x60, 0x80, 60, 0, 0x00, 0xFF, 0x2F,
        0x00,
    ]
}

#[test]
fn header_and_chunk_fields_at_their_edges() {
    let good = smf(1, 96, &[notes(), notes()]);
    // Header length, format, track count, division (ticks and SMPTE).
    for (off, n, real) in [(4, 4, 6u64), (8, 2, 1), (10, 2, 2), (12, 2, 96)] {
        for v in edges(8 * n as u32, real).into_iter().chain([0xE250, 0xE728, 0x8000, 0x7FFF]) {
            if v >> (8 * n) != 0 {
                continue;
            }
            let mut b = good.clone();
            put_be(&mut b, off, n, v);
            for opt in [0, 1, 0xFE] {
                run(opt, &b);
            }
        }
    }
    // Each track chunk's length.
    let first = 14 + 4;
    let second = first + 4 + notes().len() + 4;
    for at in [first, second] {
        for v in edges(32, notes().len() as u64) {
            let mut b = good.clone();
            put_be(&mut b, at, 4, v);
            run(0, &b);
        }
    }
    // A chunk that is not a track, an unknown chunk before the tracks, a
    // RIFF (RMID) wrapper of any length.
    let mut other = good.clone();
    other[14..18].copy_from_slice(b"XXXX");
    run(0, &other);
    let mut riff = b"RIFF".to_vec();
    riff.extend(0u32.to_le_bytes());
    riff.extend_from_slice(b"RMIDdata");
    riff.extend((good.len() as u32).to_le_bytes());
    riff.extend_from_slice(&good);
    for v in edges(32, good.len() as u64) {
        let mut b = riff.clone();
        b[16..20].copy_from_slice(&(v as u32).to_le_bytes());
        run(0, &b);
        b[4..8].copy_from_slice(&(v as u32).to_le_bytes());
        run(0, &b);
    }
}

#[test]
fn hostile_events() {
    let ev: Vec<Vec<u8>> = vec![
        // Overlong and unterminated delta times.
        vec![0xFF, 0xFF, 0xFF, 0xFF, 0x7F, 0x90, 60, 100],
        vec![0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF],
        vec![0x8F, 0xFF, 0xFF, 0x7F, 0x90, 60, 100, 0x8F, 0xFF, 0xFF, 0x7F, 0x80, 60, 0],
        // Running status with no status before it; data bytes only.
        vec![0x00, 60, 100, 0x00, 61, 100],
        // Meta events with lengths at their edges and past the chunk.
        vec![0x00, 0xFF, 0x51, 0x00],
        vec![0x00, 0xFF, 0x51, 0x03, 0, 0, 0],
        vec![0x00, 0xFF, 0x51, 0x03, 0xFF, 0xFF, 0xFF],
        vec![0x00, 0xFF, 0x58, 0x04, 0, 0, 0, 0],
        vec![0x00, 0xFF, 0x05, 0x8F, 0xFF, 0xFF, 0x7F, b'x'],
        vec![0x00, 0xFF, 0x01, 0xFF, 0xFF, 0xFF, 0xFF, 0x0F],
        vec![0x00, 0xFF, 0x2F, 0x05, 1, 2, 3, 4, 5, 0x00, 0x90, 60, 1],
        // SysEx and escapes of any length.
        vec![0x00, 0xF0, 0x7F, 0x01, 0x02],
        vec![0x00, 0xF0, 0x00, 0x00, 0xF7, 0x00],
        vec![0x00, 0xF7, 0x8F, 0xFF, 0xFF, 0x7F],
        // System real-time and undefined status bytes inside a track.
        vec![0x00, 0xF8, 0x00, 0xFA, 0x00, 0xF4, 0x00, 0xFD, 0x00, 0xF1, 0x00],
        // Every channel message with every status, data 0x7F and 0xFF.
        (0x80..=0xEFu8).flat_map(|s| [0x00, s, 0x7F, 0x7F]).collect(),
        (0x80..=0xEFu8).flat_map(|s| [0x00, s, 0xFF, 0xFF]).collect(),
        // Program and bank changes to every value, pitch bend extremes,
        // RPNs with nothing after them.
        (0..=127u8).flat_map(|p| [0x00, 0xC9, p, 0x00, 0xB0, 0x00, p, 0x00, 0xC0, p]).collect(),
        vec![
            0x00, 0xE0, 0x00, 0x00, 0x00, 0xE0, 0x7F, 0x7F, 0x00, 0xB0, 101, 0, 0x00, 0xB0, 100, 0, 0x00,
            0xB0, 6, 127,
        ],
        // A thousand notes on at once, never off.
        (0..1000u32).flat_map(|i| [0x00, 0x90 | (i % 16) as u8, (i % 128) as u8, 127]).collect(),
        // Lyrics and karaoke text with control and invalid UTF-8 bytes.
        vec![0x00, 0xFF, 0x05, 0x06, b'@', b'K', 0x00, 0xFF, 0xC0, b'\\'],
        vec![0x00, 0xFF, 0x01, 0x04, b'@', b'T', 0x1B, 0x07],
    ];
    for e in &ev {
        for division in [96u16, 1, 0, 0x7FFF, 0xE728, 0x8000] {
            for opt in [0u8, 1, 0x81] {
                run(opt, &smf(0, division, std::slice::from_ref(e)));
                let mut cut = e.clone();
                cut.truncate(e.len() / 2);
                run(opt, &smf(1, division, &[notes(), cut, e.clone()]));
            }
        }
    }
    // Many tracks, empty tracks, a track count that lies.
    let many: Vec<Vec<u8>> = (0..70).map(|_| notes()).collect();
    run(0, &smf(1, 96, &many));
    run(0, &smf(1, 96, &vec![Vec::new(); 70]));
    let mut lies = smf(1, 96, &[notes()]);
    lies[10..12].copy_from_slice(&0xFFFFu16.to_be_bytes());
    run(0, &lies);
}

fn event() -> impl Strategy<Value = Vec<u8>> {
    let delta = prop_oneof![
        Just(vec![0u8]),
        Just(vec![0x60]),
        Just(vec![0x81, 0x00]),
        Just(vec![0xFF, 0xFF, 0xFF, 0x7F])
    ];
    let body = prop_oneof![
        (0x80u8..=0xEF, any::<u8>(), any::<u8>()).prop_map(|(s, a, b)| vec![s, a & 0x7F, b & 0x7F]),
        (any::<u8>(), any::<u8>()).prop_map(|(a, b)| vec![a & 0x7F, b & 0x7F]),
        (any::<u8>(), prop::collection::vec(any::<u8>(), 0..8)).prop_map(|(t, d)| {
            let mut v = vec![0xFF, t, d.len() as u8];
            v.extend(d);
            v
        }),
        prop::collection::vec(any::<u8>(), 0..6).prop_map(|d| {
            let mut v = vec![0xF0, d.len() as u8];
            v.extend(d);
            v
        }),
        prop::collection::vec(any::<u8>(), 1..4),
    ];
    (delta, body).prop_map(|(mut d, b)| {
        d.extend(b);
        d
    })
}

proptest! {
    #![proptest_config(config("midi-event-soup", 128))]

    /// Tracks of events in any order and any values.
    #[test]
    fn event_soup(fmt in 0u16..3, division in any::<u16>(), opt in any::<u8>(),
                  tracks in prop::collection::vec(prop::collection::vec(event(), 0..40), 1..5)) {
        let t: Vec<Vec<u8>> = tracks.into_iter().map(|evs| evs.concat()).collect();
        run(opt, &smf(fmt, division, &t));
    }
}
