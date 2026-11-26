//! Playback: timing exact to the sample through tempo changes, golden
//! renders, the GM program sweep, seeking, lyrics, mute/solo, duration and
//! position, block-size independence, RAM, the sound bank.

mod common;

use common::{demo, file, hash, player, render, render_song, rms_db};
use orpheus_midi::kar::{Break, LyricMode};
use orpheus_midi::write::TrackBuf;
use orpheus_midi::{Division, MidiPlayer, Options, SliceSource, demo as songs, gm, summary};

const GOLDEN: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/tests/goldens.txt");
const BANK: &str = concat!(env!("CARGO_MANIFEST_DIR"), "/data/gm-sounds.osb");

fn write_goldens() -> bool {
    std::env::var_os("ORPHEUS_WRITE_GOLDENS").is_some()
}

/// Exact frame of `tick` under the tempo list `(tick, µs)`, computed
/// segment by segment as one rational (independent of the clock's code).
fn exact_frame(tick: u64, tempos: &[(u64, u32)], tpq: u64, rate: u64) -> u64 {
    let mut num: u128 = 0;
    let mut at = 0u64;
    let mut us = 500_000u128;
    for &(t, u) in tempos {
        if t >= tick {
            break;
        }
        num += u128::from(t - at) * us * u128::from(rate);
        at = t;
        us = u128::from(u);
    }
    num += u128::from(tick - at) * us * u128::from(rate);
    (num / (u128::from(tpq) * 1_000_000)) as u64
}

/// Notes at awkward ticks under awkward tempo changes land on the floor of
/// their exact time, and the rendered audio is bit-identical to the same
/// notes placed by hand on those frames (a file whose ticks are frames).
#[test]
fn tempo_changes_are_exact_to_the_sample() {
    let tpq = 96u64;
    let tempos =
        [(0u64, 500_000u32), (37, 433_333), (101, 1_234_567), (250, 342_270), (251, 600_001), (700, 3_000)];
    let notes: Vec<u64> =
        vec![0, 1, 36, 37, 38, 100, 101, 102, 249, 250, 251, 252, 400, 699, 700, 701, 900, 1300];
    for rate in [44_100u64, 48_000] {
        let f = file(1, Division::Metrical(tpq as u16), |w| {
            w.track();
            for &(t, us) in &tempos {
                w.tempo(t, us);
            }
            w.end_track(1400);
            w.track();
            for (i, &t) in notes.iter().enumerate() {
                w.channel(t, 0x90, 60 + (i as u8 % 12), 100);
                w.channel(t + 1, 0x80, 60 + (i as u8 % 12), 0);
            }
            w.end_track(1400);
        });
        let (ev, _) = common::events(&f).unwrap();
        // The clock as the player runs it.
        let mut c = orpheus_midi::timing::Clock::new(rate as u32, Division::Metrical(tpq as u16));
        let mut seen = 0;
        for e in &ev {
            let fr = c.frame(e.tick);
            if let orpheus_midi::Msg::Tempo(us) = e.msg {
                c.set_tempo(e.tick, us);
            }
            if let orpheus_midi::Msg::Channel { status: 0x90, .. } = e.msg {
                assert_eq!(fr, exact_frame(e.tick, &tempos[1..], tpq, rate), "tick {} at {rate}", e.tick);
                seen += 1;
            }
        }
        assert_eq!(seen, notes.len());
    }
    // Audio: the same notes on their frames in a file where a tick is a
    // frame (480 ticks per quarter at 10 ms per quarter, 48 kHz).
    let rate = 48_000u64;
    let notes_file = |frames: &dyn Fn(u64) -> u64, tempo_track: bool, division: u16| {
        file(1, Division::Metrical(division), |w| {
            w.track();
            if tempo_track {
                for &(t, us) in &tempos {
                    w.tempo(t, us);
                }
            } else {
                w.tempo(0, 10_000);
            }
            w.end_track(frames(1400));
            w.track();
            w.channel(0, 0xC0, 81, 0); // saw lead
            w.channel(0, 0xB0, 91, 0);
            for (i, &t) in notes.iter().enumerate() {
                w.channel(frames(t), 0x90, 60 + (i as u8 % 12), 100);
                w.channel(frames(t + 1), 0x80, 60 + (i as u8 % 12), 0);
            }
            w.end_track(frames(1400));
        })
    };
    let a = notes_file(&|t| t, true, tpq as u16);
    let b = notes_file(&|t| exact_frame(t, &tempos[1..], tpq, rate), false, 480);
    let mut o = Options::new(48_000);
    o.effects = false;
    let ra = render_song(&a, o, false);
    let mut pb = player(&b, o, false);
    let rb = render(&mut pb, &b, ra.len() / 2, 1152);
    assert!(ra.iter().any(|&s| s != 0));
    assert_eq!(ra.len(), rb.len());
    assert!(
        ra == rb,
        "renders differ at frame {:?}",
        ra.iter().zip(&rb).position(|(x, y)| x != y).map(|i| i / 2)
    );
}

