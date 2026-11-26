//! The project file: exact round trips, sizes, and refusal of anything
//! damaged, newer or impossible; forward compatibility (unknown records,
//! longer records) and defaults for shorter ones.

mod common;

use common::{data, loaded, random, random_drums, same, saved};
use orpheus_seq::file::{self, FileError, MAX_FILE, VERSION, crc32};
use orpheus_seq::model::{LOCKS, MAX_STEPS, PATTERNS, SYNTH_TRACKS, TRACKS};
use orpheus_seq::{Chord, LockParam, Project, Scale, Step, SynthStep, demo};

fn recrc(mut d: Vec<u8>) -> Vec<u8> {
    let n = d.len() - 4;
    let c = crc32(0, &d[..n]);
    d[n..].copy_from_slice(&c.to_le_bytes());
    d
}

#[test]
fn projects_round_trip_exactly() {
    let mut d = Box::new(Project::NEW);
    demo::beat(&mut d);
    let mut s = Box::new(Project::NEW);
    demo::song(&mut s);
    for p in [Box::new(Project::NEW), d, s] {
        let b = saved(&p);
        let q = loaded(&b).unwrap();
        assert!(same(&p, &q));
        assert_eq!(saved(&q), b, "and saves the same bytes");
    }
    for seed in 1..40 {
        let p = random(seed);
        let b = saved(&p);
        assert!(b.len() <= MAX_FILE);
        let q = loaded(&b).unwrap_or_else(|e| panic!("seed {seed}: {e:?}"));
        assert!(same(&p, &q), "seed {seed}");
        assert_eq!(saved(&q), b);
    }
}

#[test]
fn sizes() {
    let new = saved(&Project::NEW);
    assert!(new.len() < 1000, "a new project: {} B", new.len());
    let mut d = Box::new(Project::NEW);
    demo::beat(&mut d);
    let demo = saved(&d);
    assert!(demo.len() < 3000, "the demo: {} B", demo.len());
    let mut s = Box::new(Project::NEW);
    demo::song(&mut s);
    let song = saved(&s);
    // Every pattern at 64 steps with every step of every track set and a
    // full lock store, a full song, the largest patches (FM).
    let mut p = Box::new(Project::NEW);
    for pat in 0..PATTERNS {
        p.set_len(pat, MAX_STEPS);
        for t in 0..TRACKS {
            for s in 0..MAX_STEPS {
                p.set_step(pat, t, s, Step::hit(7).with_ratchet(3));
            }
        }
        for st in 0..SYNTH_TRACKS {
            for s in 0..MAX_STEPS {
                p.set_synth_step(pat, st, s, SynthStep::note(50, 7));
            }
        }
        for k in 0..LOCKS {
            assert!(p.set_lock(pat, k, 0, LockParam::Tone, 9));
        }
        p.set_transpose(pat, 3);
    }
    p.set_preset(0, 8);
    p.set_preset(1, 8);
    p.set_name("A full project 16");
    while p.song.push(3, 4) {}
    let full = saved(&p);
    assert!(full.len() <= MAX_FILE, "{} > {MAX_FILE}", full.len());
    assert!(full.len() + 700 > MAX_FILE, "MAX_FILE is the real bound ({} of {MAX_FILE})", full.len());
    std::println!(
        "file sizes: new {} B, demo {} B, song {} B, full {} B (MAX_FILE {MAX_FILE})",
        new.len(),
        demo.len(),
        song.len(),
        full.len()
    );
    let mut small = vec![0u8; full.len() - 1];
    assert_eq!(file::save(&p, &mut small), Err(FileError::Full));
}

#[test]
fn damage_is_refused() {
    let mut d = Box::new(Project::NEW);
    demo::beat(&mut d);
    let b = saved(&d);
    for n in 0..b.len() {
        assert!(loaded(&b[..n]).is_err(), "cut at {n}");
    }
    for i in 0..b.len() {
        let mut x = b.clone();
        x[i] ^= 0x01;
        assert!(loaded(&x).is_err(), "flipped byte {i}");
    }
    assert_eq!(loaded(b"OSQ").err(), Some(FileError::Short));
    assert_eq!(loaded(b"RIFF\0\0\0\0\0\0\0\0\0\0").err(), Some(FileError::BadMagic));
    let mut newer = b.clone();
    newer[4..6].copy_from_slice(&(VERSION + 1).to_le_bytes());
    assert_eq!(loaded(&recrc(newer)).err(), Some(FileError::TooNew(VERSION + 1)));
    let mut zero = b.clone();
    zero[4..6].copy_from_slice(&0u16.to_le_bytes());
    assert!(loaded(&recrc(zero)).is_err());
}

