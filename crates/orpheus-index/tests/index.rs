//! Build + query against a mock directory and in-memory storage.

mod common;

use common::*;
use orpheus_index::{
    Builder, Error, FileName, Index, LetterTable, Locator, MIN_SCRATCH, Probe, Progress, SortBy, Spec,
    Verifier,
};

const NAME: Spec = Spec { sort: SortBy::Name, show_hidden: false, audio_only: false, images_only: false };

fn signature_of(src: &mut MockDir) -> orpheus_index::Signature {
    let mut v = Verifier::new();
    let mut name = vec![0u8; orpheus_index::MAX_NAME];
    loop {
        if let Some(s) = v.step(src, &mut name, 100).unwrap() {
            return s;
        }
    }
}

#[test]
fn empty_and_tiny_directories() {
    let mut r = Rng(1);
    for n in [0usize, 1, 2, 5, 40] {
        let entries = random_dir(&mut r, n);
        for &spec in ALL_SPECS {
            let mut store = MemStore::new();
            let mut src = MockDir::new(entries.clone());
            let ix = build(&mut store, &mut src, spec, 4096);
            check_listing(&mut store, &ix, &entries, spec);
            let again = Index::open(&mut store, DIR, spec).unwrap().expect("reopens");
            assert_eq!(again, ix);
            assert!(ix.verify(&mut store, &mut [0u8; 64]).unwrap());
            // Scratch files are gone.
            assert!(store.get(FileName::TEMP_A).is_none() && store.get(FileName::TEMP_B).is_none());
        }
    }
}

#[test]
fn ten_thousand_random_names_all_orders() {
    let mut r = Rng(0x5EED);
    let entries = random_dir(&mut r, 10_000);
    for &spec in ALL_SPECS {
        for scratch in [4096usize, 8192] {
            let mut store = MemStore::new();
            let mut src = MockDir::new(entries.clone());
            let (ix, stats) = build_with(&mut store, &mut src, spec, scratch, 64, 0).unwrap();
            check_listing(&mut store, &ix, &entries, spec);
            assert!(ix.verify(&mut store, &mut [0u8; 512]).unwrap());
            if scratch == 4096 {
                // 10 000 names do not merge in one pass from 4 KiB.
                assert!(stats.max_pass >= 1, "expected several passes, got {}", stats.max_pass);
            }
        }
    }
}

#[test]
fn equal_prefixes_and_numbers() {
    let mut r = Rng(42);
    let names: Vec<String> = (0..3000).map(|i| prefixed_name(&mut r, i)).collect();
    let entries = make_dir(names, &mut r);
    for &spec in &ALL_SPECS[..3] {
        let mut store = MemStore::new();
        let mut src = MockDir::new(entries.clone());
        let ix = build(&mut store, &mut src, spec, MIN_SCRATCH);
        check_listing(&mut store, &ix, &entries, spec);
    }
}

#[test]
fn identical_names_tie_by_locator() {
    let mut r = Rng(9);
    let names: Vec<String> = (0..600)
        .map(|i| ["same.mp3", "SAME.mp3", "same.MP3", "Same 1.mp3", "same 01.mp3"][i % 5].into())
        .collect();
    let entries = make_dir(names, &mut r);
    let mut store = MemStore::new();
    let mut src = MockDir::new(entries.clone());
    let spec = Spec { show_hidden: true, ..NAME };
    let ix = build(&mut store, &mut src, spec, MIN_SCRATCH);
    check_listing(&mut store, &ix, &entries, spec);
}

#[test]
fn hidden_entries() {
    let mut r = Rng(5);
    let mut entries = random_dir(&mut r, 500);
    entries[10].name = ".hidden".into();
    entries[11].name = "System Volume Information".into();
    entries[11].is_dir = true;
    entries[12].name = "visible.mp3".into();
    entries[12].hidden = true; // FAT attribute
    for show in [false, true] {
        let spec = Spec { show_hidden: show, ..NAME };
        let mut store = MemStore::new();
        let mut src = MockDir::new(entries.clone());
        let ix = build(&mut store, &mut src, spec, 4096);
        check_listing(&mut store, &ix, &entries, spec);
        for e in &entries[10..13] {
            assert_eq!(ix.position_of(&mut store, e.loc).unwrap().is_some(), show);
        }
    }
}

