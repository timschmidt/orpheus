//! Export (SEQ-2): the WAV export (its header, the same samples in any
//! piece size, the same as Play for the song's length, a fixed hash of
//! the demo song) and the MIDI export (exact ticks against the model,
//! parsed back by `orpheus-midi` equal to what was played, the file's
//! structure).

mod common;

use common::{fnv, project};
use orpheus_midi::{Division, MidiEvent, MidiSource, Msg, Pull, SliceSource, Smf};
use orpheus_seq::export::{self, TICKS_PER_STEP, WavExport};
use orpheus_seq::model::TRACKS;
use orpheus_seq::player::{UNITS_PER_STEP, step_units};
use orpheus_seq::{PlayMode, Project, Sequencer, Step, StepLen, SynthStep, demo};
use orpheus_synth::Engine;

fn wav(p: &Project, rate: u32, piece: &[usize]) -> (Vec<u8>, Vec<i16>) {
    let mut x = WavExport::new(p, rate, StepLen::Sixteenth);
    let mut e = Box::new(Engine::new(rate, 1));
    let mut out = vec![0i16; 2 * x.total_frames() as usize];
    let mut at = 0;
    let mut k = 0;
    while !x.finished() {
        let n = piece[k % piece.len()];
        let to = (2 * (at + n)).min(out.len());
        let got = x.render(p, &mut e, &mut out[2 * at..to]);
        at += got;
        k += 1;
    }
    assert_eq!(at as u64, x.total_frames());
    (x.header().to_vec(), out)
}

/// The header says what follows; the song plus two seconds of tails.
#[test]
fn wav_header_and_length() {
    let mut p = Box::new(Project::NEW);
    demo::song(&mut p);
    let rate = 22_050;
    let (h, s) = wav(&p, rate, &[1 << 20]);
    let frames = export::song_frames(&p, rate, StepLen::Sixteenth) + 2 * u64::from(rate);
    assert_eq!(s.len() as u64, 2 * frames);
    assert_eq!(&h[0..4], b"RIFF");
    assert_eq!(&h[8..16], b"WAVEfmt ");
    assert_eq!(u16::from_le_bytes([h[22], h[23]]), 2, "stereo");
    assert_eq!(u32::from_le_bytes([h[24], h[25], h[26], h[27]]), rate);
    assert_eq!(u16::from_le_bytes([h[34], h[35]]), 16);
    assert_eq!(u32::from_le_bytes([h[40], h[41], h[42], h[43]]) as usize, 2 * s.len());
    assert_eq!(u32::from_le_bytes([h[4], h[5], h[6], h[7]]) as usize, 36 + 2 * s.len());
    // One pass: A A B B C C (32 steps each) A B D.
    assert_eq!(export::song_steps(&p), 16 * 7 + 32 * 2);
    // The tails die out: the last tenth of a second is silent.
    let tail = &s[s.len() - 2 * rate as usize / 10..];
    assert!(tail.iter().all(|x| x.unsigned_abs() < 40), "quiet at the end");
}

/// Any piece size gives the same file, and the song part is what Play
/// plays (rendered on the transport and stopped at the song's end).
#[test]
fn wav_export_is_what_play_plays_in_any_pieces() {
    let mut p = Box::new(Project::NEW);
    demo::song(&mut p);
    let rate = 16_000;
    let (_, a) = wav(&p, rate, &[1 << 20]);
    let (_, b) = wav(&p, rate, &[1, 997, 4096, 333]);
    assert!(a == b, "piece sizes");
    let song = export::song_frames(&p, rate, StepLen::Sixteenth) as usize;
    let mut e = Box::new(Engine::new(rate, 1));
    let mut s = Sequencer::new(rate);
    s.play();
    let mut out = vec![0i16; 2 * song];
    s.render_i16(&p, &mut e, &mut out);
    assert!(out[..] == a[..2 * song], "the song as Play renders it");
}

/// 05 §7.16 "render of a saved song is bit-identical", for the export:
/// the demo song's WAV at 22,050 Hz has this hash (any change of the
/// engine, the player or the song shows here; bless by replacing it).
#[test]
fn wav_export_hash() {
    let mut p = Box::new(Project::NEW);
    demo::song(&mut p);
    let (_, s) = wav(&p, 22_050, &[4096]);
    let h = fnv(&s);
    std::println!("demo song WAV at 22,050 Hz: {} frames, FNV {h:#018x}", s.len() / 2);
    assert_eq!(h, WAV_HASH, "demo song WAV hash");
    // And the saved song renders the same.
    let q = common::loaded(&common::saved(&p)).unwrap();
    assert_eq!(fnv(&wav(&q, 22_050, &[1000]).1), h);
}