/// The golden renders: FNV-1a of the first seconds of each demo song.
#[test]
fn golden_renders() {
    type Song = fn(&mut [u8]) -> Option<usize>;
    let cases: [(&str, Song, bool, u32, u32); 4] = [
        ("etude-48k", songs::etude, false, 48_000, 8),
        ("etude-44k", songs::etude, false, 44_100, 4),
        ("karaoke-44k", songs::karaoke, true, 44_100, 8),
        ("tour-48k", songs::tour, false, 48_000, 12),
    ];
    let mut table = String::new();
    for (name, f, kar, rate, secs) in cases {
        let d = demo(f);
        let mut p = player(&d, Options::new(rate), kar);
        let out = render(&mut p, &d, (rate * secs) as usize, 1152);
        let peak = out.iter().map(|s| s.unsigned_abs()).max().unwrap();
        assert!(peak > 3000, "{name}: peak {peak}");
        table.push_str(&format!("{name} {:016x} {}\n", hash(&out), out.len() / 2));
    }
    if write_goldens() {
        std::fs::write(GOLDEN, &table).unwrap();
        return;
    }
    let want = std::fs::read_to_string(GOLDEN).expect("tests/goldens.txt: run with ORPHEUS_WRITE_GOLDENS=1");
    assert_eq!(table, want, "golden renders differ (listen, then ORPHEUS_WRITE_GOLDENS=1)");
}

/// Rendering in any block sizes gives the same samples.
#[test]
fn render_is_block_size_independent() {
    let d = demo(songs::etude);
    let frames = 48_000 * 5;
    let whole = {
        let mut p = player(&d, Options::new(48_000), false);
        render(&mut p, &d, frames, frames)
    };
    for block in [1usize, 7, 333, 1152, 4096] {
        let mut p = player(&d, Options::new(48_000), false);
        let part = render(&mut p, &d, frames, block);
        assert!(part == whole, "block {block}");
    }
}

/// Every GM program at the extremes of its range and in a six-note chord,
/// at full velocity with full reverb and chorus sends, then every drum
/// kit's notes: finite, never at full scale (the limiter soft-clips from
/// −1.9 dBFS), and audible. Prints the programs that reached the limiter.
#[test]
fn gm_program_sweep_renders_clean() {
    let f = file(0, Division::Metrical(480), |w| {
        w.track();
        w.tempo(0, 500_000);
        let mut t = TrackBuf::<4096>::new();
        for p in 0..128u64 {
            let b = p * 960;
            let ch = (p % 2) as u8;
            t.program(b, ch, p as u8);
            t.cc(b, ch, 91, 127);
            t.cc(b, ch, 93, 127);
            for (i, n) in [24u8, 60, 108].iter().enumerate() {
                t.note(b + i as u64 * 120, 110, ch, *n, 127);
            }
            for n in [48u8, 55, 60, 64, 67, 72] {
                t.note(b + 400, 500, ch, n, 127);
            }
        }
        let k = 128 * 960;
        for (i, prog) in [0u8, 16, 25, 32, 48].iter().enumerate() {
            let b = k + i as u64 * 47 * 60;
            t.program(b, 9, *prog);
            for (j, n) in (35..=81u8).enumerate() {
                t.note(b + j as u64 * 60, 60, 9, n, 127);
            }
        }
        t.write(w);
        w.end_track(k + 5 * 47 * 60 + 960);
    });
    let mut p = player(&f, Options::new(48_000), false);
    let mut src = SliceSource::new(&f);
    let mut buf = vec![0f32; 2 * 24_000];
    let mut peak_seg = Vec::new();
    let mut limited = Vec::new();
    loop {
        let n = p.render_f32_from(&mut src, &mut buf).unwrap();
        if n == 0 {
            break;
        }
        let mut pk = 0f32;
        for &s in &buf[..2 * n] {
            assert!(s.is_finite(), "NaN or infinity at frame {}", p.position());
            assert!(s.abs() < 1.0, "{s} at frame {} (program {})", p.position(), p.position() / 48_000);
            pk = pk.max(s.abs());
        }
        peak_seg.push(pk);
        if p.engine().limiter_gain() < 1.0 {
            limited.push((p.position() / 48_000, p.engine().limiter_gain()));
        }
    }
    println!("limiter engaged (program, gain): {limited:?}");
    assert!(limited.len() < 16, "the limiter is for mixes, not single parts");
    // Every program's second (2 per program at 0.5 s blocks) is audible.
    for (prog, w) in peak_seg.chunks(2).take(128).enumerate() {
        let pk = w.iter().copied().fold(0.0, f32::max);
        assert!(pk > 0.01, "program {prog}: peak {pk}");
    }
}