#[test]
fn suspend_and_resume_with_other_scratch() {
    let mut r = Rng(77);
    let entries = random_dir(&mut r, 5000);
    for &spec in &ALL_SPECS[..3] {
        for (every, budget) in [(1u32, 7u32), (3, 50), (2, 1)] {
            let mut store = MemStore::new();
            let mut src = MockDir::new(entries.clone());
            let (ix, _) = build_with(&mut store, &mut src, spec, 4096, budget, every).unwrap();
            check_listing(&mut store, &ix, &entries, spec);
        }
    }
}

#[test]
fn resume_with_too_small_scratch_is_refused() {
    let mut r = Rng(78);
    let entries = random_dir(&mut r, 3000);
    let mut store = MemStore::new();
    let mut src = MockDir::new(entries.clone());
    let mut big = vec![0u8; 8192];
    let mut b = Builder::new(&mut big, DIR, NAME, 0).unwrap();
    // Run into the merge with a group of the widest kind.
    loop {
        if let Progress::Merging { .. } = b.step(&mut store, &mut src, 1).unwrap() {
            break;
        }
    }
    b.step(&mut store, &mut src, 1).unwrap();
    let s = b.suspend(&mut store).unwrap();
    let mut small = vec![0u8; MIN_SCRATCH];
    assert!(s.resume(&mut small).is_none(), "a minimal scratch cannot continue a wide merge");
    let mut again = vec![0u8; 8192];
    let mut b = s.resume(&mut again).unwrap();
    let ix = loop {
        if let Progress::Done(ix) = b.step(&mut store, &mut src, 100).unwrap() {
            break ix;
        }
    };
    check_listing(&mut store, &ix, &entries, NAME);
    assert!(Builder::new(&mut [0u8; MIN_SCRATCH - 1], DIR, NAME, 0).is_none());
}

#[test]
fn power_cut_during_build_is_detected_and_rebuilt() {
    let mut r = Rng(0xC0FFEE);
    let entries = random_dir(&mut r, 2500);
    // Total bytes a full build writes.
    let mut probe = MemStore::new();
    build(&mut probe, &mut MockDir::new(entries.clone()), NAME, 4096);
    let total = probe.write_bytes;
    // An index for a different, older directory content is in place first.
    let mut cuts: Vec<u64> = (0..40).map(|i| total * i / 40).collect();
    cuts.extend([total - 1, total - 60, total - 600, total - 512, 1, 511, 512, 513]);
    for cut in cuts {
        let mut store = MemStore::new();
        let old: Vec<_> = entries[..100].to_vec();
        build(&mut store, &mut MockDir::new(old), NAME, 4096);
        store.cut_after = Some(cut);
        let res = build_with(&mut store, &mut MockDir::new(entries.clone()), NAME, 4096, 64, 0);
        assert!(matches!(res, Err(Error::Io(IoErr::Cut))), "cut at {cut} of {total}: {:?}", res.is_ok());
        store.revive();
        // After the cut: no index, or (cut inside the very last write
        // only) never a half-written one passed off as complete.
        match Index::open(&mut store, DIR, NAME).unwrap() {
            None => {}
            Some(ix) => {
                assert!(ix.verify(&mut store, &mut [0u8; 256]).unwrap(), "cut at {cut}");
                check_listing(&mut store, &ix, &entries, NAME);
            }
        }
        let mut src = MockDir::new(entries.clone());
        let ix = build(&mut store, &mut src, NAME, 4096);
        check_listing(&mut store, &ix, &entries, NAME);
    }
}

