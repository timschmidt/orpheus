//! The Standard MIDI File parser: formats, running status, meta events,
//! SysEx, divisions, merge order, and adversarial files (lengths, deltas,
//! missing ends, corrupt bytes, too many tracks).

mod common;

use common::{events, file};
use orpheus_midi::smf::{MAX_TRACKS, TEXT_MAX, TextKind};
use orpheus_midi::timing::{Clock, MAX_SECONDS};
use orpheus_midi::{Division, MidiError, Msg, SliceSource, summary};

const D96: Division = Division::Metrical(96);

fn msgs(data: &[u8]) -> Vec<(u64, u8, Msg)> {
    events(data).unwrap().0.into_iter().map(|e| (e.tick, e.track, e.msg)).collect()
}

fn ch(status: u8, d1: u8, d2: u8) -> Msg {
    Msg::Channel { status, d1, d2 }
}

#[test]
fn format0_running_status_and_meta_events() {
    let f = file(0, D96, |w| {
        w.running_status(true);
        w.track();
        w.meta(0, 0x03, b"Name");
        w.tempo(0, 600_000);
        w.meta(0, 0x58, &[3, 2, 24, 8]);
        w.meta(0, 0x59, &[0xFE, 1]);
        w.channel(0, 0x90, 60, 100);
        w.channel(10, 0x90, 64, 90); // running status
        w.channel(20, 0x90, 60, 0); // note-on velocity 0
        w.channel(20, 0xC0, 5, 0);
        w.channel(21, 0xD0, 40, 0);
        w.meta(30, 0x05, b"la");
        w.meta(30, 0x06, b"verse");
        w.channel(40, 0x80, 64, 0);
        w.end_track(50);
    });
    // Running status really used: one status byte for the three note-ons.
    assert_eq!(f.windows(3).filter(|w| w == &[0x90, 60, 100]).count(), 1);
    let (ev, _) = events(&f).unwrap();
    let m: Vec<Msg> = ev.iter().map(|e| e.msg).collect();
    assert_eq!(
        m,
        [
            Msg::Text(TextKind::TrackName),
            Msg::Tempo(600_000),
            Msg::TimeSig { num: 3, den: 2, clocks: 24, n32: 8 },
            Msg::KeySig { sharps: -2, minor: true },
            ch(0x90, 60, 100),
            ch(0x90, 64, 90),
            ch(0x90, 60, 0),
            ch(0xC0, 5, 0),
            ch(0xD0, 40, 0),
            Msg::Text(TextKind::Lyric),
            Msg::Text(TextKind::Marker),
            ch(0x80, 64, 0),
            Msg::EndOfTrack,
        ]
    );
    assert_eq!(ev[0].text(), b"Name");
    assert_eq!(ev[9].text(), b"la");
    let ticks: Vec<u64> = ev.iter().map(|e| e.tick).collect();
    assert_eq!(ticks, [0, 0, 0, 0, 0, 10, 20, 20, 21, 30, 30, 40, 50]);
}

#[test]
fn running_status_survives_meta_only_when_restated() {
    // A meta event cancels running status: a data byte after it ends the
    // track (the rest of the file still plays).
    let f = file(1, D96, |w| {
        w.track();
        w.channel(0, 0x90, 60, 100);
        w.meta(1, 0x01, b"x");
        w.raw(&[0x01, 62, 100]); // delta 1, data bytes without a status
        w.channel(5, 0x90, 64, 100);
        w.end_track(10);
        w.track();
        w.channel(3, 0x91, 50, 80);
        w.end_track(10);
    });
    let m = msgs(&f);
    assert_eq!(m[0], (0, 0, ch(0x90, 60, 100)));
    assert_eq!(m[1], (1, 0, Msg::Text(TextKind::Text)));
    assert_eq!(m[2], (2, 0, Msg::EndOfTrack), "track 0 ends at the bad byte");
    assert!(m.contains(&(3, 1, ch(0x91, 50, 80))), "track 1 plays on");
}