/// A file with two phrases and a silence between them, on patches without
/// noise, glide or LFO (a voice slot's LFO runs on from note to note, so its
/// phase depends on the slot's history), reverb sends 0.
fn two_phrases() -> Vec<u8> {
    file(1, Division::Metrical(480), |w| {
        w.track();
        w.tempo(0, 500_000); // a quarter is 0.5 s
        w.end_track(14 * 480);
        let mut t = TrackBuf::<512>::new();
        for (ch, prog) in [(0u8, 0u8), (1, 19), (2, 33)] {
            t.program(0, ch, prog);
            t.cc(0, ch, 91, 0);
        }
        for phrase in [0u64, 10 * 480] {
            for i in 0..8u64 {
                t.note(phrase + i * 240, 200, 0, 60 + (i * 3 % 12) as u8, 100);
            }
            t.note(phrase, 4 * 480, 1, 67, 80);
            t.note(phrase, 4 * 480, 1, 72, 80);
            t.cc(phrase + 960, 1, 7, 70);
            t.cc(phrase + 1200, 1, 10, 20);
            t.bend(phrase + 1000, 1, 2000);
            t.note(phrase, 960, 2, 36, 100);
            t.note(phrase + 960, 960, 2, 43, 100);
        }
        w.track();
        t.write(w);
        w.end_track(14 * 480);
    })
}

/// Seeking into the silence between phrases gives exactly the samples of
/// playing from the start; seeking into a phrase re-strikes the held notes
/// and is exact again from the next phrase on.
#[test]
fn seek_gives_the_same_output_as_playing_from_the_start() {
    let d = two_phrases();
    let rate = 48_000u64;
    let full = render_song(&d, Options::new(48_000), false);
    let total = full.len() / 2;
    // Into the gap (phrase 1 ends at 2.1 s plus release; phrase 2 at 5 s).
    for target in [4 * rate, 4 * rate + 17, 4 * rate + 31] {
        let mut p = player(&d, Options::new(48_000), false);
        let at = p.seek_from(&mut SliceSource::new(&d), target).unwrap();
        assert_eq!(at, target);
        let rest = render(&mut p, &d, total, 1152);
        assert_eq!(rest.len() / 2, total - target as usize);
        assert!(
            rest[..] == full[2 * target as usize..],
            "seek to {target}: first difference at {:?}",
            rest.iter().zip(&full[2 * target as usize..]).position(|(a, b)| a != b).map(|i| i / 2)
        );
    }
    // Into the first phrase: the held notes sound, and from phrase 2 on the
    // output is the same.
    let target = rate + 100;
    let mut p = player(&d, Options::new(48_000), false);
    p.seek_from(&mut SliceSource::new(&d), target).unwrap();
    assert!(p.engine().active_voices() >= 3, "held notes re-struck: {}", p.engine().active_voices());
    let rest = render(&mut p, &d, total, 1152);
    let t = target as usize;
    let a = rms_db(&rest[..2 * 9600]);
    let b = rms_db(&full[2 * t..2 * t + 2 * 9600]);
    assert!((a - b).abs() < 6.0, "after the seek {a:.1} dB, from the start {b:.1} dB");
    let p2 = 5 * rate as usize;
    assert!(rest[2 * (p2 - t)..] == full[2 * p2..], "phrase 2 differs after a seek into phrase 1");
    // Seek to 0 is the start; past the end ends.
    let mut p = player(&d, Options::new(48_000), false);
    render(&mut p, &d, 30_000, 1152);
    p.seek_from(&mut SliceSource::new(&d), 0).unwrap();
    assert!(render(&mut p, &d, total, 4096) == full);
    p.seek_from(&mut SliceSource::new(&d), u64::MAX).unwrap();
    assert_eq!(p.position(), p.duration());
    assert_eq!(render(&mut p, &d, 100, 100).len(), 0);
}