#[test]
fn damaged_or_foreign_files_are_rejected() {
    let mut r = Rng(3);
    let entries = random_dir(&mut r, 300);
    let mut store = MemStore::new();
    build(&mut store, &mut MockDir::new(entries.clone()), NAME, 4096);
    let f = FileName::index(DIR, NAME);
    assert!(Index::open(&mut store, DIR, NAME).unwrap().is_some());
    assert!(Index::open(&mut store, DIR + 1, NAME).unwrap().is_none());
    assert!(Index::open(&mut store, DIR, Spec { sort: SortBy::Size, ..NAME }).unwrap().is_none());
    for at in [0usize, 5, 16, 40, 63, 64, 66] {
        let mut s = store.clone();
        s.get_mut(f).unwrap()[at] ^= 0x10;
        assert!(Index::open(&mut s, DIR, NAME).unwrap().is_none(), "flip at {at}");
    }
    let mut s = store.clone();
    s.get_mut(f).unwrap().pop();
    assert!(Index::open(&mut s, DIR, NAME).unwrap().is_none(), "short file");
    let mut s = store.clone();
    let ix = Index::open(&mut s, DIR, NAME).unwrap().unwrap();
    s.get_mut(f).unwrap()[600] ^= 1;
    assert!(!ix.verify(&mut s, &mut [0u8; 100]).unwrap(), "row damage found by verify");
}

#[test]
fn staleness_after_one_change() {
    let mut r = Rng(11);
    let entries = random_dir(&mut r, 2000);
    let mut store = MemStore::new();
    let mut src = MockDir::new(entries.clone());
    let ix = build(&mut store, &mut src, NAME, 4096);
    assert!(ix.is_current(&signature_of(&mut src)));
    assert_eq!(ix.quick(), 77);
    assert_eq!(ix.signature().entries, 2000);

    // Order on disk is irrelevant (the hash is a sum).
    let mut shuffled = entries.clone();
    shuffled.reverse();
    assert!(ix.is_current(&signature_of(&mut MockDir::new(shuffled))));

    let mut changes: Vec<(&str, Vec<MockEntry>)> = Vec::new();
    let mut add = entries.clone();
    add.push(MockEntry { loc: Locator { dir: DIR, index: 99_999 }, ..entries[5].clone() });
    changes.push(("add", add));
    let mut del = entries.clone();
    del.remove(1234);
    changes.push(("remove", del));
    let mut ren = entries.clone();
    let mut name: Vec<char> = ren[700].name.chars().collect();
    name[0] = if name[0] == 'q' { 'r' } else { 'q' };
    ren[700].name = name.into_iter().collect();
    changes.push(("rename, same length", ren));
    let mut ren2 = entries.clone();
    ren2[3].name.push('x');
    changes.push(("rename", ren2));
    let mut touch = entries.clone();
    touch[9].mtime += 1;
    changes.push(("touch", touch));
    let mut grow = entries.clone();
    grow[9].size += 1;
    changes.push(("size", grow));
    let mut moved = entries.clone();
    moved[50].loc.index += 1_000_000;
    changes.push(("moved slot", moved));
    let mut attr = entries.clone();
    attr[60].hidden = !attr[60].hidden;
    changes.push(("attribute", attr));
    for (what, changed) in changes {
        let mut src = MockDir::new(changed.clone());
        let sig = signature_of(&mut src);
        assert!(!ix.is_current(&sig), "{what} not detected");
        // Rebuild and the new index is current again and correct.
        let ix2 = build(&mut store, &mut src, NAME, 4096);
        assert!(ix2.is_current(&sig));
        check_listing(&mut store, &ix2, &changed, NAME);
    }
}