#[test]
fn sysex_and_unknown_meta_are_skipped() {
    let big = [0x11u8; 300];
    let f = file(0, D96, |w| {
        w.track();
        w.sysex(0, &[0x7E, 0x7F, 0x09, 0x01, 0xF7]);
        w.sysex(1, &big);
        w.raw(&[0x00, 0xF7, 0x02, 0x01, 0x02]); // escape
        w.meta(2, 0x7F, &big); // sequencer specific
        w.meta(2, 0x54, &[1, 2, 3, 4, 5]); // SMPTE offset
        w.channel(3, 0x90, 60, 100);
        w.end_track(4);
    });
    let m = msgs(&f);
    assert_eq!(m.iter().filter(|x| x.2 == Msg::Other).count(), 5);
    assert_eq!(m[5], (3, 0, ch(0x90, 60, 100)));
    assert_eq!(m[6].2, Msg::EndOfTrack);
}

#[test]
fn long_text_is_cut_and_the_rest_skipped() {
    let text: Vec<u8> = (0..200u8).map(|i| b'a' + i % 26).collect();
    let f = file(0, D96, |w| {
        w.track();
        w.meta(0, 0x01, &text);
        w.meta(0, 0x01, b"");
        w.channel(1, 0x90, 60, 100);
        w.end_track(2);
    });
    let (ev, _) = events(&f).unwrap();
    assert_eq!(ev[0].text(), &text[..TEXT_MAX]);
    assert_eq!(ev[1].msg, Msg::Text(TextKind::Text));
    assert!(ev[1].text().is_empty(), "zero-length text");
    assert_eq!(ev[2].msg, ch(0x90, 60, 100));
}

#[test]
fn zero_length_and_short_meta_events() {
    let f = file(0, D96, |w| {
        w.track();
        w.meta(0, 0x51, &[]); // tempo without data
        w.meta(0, 0x51, &[0, 0, 0]); // tempo 0
        w.meta(0, 0x58, &[4]); // short time signature
        w.meta(0, 0x59, &[]);
        w.channel(1, 0x90, 60, 100);
        w.end_track(2);
    });
    let m = msgs(&f);
    assert!(m[..4].iter().all(|x| x.2 == Msg::Other), "{m:?}");
    assert_eq!(m[4].2, ch(0x90, 60, 100));
}

#[test]
fn format1_merges_in_tick_then_track_order() {
    let f = file(1, D96, |w| {
        for t in 0..3u8 {
            w.track();
            for k in 0..4u64 {
                w.channel(k * 10 + u64::from(t), 0x90 | t, 60 + t, 100);
                w.channel(k * 10 + 5, 0x80 | t, 60 + t, 0);
            }
            w.end_track(50);
        }
    });
    let m = msgs(&f);
    for p in m.windows(2) {
        assert!(p[0].0 < p[1].0 || (p[0].0 == p[1].0 && p[0].1 <= p[1].1), "{:?} then {:?}", p[0], p[1]);
    }
    assert_eq!(m.len(), 3 * 9);
}

#[test]
fn format2_patterns_play_one_after_the_other() {
    let f = file(2, D96, |w| {
        w.track();
        w.tempo(0, 1_000_000);
        w.channel(0, 0x90, 60, 100);
        w.end_track(96); // one quarter at 60 BPM: 1 s
        w.track();
        w.channel(48, 0x90, 62, 100); // half a quarter at 120 BPM (reset)
        w.end_track(96);
    });
    let m = msgs(&f);
    let pat: Vec<_> = m.iter().filter(|x| matches!(x.2, Msg::Pattern(_))).collect();
    assert_eq!(pat.len(), 1);
    assert_eq!(pat[0].0, 96, "pattern 1 starts where pattern 0 ended");
    assert!(m.contains(&(96 + 48, 1, ch(0x90, 62, 100))));
    let (info, _) = summary(&mut SliceSource::new(&f), 48_000, false, |_| {}).unwrap();
    // 1 s of pattern 0 at 60 BPM, then 96 ticks at the default 120 BPM.
    assert_eq!(info.end_frame, 48_000 + 24_000);
}

#[test]
fn smpte_divisions_ignore_tempo() {
    for (fps, tpf, tick, frames) in
        [(25u8, 40u8, 1000u64, 48_000u64), (24, 4, 96, 48_000), (30, 80, 2400, 48_000)]
    {
        let f = file(0, Division::Smpte { fps, tpf }, |w| {
            w.track();
            w.tempo(0, 250_000);
            w.channel(tick, 0x90, 60, 100);
            w.end_track(tick);
        });
        let (info, _) = summary(&mut SliceSource::new(&f), 48_000, false, |_| {}).unwrap();
        assert_eq!(info.division, Division::Smpte { fps, tpf });
        assert_eq!(info.end_frame, frames, "{fps} fps × {tpf}");
    }
    // 29.97 drop frame: 30 frames of 1 tick are 1.001 s.
    let c = Clock::new(48_000, Division::Smpte { fps: 29, tpf: 1 });
    assert_eq!(c.frame(30), 48_048);
    assert_eq!(c.frame(30_000), 48_000 * 1001);
}