/// Seeking a real song with drums, effects, vibrato and a sustain pedal:
/// the levels after the seek follow the levels of playing from the start
/// (not the samples: the drums' noise seeds, the slots' LFO phases, the
/// effect tails and the notes under the pedal differ). Without the drums
/// within 1 dB per half second; with them within 3 dB per second.
#[test]
fn seek_in_a_song_follows_the_song() {
    let d = demo(songs::etude);
    let target = 48_000 * 9 + 1234;
    for (drums, win, tol) in [(false, 24_000, 1.0), (true, 48_000, 3.0)] {
        let mut q = player(&d, Options::new(48_000), false);
        q.set_mute(9, !drums);
        let full = render(&mut q, &d, 48_000 * 16, 1152);
        let mut p = player(&d, Options::new(48_000), false);
        p.set_mute(9, !drums);
        p.seek_from(&mut SliceSource::new(&d), target as u64).unwrap();
        let rest = render(&mut p, &d, 48_000 * 6, 1152);
        for w in 0..6 * 48_000 / win {
            let r = 2 * w * win;
            let a = rms_db(&rest[r..r + 2 * win]);
            let b = rms_db(&full[2 * target + r..2 * target + r + 2 * win]);
            assert!((a - b).abs() < tol, "drums {drums}, window {w}: {a:.2} vs {b:.2} dB");
        }
    }
}

/// A seek re-scans in bounded steps: each `step` handles at most its
/// budget of events.
#[test]
fn seek_steps_are_bounded() {
    let d = demo(songs::tour);
    let mut p = player(&d, Options::new(48_000), false);
    let mut src = SliceSource::new(&d);
    p.seek(48_000 * 40).unwrap();
    let mut steps = 0;
    loop {
        match p.step(4).unwrap() {
            orpheus_midi::Pull::Need(n) => {
                let mut b = [0u8; 64];
                let k = orpheus_midi::MidiSource::read_at(&mut src, n.offset, &mut b[..usize::from(n.len)])
                    .unwrap();
                p.supply(&b[..k]);
            }
            orpheus_midi::Pull::Ready(true) => break,
            _ => steps += 1,
        }
        assert!(steps < 10_000);
    }
    // ≈ 470 events before 0:40 in steps of 4 (a window refill also ends
    // a step).
    assert!(steps >= 50, "{steps}");
    assert_eq!(p.position(), 48_000 * 40);
}

/// Karaoke lyrics: every syllable at the exact frame of its tick, with its
/// breaks; the title from `@T`; the same pieces from the player as from
/// the summary. A tempo change moves the later syllables exactly.
#[test]
fn kar_lyric_timing() {
    let d = demo(songs::karaoke);
    let rate = 48_000u64;
    let mut got = Vec::new();
    let (info, _) = summary(&mut SliceSource::new(&d), rate as u32, true, |l| {
        got.push((l.frame, l.before, l.text().to_vec()));
    })
    .unwrap();
    assert_eq!(info.lyrics, LyricMode::Kar);
    assert_eq!(info.title(), b"Hill Song");
    assert_eq!(got.len(), songs::LYRICS.len());
    // 100 BPM, 480 ticks per quarter: a tick is 60 frames at 48 kHz.
    let mut tick = songs::KAR_INTRO_BARS * 4 * 480;
    for (g, &(s, len, _)) in got.iter().zip(&songs::LYRICS) {
        assert_eq!(g.0, tick * 60, "{s}");
        let (brk, text) = match s.as_bytes() {
            [b'\\', t @ ..] => (Break::Paragraph, t),
            [b'/', t @ ..] => (Break::Line, t),
            t => (Break::None, t),
        };
        assert_eq!((g.1, &g.2[..]), (brk, text));
        tick += len * 240;
    }
    // The player queues the same pieces as it reaches them.
    let mut p = player(&d, Options::new(48_000), true);
    let mut src = SliceSource::new(&d);
    let mut buf = vec![0i16; 2 * 1152];
    let mut played = Vec::new();
    while p.render_from(&mut src, &mut buf).unwrap() > 0 {
        while let Some(l) = p.pop_lyric() {
            assert!(l.frame < p.position() && l.frame + 1152 >= p.position(), "queued as reached");
            played.push((l.frame, l.before, l.text().to_vec()));
        }
    }
    assert_eq!(played, got);
    // Lyric meta events (not karaoke) with a tempo change half-way.
    let f = file(0, Division::Metrical(96), |w| {
        w.track();
        w.meta(0, 0x05, b"one ");
        w.meta(96, 0x05, b"two\r");
        w.tempo(144, 250_000);
        w.meta(192, 0x05, b"three");
        w.meta(195, 0x01, b"not sung");
        w.end_track(200);
    });
    let mut got = Vec::new();
    let (info, _) =
        summary(&mut SliceSource::new(&f), 44_100, false, |l| got.push((l.frame, l.after))).unwrap();
    assert_eq!(info.lyrics, LyricMode::Lyric);
    // 144 ticks at 120 BPM = 0.75 s, then 48 ticks at 240 BPM = 0.125 s.
    assert_eq!(got, [(0, Break::None), (22_050, Break::Line), (33_075 + 5_512, Break::None)]);
}