#[test]
fn impossible_records_are_refused_or_clamped() {
    let p = Box::new(Project::NEW);
    let b = saved(&p);
    let body_end = b.len() - 4;
    let with = |rec: &[u8]| {
        let mut x = b[..body_end].to_vec();
        x.extend_from_slice(rec);
        x.extend([0; 4]);
        loaded(&recrc(x))
    };
    // A pattern index past 15, length 0 or 65, n < len, steps short.
    assert!(with(&[3, 4, 0, 16, 4, 8, 4]).is_err());
    assert!(with(&[3, 4, 0, 0, 0, 8, 0]).is_err());
    assert!(with(&[3, 4, 0, 0, 65, 8, 65]).is_err());
    assert!(with(&[3, 4, 0, 0, 4, 8, 2]).is_err());
    assert!(with(&[3, 4, 0, 0, 4, 8, 4]).is_err(), "no step bytes");
    // A record running past the end.
    assert!(with(&[3, 0xFF, 0x7F]).is_err());
    // Values out of range are clamped: tempo 1, swing 99, current 200;
    // a track's note 200 and level 255; a song entry of pattern 99 × 0.
    let mut rec = vec![1, 25, 0];
    rec.extend_from_slice(b"Clamped\0\0\0\0\0\0\0\0\0");
    rec.extend([1, 0, 99, 200, 0, 1, 2, 3, 4]);
    let mut tr = vec![2, 4, 0, 1, 200, 255, 3];
    tr.extend([0; 0]);
    let mut x = b[..body_end].to_vec();
    x.extend(&rec);
    x.extend(&tr);
    x.extend([4, 3, 0, 1, 99, 0]);
    x.extend([0; 4]);
    let q = loaded(&recrc(x)).unwrap();
    assert_eq!(q.name(), "Clamped");
    assert_eq!((q.tempo(), q.swing(), q.current()), (4_000, 75, 15));
    assert_eq!((q.track(0).note, q.track(0).level, q.track(0).mute, q.track(0).solo), (127, 100, true, true));
    assert_eq!((q.song.entries()[0].pattern, q.song.entries()[0].repeats), (15, 1));
    assert_eq!(q.seed, 0x0403_0201);
}

/// Forward compatibility: records of unknown tags are skipped, longer
/// bodies keep their known prefix, a shorter project record keeps the
/// defaults it lacks.
#[test]
fn later_versions_can_add_fields_and_records() {
    let mut d = Box::new(Project::NEW);
    demo::beat(&mut d);
    let b = saved(&d);
    let body_end = b.len() - 4;
    // An unknown record in the middle (after the header).
    let mut x = b[..8].to_vec();
    x.extend([0x40, 5, 0, 9, 9, 9, 9, 9]);
    x.extend(&b[8..]);
    let q = loaded(&recrc(x)).unwrap();
    assert!(same(&d, &q));
    // The project record grown by 6 bytes (a later version's fields).
    assert_eq!(b[8], 1);
    let len = usize::from(u16::from_le_bytes([b[9], b[10]]));
    let mut y = b[..8].to_vec();
    y.extend([1]);
    y.extend(((len + 6) as u16).to_le_bytes());
    y.extend(&b[11..11 + len]);
    y.extend([0xAA; 6]);
    y.extend(&b[11 + len..body_end + 4]);
    let q = loaded(&recrc(y)).unwrap();
    assert!(same(&d, &q));
    // A project record with only the name: defaults for the rest.
    let mut z = b[..8].to_vec();
    z.extend([1, 16, 0]);
    z.extend(&b[11..27]);
    z.extend(&b[11 + len..body_end + 4]);
    let q = loaded(&recrc(z)).unwrap();
    assert_eq!(q.name(), "Night Bus");
    assert_eq!((q.tempo(), q.swing()), (12_000, 50), "defaults");
}

#[test]
fn the_name_is_read_without_loading() {
    let mut d = Box::new(Project::NEW);
    demo::beat(&mut d);
    let b = saved(&d);
    assert_eq!(file::peek_name(&b).unwrap().as_str(), "Night Bus");
    let mut bad = b.clone();
    bad[20] ^= 1;
    assert_eq!(file::peek_name(&bad).err(), Some(FileError::BadCrc));
}