const WAV_HASH: u64 = 0xf3cd_9d8f_0d17_6097;

// ------------------------------------------------------------ MIDI

/// Every event of `data` in order.
fn events(data: &[u8]) -> (orpheus_midi::smf::Header, Vec<MidiEvent>) {
    let mut src = SliceSource::new(data);
    let mut smf = Smf::new();
    smf.begin(src.len());
    let mut buf = [0u8; 64];
    let h = loop {
        match smf.open().expect("opens") {
            Pull::Need(n) => {
                let k = src.read_at(n.offset, &mut buf[..usize::from(n.len)]).unwrap();
                smf.supply(&buf[..k]);
            }
            Pull::Ready(h) => break h,
            Pull::End => unreachable!(),
        }
    };
    let mut out = Vec::new();
    loop {
        match smf.next_event() {
            Pull::Need(n) => {
                let k = src.read_at(n.offset, &mut buf[..usize::from(n.len)]).unwrap();
                smf.supply(&buf[..k]);
            }
            Pull::Ready(e) => out.push(e),
            Pull::End => return (h, out),
        }
    }
}

fn midi(p: &Project, len: StepLen) -> Vec<u8> {
    let mut buf = vec![0u8; 1 << 20];
    let n = export::midi(p, len, &mut buf).expect("fits");
    buf.truncate(n);
    buf
}

/// (tick, track, status, d1, d2) of the channel messages.
fn channel(ev: &[MidiEvent]) -> Vec<(u64, u8, u8, u8, u8)> {
    ev.iter()
        .filter_map(|e| match e.msg {
            Msg::Channel { status, d1, d2 } => Some((e.tick, e.track, status, d1, d2)),
            _ => None,
        })
        .collect()
}

/// Ticks are exact: with swing, delays, ratchets and gates every note
/// lands on the tick the model says (a step is 600 ticks; computed here
/// from the steps' lengths), at each step length.
#[test]
fn midi_ticks_are_exact() {
    for len in [StepLen::Sixteenth, StepLen::Eighth, StepLen::ThirtySecond] {
        let mut p = project();
        p.set_tempo(11_337);
        p.set_swing(63);
        p.set_len(0, 7);
        p.set_step(0, 0, 0, Step::hit(100));
        p.set_step(0, 1, 1, Step::hit(90).with_micro(5));
        p.set_step(0, 3, 3, Step::hit(80).with_ratchet(3));
        p.set_synth_step(0, 0, 2, SynthStep::note(40, 70).with_gate(6));
        let s = SynthStep::note(64, 60);
        p.set_synth_step(0, 1, 5, s.with_common(s.common().with_micro(12)).with_gate(1));
        let (h, ev) = events(&midi(&p, len));
        assert_eq!(h.division, Division::Metrical((TICKS_PER_STEP * u64::from(len.beat_steps())) as u16));
        let starts: Vec<u64> = (0..8)
            .scan(0u64, |u, k| {
                let s = *u;
                *u += step_units(63, k);
                Some(s)
            })
            .collect();
        let t = |units: u64| units * TICKS_PER_STEP / UNITS_PER_STEP;
        let on: Vec<(u64, u8)> =
            channel(&ev).iter().filter(|e| e.2 & 0xF0 == 0x90).map(|e| (e.0, e.3)).collect();
        let d3 = step_units(63, 3);
        let want = vec![
            (t(starts[0]), 36),
            (t(starts[1] + 5 * 1200), 38),
            (t(starts[2]), 40),
            (t(starts[3]), 42),
            (t(starts[3] + d3 / 3), 42),
            (t(starts[3] + 2 * d3 / 3), 42),
            (t(starts[5] + 12 * 1200), 64),
        ];
        let mut on = on;
        on.sort();
        let mut want = want;
        want.sort();
        assert_eq!(on, want, "{len:?}");
        // The bass note's end: 12 quarter steps after its start.
        let off = channel(&ev).into_iter().find(|e| e.2 == 0x80 && e.3 == 40).unwrap();
        assert_eq!(off.0, t(starts[2] + 12 * 7200));
        // The lead's end: two quarter steps.
        let off = channel(&ev).into_iter().find(|e| e.2 == 0x81 && e.3 == 64).unwrap();
        assert_eq!(off.0, t(starts[5] + 12 * 1200 + 2 * 7200));
    }
}