/// Mute and solo: soloing a channel equals muting all the others; muting
/// everything is silence; muting mid-song releases the channel's notes.
#[test]
fn mute_and_solo() {
    let d = demo(songs::etude);
    let n = 48_000 * 4;
    let mut a = player(&d, Options::new(48_000), false);
    a.set_solo(1, true);
    let ra = render(&mut a, &d, n, 1152);
    let mut b = player(&d, Options::new(48_000), false);
    for c in (0..16).filter(|&c| c != 1) {
        b.set_mute(c, true);
    }
    let rb = render(&mut b, &d, n, 1152);
    assert!(ra == rb);
    assert!(rms_db(&ra) > -40.0, "the bass sounds: {:.1}", rms_db(&ra));
    let full = {
        let mut p = player(&d, Options::new(48_000), false);
        render(&mut p, &d, n, 1152)
    };
    assert!(ra != full);
    let mut c = player(&d, Options::new(48_000), false);
    for ch in 0..16 {
        c.set_mute(ch, true);
    }
    assert!(render(&mut c, &d, n, 1152).iter().all(|&s| s == 0));
    // Mute the piano after 2 s: it fades within its release.
    let mut p = player(&d, Options::new(48_000), false);
    p.set_solo(0, true);
    render(&mut p, &d, 96_000, 1152);
    p.set_mute(0, true);
    let after = render(&mut p, &d, 48_000, 1152);
    assert!(rms_db(&after[2 * 24_000..]) < -70.0, "{:.1}", rms_db(&after[2 * 24_000..]));
    assert!(p.sequencer().mutes() & 1 == 1 && p.sequencer().solos() == 1);
}

/// Duration and position: the song is the last event plus the tail, the
/// position counts frames, `render` ends exactly at the duration.
#[test]
fn duration_and_position() {
    let d = two_phrases();
    let mut p = player(&d, Options::new(44_100), false);
    let info = *p.info().unwrap();
    // 14 quarters at 120 BPM = 7 s, + 2 s.
    assert_eq!(info.end_frame, 7 * 44_100);
    assert_eq!(p.duration(), 9 * 44_100);
    assert_eq!(info.duration_ms(), 9000);
    assert_eq!(info.channels, 0b111);
    let mut src = SliceSource::new(&d);
    let mut buf = vec![0i16; 2 * 1000];
    let mut total = 0;
    loop {
        let n = p.render_from(&mut src, &mut buf).unwrap();
        total += n;
        assert_eq!(p.position(), total as u64);
        if n == 0 {
            break;
        }
    }
    assert_eq!(total as u64, 9 * 44_100);
    // Pausing is not calling: a pause between blocks changes nothing.
    let whole = render_song(&d, Options::new(44_100), false);
    assert_eq!(whole.len() / 2, total);
}

/// The player's RAM, and that it fits the 72 KiB decoder arena.
#[test]
fn ram_table() {
    use orpheus_midi::ram;
    println!(
        "player {} B (engine {}, parser {} = {} tracks × ≈{}, sequencer {})",
        ram::player(),
        ram::engine(),
        ram::smf(),
        orpheus_midi::smf::MAX_TRACKS,
        ram::smf() / orpheus_midi::smf::MAX_TRACKS,
        ram::sequencer()
    );
    assert!(ram::player() <= 72 * 1024, "{} B", ram::player());
}