#[test]
fn queries_positions_find_and_letters() {
    let mut r = Rng(0xF00D);
    let mut names: Vec<String> = (0..1500).map(|_| random_name(&mut r)).collect();
    names.extend((0..500).map(|i| prefixed_name(&mut r, i)));
    names.extend(["___", "!!!", "(Live) Zebra.mp3", "-", "..."].map(String::from));
    let entries = make_dir(names, &mut r);
    for &spec in ALL_SPECS {
        let mut store = MemStore::new();
        let mut src = MockDir::new(entries.clone());
        let ix = build(&mut store, &mut src, spec, 4096);
        let want = oracle(&entries, spec);
        assert_eq!(ix.files() + ix.dirs(), ix.count());
        // position_of and find for every row.
        let mut namebuf = vec![0u8; orpheus_index::MAX_NAME];
        for (i, e) in want.iter().enumerate() {
            if i % 7 == 0 {
                assert_eq!(ix.position_of(&mut store, e.loc).unwrap(), Some(i as u32));
            }
            let p = Probe {
                name: &e.name,
                loc: e.loc,
                is_dir: e.is_dir,
                hidden: e.hidden,
                size: e.size,
                mtime: e.mtime,
            };
            assert_eq!(
                ix.find(&mut store, &mut src, &p, &mut namebuf).unwrap(),
                Ok(i as u32),
                "{:?}",
                e.name
            );
        }
        assert_eq!(ix.position_of(&mut store, Locator { dir: 1, index: 1 }).unwrap(), None);
        // find for entries not in the listing: insertion point.
        for k in 0..200 {
            let name = random_name(&mut r);
            let e = MockEntry {
                name,
                is_dir: k % 9 == 0,
                hidden: false,
                size: r.below(5000),
                mtime: 0x5A00_0000 + r.below(500) as u32,
                loc: Locator { dir: DIR, index: 1_000_000 + k },
            };
            let mut with = want.clone();
            with.push(e.clone());
            let sorted =
                oracle(&with, Spec { show_hidden: true, audio_only: false, images_only: false, ..spec });
            let sorted: Vec<_> = sorted
                .into_iter()
                .filter(|x| want.iter().any(|w| w.loc == x.loc) || x.loc == e.loc)
                .collect();
            let at = sorted.iter().position(|x| x.loc == e.loc).unwrap();
            let p = Probe {
                name: &e.name,
                loc: e.loc,
                is_dir: e.is_dir,
                hidden: false,
                size: e.size,
                mtime: e.mtime,
            };
            assert_eq!(
                ix.find(&mut store, &mut src, &p, &mut namebuf).unwrap(),
                Err(at as u32),
                "{:?}",
                e.name
            );
        }
        // Letters and letter jump against the UI's scan.
        let letters: Vec<Option<char>> = want.iter().map(|e| orpheus_index::letter_of(&e.name)).collect();
        for i in 0..want.len() as u32 {
            if i % 3 == 0 {
                assert_eq!(ix.letter(&mut store, i).unwrap(), letters[i as usize]);
            }
            for fwd in [true, false] {
                assert_eq!(
                    ix.seek_letter(&mut store, i, fwd).unwrap(),
                    ref_seek_letter(&letters, i, fwd),
                    "from {i} fwd {fwd} spec {spec:?}"
                );
            }
        }
        assert_eq!(ix.seek_letter(&mut store, ix.count(), true).unwrap(), None);
        assert_eq!(ix.row(&mut store, ix.count()).unwrap(), None);
    }
}

#[test]
fn long_and_bad_names() {
    let mut r = Rng(8);
    // 255 three-byte characters: the longest name a FAT/exFAT entry holds.
    let longest: String = std::iter::repeat_n('日', 255).collect();
    let mut entries = random_dir(&mut r, 400);
    entries[0].name = longest.clone();
    entries[1].name = format!("{}1", &longest[..762]);
    entries[2].name = std::iter::repeat_n('9', 700).collect();
    entries[3].name = std::iter::repeat_n('9', 701).collect();
    for scratch in [MIN_SCRATCH, 4096] {
        let mut store = MemStore::new();
        let mut src = MockDir::new(entries.clone());
        let ix = build(&mut store, &mut src, NAME, scratch);
        check_listing(&mut store, &ix, &entries, NAME);
    }
    let mut bad = entries.clone();
    bad[5].name = format!("{longest}x");
    let mut store = MemStore::new();
    let res = build_with(&mut store, &mut MockDir::new(bad), NAME, 4096, 64, 0);
    assert!(matches!(res, Err(Error::BadName)));
}

