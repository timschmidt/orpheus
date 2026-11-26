//! The project file: exact round trips, sizes, and refusal of anything
//! damaged, newer or impossible; forward compatibility (unknown records,
//! longer records) and defaults for shorter ones.

mod common;

use common::{loaded, random, same, saved};
use orpheus_seq::file::{self, FileError, MAX_FILE, VERSION, crc32};
use orpheus_seq::model::{MAX_STEPS, PATTERNS, TRACKS};
use orpheus_seq::{Project, Step, demo};

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
    for p in [Box::new(Project::NEW), d] {
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
    assert!(new.len() < 400, "a new project: {} B", new.len());
    let mut d = Box::new(Project::NEW);
    demo::beat(&mut d);
    let demo = saved(&d);
    assert!(demo.len() < 2500, "the demo: {} B", demo.len());
    // Every pattern at 64 steps with every step set, a full song.
    let mut p = Box::new(Project::NEW);
    for pat in 0..PATTERNS {
        p.set_len(pat, MAX_STEPS);
        for t in 0..TRACKS {
            for s in 0..MAX_STEPS {
                p.set_step(pat, t, s, Step::hit(7).with_ratchet(3));
            }
        }
    }
    while p.song.push(3, 4) {}
    let full = saved(&p);
    assert!(full.len() <= MAX_FILE, "{} > {MAX_FILE}", full.len());
    assert!(full.len() + 330 > MAX_FILE, "MAX_FILE is the real bound ({} of {MAX_FILE})", full.len());
    std::println!(
        "file sizes: new {} B, demo {} B, full {} B (MAX_FILE {MAX_FILE})",
        new.len(),
        demo.len(),
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

/// A name with bytes after its end or invalid UTF-8 is kept as its valid
/// text, so the project saves what it shows.
#[test]
fn names_are_cleaned() {
    assert_eq!(file::clean(b"ab\0cd\0\0\0\0\0\0\0\0\0\0\0").as_str(), "ab");
    assert_eq!(file::clean(&[b'o', b'k', 0xFF, b'x']).as_str(), "ok");
    assert_eq!(file::clean("Груув".as_bytes()).as_str(), "Груув");
}
