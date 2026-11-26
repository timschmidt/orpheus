//! Fuzz-target body of `orpheus-midi` (VER-4), shared by the cargo-fuzz
//! target in `fuzz_targets/` (nightly) and the stable replay of the
//! committed corpus (`tests/fuzz_corpus.rs`, every `cargo test`).
//!
//! [`midi_parse`] — arbitrary bytes as a MIDI file (the first byte picks
//! options: karaoke, effects, mute/solo, the seek target), through every
//! layer: the parser event by event, the summary (scan, tempo map,
//! lyrics), and the player (open, render in odd blocks, seek, render
//! again). Oracles: no panic; the parser makes progress (no more events
//! than bytes plus two per track) and ends; the player's steps end;
//! every output sample finite and within [−1, 1]; the position never
//! passes the duration; lyric frames inside the song.

#![forbid(unsafe_code)]

use orpheus_midi::smf::MAX_TRACKS;
use orpheus_midi::{MidiPlayer, MidiSource, Options, Pull, SliceSource, Smf, summary};

/// Bytes the body looks at (larger inputs are cut).
pub const MAX_INPUT: usize = 64 * 1024;

/// The target body.
pub fn midi_parse(data: &[u8]) {
    let (opt, file) = match data.split_first() {
        Some((&o, rest)) => (o, &rest[..rest.len().min(MAX_INPUT)]),
        None => (0, data),
    };
    parse_only(file);
    let kar = opt & 1 != 0;
    let mut last_frame = 0;
    if let Ok((info, map)) = summary(&mut SliceSource::new(file), 8000, kar, |l| {
        assert!(l.frame >= last_frame, "lyrics in order");
        last_frame = l.frame;
    }) {
        assert!(last_frame <= info.end_frame);
        assert!(info.duration >= info.end_frame);
        assert!(!map.points().is_empty());
        play(file, opt, info.duration);
    }
}

fn parse_only(file: &[u8]) {
    let mut src = SliceSource::new(file);
    let mut smf = Smf::new();
    smf.begin(src.len());
    let mut buf = [0u8; 64];
    let mut fill = |smf: &mut Smf, src: &mut SliceSource<'_>, off: u32, len: u16| {
        let n = src.read_at(off, &mut buf[..usize::from(len)]).unwrap();
        smf.supply(&buf[..n]);
    };
    let mut calls = 0usize;
    loop {
        calls += 1;
        assert!(calls < 400, "open makes progress");
        match smf.open() {
            Err(_) => return,
            Ok(Pull::Need(n)) => fill(&mut smf, &mut src, n.offset, n.len),
            Ok(Pull::Ready(_)) => break,
            Ok(Pull::End) => unreachable!(),
        }
    }
    let tracks = smf.tracks();
    assert!((1..=MAX_TRACKS).contains(&tracks));
    let mut events = 0usize;
    let mut needs = 0usize;
    let mut last = [0u64; MAX_TRACKS];
    loop {
        match smf.next_event() {
            Pull::Need(n) => {
                needs += 1;
                assert!(n.len > 0 && usize::from(n.len) <= 64);
                fill(&mut smf, &mut src, n.offset, n.len);
            }
            Pull::Ready(e) => {
                events += 1;
                let t = usize::from(e.track);
                assert!(t < tracks);
                assert!(e.tick >= last[t], "ticks go forward within a track");
                last[t] = e.tick;
            }
            Pull::End => break,
        }
        assert!(events <= file.len() + 2 * tracks, "{events} events from {} bytes", file.len());
        assert!(needs <= file.len() + tracks, "{needs} refills");
    }
}

fn play(file: &[u8], opt: u8, duration: u64) {
    let mut o = Options::new(8000);
    o.effects = opt & 2 != 0;
    o.chase = opt & 4 != 0;
    let mut p = Box::new(MidiPlayer::new(o));
    let mut src = SliceSource::new(file);
    if p.open_from(&mut src, opt & 1 != 0).is_err() {
        return;
    }
    assert_eq!(p.duration(), duration);
    if opt & 8 != 0 {
        p.set_mute(opt >> 4, true);
    }
    if opt & 16 != 0 {
        p.set_solo(opt >> 5, true);
    }
    let mut out = [0f32; 2 * 700];
    let sizes = [1usize, 64, 333, 700];
    let mut rendered = 0;
    for (i, &n) in sizes.iter().cycle().take(12).enumerate() {
        let got = p.render_f32_from(&mut src, &mut out[..2 * n]).unwrap();
        for &s in &out[..2 * got] {
            assert!(s.is_finite() && s.abs() <= 1.0, "{s}");
        }
        rendered += got;
        assert!(p.position() <= p.duration());
        if i == 5 {
            // Seek somewhere the input chooses (also past the end).
            let target = u64::from(opt) * duration / 128 + rendered as u64;
            let at = p.seek_from(&mut src, target).unwrap();
            assert_eq!(at, target.min(duration));
        }
        while p.pop_lyric().is_some() {}
    }
}