#[test]
fn mtime_packing_orders_like_the_simulator() {
    use orpheus_index::mtime_of;
    let t = |year, month, day, hour, minute, second| orpheus_fs::DateTime {
        year,
        month,
        day,
        hour,
        minute,
        second,
        centis: 0,
    };
    let seq = [
        t(1980, 1, 1, 0, 0, 0),
        t(1999, 12, 31, 23, 59, 59),
        t(2000, 1, 1, 0, 0, 0),
        t(2000, 1, 1, 0, 1, 0),
        t(2000, 1, 1, 1, 0, 0),
        t(2000, 1, 2, 0, 0, 0),
        t(2000, 2, 1, 0, 0, 0),
        t(2107, 12, 31, 23, 59, 59),
    ];
    for w in seq.windows(2) {
        assert!(mtime_of(&w[0]) < mtime_of(&w[1]), "{:?} < {:?}", w[0], w[1]);
    }
    // Seconds do not count.
    assert_eq!(mtime_of(&t(2024, 5, 6, 7, 8, 0)), mtime_of(&t(2024, 5, 6, 7, 8, 59)));
}

/// Property test: random directories, specs, scratch sizes, budgets and
/// suspend points always give exactly the reference order.
#[test]
fn property_random_builds_match_full_sort() {
    let mut r = Rng(0xDEC0DE);
    for case in 0..120 {
        let n = match case % 4 {
            0 => r.below(50) as usize,
            1 => r.below(600) as usize,
            2 => r.below(3000) as usize,
            _ => 3000 + r.below(3000) as usize,
        };
        let names: Vec<String> = (0..n)
            .map(|i| if r.chance(30) { prefixed_name(&mut r, i) } else { random_name(&mut r) })
            .collect();
        let entries = make_dir(names, &mut r);
        let spec = *r.pick(ALL_SPECS);
        let scratch = MIN_SCRATCH + r.below(14_000) as usize;
        let budget = 1 + r.below(300) as u32;
        let every = if r.chance(30) { 1 + r.below(20) as u32 } else { 0 };
        let mut store = MemStore::new();
        let mut src = MockDir::new(entries.clone());
        let (ix, _) = build_with(&mut store, &mut src, spec, scratch, budget, every).unwrap();
        check_listing(&mut store, &ix, &entries, spec);
        assert!(ix.verify(&mut store, &mut [0u8; 300]).unwrap());
    }
}

#[test]
fn letter_table_answers_like_the_index() {
    let mut r = Rng(0x1E77);
    for n in [0usize, 1, 7, 300, 2000] {
        let entries = random_dir(&mut r, n);
        for &spec in ALL_SPECS {
            let mut store = MemStore::new();
            let mut src = MockDir::new(entries.clone());
            let ix = build(&mut store, &mut src, spec, 4096);
            let mut t = LetterTable::<128>::EMPTY;
            let fits = t.load(&ix, &mut store).unwrap();
            assert_eq!(
                fits,
                ix.letter_runs() as usize <= 128 && ix.letter_runs() > 0 || ix.letter_runs() == 0
            );
            if !fits || !t.is_loaded() {
                assert_eq!(t, LetterTable::EMPTY);
                continue;
            }
            for i in 0..ix.count() + 1 {
                assert_eq!(t.letter(i), ix.letter(&mut store, i).unwrap(), "{spec:?} row {i}");
                for fwd in [true, false] {
                    assert_eq!(
                        t.seek_letter(i, fwd),
                        ix.seek_letter(&mut store, i, fwd).unwrap(),
                        "{spec:?} {i} {fwd}"
                    );
                }
            }
        }
    }
    // Too many runs: not loaded.
    let entries = random_dir(&mut r, 3000);
    let spec = Spec { sort: SortBy::Size, show_hidden: true, audio_only: false, images_only: false };
    let mut store = MemStore::new();
    let ix = build(&mut store, &mut MockDir::new(entries), spec, 4096);
    assert!(ix.letter_runs() > 128);
    let mut t = LetterTable::<128>::EMPTY;
    assert!(!t.load(&ix, &mut store).unwrap());
    assert!(!t.is_loaded());
}