#[test]
fn rmid_wrapper() {
    let inner = file(0, D96, |w| {
        w.track();
        w.channel(0, 0x90, 60, 100);
        w.end_track(1);
    });
    let mut f = Vec::new();
    f.extend_from_slice(b"RIFF");
    f.extend_from_slice(&((inner.len() + 12) as u32).to_le_bytes());
    f.extend_from_slice(b"RMIDdata");
    f.extend_from_slice(&(inner.len() as u32).to_le_bytes());
    f.extend_from_slice(&inner);
    assert!(orpheus_midi::probe(&f));
    assert_eq!(msgs(&f)[0].2, ch(0x90, 60, 100));
}

#[test]
fn header_errors() {
    let ok = file(0, D96, |w| {
        w.track();
        w.end_track(0);
    });
    let err = |d: &[u8]| events(d).err();
    assert_eq!(err(b""), Some(MidiError::Truncated));
    assert_eq!(err(b"MTh"), Some(MidiError::Truncated));
    assert_eq!(err(&ok[..10]), Some(MidiError::Truncated));
    assert_eq!(err(b"RIFF\0\0\0\0WAVEfmt "), Some(MidiError::NotMidi));
    let mut f = ok.clone();
    f[9] = 3;
    assert_eq!(err(&f), Some(MidiError::Format(3)));
    let mut f = ok.clone();
    f[12] = 0;
    f[13] = 0;
    assert_eq!(err(&f), Some(MidiError::Division));
    let mut f = ok.clone();
    f[12] = 0xE7; // −25 fps
    f[13] = 0;
    assert_eq!(err(&f), Some(MidiError::Division), "0 ticks per frame");
    f[12] = 0xE6; // −26 fps
    f[13] = 4;
    assert_eq!(err(&f), Some(MidiError::Division));
    assert_eq!(err(&ok[..14]), Some(MidiError::NoTracks));
    // A header longer than 6 bytes: the extra bytes are skipped.
    let mut f = ok[..8].to_vec();
    f[7] = 10;
    f.extend_from_slice(&ok[8..14]);
    f.extend_from_slice(&[9, 9, 9, 9]);
    f.extend_from_slice(&ok[14..]);
    assert_eq!(msgs(&f), [(0, 0, Msg::EndOfTrack)]);
}

#[test]
fn too_many_tracks_are_refused_with_a_message() {
    let f = file(1, D96, |w| {
        for _ in 0..=MAX_TRACKS {
            w.track();
            w.end_track(0);
        }
    });
    let e = events(&f).unwrap_err();
    assert_eq!(e, MidiError::TooManyTracks);
    assert!(e.message().contains("64"), "{}", e.message());
    // The cap itself is fine.
    let f = file(1, D96, |w| {
        for t in 0..MAX_TRACKS {
            w.track();
            w.channel(t as u64, 0x90, 60, 1);
            w.end_track(t as u64 + 1);
        }
    });
    assert_eq!(events(&f).unwrap().0.len(), 2 * MAX_TRACKS);
}

#[test]
fn lengths_are_never_trusted() {
    // Unknown chunks skipped; the header's track count ignored; a track
    // running past the end of the file is cut there and ends without an
    // end-of-track event.
    let mut f = file(1, D96, |w| {
        w.raw(b"JUNK\0\0\0\x03abc");
        w.track();
        w.channel(0, 0x90, 60, 100);
        w.end_track(5);
        w.track();
        w.channel(0, 0x91, 61, 100);
        w.channel(7, 0x81, 61, 0);
        w.end_track(9);
    });
    f[10..12].copy_from_slice(&40u16.to_be_bytes()); // claims 40 tracks
    let cut = f.len() - 6; // inside the last track's note-off
    // The last track claims 1 MiB.
    let at = f.windows(4).rposition(|w| w == b"MTrk").unwrap();
    f[at + 4..at + 8].copy_from_slice(&(1u32 << 20).to_be_bytes());
    let m = msgs(&f[..cut]);
    assert_eq!(m[0], (0, 0, ch(0x90, 60, 100)));
    assert_eq!(m[1], (0, 1, ch(0x91, 61, 100)));
    assert!(m.contains(&(5, 0, Msg::EndOfTrack)));
    assert_eq!(m.iter().filter(|x| x.2 == Msg::EndOfTrack).count(), 2, "{m:?}");
    // A chunk claiming 4 GiB ends the walk.
    let mut g = file(1, D96, |w| {
        w.raw(b"XXXX\xff\xff\xff\xff");
        w.track();
        w.end_track(0);
    });
    g[10] = 0;
    assert_eq!(events(&g).err(), Some(MidiError::NoTracks));
}