/// Migration (SEQ-2): the files SEQ-1 wrote (version 1, committed in
/// `tests/data/` as SEQ-1's code saved them) load unchanged — the same
/// drum project SEQ-1's code built — with a new project's synth tracks,
/// kit B, scale and no locks; saved again they are version 2 and load back
/// the same.
#[test]
fn seq1_files_load_unchanged() {
    let mut beat = Box::new(Project::NEW);
    demo::beat(&mut beat);
    let cases: [(&str, Box<Project>); 4] = [
        ("seq1-new.osq", Box::new(Project::NEW)),
        ("seq1-demo.osq", beat),
        ("seq1-random7.osq", random_drums(7)),
        ("seq1-random42.osq", random_drums(42)),
    ];
    for (name, want) in cases {
        let d = data(name);
        assert_eq!(u16::from_le_bytes([d[4], d[5]]), 1, "{name} is a version-1 file");
        let p = loaded(&d).unwrap_or_else(|e| panic!("{name}: {e:?}"));
        assert!(same(&p, &want), "{name} loads as SEQ-1 made it");
        for st in 0..SYNTH_TRACKS {
            assert!(p.synth(st).same_as(Project::NEW.synth(st)), "{name}: default synth track");
            for pat in 0..PATTERNS {
                assert!(p.pattern(pat).synth_blank(), "{name}: no synth steps or locks");
            }
        }
        assert_eq!(p.kit_name_of(1), Project::NEW.kit_name_of(1));
        assert!(p.tracks().iter().all(|t| t.kit == 0));
        assert_eq!(p.scale(), (Scale::Chromatic, 0));
        assert_eq!(p.fill(), None);
        let again = saved(&p);
        assert_eq!(u16::from_le_bytes([again[4], again[5]]), VERSION);
        let q = loaded(&again).unwrap();
        assert!(same(&p, &q), "{name}: v1 → v2 → load");
    }
}

/// The SEQ-2 parts survive the file exactly: synth steps (note, gate,
/// chord, the shared parameters), transposes, locks, kit B and the
/// tracks playing it, patches edited off their preset, levels, sends,
/// mute/solo of the synth tracks, scale, root, fill.
#[test]
fn synth_tracks_and_locks_round_trip() {
    let mut p = Box::new(Project::NEW);
    p.set_synth_step(2, 1, 5, SynthStep::note(64, 99).with_gate(9).with_chord(Chord::Seventh));
    p.set_synth_step(2, 0, 63, SynthStep::off(30));
    p.set_transpose(4, -7);
    assert!(p.set_lock(4, 3, 9, LockParam::Cutoff, 77));
    assert!(p.set_lock(4, 3, 2, LockParam::Tune, 130));
    p.set_track_kit(6, 1);
    p.set_factory_kit_of(1, 3);
    p.edit_patch(1, |x| x.filter.resonance = 0.61);
    p.set_synth_sends(0, 12, 34);
    p.set_mute(8, true);
    p.set_solo(9, true);
    p.set_scale(Scale::Dorian, 14);
    p.set_fill(Some(11));
    let b = saved(&p);
    let q = loaded(&b).unwrap();
    assert!(same(&p, &q));
    assert_eq!(
        q.pattern(2).synth_step(1, 5),
        SynthStep::note(64, 99).with_gate(9).with_chord(Chord::Seventh)
    );
    assert_eq!(q.pattern(2).synth_step(0, 63).pitch(), 30, "a changed empty step past the length is kept");
    assert_eq!(q.pattern(4).transpose(), -7);
    assert_eq!(q.pattern(4).locks.get(3, 9, LockParam::Cutoff), Some(77));
    assert_eq!(q.pattern(4).locks.get(3, 2, LockParam::Tune), Some(130));
    assert_eq!((q.track(6).kit, q.kit_name_of(1)), (1, "tight"));
    assert_eq!(q.synth(1).name.as_str(), "custom");
    assert_eq!(q.synth(1).patch.filter.resonance.to_bits(), 0.61f32.to_bits());
    assert_eq!((q.synth(0).delay, q.synth(0).reverb), (12, 34));
    assert!(q.synth(0).mute && q.synth(1).solo);
    assert_eq!(q.scale(), (Scale::Dorian, 2));
    assert_eq!(q.fill(), Some(11));
    for seed in 100..120 {
        let p = random(seed);
        let q = loaded(&saved(&p)).unwrap();
        assert!(same(&p, &q), "seed {seed}");
    }
}