/// The sound set as an `OSB1` bank: the committed file is current and
/// decodes to the very patches.
#[test]
fn sound_bank_round_trips() {
    use orpheus_synth::bank::{Record, records};
    let mut buf = vec![0u8; 32 * 1024];
    let n = gm::write_bank(&mut buf).unwrap();
    buf.truncate(n);
    if write_goldens() {
        std::fs::write(BANK, &buf).unwrap();
    }
    let on_disk = std::fs::read(BANK).expect("data/gm-sounds.osb: run with ORPHEUS_WRITE_GOLDENS=1");
    assert!(on_disk == buf, "data/gm-sounds.osb is stale: ORPHEUS_WRITE_GOLDENS=1");
    let mut patches = 0;
    let mut kits = 0;
    for r in records(&buf).unwrap() {
        match r.unwrap() {
            Record::Patch(name, p) => {
                let (want_name, want) = &gm::SET[patches];
                assert_eq!(name.as_str(), *want_name);
                assert_eq!(p, want.sanitized());
                patches += 1;
            }
            Record::Kit(..) => kits += 1,
        }
    }
    assert_eq!((patches, kits), (gm::SOUNDS, orpheus_synth::kit::FACTORY.len()));
    println!("bank: {n} bytes");
}

/// Program changes install a sound only when the sound changes (programs
/// of a family share one), and channel 10 switches kits.
#[test]
fn programs_and_kits() {
    let f = file(0, Division::Metrical(96), |w| {
        w.track();
        w.channel(0, 0xC0, 33, 0); // finger bass
        w.channel(1, 0xC0, 34, 0); // pick bass: same sound
        w.channel(2, 0xC9, 25, 0); // drum channel: 808 → analog
        w.channel(3, 0x90, 40, 100);
        w.channel(3, 0x99, 36, 100);
        w.end_track(96);
    });
    let mut p = player(&f, Options::new(48_000), false);
    render(&mut p, &f, 4800, 1152);
    assert_eq!(p.sequencer().sound(0), 12);
    assert_eq!(p.sequencer().channel(0).program, 34);
    assert_eq!(p.engine().kit(0), Some(&gm::kit(1)));
    let patch = p.engine().patch(0).unwrap();
    assert_eq!(patch.bend_range, 24.0);
    assert!(p.engine().active_voices() >= 2);
}

/// The player renders the same at any engine block cut even when program
/// changes fall inside blocks (they cut the block).
#[test]
fn program_change_inside_a_block() {
    let d = demo(songs::tour);
    let frames = 48_000 * 6;
    let a = {
        let mut p = player(&d, Options::new(48_000), false);
        render(&mut p, &d, frames, frames)
    };
    let b = {
        let mut p = player(&d, Options::new(48_000), false);
        render(&mut p, &d, frames, 777)
    };
    assert!(a == b);
}

/// A boxed player is what the tests use; check the in-place reopen works.
#[test]
fn reopen_another_file() {
    let a = demo(songs::etude);
    let b = demo(songs::karaoke);
    let mut p = Box::new(MidiPlayer::new(Options::new(48_000)));
    p.open_from(&mut SliceSource::new(&a), false).unwrap();
    render(&mut p, &a, 10_000, 1152);
    let info = p.open_from(&mut SliceSource::new(&b), true).unwrap();
    assert_eq!(info.lyrics, LyricMode::Kar);
    let fresh = {
        let mut q = player(&b, Options::new(48_000), true);
        render(&mut q, &b, 48_000, 1152)
    };
    assert!(render(&mut p, &b, 48_000, 1152) == fresh);
}

/// Regression (MIDI-1, 2026-11-25): the first clock re-anchored at each
/// tempo change on the floored frame, so every change could move later
/// events up to a frame early; a file of many tempo changes drifted by
/// hundreds of frames. Now the anchor keeps the exact time.
#[test]
fn regress_tempo_changes_drifted_by_a_frame_each() {
    let tpq = 96u64;
    let tempos: Vec<(u64, u32)> = (1..=600u64).map(|i| (i, 500_001 + (i as u32 % 7) * 13)).collect();
    let f = file(0, Division::Metrical(tpq as u16), |w| {
        w.track();
        for &(t, us) in &tempos {
            w.tempo(t, us);
        }
        w.channel(601, 0x90, 60, 100);
        w.end_track(602);
    });
    let (ev, _) = common::events(&f).unwrap();
    let mut c = orpheus_midi::timing::Clock::new(44_100, Division::Metrical(tpq as u16));
    for e in &ev {
        let fr = c.frame(e.tick);
        if let orpheus_midi::Msg::Tempo(us) = e.msg {
            c.set_tempo(e.tick, us);
        }
        if let orpheus_midi::Msg::Channel { .. } = e.msg {
            assert_eq!(fr, exact_frame(601, &tempos, tpq, 44_100));
        }
    }
}