#[test]
fn missing_end_of_track_and_truncation_anywhere() {
    let f = orpheus_midi_demo();
    // Cut the file at every length: never a panic, never more events than
    // the whole file has, and the steps stay bounded.
    let (all, _) = events(&f).unwrap();
    for n in 0..f.len() {
        if let Ok((ev, calls)) = events(&f[..n]) {
            assert!(ev.len() <= all.len() + 6);
            assert!(calls <= n + 6 * 3 + 2, "{calls} calls for {n} bytes");
        }
    }
}

fn orpheus_midi_demo() -> Vec<u8> {
    common::demo(orpheus_midi::demo::karaoke)
}

#[test]
fn huge_deltas_cap_the_song() {
    let f = file(0, D96, |w| {
        w.track();
        w.channel(0, 0x90, 60, 100);
        for i in 1..=40u64 {
            w.channel(i * 0x0FFF_FFFF, 0x80, 60, 0);
        }
        w.end_track(40 * 0x0FFF_FFFF);
    });
    let m = msgs(&f);
    assert_eq!(m.last().unwrap().0, 40 * 0x0FFF_FFFF, "ticks add up in 64 bits");
    let (info, _) = summary(&mut SliceSource::new(&f), 48_000, false, |_| {}).unwrap();
    assert_eq!(info.end_frame, 0, "the first event past the cap ends the song");
    // Ten minutes per delta (115 200 ticks at 120 BPM), 200 of them: the
    // song is cut at the last event within 24 h.
    let f = file(0, D96, |w| {
        w.track();
        for i in 0..200u64 {
            w.channel(i * 115_200, 0x90, 60, 100);
        }
        w.end_track(200 * 115_200);
    });
    let (info, _) = summary(&mut SliceSource::new(&f), 48_000, false, |_| {}).unwrap();
    assert_eq!(info.end_frame, 144 * 600 * 48_000, "the event at 24:00:00 is the last");
    assert!(info.end_frame <= MAX_SECONDS * 48_000);
}

#[test]
fn corrupt_bytes_end_only_their_track() {
    for bad in [
        &[0x00, 0xF4, 0x00][..],                           // undefined status
        &[0x00, 0x90, 0x3C, 0x85],                         // data byte with the top bit
        &[0x80, 0x80, 0x80, 0x80, 0x00],                   // five-byte delta
        &[0x00, 0xFF, 0x01, 0x80, 0x80, 0x80, 0x80, 0x00], // five-byte length
    ] {
        let f = file(1, D96, |w| {
            w.track();
            w.channel(0, 0x90, 60, 100);
            w.raw(bad);
            w.raw(&[0, 0x90, 62, 100]);
            w.end_track(10);
            w.track();
            w.channel(20, 0x91, 50, 80);
            w.end_track(30);
        });
        let m = msgs(&f);
        assert_eq!(m[0].2, ch(0x90, 60, 100), "{bad:02x?}");
        assert!(!m.contains(&(0, 0, ch(0x90, 62, 100))), "{bad:02x?}: nothing after the bad bytes");
        assert!(m.contains(&(20, 1, ch(0x91, 50, 80))), "{bad:02x?}");
    }
}

#[test]
fn every_demo_parses_in_bounded_steps() {
    for f in [orpheus_midi::demo::etude, orpheus_midi::demo::karaoke, orpheus_midi::demo::tour] {
        let d = common::demo(f);
        let (ev, calls) = events(&d).unwrap();
        assert!(ev.len() > 100);
        // A window refill per 64 bytes per track at most, plus one call per
        // event.
        assert!(calls <= ev.len() + d.len() / 16 + 64, "{calls} calls for {} events", ev.len());
    }
}