/// The demo song exported and parsed back by `orpheus-midi`: format 1,
/// four tracks (conductor, drums on channel 10, bass, lead), the tempo,
/// the names, programs and volumes; every note the dry run plays, at its
/// tick, with its velocity, in each track; every note ended.
#[test]
fn midi_export_parses_back_equal() {
    let mut p = Box::new(Project::NEW);
    demo::song(&mut p);
    let data = midi(&p, StepLen::Sixteenth);
    let (h, ev) = events(&data);
    assert_eq!((h.format, h.declared_tracks), (1, 4));
    assert_eq!(h.division, Division::Metrical(2400));
    assert!(ev.iter().any(|e| e.msg == Msg::Tempo(576_923)), "104 BPM");
    let names: Vec<&[u8]> = ev
        .iter()
        .filter(|e| matches!(e.msg, Msg::Text(orpheus_midi::smf::TextKind::TrackName)))
        .map(|e| e.text())
        .collect();
    assert_eq!(names, [&b"Tram Lights"[..], b"Drums", b"Bass", b"Lead"]);
    let ch = channel(&ev);
    assert!(ch.contains(&(0, 2, 0xC0, 38, 0)), "acid bass: Synth Bass 1");
    assert!(ch.contains(&(0, 3, 0xC1, 4, 0)), "the FM piano: Electric Piano 1");
    assert!(ch.contains(&(0, 2, 0xB0, 7, 95)), "the bass's level 75 %");
    for (track, keep) in [(1u8, None), (2, Some(TRACKS as u8)), (3, Some(TRACKS as u8 + 1))] {
        let mut want: Vec<(u64, u8, u8)> = Vec::new();
        export::notes(
            &p,
            StepLen::Sixteenth,
            |t| keep.map_or(usize::from(t) < TRACKS, |k| t == k),
            |tick, hit| {
                if hit.velocity > 0 {
                    want.push((tick, hit.note, hit.velocity));
                }
            },
        );
        let got: Vec<(u64, u8, u8)> = ch
            .iter()
            .filter(|e| e.1 == track && e.2 & 0xF0 == 0x90 && e.4 > 0)
            .map(|e| (e.0, e.3, e.4))
            .collect();
        assert_eq!(got, want, "track {track}");
        assert!(!got.is_empty());
        // Every note-on ended, never two at once of a note.
        let mut on = std::collections::BTreeSet::new();
        for e in ch.iter().filter(|e| e.1 == track && e.2 & 0xE0 == 0x80) {
            if e.2 & 0xF0 == 0x90 && e.4 > 0 {
                assert!(on.insert(e.3), "track {track}: note {} again before its end at {}", e.3, e.0);
            } else {
                assert!(on.remove(&e.3), "track {track}: an end without a start");
            }
        }
        assert!(on.is_empty(), "track {track}: every note ends");
    }
    // The drums on channel 10 only, the synth tracks on 1 and 2.
    assert!(ch.iter().all(|e| match e.1 {
        1 => e.2 & 0x0F == 9,
        2 => e.2 & 0x0F == 0,
        3 => e.2 & 0x0F == 1,
        _ => false,
    }));
    // The end: one pass (song steps × 600 ticks).
    let end = ev.iter().filter(|e| e.msg == Msg::EndOfTrack).map(|e| e.tick).max().unwrap();
    assert!(end >= export::song_steps(&p) * TICKS_PER_STEP);
}

/// One pass: in song mode the song, else the current pattern once; a
/// buffer too small says so.
#[test]
fn midi_export_bounds() {
    let mut p = Box::new(Project::NEW);
    demo::song(&mut p);
    assert_eq!(export::song_steps(&p), 176);
    p.set_mode(PlayMode::Pattern);
    p.set_current(2);
    assert_eq!(export::song_steps(&p), 32);
    let mut small = [0u8; 100];
    assert_eq!(export::midi(&p, StepLen::Sixteenth, &mut small), Err(export::ExportError::Full));
    let empty = midi(&project(), StepLen::Sixteenth);
    let (h, ev) = events(&empty);
    assert_eq!(h.declared_tracks, 4);
    assert!(channel(&ev).iter().all(|e| e.2 & 0xF0 != 0x90), "no notes");
}