/// Impossible SEQ-2 records: refused when their header is impossible,
/// impossible contents (a lock of the other kind of track, a step past
/// 64, more locks than a pattern holds, a later version's pattern kind or
/// synth track) dropped or clamped — never loaded as they are.
#[test]
fn impossible_synth_records_are_refused_or_dropped() {
    let p = Box::new(Project::NEW);
    let b = saved(&p);
    let body_end = b.len() - 4;
    let with = |rec: &[u8]| {
        let mut x = b[..body_end].to_vec();
        x.extend_from_slice(rec);
        x.extend([0; 4]);
        loaded(&recrc(x))
    };
    // Pattern kind: index 16, n 65, steps short.
    assert!(with(&[6, 5, 0, 16, 1, 2, 1, 0]).is_err());
    assert!(with(&[6, 5, 0, 0, 1, 2, 65, 0]).is_err());
    assert!(with(&[6, 5, 0, 0, 1, 2, 1, 0]).is_err(), "no step bytes");
    // An unknown kind is skipped; transpose 100 is clamped to 24.
    let q = with(&[6, 5, 0, 0, 7, 2, 1, 0]).unwrap();
    assert!(q.pattern(0).synth_blank());
    let q = with(&[6, 5, 0, 3, 1, 2, 0, 100]).unwrap();
    assert_eq!(q.pattern(3).transpose(), 24);
    // Locks: pattern 16 refused; a drum parameter on a synth track, step
    // 64, track 10 and code 9 dropped; the value clamped.
    assert!(with(&[9, 2, 0, 16, 0]).is_err());
    assert!(with(&[9, 6, 0, 0, 1, 0, 0, 0]).is_err(), "lock bytes short");
    let q = with(&[9, 22, 0, 1, 5, 0, 9, 0, 9, 64, 1, 1, 9, 0, 10, 3, 9, 0, 2, 9, 9, 0, 9, 4, 250]).unwrap();
    assert_eq!(q.pattern(1).locks.items().len(), 1);
    assert_eq!(q.pattern(1).locks.get(0, 9, LockParam::Resonance), Some(100));
    // More than a store holds: the first LOCKS kept.
    let mut rec = vec![9, 0, 0, 2, 40];
    for k in 0..40u8 {
        rec.extend([k, 0, 2, 50]);
    }
    let len = (rec.len() - 3) as u16;
    rec[1..3].copy_from_slice(&len.to_le_bytes());
    let q = with(&rec).unwrap();
    assert_eq!(q.pattern(2).locks.len(), LOCKS);
    assert_eq!(saved(&q), saved(&loaded(&saved(&q)).unwrap()));
    // A synth track of a later version is skipped; a bad bank refused.
    assert!(with(&[8, 5, 0, 2, 50, 0, 0, 0]).is_ok());
    assert!(with(&[8, 9, 0, 0, 50, 0, 0, 0, b'X', b'S', b'B', b'1']).is_err());
}

/// The lock store refuses gracefully when full: `set` says no and
/// nothing changes; updating a lock it holds still works; clearing makes
/// room.
#[test]
fn a_full_lock_store_refuses_gracefully() {
    let mut p = Box::new(Project::NEW);
    for k in 0..LOCKS {
        assert!(p.set_lock(0, k, 8, LockParam::Cutoff, k as u8));
    }
    let before = p.pattern(0).locks;
    let rev = p.rev();
    assert!(!p.set_lock(0, 40, 8, LockParam::Cutoff, 1), "full");
    assert!(!p.set_lock(0, 2, 8, LockParam::Glide, 1), "full");
    assert_eq!(p.pattern(0).locks, before, "unchanged");
    assert_eq!(p.rev(), rev, "not even the revision moved");
    assert!(p.set_lock(0, 3, 8, LockParam::Cutoff, 99), "an existing lock changes");
    assert_eq!(p.pattern(0).locks.get(3, 8, LockParam::Cutoff), Some(99));
    p.clear_lock(0, 5, 8, LockParam::Cutoff);
    assert!(p.set_lock(0, 40, 8, LockParam::Cutoff, 1), "room again");
    // Other patterns have their own store.
    assert!(p.set_lock(1, 0, 0, LockParam::Tune, 100));
    // Impossible locks are refused.
    assert!(!p.set_lock(1, 64, 0, LockParam::Tune, 1));
    assert!(!p.set_lock(1, 0, 0, LockParam::Cutoff, 1), "a synth parameter on a drum track");
    assert!(!p.set_lock(1, 0, 10, LockParam::Cutoff, 1));
    // Sorted, so the player finds them by binary search.
    let items = p.pattern(0).locks.items();
    assert!(items.windows(2).all(|w| (w[0].step, w[0].track) < (w[1].step, w[1].track)));
}

/// A name with bytes after its end or invalid UTF-8 is kept as its valid
/// text, so the project saves what it shows.
#[test]
fn names_are_cleaned() {
    assert_eq!(file::clean(b"ab\0cd\0\0\0\0\0\0\0\0\0\0\0").as_str(), "ab");
    assert_eq!(file::clean(&[b'o', b'k', 0xFF, b'x']).as_str(), "ok");
    assert_eq!(file::clean("Груув".as_bytes()).as_str(), "Груув");
}
